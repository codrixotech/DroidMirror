//! Starting scrcpy-server on the device and opening its sockets.
//!
//! Mirrors `app/src/server.c` and `app/src/adb/adb_tunnel.c` from scrcpy
//! v5.0.1: push the jar, open an adb tunnel (reverse by default, forward as
//! a fallback), run `app_process`, then open the video and control sockets in
//! that order and read the device name from the first one.

use std::hash::{BuildHasher, Hasher, RandomState};
use std::io::{BufRead, BufReader, Read};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};

use crate::adb::Adb;
use crate::protocol::{DEVICE_NAME_LEN, parse_device_name};

/// scrcpy-server version this client speaks, from `scrcpy-server.lock`
/// (verified by build.rs). `Options.parse` on the device throws if the first
/// argument differs from its own version.
pub const SERVER_VERSION: &str = env!("SCRCPY_SERVER_VERSION");

/// The server jar, embedded so the binary is self-contained.
static SERVER_JAR: &[u8] = include_bytes!(env!("SCRCPY_SERVER_PATH"));

/// Same path as scrcpy (`SC_DEVICE_SERVER_PATH`). The server deletes it at
/// startup (`CleanUp.unlinkSelf`), so it is pushed on every start.
const DEVICE_SERVER_PATH: &str = "/data/local/tmp/scrcpy-server.jar";

/// scrcpy's default local port range (`DEFAULT_LOCAL_PORT_RANGE_*`).
const PORTS: std::ops::RangeInclusive<u16> = 27183..=27199;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct Params {
    pub scid: u32,
    pub max_size: u32,
    pub video_bit_rate: u32,
    pub max_fps: u32,
    pub tunnel_forward: bool,
}

impl Params {
    /// The settings DroidMirror always uses, with a fresh scid.
    pub fn new(tunnel_forward: bool) -> Self {
        Self {
            scid: random_scid(),
            max_size: 1024,
            video_bit_rate: 3_000_000,
            max_fps: 30,
            tunnel_forward,
        }
    }

    /// `scrcpy_%08x`, as in `DesktopConnection.getSocketName`.
    pub fn socket_name(&self) -> String {
        format!("scrcpy_{:08x}", self.scid)
    }

    /// Arguments after `adb shell`. Every key below is checked against
    /// `Options.parse` in the v5.0.1 server. Note `scid` is parsed as hex.
    pub fn shell_args(&self) -> Vec<String> {
        let mut a = vec![
            format!("CLASSPATH={DEVICE_SERVER_PATH}"),
            "app_process".into(),
            "/".into(), // unused by the server
            "com.genymobile.scrcpy.Server".into(),
            SERVER_VERSION.into(),
            format!("scid={:08x}", self.scid),
            "log_level=info".into(),
            "video_codec=h264".into(),
            format!("max_size={}", self.max_size),
            format!("video_bit_rate={}", self.video_bit_rate),
            format!("max_fps={}", self.max_fps),
            "audio=false".into(),
            "control=true".into(),
        ];
        if self.tunnel_forward {
            a.push("tunnel_forward=true".into());
        }
        a
    }
}

/// Random 31-bit id so concurrent clients on one device don't collide.
/// `RandomState` is randomly seeded per process, which is enough here.
fn random_scid() -> u32 {
    let mut h = RandomState::new().build_hasher();
    h.write_u32(std::process::id());
    (h.finish() as u32) & 0x7fff_ffff
}

/// The running server process (`adb shell app_process ...`).
/// Killing it on drop ends the server, which closes its sockets.
pub struct Server {
    child: Child,
}

