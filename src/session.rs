//! Connection lifecycle on the video thread: find the device, start the
//! server, stream video until it ends, then reconnect. The UI is told what
//! happens through `Ui::post`; it never blocks on any of this.

use std::net::{Shutdown, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::adb::{self, Adb};
use crate::call::{self, CallState};
use crate::controller::{self, ControlSender};
use crate::pairing::{self, PairingOffer, ServiceKind};
use crate::protocol::control::ControlMsg;
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
            log::warn!("no device connected; not pushing files");
            self.ui.post(UiEvent::Notice("No phone connected".into()));
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
        let serial = self.resolve_device()?;
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

        let ui = self.ui.clone();
        let control =
            controller::start(conn.control, move |text| ui.post(UiEvent::Clipboard(text)))?;
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
