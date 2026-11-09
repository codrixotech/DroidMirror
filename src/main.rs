//! DroidMirror: mirror and control an Android device from macOS, using the
//! official scrcpy-server running on the device.

mod adb;
mod logger;
mod protocol;
mod server;

use std::io::Read;
use std::net::TcpStream;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::protocol::video::{Event, VideoStream};
use crate::server::SERVER_VERSION;

const USAGE: &str = "\
usage: droidmirror [--serial SERIAL] [--forward]

  -s, --serial SERIAL   device to use when several are connected
      --forward         use `adb forward` instead of `adb reverse`
  -V, --version         print version
  -h, --help            print this help

environment:
  DROIDMIRROR_LOG       error | warn | info | debug | trace (default: info)
  ADB                   path to the adb binary";

struct Args {
    serial: Option<String>,
    force_forward: bool,
}

fn parse_args() -> Result<Option<Args>> {
    let mut args = Args {
        serial: std::env::var("ANDROID_SERIAL")
            .ok()
            .filter(|s| !s.is_empty()),
        force_forward: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-s" | "--serial" => args.serial = Some(it.next().context("--serial needs a value")?),
            "--forward" => args.force_forward = true,
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
            other => bail!("unknown argument `{other}` (try --help)"),
        }
    }
    Ok(Some(args))
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
    if let Err(e) = run(args) {
        log::error!("{e:#}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<()> {
    log::info!(
        "droidmirror {} using scrcpy-server {SERVER_VERSION}",
        env!("CARGO_PKG_VERSION")
    );

    let adb = adb::Adb::locate()?;
    let serial = adb::select_device(&adb.devices()?, args.serial.as_deref())?;
    log::info!("using device {serial}");

    let (_server, conn) = server::start(&adb, &serial, server::Params::new(args.force_forward))?;
    log::info!("connected to \"{}\"", conn.device_name);

    spawn_control_drain(conn.control)?;
    let video = thread::Builder::new()
        .name("video".into())
        .spawn(move || read_video(conn.video))?;

    match video.join() {
        Ok(result) => result,
        Err(_) => bail!("video thread panicked"),
    }
    // `_server` drops here, stopping the server process.
}

/// Milestone 2: read packets as fast as they arrive and log throughput.
fn read_video(socket: TcpStream) -> Result<()> {
    let mut stream = VideoStream::new(socket);
    stream.read_codec()?;
    log::info!("video codec: h264");

    let mut stats = Stats::new();
    while let Some(event) = stream.next_event()? {
        match event {
            Event::Session { width, height } => log::info!("video session: {width}x{height}"),
            Event::Packet(p) => {
                if p.config {
                    log::info!("config packet ({} bytes)", p.data.len());
                }
                stats.add(p.data.len(), p.key_frame);
            }
        }
        stats.report_if_due();
    }
    log::info!("video stream ended (device disconnected or server stopped)");
    Ok(())
}

struct Stats {
    since: Instant,
    packets: u64,
    bytes: u64,
    key_frames: u64,
    total_packets: u64,
}

impl Stats {
    const PERIOD: Duration = Duration::from_secs(1);

    fn new() -> Self {
        Self {
            since: Instant::now(),
            packets: 0,
            bytes: 0,
            key_frames: 0,
            total_packets: 0,
        }
    }

    fn add(&mut self, len: usize, key_frame: bool) {
        self.packets += 1;
        self.total_packets += 1;
        self.bytes += len as u64;
        self.key_frames += key_frame as u64;
    }

    /// Checked on each packet rather than by a timer.
    fn report_if_due(&mut self) {
        let elapsed = self.since.elapsed();
        if elapsed < Self::PERIOD {
            return;
        }
        let secs = elapsed.as_secs_f64();
        log::info!(
            "{:5.1} pkt/s  {:6.0} kbit/s  {} key  ({} total)",
            self.packets as f64 / secs,
            self.bytes as f64 * 8.0 / 1000.0 / secs,
            self.key_frames,
            self.total_packets
        );
        self.since = Instant::now();
        self.packets = 0;
        self.bytes = 0;
        self.key_frames = 0;
    }
}

/// The device sends clipboard and other messages on the control socket.
/// They must be read even if unused, or the device side blocks.
fn spawn_control_drain(mut socket: TcpStream) -> Result<()> {
    thread::Builder::new()
        .name("control-read".into())
        .spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match socket.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => log::debug!("discarded {n} bytes of device messages"),
                }
            }
        })?;
    Ok(())
}