impl Server {
    pub fn try_exited(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct Connection {
    pub device_name: String,
    /// Positioned just after the device name: the codec id comes next.
    pub video: TcpStream,
    pub control: TcpStream,
}

enum Tunnel {
    Reverse(TcpListener),
    Forward(u16),
}

/// Pushes and starts the server, then completes the socket handshake.
pub fn start(adb: &Adb, serial: &str, mut params: Params) -> Result<(Server, Connection)> {
    let jar = write_jar()?;
    adb.push(serial, &jar, DEVICE_SERVER_PATH)
        .context("could not push scrcpy-server to the device")?;
    log::debug!("pushed {} to {DEVICE_SERVER_PATH}", jar.display());

    let socket = params.socket_name();
    let tunnel = open_tunnel(adb, serial, &mut params)?;

    let result = (|| {
        let mut server = spawn(adb, serial, &params)?;
        let conn = match &tunnel {
            Tunnel::Reverse(listener) => accept_sockets(listener, &mut server)?,
            Tunnel::Forward(port) => connect_sockets(*port, &mut server)?,
        };
        Ok((server, conn))
    })();

    // The tunnel is only needed to open the sockets.
    let removed = match tunnel {
        Tunnel::Reverse(_) => adb.reverse_remove(serial, &socket),
        Tunnel::Forward(port) => adb.forward_remove(serial, port),
    };
    if let Err(e) = removed {
        log::warn!("could not remove adb tunnel: {e:#}");
    }
    result
}

/// Writes the embedded jar to a temp file for `adb push` (skipped when an
/// identical copy is already there).
fn write_jar() -> Result<PathBuf> {
    let path =
        std::env::temp_dir().join(format!("droidmirror-scrcpy-server-v{SERVER_VERSION}.jar"));
    let up_to_date = std::fs::read(&path).is_ok_and(|b| b == SERVER_JAR);
    if !up_to_date {
        std::fs::write(&path, SERVER_JAR)
            .with_context(|| format!("could not write {}", path.display()))?;
    }
    Ok(path)
}

fn open_tunnel(adb: &Adb, serial: &str, params: &mut Params) -> Result<Tunnel> {
    let socket = params.socket_name();
    if !params.tunnel_forward {
        match listen_reverse(adb, serial, &socket) {
            Ok(listener) => return Ok(Tunnel::Reverse(listener)),
            Err(e) => {
                log::warn!("adb reverse failed ({e:#}), falling back to adb forward");
                params.tunnel_forward = true;
            }
        }
    }
    for port in PORTS {
        match adb.forward(serial, port, &socket) {
            Ok(()) => {
                log::debug!("adb forward tcp:{port} -> {socket}");
                return Ok(Tunnel::Forward(port));
            }
            Err(e) => log::debug!("adb forward on port {port} failed: {e:#}"),
        }
    }
    bail!("could not open an adb tunnel on any port in {PORTS:?}")
}

fn listen_reverse(adb: &Adb, serial: &str, socket: &str) -> Result<TcpListener> {
    for port in PORTS {
        let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, port)) else {
            log::debug!("port {port} busy, trying the next one");
            continue;
        };
        // If the command itself fails it will fail on every port: give up.
        adb.reverse(serial, socket, port)?;
        log::debug!("adb reverse {socket} -> tcp:{port}");
        return Ok(listener);
    }
    bail!("no free local port in {PORTS:?}")
}

fn spawn(adb: &Adb, serial: &str, params: &Params) -> Result<Server> {
    let args = params.shell_args();
    log::debug!("starting server: adb shell {}", args.join(" "));
    let mut child = adb.spawn_shell(serial, &args)?;
    if let Some(out) = child.stdout.take() {
        forward_log(out, "stdout");
    }
    if let Some(err) = child.stderr.take() {
        forward_log(err, "stderr");
    }
    Ok(Server { child })
}

/// Relays the server's output (`[server] INFO: ...`) into our log.
fn forward_log(pipe: impl Read + Send + 'static, name: &str) {
    let spawned = thread::Builder::new()
        .name(format!("server-{name}"))
        .spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                let line = line.strip_prefix("[server] ").unwrap_or(&line);
                if let Some(msg) = line.strip_prefix("ERROR: ") {
                    log::error!(target: "server", "{msg}");
                } else if let Some(msg) = line.strip_prefix("WARN: ") {
                    log::warn!(target: "server", "{msg}");
                } else if let Some(msg) = line.strip_prefix("INFO: ") {
                    log::info!(target: "server", "{msg}");
                } else if let Some(msg) = line
                    .strip_prefix("DEBUG: ")
                    .or(line.strip_prefix("VERBOSE: "))
                {
                    log::debug!(target: "server", "{msg}");
                } else if !line.trim().is_empty() {
                    log::info!(target: "server", "{line}");
                }
            }
        });
    if let Err(e) = spawned {
        log::warn!("could not start server log thread: {e}");
    }
}

