//! Connection lifecycle on the video thread: find the device, start the
//! server, stream video until it ends, then reconnect. The UI is told what
//! happens through `Ui::post`; it never blocks on any of this.

use std::io::Read;
use std::net::{Shutdown, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::adb::{self, Adb};
use crate::call::{self, CallState};
use crate::companion::{self, Channel, Companion, CompanionError, Peer};
use crate::controller::{self, ControlSender};
use crate::pairing::{self, PairingOffer, ServiceKind};
use crate::protocol::control::ControlMsg;
use crate::protocol::device_msg::DeviceMsg;
use crate::protocol::parse_device_name;
use crate::server::{self, ServerHandle, VersionMismatch};
use crate::video::{self, VideoEvents, VideoSink};

/// Events for the UI thread.
pub enum UiEvent {
    /// Not mirroring; show this message (waiting, error, reconnecting).
    Status(String),
    Connected {
        device_name: String,
        control: ControlSender,
    },
    /// Video frame size changed (first frame, rotation).
    VideoSize {
        width: u32,
        height: u32,
    },
    Disconnected,
    /// The device clipboard changed.
    Clipboard(String),
    /// Unrecoverable; the session has stopped.
    Fatal(String),
    /// Brief message (file transfers).
    Notice(String),
    /// Phone call state changed.
    Call(CallState),
    /// No phone found: show Wi-Fi setup with this QR code to pair one.
    Pairing {
        qr: String,
    },
    /// Progress text for the setup screen.
    PairingProgress(String),
}

pub trait Ui: Send + Sync + 'static {
    fn post(&self, event: UiEvent);
}

#[derive(Debug, Clone)]
pub enum Tcpip {
    /// Switch the USB device to wireless debugging and connect over Wi-Fi.
    Auto,
    /// Connect to this `host[:port]`.
    Addr(String),
}

#[derive(Debug, Clone)]
pub struct Config {
    pub serial: Option<String>,
    pub force_forward: bool,
    pub tcpip: Option<Tcpip>,
    pub stats_level: log::Level,
    /// Watch for incoming calls (`dumpsys telephony.registry` once a second).
    pub call_alerts: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

#[derive(Default)]
struct State {
    stopping: bool,
    server: Option<ServerHandle>,
    sockets: Vec<TcpStream>,
    serial: Option<String>,
}

impl Shared {
    /// Sleeps up to `d`, returning early (with `true`) when stopping.
    fn sleep(&self, d: Duration) -> bool {
        let guard = self.state.lock().unwrap();
        let (guard, _) = self
            .wake
            .wait_timeout_while(guard, d, |s| !s.stopping)
            .unwrap();
        guard.stopping
    }

    fn stopping(&self) -> bool {
        self.state.lock().unwrap().stopping
    }
}

pub struct Session {
    shared: Arc<Shared>,
    adb: Adb,
    ui: Arc<dyn Ui>,
    thread: Option<JoinHandle<()>>,
}

impl Session {
    pub fn start(
        adb: Adb,
        config: Config,
        sink: Box<dyn VideoSink>,
        ui: Arc<dyn Ui>,
        companion: Option<Companion>,
    ) -> Result<Self> {
        let shared = Arc::new(Shared::default());
        let runner = Runner {
            adb: adb.clone(),
            config,
            shared: shared.clone(),
            ui: ui.clone(),
            wireless_addr: None,
            offer: None,
            offer_shown: false,
            companion,
            declined: Vec::new(),
        };
        let thread = thread::Builder::new()
            .name("video".into())
            .spawn(move || runner.run(sink))?;
        Ok(Self {
            shared,
            adb,
            ui,
            thread: Some(thread),
        })
    }

    /// Stops the server and unblocks the video thread. Safe to call from
    /// the main thread: it doesn't wait for anything slow.
    pub fn stop(&mut self) {
        let mut state = self.shared.state.lock().unwrap();
        state.stopping = true;
        for s in state.sockets.drain(..) {
            let _ = s.shutdown(Shutdown::Both);
        }
        if let Some(server) = state.server.take() {
            server.kill();
        }
        drop(state);
        self.shared.wake.notify_all();
        log::info!("stopped");
    }

