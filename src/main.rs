//! DroidMirror: mirror and control an Android device from macOS, using the
//! official scrcpy-server running on the device.

// The input mapping and parts of the protocol are only used by the macOS
// window; on other platforms (headless mode, tests) they'd warn.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

mod adb;
mod call;
mod companion;
mod controller;
mod h264;
mod input;
mod logger;
#[cfg(target_os = "macos")]
mod macos;
mod pairing;
mod protocol;
mod server;
mod session;
mod sha256;
mod video;

use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::server::SERVER_VERSION;
use crate::session::{Config, Session, Tcpip, Ui, UiEvent};

const USAGE: &str = "\
usage: droidmirror [options]

  -s, --serial SERIAL   device to use when several are connected
      --tcpip[=IP[:PORT]]
                        connect over Wi-Fi: with an address, `adb connect` to
                        it; without, switch the USB device to wireless
                        debugging first (you can then unplug it)
      --forward         use `adb forward` instead of `adb reverse`
      --headless        no window: log video packet rate and bitrate only
      --no-call-alerts  don't watch for incoming calls
  -V, --version         print version
  -h, --help            print this help

shortcuts: Cmd+B Back, Cmd+H Home, Cmd+S Recent apps, right click Back,
           Cmd+C copy on the phone, Cmd+V paste the Mac clipboard,
           Cmd+Return answer a call, Cmd+Delete decline or end it

environment:
  DROIDMIRROR_LOG       error | warn | info | debug | trace (default: info)
  ADB                   path to the adb binary";

pub struct Args {
    pub config: Config,
    pub headless: bool,
}

fn parse_args() -> Result<Option<Args>> {
    let mut config = Config {
        serial: std::env::var("ANDROID_SERIAL")
            .ok()
            .filter(|s| !s.is_empty()),
        force_forward: false,
        tcpip: None,
        stats_level: log::Level::Debug,
        call_alerts: true,
    };
    let mut headless = false;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-s" | "--serial" => config.serial = Some(it.next().context("--serial needs a value")?),
            "--forward" => config.force_forward = true,
            "--tcpip" => config.tcpip = Some(Tcpip::Auto),
            a if a.starts_with("--tcpip=") => {
                config.tcpip = Some(Tcpip::Addr(a["--tcpip=".len()..].to_owned()));
            }
            "--headless" => headless = true,
            "--no-call-alerts" => config.call_alerts = false,
            "-V" | "--version" => {
                println!(
                    "droidmirror {} (scrcpy-server {SERVER_VERSION})",
                    env!("CARGO_PKG_VERSION")
                );
                return Ok(None);
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            // Finder used to pass a process serial number to apps.
            a if a.starts_with("-psn_") => {}
            other => bail!("unknown argument `{other}` (try --help)"),
        }
    }
    if headless {
        config.stats_level = log::Level::Info;
    }
    Ok(Some(Args { config, headless }))
}

fn main() {
    let args = match parse_args() {
        Ok(Some(a)) => a,
        Ok(None) => return,
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::exit(2);
        }
    };
    logger::init();
    log::info!(
        "droidmirror {} using scrcpy-server {SERVER_VERSION}",
        env!("CARGO_PKG_VERSION")
    );
    let result = if args.headless {
        run_headless(args.config)
    } else {
        run_app(args.config)
    };
    if let Err(e) = result {
        log::error!("{e:#}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "macos")]
fn run_app(config: Config) -> Result<()> {
    macos::run(adb::Adb::locate()?, config)
}

#[cfg(not(target_os = "macos"))]
fn run_app(_config: Config) -> Result<()> {
    bail!("the mirroring window needs macOS; use --headless on this platform")
}

/// Logs what a window would show. Exits when the device disconnects.
struct LogUi {
    done: std::sync::Mutex<std::sync::mpsc::Sender<Result<(), String>>>,
}

impl Ui for LogUi {
    fn post(&self, event: UiEvent) {
        let done = |r| {
            let _ = self.done.lock().unwrap().send(r);
        };
        match event {
            UiEvent::Status(s) => log::info!("status: {s}"),
            UiEvent::Connected { device_name, .. } => log::info!("mirroring \"{device_name}\""),
            UiEvent::VideoSize { .. } => {}
            UiEvent::Disconnected => done(Ok(())),
            UiEvent::Clipboard(t) => log::info!("device clipboard: {} bytes", t.len()),
            UiEvent::Fatal(e) => done(Err(e)),
            UiEvent::Notice(n) => log::info!("{n}"),
            UiEvent::Call(state) => log::info!("call: {state:?}"),
            UiEvent::Pairing { .. } => log::info!(
                "no phone found. Turn on Developer options → Wireless debugging, then either \
                 run DroidMirror with a window and scan its QR code, or pair with a code: \
                 `adb pair IP:PORT CODE` (Wireless debugging → Pair device with pairing code)"
            ),
            UiEvent::PairingProgress(p) => log::info!("{p}"),
        }
    }
}

fn run_headless(config: Config) -> Result<()> {
    let adb = adb::Adb::locate()?;
    let (tx, rx) = std::sync::mpsc::channel();
    let ui = Arc::new(LogUi {
        done: std::sync::Mutex::new(tx),
    });
    let mut session = Session::start(adb, config, Box::new(video::NullSink), ui, None)?;
    let result = rx.recv().unwrap_or(Ok(()));
    session.stop();
    session.join();
    result.map_err(anyhow::Error::msg)
}
