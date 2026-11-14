//! Minimal logger for the `log` facade.
//!
//! Level comes from `DROIDMIRROR_LOG` (error, warn, info, debug, trace, off);
//! the default is `info`. Each line carries seconds since startup.
//!
//! Lines go to stderr. When stderr isn't a terminal (the app was opened from
//! Finder), they also go to `~/Library/Logs/DroidMirror/droidmirror.log`, so
//! users can attach a log to bug reports. The previous run's log is kept as
//! `droidmirror.old.log`.

use std::fs::File;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use log::{LevelFilter, Log, Metadata, Record};

struct Logger {
    start: Instant,
    file: Mutex<Option<File>>,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// Where the log file lives (whether or not it is being written).
pub fn log_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Logs/DroidMirror/droidmirror.log"))
}

fn open_log_file() -> Option<File> {
    let path = log_path()?;
    std::fs::create_dir_all(path.parent()?).ok()?;
    let _ = std::fs::rename(&path, path.with_file_name("droidmirror.old.log"));
    File::create(path).ok()
}

pub fn init() {
    let level = std::env::var("DROIDMIRROR_LOG")
        .ok()
        .and_then(|v| parse_level(&v))
        .unwrap_or(LevelFilter::Info);
    let logger = LOGGER.get_or_init(|| Logger {
        start: Instant::now(),
        file: Mutex::new(if std::io::stderr().is_terminal() {
            None
        } else {
            open_log_file()
        }),
    });
    // Fails only if a logger is already installed; keeping that one is fine.
    let _ = log::set_logger(logger);
    log::set_max_level(level);
}

fn parse_level(s: &str) -> Option<LevelFilter> {
    s.trim().parse().ok()
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let t = self.start.elapsed();
        // One write per line so lines from different threads don't interleave.
        let line = format!(
            "[{:>4}.{:03} {:<5} {}] {}\n",
            t.as_secs(),
            t.subsec_millis(),
            record.level(),
            record.target(),
            record.args()
        );
        let _ = std::io::stderr().lock().write_all(line.as_bytes());
        if let Ok(mut file) = self.file.lock()
            && let Some(f) = file.as_mut()
        {
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
        if let Ok(mut file) = self.file.lock()
            && let Some(f) = file.as_mut()
        {
            let _ = f.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_levels_case_insensitively() {
        assert_eq!(parse_level("debug"), Some(LevelFilter::Debug));
        assert_eq!(parse_level(" WARN "), Some(LevelFilter::Warn));
        assert_eq!(parse_level("off"), Some(LevelFilter::Off));
        assert_eq!(parse_level("loud"), None);
    }

    #[test]
    fn log_path_is_under_library_logs() {
        if let Some(p) = log_path() {
            assert!(p.ends_with("Library/Logs/DroidMirror/droidmirror.log"));
        }
    }
}