    /// Waits for the video thread (headless mode).
    pub fn join(&mut self) {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    /// Pushes files to /sdcard/Download in the background and asks the
    /// media scanner to index them.
    pub fn push_files(&self, paths: Vec<PathBuf>, control: Option<ControlSender>) {
        let Some(serial) = self.shared.state.lock().unwrap().serial.clone() else {
            log::warn!("no adb connection; not pushing files");
            self.ui.post(UiEvent::Notice(
                "Sending files needs a Developer options (adb) connection".into(),
            ));
            return;
        };
        let adb = self.adb.clone();
        let ui = self.ui.clone();
        let spawned = thread::Builder::new().name("push".into()).spawn(move || {
            let total = paths.len();
            let mut sent = 0;
            for path in paths {
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                ui.post(UiEvent::Notice(format!("Sending {name}…")));
                let remote = format!("/sdcard/Download/{name}");
                match adb.push(&serial, &path, &remote) {
                    Ok(()) => {
                        log::info!("pushed {} to {remote}", path.display());
                        sent += 1;
                        if let Some(c) = &control {
                            c.send(ControlMsg::ScanFile(remote));
                        }
                    }
                    Err(e) => {
                        log::error!("could not push {}: {e:#}", path.display());
                        ui.post(UiEvent::Notice(format!("Couldn't send {name}: {e:#}")));
                        return;
                    }
                }
            }
            ui.post(UiEvent::Notice(if total == 1 {
                "Saved to Download".to_owned()
            } else {
                format!("Saved {sent} files to Download")
            }));
        });
        if let Err(e) = spawned {
            log::error!("could not start push thread: {e}");
        }
    }
}

struct Runner {
    adb: Adb,
    config: Config,
    shared: Arc<Shared>,
    ui: Arc<dyn Ui>,
    /// Remembered after switching a USB device to Wi-Fi.
    wireless_addr: Option<String>,
    /// Current QR pairing invitation, and whether the UI shows it.
    offer: Option<PairingOffer>,
    offer_shown: bool,
    /// Companion-app mode (macOS); `None` in headless mode.
    companion: Option<Companion>,
    /// Phones whose pairing was declined recently: not asked again for a while.
    declined: Vec<(String, Instant)>,
}

impl Runner {
    fn run(mut self, mut sink: Box<dyn VideoSink>) {
        let mut backoff = Duration::from_secs(1);
        // Repeated identical errors (e.g. "no device" every second) are
        // reported once.
        let mut last_error = String::new();
        while !self.shared.stopping() {
            match self.connect_and_stream(sink.as_mut()) {
                Ok(()) => {
                    backoff = Duration::from_secs(1);
                    last_error.clear();
                    self.ui.post(UiEvent::Disconnected);
                    if self.shared.stopping() {
                        break;
                    }
                    self.ui
                        .post(UiEvent::Status("Disconnected. Reconnecting…".into()));
                }
                Err(e) if e.downcast_ref::<VersionMismatch>().is_some() => {
                    log::error!("{e:#}");
                    self.ui.post(UiEvent::Fatal(format!(
                        "{e:#}\n\nThis app needs scrcpy-server {}.",
                        server::SERVER_VERSION
                    )));
                    return;
                }
                Err(e) => {
                    if self.shared.stopping() {
                        break;
                    }
                    let msg = format!("{e:#}");
                    // While the pairing screen is up, "no device" is implied.
                    let waiting = matches!(
                        e.downcast_ref::<adb::DeviceError>(),
                        Some(adb::DeviceError::NoDevice)
                    );
                    if waiting && self.offer_shown {
                        last_error = msg.clone();
                    }
                    if msg != last_error {
                        log::warn!("{msg}");
                        self.ui.post(UiEvent::Status(msg.clone()));
                        last_error = msg;
                    }
                    // Waiting for a device: poll quickly. Other errors: back off.
                    let waiting = e.downcast_ref::<adb::DeviceError>().is_some();
                    let delay = if waiting {
                        Duration::from_secs(1)
                    } else {
                        backoff
                    };
                    backoff = (backoff * 2).min(Duration::from_secs(8));
                    if self.shared.sleep(delay) {
                        break;
                    }
                    continue;
                }
            }
            if self.shared.sleep(Duration::from_secs(1)) {
                break;
            }
        }
    }