fn server_died(server: &mut Server) -> Option<anyhow::Error> {
    server.try_exited().map(|status| {
        anyhow!("scrcpy-server exited during startup ({status}); see the server log above")
    })
}

/// Reverse tunnel: the device connects to us, video first, then control.
fn accept_sockets(listener: &TcpListener, server: &mut Server) -> Result<Connection> {
    // Non-blocking accept so a dead server or a timeout can't hang us.
    // This short wait loop only runs during startup.
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let mut accept = |what: &str| -> Result<TcpStream> {
        loop {
            match listener.accept() {
                Ok((s, _)) => {
                    // Accepted sockets inherit O_NONBLOCK on macOS.
                    s.set_nonblocking(false)?;
                    return Ok(s);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if let Some(e) = server_died(server) {
                        return Err(e);
                    }
                    if Instant::now() > deadline {
                        bail!("timed out waiting for the {what} connection from the device");
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(e) => return Err(e).context("accept failed"),
            }
        }
    };
    let mut video = accept("video")?;
    let control = accept("control")?;
    finish(&mut video, &control)?;
    Ok(Connection {
        device_name: read_device_name(&mut video)?,
        video,
        control,
    })
}

/// Forward tunnel: we connect; the first socket gets a dummy byte once the
/// server is really listening (connecting succeeds as soon as adb listens).
fn connect_sockets(port: u16, server: &mut Server) -> Result<Connection> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let mut video = loop {
        if let Ok(mut s) = TcpStream::connect(addr) {
            let mut dummy = [0u8; 1];
            if s.read_exact(&mut dummy).is_ok() {
                break s;
            }
        }
        if let Some(e) = server_died(server) {
            return Err(e);
        }
        if Instant::now() > deadline {
            bail!("timed out connecting to the server through adb forward");
        }
        thread::sleep(Duration::from_millis(100));
    };
    let control = TcpStream::connect(addr).context("could not open the control socket")?;
    finish(&mut video, &control)?;
    Ok(Connection {
        device_name: read_device_name(&mut video)?,
        video,
        control,
    })
}

fn finish(video: &mut TcpStream, control: &TcpStream) -> Result<()> {
    // Control messages are tiny and latency-sensitive.
    control.set_nodelay(true)?;
    // Bound the wait for the device name; cleared afterwards since the video
    // socket legitimately goes quiet when the screen doesn't change.
    video.set_read_timeout(Some(CONNECT_TIMEOUT))?;
    Ok(())
}

fn read_device_name(video: &mut TcpStream) -> Result<String> {
    let mut field = [0u8; DEVICE_NAME_LEN];
    video
        .read_exact(&mut field)
        .context("server closed the connection before sending the device name")?;
    video.set_read_timeout(None)?;
    Ok(parse_device_name(&field))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_args_match_server_options() {
        let p = Params {
            scid: 0x0012_abcd,
            max_size: 1024,
            video_bit_rate: 3_000_000,
            max_fps: 30,
            tunnel_forward: false,
        };
        assert_eq!(
            p.shell_args(),
            [
                "CLASSPATH=/data/local/tmp/scrcpy-server.jar",
                "app_process",
                "/",
                "com.genymobile.scrcpy.Server",
                SERVER_VERSION,
                "scid=0012abcd",
                "log_level=info",
                "video_codec=h264",
                "max_size=1024",
                "video_bit_rate=3000000",
                "max_fps=30",
                "audio=false",
                "control=true",
            ]
        );
        assert_eq!(p.socket_name(), "scrcpy_0012abcd");

        let fwd = Params {
            tunnel_forward: true,
            ..p
        };
        assert_eq!(fwd.shell_args().last().unwrap(), "tunnel_forward=true");
    }

    #[test]
    fn scid_is_31_bit() {
        for _ in 0..100 {
            assert!(random_scid() <= 0x7fff_ffff);
        }
    }

    #[test]
    fn embedded_jar_is_present() {
        // Zip local file header.
        assert_eq!(&SERVER_JAR[..4], b"PK\x03\x04");
    }
}
