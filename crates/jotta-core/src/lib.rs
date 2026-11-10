//! jotta-core: the engine layer of Jotta Desktop.
//!
//! Zero external dependencies on purpose:
//! - compiles/link anywhere (also inside a Flatpak builder sandbox),
//! - fully unit-testable without a running jottad,
//! - the GUI (gtk4-rs) and tray (ksni) crates are thin layers on top.
//!
//! Modules:
//! - `json`: minimal JSON parser/serializer (jotta-cli emits `--json`).
//! - `cli`: typed wrapper around the `jotta-cli` binary (Model B: we ship it).
//! - `daemon`: supervision of the bundled `jottad` daemon (start/stop/wait).
//! - `status`: tolerant extraction of status information from JSON.
//! - `remote`: tolerant parsers for `jotta-cli ls`/`list downloads`/`trash ls`.
//! - `login`: prompt state machine for graphical `jotta-cli login` (pure data).

pub mod cli;
pub mod daemon;
pub mod error;
pub mod json;
pub mod login;
pub mod observe;
pub mod remote;
pub mod status;
pub mod synclog;
pub mod textstatus;

pub use error::{Error, Result};

// ---------------------------------------------------------------------------
// Minimal verbose logging (std-only), following common CLI conventions:
// `-v`/`--verbose` (repeatable), `--debug`, `-q`/`--quiet`, or the env var
// `JOTTA_DESKTOP_VERBOSE` ("1"/"true" = 1, "debug" = 2, "trace" = 3).
// Level 0 = silent (default). Logs go to stderr with a UTC timestamp, so
// `cargo run -p jotta-gui -- --verbose 2>jotta.log` captures everything.
// ---------------------------------------------------------------------------

use std::sync::atomic::{AtomicU8, Ordering};

static VERBOSE: AtomicU8 = AtomicU8::new(0);

/// Set the global verbosity level (clamped to 0..=3).
pub fn set_verbose(level: u8) {
    VERBOSE.store(level.min(3), Ordering::Relaxed);
}

/// Current global verbosity level.
pub fn verbose() -> u8 {
    VERBOSE.load(Ordering::Relaxed)
}

/// Parse a verbosity value from a CLI flag or env var
/// ("1"/"true"/"yes" -> 1, "debug" -> 2, "trace" -> 3, else the number).
pub fn parse_verbose(s: &str) -> Option<u8> {
    let t = s.trim().to_ascii_lowercase();
    match t.as_str() {
        "1" | "true" | "yes" | "on" | "v" | "verbose" => Some(1),
        "2" | "debug" => Some(2),
        "3" | "trace" => Some(3),
        _ => t.parse::<u8>().ok().filter(|&n| n > 0).map(|n| n.min(3)),
    }
}

/// Current UTC time as `HH:MM:SS` (std-only, no chrono).
pub fn now_utc_hms() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    // civil_from_days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let _ = (y, m, d); // date not needed for the clock
    format!("{:02}:{:02}:{:02}", rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// Log a message at the given verbosity level (no-op when below the global
/// level). Usage: `jotta_core::vlog!(2, "spawned {:?}", argv);`
#[macro_export]
macro_rules! vlog {
    ($lvl:expr, $($arg:tt)*) => {
        if $crate::verbose() >= $lvl {
            eprintln!("[jotta {} L{}] {}", $crate::now_utc_hms(), $lvl, format!($($arg)*));
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbose_parse() {
        assert_eq!(parse_verbose("1"), Some(1));
        assert_eq!(parse_verbose("true"), Some(1));
        assert_eq!(parse_verbose("VERBOSE"), Some(1));
        assert_eq!(parse_verbose("debug"), Some(2));
        assert_eq!(parse_verbose("2"), Some(2));
        assert_eq!(parse_verbose("trace"), Some(3));
        assert_eq!(parse_verbose("3"), Some(3));
        assert_eq!(parse_verbose("9"), Some(3)); // clamped
        assert_eq!(parse_verbose("0"), None);
        assert_eq!(parse_verbose("no"), None);
        assert_eq!(parse_verbose(""), None);
    }

    #[test]
    fn verbose_set_get() {
        set_verbose(2);
        assert_eq!(verbose(), 2);
        set_verbose(9);
        assert_eq!(verbose(), 3);
        set_verbose(0);
        assert_eq!(verbose(), 0);
    }

    #[test]
    fn utc_clock_format() {
        let t = now_utc_hms();
        assert_eq!(t.len(), 8);
        assert!(t.chars().enumerate().all(|(i, c)| {
            if i == 2 || i == 5 { c == ':' } else { c.is_ascii_digit() }
        }));
        // 1970-01-01 00:00:00 UTC boundary sanity: epoch -> 00:00:00
        let secs = 0u64;
        assert_eq!(format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60), "00:00:00");
    }
}