    /// One connection, start to end. `Ok` means the stream ended normally
    /// (device unplugged, server stopped).
    fn connect_and_stream(&mut self, sink: &mut dyn VideoSink) -> Result<()> {
        let serial = match self.resolve_device() {
            Ok(serial) => serial,
            Err(e) => {
                // No adb phone: try a phone running the companion app.
                let no_device = matches!(
                    e.downcast_ref::<adb::DeviceError>(),
                    Some(adb::DeviceError::NoDevice)
                );
                if no_device && let Some(peer) = self.companion_peer() {
                    return self.stream_companion(&peer, sink);
                }
                return Err(e);
            }
        };
        log::info!("using device {serial}");
        self.ui
            .post(UiEvent::Status(format!("Connecting to {serial}…")));

        let shared = self.shared.clone();
        let (server, conn) = server::start(
            &self.adb,
            &serial,
            server::Params::new(self.config.force_forward),
            &|handle| shared.state.lock().unwrap().server = Some(handle),
        )?;
        log::info!("connected to \"{}\"", conn.device_name);

        {
            let mut state = self.shared.state.lock().unwrap();
            if state.stopping {
                return Ok(()); // quit while connecting; `server` drops here
            }
            state.serial = Some(serial.clone());
            state.sockets = vec![conn.video.try_clone()?, conn.control.try_clone()?];
        }

        let control = controller::start(conn.control, device_message_handler(self.ui.clone()))?;
        self.ui.post(UiEvent::Connected {
            device_name: conn.device_name,
            control: control.clone(),
        });

        let call_watch = Arc::new(AtomicBool::new(true));
        if self.config.call_alerts {
            call::spawn_watcher(
                self.adb.clone(),
                serial,
                self.ui.clone(),
                call_watch.clone(),
            );
        }

        let mut events = Events {
            ui: self.ui.clone(),
            control: control.clone(),
        };
        let result = video::run(conn.video, sink, &mut events, self.config.stats_level);

        // Tear down this connection.
        call_watch.store(false, Ordering::Relaxed);
        control.stop_writer();
        let mut state = self.shared.state.lock().unwrap();
        for s in state.sockets.drain(..) {
            let _ = s.shutdown(Shutdown::Both);
        }
        state.server = None;
        state.serial = None;
        drop(state);
        drop(server);

        match result {
            Ok(()) => {
                log::info!("video stream ended");
                Ok(())
            }
            Err(_) if self.shared.stopping() => Ok(()),
            Err(e) => Err(e.context("video stream failed")),
        }
    }

    /// A companion phone to use: a paired one if visible, else an unpaired
    /// one that hasn't declined recently.
    fn companion_peer(&mut self) -> Option<Peer> {
        let c = self.companion.as_ref()?;
        let peers = c.discovery.peers();
        if let Some(p) = peers.iter().find(|p| c.store.get(&p.id).is_some()) {
            return Some(p.clone());
        }
        self.declined
            .retain(|(_, t)| t.elapsed() < Duration::from_secs(60));
        peers
            .into_iter()
            .find(|p| !self.declined.iter().any(|(id, _)| *id == p.id))
    }

