//! Parser for `jotta-cli sync log --watch` — the only CLI source that NAMES
//! the file currently being synced (the `observe` TUI shows it too, but has no
//! machine-readable output on jotta-cli 0.17.x).
//!
//! Verified line format (lorentzfb/omarchy-jottacloud, a working widget):
//! ```text
//! 2026-01-20 20:10:22.566 +0100 CET :: Upload /hello.txt
//! 2026-01-20 20:10:22.6   +0100 CET :: Download /docs/a.pdf
//! 2026-01-20 20:10:23    +0100 CET :: DeleteFile /old.txt
//! 2026-01-20 20:10:24    +0100 CET :: Finished
//! ```
//! Lines may carry terminal escape prefixes, so matching is unanchored on `::`.

/// What kind of sync event a log line describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncLogAction {
    Upload,
    Download,
    Delete,
    Other,
}

/// One parsed `sync log --watch` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncLogEvent {
    /// Event timestamp in ms since Unix epoch (0 if the line had no parseable
    /// timestamp — the action/path are still usable).
    pub ts_ms: u64,
    pub action: SyncLogAction,
    /// Path after the action word ("" for e.g. `Finished`).
    pub path: String,
}

/// Does `jotta-cli sync --help` output advertise a `log` subcommand?
pub fn sync_help_lists_log(help: &str) -> bool {
    help.lines().any(|l| {
        let l = l.trim();
        l.starts_with("log") || l.contains(" log ") || l.ends_with(" log")
    })
}

/// Days from 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// civil-from-days algorithm, inverted). Zero-dep.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (m as i64 + 9) % 12; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Parse the leading `<date> <time>[.frac] <±hhmm>` timestamp of a log line
/// into ms since Unix epoch (UTC). Returns 0 when nothing parseable is found.
fn parse_ts_ms(head: &str) -> u64 {
    let toks: Vec<&str> = head.split_whitespace().collect();
    let mut date: Option<(i64, u32, u32)> = None;
    let mut time: Option<(u32, u32, u32, u32)> = None; // h, m, s, ms
    let mut offset_min: i64 = 0;
    for t in &toks {
        let b = t.as_bytes();
        if date.is_none()
            && b.len() == 10
            && b[4] == b'-'
            && b[7] == b'-'
            && b.iter().enumerate().all(|(i, &c)| i == 4 || i == 7 || c.is_ascii_digit())
        {
            if let (Ok(y), Ok(m), Ok(d)) = (
                t[0..4].parse::<i64>(),
                t[5..7].parse::<u32>(),
                t[8..10].parse::<u32>(),
            ) {
                date = Some((y, m, d));
                continue;
            }
        }
        if time.is_none() && t.contains(':') && b.iter().filter(|&&c| c == b':').count() == 2 {
            let parts: Vec<&str> = t.split(':').collect();
            if parts.len() == 3 {
                let (mut sec, mut ms) = (0u32, 0u32);
                let sp = parts[2];
                if let Some(dot) = sp.find('.') {
                    if let Ok(s) = sp[..dot].parse::<u32>() {
                        sec = s;
                    }
                    let frac = &sp[dot + 1..];
                    if !frac.is_empty() && frac.chars().all(|c| c.is_ascii_digit()) {
                        ms = frac
                            .chars()
                            .take(3)
                            .collect::<String>()
                            .parse::<u32>()
                            .unwrap_or(0);
                        // scale 1-2 digit fractions to ms
                        if frac.len() == 1 {
                            ms *= 100;
                        } else if frac.len() == 2 {
                            ms *= 10;
                        }
                    }
                } else if let Ok(s) = sp.parse::<u32>() {
                    sec = s;
                }
                if let (Ok(h), Ok(m)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                    time = Some((h, m, sec, ms));
                    continue;
                }
            }
        }
        if b.len() == 5 && (b[0] == b'+' || b[0] == b'-') && b[1..].iter().all(|c| c.is_ascii_digit()) {
            let sign = if b[0] == b'-' { -1i64 } else { 1 };
            let hh: i64 = t[1..3].parse().unwrap_or(0);
            let mm: i64 = t[3..5].parse().unwrap_or(0);
            offset_min = sign * (hh * 60 + mm);
        }
    }
    let (Some((y, mo, d)), Some((h, mi, s, ms))) = (date, time) else {
        return 0;
    };
    let days = days_from_civil(y, mo, d);
    let local = days * 86400 + h as i64 * 3600 + mi as i64 * 60 + s as i64;
    let utc = local - offset_min * 60;
    (utc * 1000 + ms as i64).max(0) as u64
}

