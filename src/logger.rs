//! Minimal stderr logger for the `log` facade.
//!
//! Level comes from `DROIDMIRROR_LOG` (error, warn, info, debug, trace, off);
//! the default is `info`. Each line carries seconds since startup.

use std::io::Write;
use std::sync::OnceLock;
use std::time::Instant;

use log::{LevelFilter, Log, Metadata, Record};

struct Logger {
    start: Instant,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

pub fn init() {
    let level = std::env::var("DROIDMIRROR_LOG")
        .ok()
        .and_then(|v| parse_level(&v))
        .unwrap_or(LevelFilter::Info);
    let logger = LOGGER.get_or_init(|| Logger {
        start: Instant::now(),
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
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
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
}