    /// Companion mode: pair if needed, open the three channels, stream.
    fn stream_companion(&mut self, peer: &Peer, sink: &mut dyn VideoSink) -> Result<()> {
        let c = self.companion.clone().expect("companion mode configured");
        let record = match c.store.get(&peer.id) {
            Some(r) => r,
            None => {
                log::info!(
                    "pairing with companion app on {} ({})",
                    peer.name,
                    peer.addr
                );
                let ui = self.ui.clone();
                let name = peer.name.clone();
                let shared = self.shared.clone();
                let result = companion::pair(
                    &c,
                    peer,
                    &|code| {
                        ui.post(UiEvent::PairingProgress(format!(
                            "Pairing with the DroidMirror app on {name}.\n\
                             Check the phone shows {code}, then tap Allow."
                        )))
                    },
                    &|tcp| {
                        if let Ok(t) = tcp.try_clone() {
                            shared.state.lock().unwrap().sockets.push(t);
                        }
                    },
                );
                self.shared.state.lock().unwrap().sockets.clear();
                match result {
                    Ok(r) => {
                        log::info!("paired with {}", peer.name);
                        r
                    }
                    Err(e) => {
                        if e.downcast_ref::<CompanionError>() == Some(&CompanionError::Denied) {
                            self.declined.push((peer.id.clone(), Instant::now()));
                        }
                        return Err(e.context(format!("pairing with {} failed", peer.name)));
                    }
                }
            }
        };

        self.ui.post(UiEvent::Status(format!(
            "Connecting to {} (DroidMirror app)…",
            peer.name
        )));
        let open = |channel| match companion::open(&c, peer, &record, channel) {
            Err(e) if e.downcast_ref::<CompanionError>() == Some(&CompanionError::UnknownToken) => {
                // The phone forgot us (app data cleared): pair again next time.
                let _ = c.store.remove(&peer.id);
                Err(e)
            }
            other => other,
        };
        let mut video = open(Channel::Video)?;
        let mut name = [0u8; crate::protocol::DEVICE_NAME_LEN];
        video
            .stream
            .read_exact(&mut name)
            .context("no device name from the phone")?;
        let device_name = parse_device_name(&name);
        let control_conn = open(Channel::Control)?;
        let events_conn = open(Channel::Events)?;
        log::info!("connected to \"{device_name}\" through the companion app");

        {
            let mut state = self.shared.state.lock().unwrap();
            if state.stopping {
                return Ok(());
            }
            state.serial = None;
            state.sockets = vec![
                video.tcp.try_clone()?,
                control_conn.tcp.try_clone()?,
                events_conn.tcp.try_clone()?,
            ];
        }
        let control = controller::start_split(
            control_conn.stream,
            events_conn.stream,
            device_message_handler(self.ui.clone()),
        )?;
        self.ui.post(UiEvent::Connected {
            device_name,
            control: control.clone(),
        });
        let mut events = Events {
            ui: self.ui.clone(),
            control: control.clone(),
        };
        let result = video::run(video.stream, sink, &mut events, self.config.stats_level);

        control.stop_writer();
        let mut state = self.shared.state.lock().unwrap();
        for s in state.sockets.drain(..) {
            let _ = s.shutdown(Shutdown::Both);
        }
        drop(state);
        self.ui.post(UiEvent::Call(CallState::Idle));

        match result {
            Ok(()) => {
                log::info!("companion video stream ended");
                Ok(())
            }
            Err(_) if self.shared.stopping() => Ok(()),
            Err(e) => Err(e.context("video stream failed")),
        }
    }

    /// Picks the device, handling `--tcpip`.
    fn resolve_device(&mut self) -> Result<String> {
        let wanted = match (&self.config.tcpip, &self.wireless_addr) {
            (Some(Tcpip::Addr(a)), _) => Some(adb::with_default_port(a)),
            (Some(Tcpip::Auto), Some(a)) => Some(a.clone()),
            _ => None,
        };
        if let Some(addr) = wanted {
            let ready = self
                .adb
                .devices()?
                .iter()
                .any(|d| d.serial == addr && d.state == "device");
            if !ready {
                self.ui
                    .post(UiEvent::Status(format!("Connecting to {addr} over Wi-Fi…")));
                self.adb.connect(&addr)?;
            }
            return Ok(adb::select_device(&self.adb.devices()?, Some(&addr))?);
        }

        let devices = self.adb.devices()?;
        let serial = match adb::select_device(&devices, self.config.serial.as_deref()) {
            Ok(s) => s,
            // Nothing over USB or already connected: look on the network.
            Err(adb::DeviceError::NoDevice) if self.config.serial.is_none() => {
                return self.find_on_network();
            }
            // adb often lists one Wi-Fi phone twice (ip:port and its mDNS
            // name). Use it if all entries are the same phone.
            Err(adb::DeviceError::Ambiguous(list)) if self.config.serial.is_none() => self
                .same_phone(&devices)
                .ok_or(adb::DeviceError::Ambiguous(list))?,
            Err(e) => return Err(e.into()),
        };
        self.offer_shown = false;
        if matches!(self.config.tcpip, Some(Tcpip::Auto)) && !serial.contains(':') {
            let addr = self.switch_to_wireless(&serial)?;
            self.wireless_addr = Some(addr.clone());
            return Ok(addr);
        }
        Ok(serial)
    }