/// Strip ANSI/VT escape sequences (ESC [ ... letter) so TUI-prefixed lines
/// parse cleanly.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Skip until and including the final byte of the sequence
            // (a letter for CSI sequences).
            for c2 in chars.by_ref() {
                if c2.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parse one line of `jotta-cli sync log --watch` output.
/// Returns None for lines without a `::` separator (blank lines, noise).
pub fn parse_sync_log_line(line: &str) -> Option<SyncLogEvent> {
    let line = strip_ansi(line);
    let idx = line.find("::")?;
    let head = &line[..idx];
    let after = line[idx + 2..].trim();
    let mut it = after.splitn(2, char::is_whitespace);
    let action_word = it.next()?.trim();
    if action_word.is_empty() || !action_word.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let path = it.next().unwrap_or("").trim().to_string();
    let action = {
        let w = action_word.to_ascii_lowercase();
        if w.contains("upload") {
            SyncLogAction::Upload
        } else if w.contains("download") {
            SyncLogAction::Download
        } else if w.contains("delete") || w.contains("remove") {
            SyncLogAction::Delete
        } else {
            SyncLogAction::Other
        }
    };
    Some(SyncLogEvent {
        ts_ms: parse_ts_ms(head),
        action,
        path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_documented_lines() {
        let e = parse_sync_log_line("2026-01-20 20:10:22.566 +0100 CET :: Upload /hello.txt").unwrap();
        assert_eq!(e.action, SyncLogAction::Upload);
        assert_eq!(e.path, "/hello.txt");
        // 2026-01-20 20:10:22.566 +0100 == 2026-01-20 19:10:22.566 UTC
        assert_eq!(e.ts_ms, 1_768_936_222_566);

        let e = parse_sync_log_line("2026-01-20 20:10:22.6 +0100 CET :: Download /docs/a b.pdf").unwrap();
        assert_eq!(e.action, SyncLogAction::Download);
        assert_eq!(e.path, "/docs/a b.pdf");
        assert_eq!(e.ts_ms, 1_768_936_222_600); // .6 == 600 ms

        let e = parse_sync_log_line("2026-01-20 20:10:23 +0100 CET :: DeleteFile /old.txt").unwrap();
        assert_eq!(e.action, SyncLogAction::Delete);
        assert_eq!(e.path, "/old.txt");

        let e = parse_sync_log_line("2026-01-20 20:10:24 +0100 CET :: Finished").unwrap();
        assert_eq!(e.action, SyncLogAction::Other);
        assert_eq!(e.path, "");
    }

    #[test]
    fn tolerates_ansi_prefix_and_garbage() {
        let e = parse_sync_log_line("\u{1b}[2K\u{1b}[1m2026-01-20 20:10:22 +0100 CET :: Upload /x.txt\u{1b}[0m").unwrap();
        assert_eq!(e.action, SyncLogAction::Upload);
        assert_eq!(e.path, "/x.txt");
        assert!(parse_sync_log_line("").is_none());
        assert!(parse_sync_log_line("no separator here").is_none());
        assert!(parse_sync_log_line(":: ").is_none());
        // Negative offset: 2026-01-20 20:10:22 -0500 == 2026-01-21 01:10:22 UTC
        let e = parse_sync_log_line("2026-01-20 20:10:22 -0500 CET :: Upload /y.txt").unwrap();
        assert_eq!(e.ts_ms, 1_768_957_822_000);
    }

    #[test]
    fn help_detection() {
        assert!(sync_help_lists_log("Available Commands:\n  log        Watch the sync log\n  start      ..."));
        assert!(!sync_help_lists_log("Available Commands:\n  start\n  stop\n"));
    }
}
