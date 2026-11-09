//! Thin wrapper around the `adb` command-line tool.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result, bail};

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error(
        "no Android device found. Connect the phone by USB and enable USB debugging (see README)."
    )]
    NoDevice,
    #[error(
        "device {0} is unauthorized. Unlock the phone and accept the \"Allow USB debugging?\" prompt, \
         then try again (unplug and replug if no prompt appears)."
    )]
    Unauthorized(String),
    #[error("device {0} is offline. Unplug and replug it, or run `adb kill-server`.")]
    Offline(String),
    #[error("device {serial} is not usable (state: {state})")]
    BadState { serial: String, state: String },
    #[error("several devices are connected ({0}). Pick one with --serial SERIAL.")]
    Ambiguous(String),
    #[error("device {0} is not connected")]
    NotFound(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceEntry {
    pub serial: String,
    pub state: String,
}

/// Parses `adb devices` output.
pub fn parse_devices(output: &str) -> Vec<DeviceEntry> {
    output
        .lines()
        .filter(|l| !l.starts_with("List of devices") && !l.starts_with('*'))
        .filter_map(|l| {
            let mut parts = l.split('\t');
            let serial = parts.next()?.trim();
            let state = parts.next()?.trim();
            (!serial.is_empty()).then(|| DeviceEntry {
                serial: serial.to_owned(),
                state: state.to_owned(),
            })
        })
        .collect()
}

/// Picks the device to use, with an actionable error when none fits.
pub fn select_device(devices: &[DeviceEntry], wanted: Option<&str>) -> Result<String, DeviceError> {
    let check = |d: &DeviceEntry| match d.state.as_str() {
        "device" => Ok(d.serial.clone()),
        "unauthorized" => Err(DeviceError::Unauthorized(d.serial.clone())),
        "offline" => Err(DeviceError::Offline(d.serial.clone())),
        other => Err(DeviceError::BadState {
            serial: d.serial.clone(),
            state: other.to_owned(),
        }),
    };

    if let Some(serial) = wanted {
        return devices
            .iter()
            .find(|d| d.serial == serial)
            .ok_or_else(|| DeviceError::NotFound(serial.to_owned()))
            .and_then(check);
    }

    match devices {
        [] => Err(DeviceError::NoDevice),
        [only] => check(only),
        many => {
            let ready: Vec<_> = many.iter().filter(|d| d.state == "device").collect();
            match ready.as_slice() {
                [one] => Ok(one.serial.clone()),
                [] => check(&many[0]),
                _ => Err(DeviceError::Ambiguous(
                    ready
                        .iter()
                        .map(|d| d.serial.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                )),
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Adb {
    exe: PathBuf,
}

impl Adb {
    /// Finds adb: `$ADB`, then `PATH`, then the usual install locations
    /// (an app launched from Finder doesn't inherit the shell's `PATH`).
    pub fn locate() -> Result<Self> {
        if let Some(p) = std::env::var_os("ADB").filter(|p| !p.is_empty()) {
            return Ok(Self { exe: p.into() });
        }
        let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).map(|d| d.join("adb")).collect())
            .unwrap_or_default();
        candidates.push("/opt/homebrew/bin/adb".into());
        candidates.push("/usr/local/bin/adb".into());
        if let Some(home) = std::env::var_os("HOME") {
            candidates.push(Path::new(&home).join("Library/Android/sdk/platform-tools/adb"));
        }
        candidates
            .into_iter()
            .find(|p| p.is_file())
            .map(|exe| Self { exe })
            .context("adb not found. Install it with `brew install android-platform-tools`, or set ADB=/path/to/adb")
    }

    fn cmd(&self, serial: Option<&str>) -> Command {
        let mut c = Command::new(&self.exe);
        if let Some(s) = serial {
            c.args(["-s", s]);
        }
        c.stdin(Stdio::null());
        c
    }

    /// Runs adb to completion, returning stdout, or an error with stderr.
    fn run(&self, serial: Option<&str>, args: &[&str]) -> Result<String> {
        let out = self
            .cmd(serial)
            .args(args)
            .output()
            .with_context(|| format!("failed to run {}", self.exe.display()))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let msg = if err.trim().is_empty() {
                String::from_utf8_lossy(&out.stdout).into_owned()
            } else {
                err.into_owned()
            };
            bail!("`adb {}` failed: {}", args.join(" "), msg.trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub fn devices(&self) -> Result<Vec<DeviceEntry>> {
        Ok(parse_devices(&self.run(None, &["devices"])?))
    }

    pub fn push(&self, serial: &str, local: &Path, remote: &str) -> Result<()> {
        let local = local.to_str().context("non-UTF-8 path")?;
        self.run(Some(serial), &["push", local, remote]).map(drop)
    }

    /// Device connects to `localabstract:<socket>`, which reaches our TCP port.
    pub fn reverse(&self, serial: &str, socket: &str, port: u16) -> Result<()> {
        let remote = format!("localabstract:{socket}");
        let local = format!("tcp:{port}");
        self.run(Some(serial), &["reverse", &remote, &local])
            .map(drop)
    }

    pub fn reverse_remove(&self, serial: &str, socket: &str) -> Result<()> {
        let remote = format!("localabstract:{socket}");
        self.run(Some(serial), &["reverse", "--remove", &remote])
            .map(drop)
    }

    /// We connect to our TCP port, which reaches the device's abstract socket.
    pub fn forward(&self, serial: &str, port: u16, socket: &str) -> Result<()> {
        let local = format!("tcp:{port}");
        let remote = format!("localabstract:{socket}");
        self.run(Some(serial), &["forward", &local, &remote])
            .map(drop)
    }

    pub fn forward_remove(&self, serial: &str, port: u16) -> Result<()> {
        let local = format!("tcp:{port}");
        self.run(Some(serial), &["forward", "--remove", &local])
            .map(drop)
    }

    /// Starts `adb shell <args>` with stdout and stderr piped.
    pub fn spawn_shell(&self, serial: &str, args: &[String]) -> Result<Child> {
        self.cmd(Some(serial))
            .arg("shell")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to run {}", self.exe.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUT: &str = "* daemon not running; starting now at tcp:5037\n\
                       * daemon started successfully\n\
                       List of devices attached\n\
                       ABC123\tdevice\n\
                       192.168.1.20:5555\tunauthorized\n\n";

    fn dev(serial: &str, state: &str) -> DeviceEntry {
        DeviceEntry {
            serial: serial.into(),
            state: state.into(),
        }
    }

    #[test]
    fn parses_device_list() {
        assert_eq!(
            parse_devices(OUT),
            vec![
                dev("ABC123", "device"),
                dev("192.168.1.20:5555", "unauthorized")
            ]
        );
        assert!(parse_devices("List of devices attached\n\n").is_empty());
    }

    #[test]
    fn selects_the_only_ready_device() {
        assert_eq!(select_device(&parse_devices(OUT), None).unwrap(), "ABC123");
    }

    #[test]
    fn reports_device_problems() {
        assert!(matches!(
            select_device(&[], None),
            Err(DeviceError::NoDevice)
        ));
        assert!(matches!(
            select_device(&[dev("X", "unauthorized")], None),
            Err(DeviceError::Unauthorized(s)) if s == "X"
        ));
        assert!(matches!(
            select_device(&[dev("X", "offline")], None),
            Err(DeviceError::Offline(_))
        ));
        assert!(matches!(
            select_device(&[dev("X", "no permissions")], None),
            Err(DeviceError::BadState { .. })
        ));
        assert!(matches!(
            select_device(&[dev("A", "device"), dev("B", "device")], None),
            Err(DeviceError::Ambiguous(_))
        ));
    }

    #[test]
    fn honours_requested_serial() {
        let list = [dev("A", "device"), dev("B", "device")];
        assert_eq!(select_device(&list, Some("B")).unwrap(), "B");
        assert!(matches!(
            select_device(&list, Some("C")),
            Err(DeviceError::NotFound(_))
        ));
        assert!(matches!(
            select_device(&[dev("A", "unauthorized")], Some("A")),
            Err(DeviceError::Unauthorized(_))
        ));
    }
}