    /// No device connected: connect to a paired phone advertising Wireless
    /// debugging, or else offer QR pairing and complete it when the phone
    /// scans the code. Returns `NoDevice` while still waiting.
    fn find_on_network(&mut self) -> Result<String> {
        let services = pairing::discover(&self.adb);

        for svc in services.iter().filter(|s| s.kind == ServiceKind::Connect) {
            match self.adb.connect(&svc.addr) {
                Ok(()) => {
                    log::info!("found {} on the network at {}", svc.instance, svc.addr);
                    self.offer_shown = false;
                    return Ok(adb::select_device(&self.adb.devices()?, Some(&svc.addr))?);
                }
                Err(e) => log::debug!("could not connect to {}: {e:#}", svc.addr),
            }
        }

        let offer = self.offer.get_or_insert_with(PairingOffer::new).clone();
        if !self.offer_shown {
            log::info!("waiting for a phone; pair one by scanning the QR code in the window");
            log::debug!("pairing offer {}", offer.name);
            self.ui.post(UiEvent::Pairing {
                qr: offer.qr_text(),
            });
            self.offer_shown = true;
        }
        if let Some(svc) = offer.find(&services) {
            log::info!("phone scanned the code; pairing with {}", svc.addr);
            self.ui.post(UiEvent::PairingProgress("Pairing…".into()));
            let out = self.adb.pair(&svc.addr, &offer.password)?;
            // A code works once; show a fresh one next time.
            self.offer = None;
            self.offer_shown = false;
            if pairing::pair_succeeded(&out) {
                log::info!("{}", out.trim());
                self.ui
                    .post(UiEvent::PairingProgress("Paired. Connecting…".into()));
                // adb auto-connects to the newly paired phone; give it a moment.
                self.shared.sleep(Duration::from_secs(2));
            } else {
                log::warn!("pairing failed: {}", out.trim());
                self.ui
                    .post(UiEvent::Notice(format!("Pairing failed: {}", out.trim())));
            }
        }
        Err(adb::DeviceError::NoDevice.into())
    }

    /// If every ready device is the same phone (same serial number),
    /// returns one of them.
    fn same_phone(&self, devices: &[adb::DeviceEntry]) -> Option<String> {
        let ready: Vec<&str> = devices
            .iter()
            .filter(|d| d.state == "device")
            .map(|d| d.serial.as_str())
            .collect();
        let ids: Vec<String> = ready
            .iter()
            .map(|s| {
                self.adb
                    .serial_number(s)
                    .unwrap_or_else(|_| (*s).to_owned())
            })
            .collect();
        let first = ids.first()?;
        ids.iter()
            .all(|id| id == first)
            .then(|| ready[0].to_owned())
    }

    /// `adb tcpip` on a USB device, then `adb connect` to its Wi-Fi address.
    fn switch_to_wireless(&self, serial: &str) -> Result<String> {
        let out = self
            .adb
            .shell_output(serial, &["ip", "-f", "inet", "addr", "show", "wlan0"])?;
        let ip = match adb::parse_device_ip(&out) {
            Some(ip) => ip,
            None => adb::parse_device_ip(&self.adb.shell_output(serial, &["ip", "route"])?)
                .context("could not find the phone's Wi-Fi address; is Wi-Fi on?")?,
        };
        let addr = format!("{ip}:{}", adb::TCPIP_PORT);
        log::info!("switching {serial} to wireless debugging at {addr}");
        self.ui
            .post(UiEvent::Status(format!("Switching to Wi-Fi ({addr})…")));
        self.adb.tcpip(serial, adb::TCPIP_PORT)?;
        // adbd restarts on the device; give it a moment.
        for attempt in 0..10 {
            if self.shared.sleep(Duration::from_millis(500)) {
                bail!("stopped");
            }
            match self.adb.connect(&addr) {
                Ok(()) => {
                    log::info!("connected over Wi-Fi; you can unplug the USB cable");
                    return Ok(addr);
                }
                Err(e) if attempt == 9 => return Err(e),
                Err(_) => {}
            }
        }
        unreachable!()
    }
}

/// Forwards device messages from the control/events stream to the UI.
fn device_message_handler(ui: Arc<dyn Ui>) -> impl Fn(DeviceMsg) + Send + 'static {
    move |msg| match msg {
        DeviceMsg::Clipboard(text) => ui.post(UiEvent::Clipboard(text)),
        DeviceMsg::CallState { state, number } => {
            let number = (!number.is_empty()).then_some(number);
            ui.post(UiEvent::Call(match state {
                1 => CallState::Ringing { number },
                2 => CallState::Active,
                _ => CallState::Idle,
            }));
        }
        DeviceMsg::ControlUnavailable => {
            log::warn!("the phone can't perform input: its Accessibility service isn't running");
            ui.post(UiEvent::Notice(
                "The phone can't be controlled yet. On the phone, open DroidMirror and follow \
                 \"Allow control\" (Settings → Accessibility → DroidMirror remote control)."
                    .into(),
            ));
        }
        other => log::debug!("device message: {other:?}"),
    }
}

struct Events {
    ui: Arc<dyn Ui>,
    control: ControlSender,
}

impl VideoEvents for Events {
    fn video_size(&mut self, width: u32, height: u32) {
        self.ui.post(UiEvent::VideoSize { width, height });
    }

    fn send_control(&mut self, msg: ControlMsg) {
        self.control.send(msg);
    }
}

#[cfg(test)]
mod tests {
    //! Companion mode end to end: no adb phone, a fake phone app on the
    //! network (plain TCP in place of TLS), pairing, three channels, video,
    //! call state.

    use super::*;
    use crate::companion::{Discovery, PairingStore, Peer, TlsConn, TlsConnector};
    use std::io::Write;
    use std::net::{SocketAddr, TcpListener};
    use std::sync::mpsc;

    const FP: [u8; 32] = [5; 32];

    struct PlainTls;
    impl TlsConnector for PlainTls {
        fn connect(&self, addr: SocketAddr, pin: Option<&[u8; 32]>) -> Result<TlsConn> {
            if pin.is_some_and(|p| *p != FP) {
                bail!("pin mismatch");
            }
            let tcp = TcpStream::connect(addr)?;
            Ok(TlsConn {
                stream: Box::new(tcp.try_clone()?),
                tcp,
                fingerprint: FP,
            })
        }
    }

    struct OnePeer(Peer);
    impl Discovery for OnePeer {
        fn peers(&self) -> Vec<Peer> {
            vec![self.0.clone()]
        }
    }

    struct Collect(Mutex<mpsc::Sender<String>>);
    impl Ui for Collect {
        fn post(&self, e: UiEvent) {
            let s = match e {
                UiEvent::Connected { device_name, .. } => format!("connected {device_name}"),
                UiEvent::VideoSize { width, height } => format!("size {width}x{height}"),
                UiEvent::Call(c) => format!("call {c:?}"),
                UiEvent::PairingProgress(p) => format!("progress {p}"),
                UiEvent::Disconnected => "disconnected".into(),
                _ => return,
            };
            let _ = self.0.lock().unwrap().send(s);
        }
    }

    struct CountSink(Arc<Mutex<usize>>);
    impl VideoSink for CountSink {
        fn configure(&mut self, _: &[u8], _: &[u8]) -> Result<()> {
            Ok(())
        }
        fn ready(&self) -> bool {
            true
        }
        fn enqueue(&mut self, _: &[u8]) -> Result<()> {
            *self.0.lock().unwrap() += 1;
            Ok(())
        }
        fn flush(&mut self) {}
    }

    fn media(config: bool, key: bool, data: &[u8]) -> Vec<u8> {
        let flags: u64 = if config {
            1 << 62
        } else if key {
            1 << 61
        } else {
            0
        };
        [
            flags.to_be_bytes().as_slice(),
            &(data.len() as u32).to_be_bytes(),
            data,
        ]
        .concat()
    }

    /// Fake phone app: approves pairing, then serves one session.
    fn fake_phone() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let token = [3u8; 32];
            for conn in listener.incoming() {
                let mut s = conn.unwrap();
                let mut head = [0u8; 5];
                if s.read_exact(&mut head).is_err() {
                    continue;
                }
                match head[4] {
                    3 => {
                        let mut n = [0u8; 17];
                        s.read_exact(&mut n).unwrap();
                        let mut name = vec![0u8; n[16] as usize];
                        s.read_exact(&mut name).unwrap();
                        let mut reply = vec![0];
                        reply.extend_from_slice(&token);
                        reply.push(5);
                        reply.extend_from_slice(b"ph-01");
                        s.write_all(&reply).unwrap();
                    }
                    ch => {
                        let mut t = [0u8; 32];
                        s.read_exact(&mut t).unwrap();
                        assert_eq!(t, token, "channel opened with the issued token");
                        s.write_all(&[0]).unwrap();
                        thread::spawn(move || match ch {
                            0 => {
                                let mut name = [0u8; 64];
                                name[..7].copy_from_slice(b"RMX3771");
                                let mut v = name.to_vec();
                                v.extend_from_slice(b"h264");
                                v.extend(
                                    [0x8000_0000u32, 456, 1024]
                                        .iter()
                                        .flat_map(|x| x.to_be_bytes()),
                                );
                                v.extend(media(
                                    true,
                                    false,
                                    b"\0\0\0\x01\x67\x42\xc0\x1f\0\0\0\x01\x68\xce\x3c\x80",
                                ));
                                v.extend(media(false, true, b"\0\0\0\x01\x65\x88"));
                                v.extend(media(false, false, b"\0\0\0\x01\x41\x9a"));
                                let _ = s.write_all(&v);
                                thread::sleep(Duration::from_millis(300));
                            }
                            1 => {
                                let mut buf = [0u8; 64];
                                while s.read(&mut buf).is_ok_and(|n| n > 0) {}
                            }
                            _ => {
                                let _ = s.write_all(&[100, 1, 0, 0, 0, 0]); // ringing
                                thread::sleep(Duration::from_millis(500));
                            }
                        });
                    }
                }
            }
        });
        addr
    }

    #[test]
    fn companion_mode_end_to_end() {
        let dir = std::env::temp_dir().join(format!("dm-session-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let adb_path = dir.join("adb");
        std::fs::write(
            &adb_path,
            "#!/bin/sh\n[ \"$1\" = devices ] && printf 'List of devices attached\\n'\n\
             [ \"$1\" = mdns ] && printf 'List of discovered mdns services\\n'\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&adb_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let peer = Peer {
            name: "realme RMX3771".into(),
            id: "ph-01".into(),
            addr: fake_phone(),
        };
        let store = Arc::new(PairingStore::new(dir.join("pairings")));
        let companion = Companion {
            tls: Arc::new(PlainTls),
            discovery: Arc::new(OnePeer(peer)),
            store: store.clone(),
            mac_name: "Test Mac".into(),
        };
        let (tx, rx) = mpsc::channel();
        let frames = Arc::new(Mutex::new(0));
        let config = Config {
            serial: None,
            force_forward: false,
            tcpip: None,
            stats_level: log::Level::Debug,
            call_alerts: false,
        };
        let mut session = Session::start(
            Adb::with_path(adb_path),
            config,
            Box::new(CountSink(frames.clone())),
            Arc::new(Collect(Mutex::new(tx))),
            Some(companion),
        )
        .unwrap();

        let mut seen = Vec::new();
        while let Ok(e) = rx.recv_timeout(Duration::from_secs(5)) {
            let done = e == "disconnected";
            seen.push(e);
            if done {
                break;
            }
        }
        session.stop();

        assert!(
            seen.iter()
                .any(|e| e.starts_with("progress Pairing with the DroidMirror app")),
            "{seen:?}"
        );
        assert!(seen.contains(&"connected RMX3771".to_owned()), "{seen:?}");
        assert!(seen.contains(&"size 456x1024".to_owned()), "{seen:?}");
        assert!(
            seen.iter().any(|e| e.starts_with("call Ringing")),
            "{seen:?}"
        );
        assert_eq!(
            *frames.lock().unwrap(),
            2,
            "key frame and delta frame displayed"
        );
        let record = store.get("ph-01").expect("pairing saved");
        assert_eq!(record.token, [3; 32]);
        assert_eq!(record.fingerprint, FP);
        let _ = std::fs::remove_dir_all(dir);
    }
}
