//! Tolerant parser for `jotta-cli observe --json` event lines.
//!
//! `jotta-cli observe` is the only CLI view that reliably shows *which file*
//! is being transferred plus per-file progress and speed during sync
//! (screenshot-verified: path, %, size done/total, MiB/s per file + totals).
//! Whether `observe` accepts `--json` is version-dependent, so callers must
//! first run `jotta-cli observe --help` and check [`observe_help_supports_json`].
//!
//! The event schema is undocumented, so the parser walks the JSON tree
//! generically (depth-limited) and extracts direction/path/bytes/rate from
//! well-known key names, case-insensitively. It NEVER fails hard: unknown
//! lines yield `None`.

use crate::json::{self, Value};
use crate::textstatus::parse_human_bytes;

/// Parse a human rate as printed by the observe TUI, e.g. `0bytes/s`,
/// `616.96KiB/s`, `50.31MiB/s` -> bytes per second.
pub fn parse_human_rate(s: &str) -> Option<u64> {
    let t = s.trim().trim_start_matches('@').trim();
    let t = t
        .strip_suffix("/s")
        .or_else(|| t.strip_suffix("/S"))
        .unwrap_or(t)
        .trim();
    parse_human_bytes(t)
}

/// One parsed fragment of the `observe` TUI screen (see [`TuiScreen`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObserveTuiEvent {
    /// Totals line, e.g. `Observing upload-totals :: 0 / 5 files complete :: 3.50MiB / 909.80MiB @ 0bytes/s`.
    Totals {
        upload: bool,
        done_files: u64,
        total_files: u64,
        done_bytes: Option<u64>,
        total_bytes: Option<u64>,
        rate_bps: Option<u64>,
    },
    /// A named file row: `Uploading <path>` / `Downloading <path>` /
    /// `Checksuming <path>` (checksumming is upload preparation).
    File { upload: bool, path: String },
}

/// A minimal terminal emulator for the `jotta-cli observe` TUI stream.
///
/// `observe` (without `--json`, which 0.17.x does not have) draws a full-screen
/// TUI with absolute cursor positioning (`ESC[r;cH`) and in-place digit
/// updates. The stream contains NO newlines, so line-based parsing cannot work.
/// Instead we maintain a character grid and apply every chunk; the current
/// screen is then scanned with [`TuiScreen::parse_events`].
///
/// Verified against a raw `script -qec "jotta-cli observe -u" /tmp/obs.txt`
/// capture (Fedora, 2026-11-10): rows contain `Checksuming /sync/... FILE5.rpm`,
/// per-file `1 % [3.50MiB/181.96MiB] @ 0bytes/s`, and the totals line
/// `Observing upload-totals :: 0 / 5 files complete :: 3.50MiB / 909.80MiB @ 0bytes/s`.
pub struct TuiScreen {
    rows: Vec<Vec<char>>,
    cur_row: usize, // 0-based
    cur_col: usize, // 0-based
    pending: Vec<u8>, // incomplete UTF-8 tail from the previous chunk
}

const MAX_ROWS: usize = 1024;
const MAX_COLS: usize = 4096;

impl Default for TuiScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl TuiScreen {
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            cur_row: 0,
            cur_col: 0,
            pending: Vec::new(),
        }
    }

    fn ensure(&mut self, r: usize, c: usize) {
        if r >= MAX_ROWS || c >= MAX_COLS {
            return;
        }
        while self.rows.len() <= r {
            self.rows.push(Vec::new());
        }
        while self.rows[r].len() <= c {
            self.rows[r].push(' ');
        }
    }

    fn put(&mut self, ch: char) {
        self.ensure(self.cur_row, self.cur_col);
        if self.cur_row < self.rows.len() && self.cur_col < self.rows[self.cur_row].len() {
            self.rows[self.cur_row][self.cur_col] = ch;
        }
        self.cur_col += 1;
    }

    fn clear_screen(&mut self) {
        self.rows.clear();
        self.cur_row = 0;
        self.cur_col = 0;
    }

    fn clear_row_from(&mut self, col: usize) {
        if self.cur_row < self.rows.len() {
            let len = self.rows[self.cur_row].len();
            self.rows[self.cur_row].truncate(col.min(len));
        }
    }

    /// Feed one raw chunk of the TUI stream. Chunks may split UTF-8 chars AND
    /// escape sequences across boundaries — unconsumed tails are kept in
    /// `pending` and re-processed with the next chunk.
    pub fn feed(&mut self, data: &[u8]) {
        self.pending.extend_from_slice(data);
        // Work on a local copy so `feed_str` can borrow `self` mutably.
        let buf = self.pending.clone();
        let (text, consumed) = match std::str::from_utf8(&buf) {
            Ok(s) => (s.to_string(), buf.len()),
            Err(e) => {
                let upto = e.valid_up_to();
                let valid = String::from_utf8_lossy(&buf[..upto]).into_owned();
                let consumed = upto + e.error_len().unwrap_or(0);
                (valid, consumed)
            }
        };
        let consumed_chars = self.feed_str(&text);
        let consumed_bytes = text
            .char_indices()
            .nth(consumed_chars)
            .map(|(b, _)| b)
            .unwrap_or(text.len());
        let tail = text[consumed_bytes..].as_bytes().to_vec();
        self.pending.drain(..consumed);
        // Re-queue the unconsumed escape-sequence tail ahead of the
        // incomplete-UTF-8 tail so both are re-processed next feed().
        let mut new_pending = tail;
        new_pending.extend_from_slice(&self.pending);
        self.pending = new_pending;
    }

    /// Process a valid-UTF-8 string; returns how many chars were consumed
    /// (less than s.chars().count() when an escape sequence is incomplete at
    /// the end of the chunk).
    fn feed_str(&mut self, s: &str) -> usize {
        let chars: Vec<char> = s.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c == '\u{1b}' {
                // Escape sequence.
                if i + 1 >= chars.len() {
                    return i; // lone ESC at chunk end: wait for next chunk
                }
                match chars[i + 1] {
                    '[' => {
                        // CSI: params until final byte 0x40..=0x7E.
                        let mut j = i + 2;
                        while j < chars.len() && !(0x40..=0x7e).contains(&(chars[j] as u32)) {
                            j += 1;
                        }
                        if j < chars.len() {
                            self.handle_csi(&chars[i + 2..j], chars[j]);
                            i = j + 1;
                        } else {
                            return i; // incomplete CSI at chunk end: re-feed later
                        }
                    }
                    ']' => {
                        // OSC: skip until BEL or ST (ESC \).
                        let mut j = i + 2;
                        while j < chars.len() && chars[j] != '\u{7}' && chars[j] != '\u{1b}' {
                            j += 1;
                        }
                        if j >= chars.len() {
                            return i; // incomplete OSC at chunk end: re-feed later
                        }
                        i = j;
                        if chars[i] == '\u{1b}' && i + 1 < chars.len() && chars[i + 1] == '\\' {
                            i += 2;
                        }
                    }
                    '(' | ')' | '#' | '%' => {
                        if i + 2 >= chars.len() {
                            return i; // incomplete: re-feed later
                        }
                        i += 3; // two-char sequence (charset etc.)
                    }
                    'M' => {
                        // Reverse index (cursor up).
                        self.cur_row = self.cur_row.saturating_sub(1);
                        i += 2;
                    }
                    _ => {
                        i += 2; // simple two-char escape (e.g. ESC =, ESC >)
                    }
                }
                continue;
            }
            match c {
                '\r' => self.cur_col = 0,
                '\n' => self.cur_row = (self.cur_row + 1).min(MAX_ROWS - 1),
                '\u{8}' => self.cur_col = self.cur_col.saturating_sub(1),
                '\u{7}' | '\t' => {
                    if c == '\t' {
                        self.cur_col = ((self.cur_col / 8) + 1) * 8;
                    }
                }
                c if (c as u32) < 0x20 => {} // other control chars: ignore
                c => self.put(c),
            }
            i += 1;
        }
        chars.len()
    }

    fn handle_csi(&mut self, params: &[char], final_byte: char) {
        let param_str: String = params.iter().collect();
        let nums: Vec<u32> = param_str
            .trim_start_matches(['?', '>', '!', '='])
            .split(';')
            .filter_map(|p| p.parse::<u32>().ok())
            .collect();
        let n = |i: usize, d: u32| nums.get(i).copied().filter(|&v| v > 0).unwrap_or(d);
        match final_byte {
            'H' | 'f' => {
                self.cur_row = (n(0, 1) as usize).saturating_sub(1).min(MAX_ROWS - 1);
                self.cur_col = (n(1, 1) as usize).saturating_sub(1).min(MAX_COLS - 1);
            }
            'A' => self.cur_row = self.cur_row.saturating_sub(n(0, 1) as usize),
            'B' => self.cur_row = (self.cur_row + n(0, 1) as usize).min(MAX_ROWS - 1),
            'C' => self.cur_col = (self.cur_col + n(0, 1) as usize).min(MAX_COLS - 1),
            'D' => self.cur_col = self.cur_col.saturating_sub(n(0, 1) as usize),
            'G' => self.cur_col = (n(0, 1) as usize).saturating_sub(1).min(MAX_COLS - 1),
            'd' => self.cur_row = (n(0, 1) as usize).saturating_sub(1).min(MAX_ROWS - 1),
            'J' => {
                let mode = nums.first().copied().unwrap_or(0);
                if mode == 2 || mode == 3 {
                    self.clear_screen();
                } else {
                    // 0 = clear from cursor to end of screen.
                    self.clear_row_from(self.cur_col);
                    if self.cur_row + 1 < self.rows.len() {
                        self.rows.truncate(self.cur_row + 1);
                    }
                }
            }
            'K' => {
                let mode = nums.first().copied().unwrap_or(0);
                if mode == 2 {
                    if self.cur_row < self.rows.len() {
                        self.rows[self.cur_row].clear();
                    }
                } else {
                    self.clear_row_from(self.cur_col);
                }
            }
            _ => {} // m (colors), h/l (modes), n (reports), t (window ops): ignore
        }
    }

    /// All rows as strings (trailing spaces trimmed).
    pub fn lines(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|r| r.iter().collect::<String>().trim_end().to_string())
            .collect()
    }

    /// Scan the current screen for totals lines and named file rows.
    pub fn parse_events(&self) -> Vec<ObserveTuiEvent> {
        parse_observe_rows(&self.lines())
    }
}

/// Parse one screen row into an event (None for headers/borders/progress).
fn parse_observe_row(row: &str) -> Option<ObserveTuiEvent> {
    let t = row.trim();
    if t.contains("Observing upload-totals") || t.contains("Observing download-totals") {
        let upload = t.contains("upload-totals");
        return parse_totals(t, upload);
    }
    // File rows may be prefixed by box-drawing chars (┌Uploading ...) — strip them.
    let t = t.trim_start_matches(['┌', '└', '│', '┐', '┘', '─', '├', '┤', '┬', '┴', '┼', ' ']);
    for (prefix, upload) in [("Uploading ", true), ("Checksuming ", true), ("Downloading ", false)] {
        if let Some(rest) = t.strip_prefix(prefix) {
            let path = rest.trim();
            if !path.is_empty() {
                return Some(ObserveTuiEvent::File {
                    upload,
                    path: path.to_string(),
                });
            }
        }
    }
    None
}

/// Parse `Observing upload-totals :: A / B files complete :: X / Y @ Z`.
fn parse_totals(t: &str, upload: bool) -> Option<ObserveTuiEvent> {
    let parts: Vec<&str> = t.splitn(3, "::").collect();
    if parts.len() < 3 {
        return None;
    }
    // parts[1] = " 0 / 5 files complete "
    let files_part = parts[1].split("files complete").next()?;
    let (done_files, total_files) = {
        let (a, b) = files_part.split_once('/')?;
        (
            a.trim().parse::<u64>().unwrap_or(0),
            b.trim().parse::<u64>().unwrap_or(0),
        )
    };
    // parts[2] = " 3.50MiB / 909.80MiB @ 0bytes/s"
    let (bytes_part, rate_part) = match parts[2].split_once('@') {
        Some((a, b)) => (a, Some(b)),
        None => (parts[2], None),
    };
    let (done_bytes, total_bytes) = match bytes_part.split_once('/') {
        Some((a, b)) => (parse_human_bytes(a.trim()), parse_human_bytes(b.trim())),
        None => (parse_human_bytes(bytes_part.trim()), None),
    };
    let rate_bps = rate_part.and_then(|r| parse_human_rate(r));
    Some(ObserveTuiEvent::Totals {
        upload,
        done_files,
        total_files,
        done_bytes,
        total_bytes,
        rate_bps,
    })
}

/// Scan a list of screen rows (e.g. from `TuiScreen::lines()`) for events.
pub fn parse_observe_rows(rows: &[String]) -> Vec<ObserveTuiEvent> {
    rows.iter().filter_map(|r| parse_observe_row(r)).collect()
}

/// Convenience wrapper for a single raw chunk: feed into a fresh screen and
/// parse (useful for tests and one-shot captures).
pub fn parse_observe_tui_chunk(raw: &str) -> Vec<ObserveTuiEvent> {
    let mut screen = TuiScreen::new();
    screen.feed(raw.as_bytes());
    screen.parse_events()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ObserveDirection {
    Upload,
    Download,
    #[default]
    Unknown,
}

/// One parsed observe event (fields optional — schema moves between releases).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObserveEvent {
    pub direction: ObserveDirection,
    pub path: Option<String>,
    /// Bytes already transferred for this file, if reported.
    pub bytes_done: Option<u64>,
    /// Total bytes of this file, if reported.
    pub total_bytes: Option<u64>,
    /// Speed in bytes/s if the event carries it directly.
    pub rate_bps: Option<u64>,
}

/// True when `jotta-cli observe --help` output advertises a `--json` flag.
pub fn observe_help_supports_json(help: &str) -> bool {
    help.lines().any(|l| l.contains("--json"))
}

/// Parse one stdout line of `observe --json`. Returns `None` for non-JSON
/// lines (banners like "connected", empty lines) and for JSON that carries
/// nothing we recognize — never panics.
pub fn parse_observe_line(line: &str) -> Option<ObserveEvent> {
    let line = line.trim();
    if !line.starts_with('{') {
        return None;
    }
    let v = json::parse(line).ok()?;
    let mut ev = ObserveEvent::default();
    collect(&v, &mut ev, 0);
    if ev.direction == ObserveDirection::Unknown
        && ev.path.is_none()
        && ev.bytes_done.is_none()
        && ev.total_bytes.is_none()
        && ev.rate_bps.is_none()
    {
        return None;
    }
    Some(ev)
}

fn is_path_key(kl: &str) -> bool {
    matches!(
        kl,
        "path" | "file" | "filepath" | "remote" | "local" | "source" | "destination" | "name"
    )
}

fn is_type_key(kl: &str) -> bool {
    matches!(
        kl,
        "type" | "event" | "kind" | "action" | "op" | "direction" | "operation"
    )
}

/// Depth-limited generic walk: keys named like "upload*"/"download*" set the
/// direction; path-ish string keys set the path; byte-ish numeric keys set
/// counters (rate keys first, then total, then done).
fn collect(v: &Value, ev: &mut ObserveEvent, depth: usize) {
    if depth > 3 {
        return;
    }
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                let kl = k.to_lowercase();
                match val {
                    Value::Object(_) | Value::Array(_) => {
                        if kl.contains("upload") {
                            ev.direction = ObserveDirection::Upload;
                        } else if kl.contains("download") {
                            ev.direction = ObserveDirection::Download;
                        }
                        collect(val, ev, depth + 1);
                    }
                    Value::String(s) => {
                        let sl = s.to_lowercase();
                        if is_type_key(&kl) {
                            if sl.contains("upload") {
                                ev.direction = ObserveDirection::Upload;
                            } else if sl.contains("download") {
                                ev.direction = ObserveDirection::Download;
                            }
                        }
                        if ev.path.is_none() && is_path_key(&kl) && !s.is_empty() {
                            ev.path = Some(s.clone());
                        }
                    }
                    Value::Number(n) => {
                        let n = *n as u64;
                        if kl.contains("rate")
                            || kl.contains("speed")
                            || kl.contains("bps")
                            || kl.contains("persecond")
                        {
                            if ev.rate_bps.is_none() {
                                ev.rate_bps = Some(n);
                            }
                        } else if kl.contains("total") || kl.contains("size") {
                            if ev.total_bytes.is_none() {
                                ev.total_bytes = Some(n);
                            }
                        } else if kl.contains("done")
                            || kl.contains("uploaded")
                            || kl.contains("downloaded")
                            || kl.contains("transferred")
                            || kl == "bytes"
                            || kl == "progress"
                        {
                            if ev.bytes_done.is_none() {
                                ev.bytes_done = Some(n);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Value::Array(arr) => {
            for item in arr {
                collect(item, ev, depth + 1);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_detection() {
        assert!(observe_help_supports_json(
            "Usage:\n  jotta-cli observe [flags]\n\nFlags:\n      --json   emit JSON events\n"
        ));
        assert!(!observe_help_supports_json("Usage:\n  jotta-cli observe [flags]\n"));
    }

    #[test]
    fn parses_flat_upload_event() {
        let ev = parse_observe_line(
            r#"{"type": "upload", "path": "/sync/a.deb", "bytes": 34221312, "total": 149422080, "speed": 2211225}"#,
        )
        .unwrap();
        assert_eq!(ev.direction, ObserveDirection::Upload);
        assert_eq!(ev.path.as_deref(), Some("/sync/a.deb"));
        assert_eq!(ev.bytes_done, Some(34221312));
        assert_eq!(ev.total_bytes, Some(149422080));
        assert_eq!(ev.rate_bps, Some(2211225));
    }

    #[test]
    fn parses_nested_download_progress() {
        let ev = parse_observe_line(
            r#"{"event": "download_progress", "data": {"file": "/x/y.txt", "BytesDone": 5, "Total": 10, "Rate": 1024}}"#,
        )
        .unwrap();
        assert_eq!(ev.direction, ObserveDirection::Download);
        assert_eq!(ev.path.as_deref(), Some("/x/y.txt"));
        assert_eq!(ev.bytes_done, Some(5));
        assert_eq!(ev.total_bytes, Some(10));
        assert_eq!(ev.rate_bps, Some(1024));
    }

    #[test]
    fn direction_from_wrapper_key() {
        let ev = parse_observe_line(r#"{"Uploads": {"Path": "/sync/z", "Bytes": 1, "Total": 2}}"#).unwrap();
        assert_eq!(ev.direction, ObserveDirection::Upload);
        assert_eq!(ev.path.as_deref(), Some("/sync/z"));
    }

    #[test]
    fn skips_non_json_and_empty() {
        assert!(parse_observe_line("").is_none());
        assert!(parse_observe_line("observe - connected").is_none());
        assert!(parse_observe_line("not json {").is_none());
        assert!(parse_observe_line("{}").is_none());
        assert!(parse_observe_line(r#"{"foo": "bar"}"#).is_none());
        // Broken JSON never panics.
        assert!(parse_observe_line(r#"{"type": "upload""#).is_none());
    }

    #[test]
    fn human_rate_parsing() {
        assert_eq!(parse_human_rate("0bytes/s"), Some(0));
        assert_eq!(parse_human_rate("@ 0bytes/s"), Some(0));
        assert_eq!(parse_human_rate("616.96KiB/s"), Some((616.96 * 1024.0) as u64));
        assert_eq!(parse_human_rate("50.31MiB/s"), Some((50.31 * 1024.0 * 1024.0) as u64));
        assert_eq!(parse_human_rate("10.52MiB/s"), Some((10.52 * 1024.0 * 1024.0) as u64));
        assert_eq!(parse_human_rate("2.92KiB/s"), Some((2.92 * 1024.0) as u64));
        assert_eq!(parse_human_rate("8.84KiB/s"), Some((8.84 * 1024.0) as u64));
        assert_eq!(parse_human_rate("1234"), Some(1234));
        assert_eq!(parse_human_rate("abc"), None);
    }

    #[test]
    fn tui_parses_owner_totals_line_verbatim() {
        // Exact segment from the owner's `cat -v /tmp/obs.txt` capture:
        // ^[[32m^[[3;1HObserving upload-totals :: 0 / 5 files complete :: 3.50MiB / 909.80MiB @ 0bytes/s
        let raw = "\u{1b}[32m\u{1b}[3;1HObserving upload-totals :: 0 / 5 files complete :: 3.50MiB / 909.80MiB @ 0bytes/s";
        let ev = parse_observe_tui_chunk(raw);
        assert_eq!(ev.len(), 1);
        match &ev[0] {
            ObserveTuiEvent::Totals {
                upload,
                done_files,
                total_files,
                done_bytes,
                total_bytes,
                rate_bps,
            } => {
                assert!(upload);
                assert_eq!(*done_files, 0);
                assert_eq!(*total_files, 5);
                assert_eq!(*done_bytes, Some((3.50 * 1024.0 * 1024.0) as u64));
                assert_eq!(*total_bytes, Some((909.80 * 1024.0 * 1024.0) as u64));
                assert_eq!(*rate_bps, Some(0));
            }
            other => panic!("expected Totals, got {other:?}"),
        }
    }

    #[test]
    fn tui_parses_totals_with_real_rate() {
        // Later redraw state from the same capture: 184.98MiB done, still 0 rate.
        let raw = "\u{1b}[32m\u{1b}[3;1HObserving upload-totals :: 1 / 5 files complete :: 184.98MiB / 909.80MiB @ 0bytes/s";
        let ev = parse_observe_tui_chunk(raw);
        match &ev[0] {
            ObserveTuiEvent::Totals { done_files, total_files, done_bytes, rate_bps, .. } => {
                assert_eq!((*done_files, *total_files), (1, 5));
                assert_eq!(*done_bytes, Some((184.98 * 1024.0 * 1024.0) as u64));
                assert_eq!(*rate_bps, Some(0));
            }
            other => panic!("expected Totals, got {other:?}"),
        }
        // And the fragment that carried the real rate: "@ 50.30MiB/s" written at col 74.
        let mut screen = TuiScreen::new();
        screen.feed("\u{1b}[3;1HObserving upload-totals :: 0 / 5 files complete :: 3.50MiB / 909.80MiB @ 0bytes/s".as_bytes());
        screen.feed("\u{1b}[3;74H50.31MiB/s".as_bytes());
        let ev = screen.parse_events();
        match &ev[0] {
            ObserveTuiEvent::Totals { rate_bps, .. } => {
                assert_eq!(*rate_bps, Some((50.31 * 1024.0 * 1024.0) as u64));
            }
            other => panic!("expected Totals, got {other:?}"),
        }
    }

    #[test]
    fn tui_parses_checksuming_file_row_verbatim() {
        // Exact segments from the capture:
        // ^[[36m^[[5;2HChecksuming /sync/jotta-gui tests/FILE5.rpm
        let raw = "\u{1b}[36m\u{1b}[5;2HChecksuming /sync/jotta-gui tests/FILE5.rpm\u{1b}(B\u{1b}[m";
        let ev = parse_observe_tui_chunk(raw);
        assert_eq!(
            ev,
            vec![ObserveTuiEvent::File {
                upload: true,
                path: "/sync/jotta-gui tests/FILE5.rpm".to_string()
            }]
        );
    }

    #[test]
    fn tui_parses_uploading_row_with_box_char() {
        // Screenshot layout: "┌Uploading /sync/jotta-gui tests/Nextcloud-4.0.6-x86_64.AppImage"
        let raw = "\u{1b}[4;1H\u{250c}Uploading /sync/jotta-gui tests/Nextcloud-4.0.6-x86_64.AppImage";
        let ev = parse_observe_tui_chunk(raw);
        assert_eq!(
            ev,
            vec![ObserveTuiEvent::File {
                upload: true,
                path: "/sync/jotta-gui tests/Nextcloud-4.0.6-x86_64.AppImage".to_string()
            }]
        );
    }

    #[test]
    fn tui_ignores_headers_borders_and_progress_rows() {
        // Real initial screen from the capture (idle state + headers).
        let raw = "\u{1b}[H\u{1b}[2J\u{1b}(B\u{1b}[m\u{1b}[31m\u{1b}[1;1Hjotta-cli observe\u{1b}[2;1HPress Q or CTRL-C to quit\u{1b}(B\u{1b}[m\u{1b}[32m\u{1b}[3;1Hno uploads\u{1b}(B\u{1b}[m\u{1b}[31m\u{1b}[4;1HUploads\u{1b}[5;1H\u{2500}\u{1b}[6;48H1 % [3.50MiB/181.96MiB] @ 0bytes/s";
        let ev = parse_observe_tui_chunk(raw);
        assert!(ev.is_empty(), "headers/borders/progress rows must not produce events: {ev:?}");
    }

    #[test]
    fn tui_handles_chunk_split_inside_escape_and_utf8() {
        let mut screen = TuiScreen::new();
        let full = "\u{1b}[36m\u{1b}[5;2HChecksuming /sync/x \u{250c} FILE.rpm".as_bytes();
        // Feed byte by byte — every possible split point must work.
        for b in full {
            screen.feed(&[*b]);
        }
        let ev = screen.parse_events();
        assert_eq!(
            ev,
            vec![ObserveTuiEvent::File {
                upload: true,
                path: "/sync/x \u{250c} FILE.rpm".to_string()
            }]
        );
    }

    #[test]
    fn tui_full_owner_capture_segment() {
        // The "connected" redraw from the capture: banner + 5 Checksuming rows + totals.
        let raw = "\u{1b}(B\u{1b}[m\u{1b}[32m\u{1b}[1;1Hjotta-cli observe - connected\u{1b}(B\u{1b}[m\u{1b}[36m\u{1b}[5;2HChecksuming /sync/jotta-gui tests/FILE5.rpm\u{1b}(B\u{1b}[m\u{1b}[42m\u{1b}[6;2H \u{1b}(B\u{1b}[m\u{1b}[31m\u{1b}[6;48H1 % [3.50MiB/181.96MiB] @ 0bytes/s\u{1b}(B\u{1b}[m\u{1b}[36m\u{1b}[8;2HChecksuming /sync/jotta-gui tests/FILE2.rpm\u{1b}(B\u{1b}[m\u{1b}[36m\u{1b}[11;2HChecksuming /sync/jotta-gui tests/FILE1.rpm\u{1b}(B\u{1b}[m\u{1b}[36m\u{1b}[14;2HChecksuming /sync/jotta-gui tests/FILE4.rpm\u{1b}(B\u{1b}[m\u{1b}[36m\u{1b}[17;2HChecksuming /sync/jotta-gui tests/FILE3.rpm\u{1b}(B\u{1b}[m\u{1b}[31m\u{1b}[18;48H0 % [0bytes/181.96MiB] @ 0bytes/s\u{1b}(B\u{1b}[m\u{1b}[32m\u{1b}[3;1HObserving upload-totals :: 0 / 5 files complete :: 3.50MiB / 909.80MiB @ 0bytes/s";
        let ev = parse_observe_tui_chunk(raw);
        let files: Vec<&str> = ev
            .iter()
            .filter_map(|e| match e {
                ObserveTuiEvent::File { path, .. } => Some(path.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            files,
            vec![
                "/sync/jotta-gui tests/FILE5.rpm",
                "/sync/jotta-gui tests/FILE2.rpm",
                "/sync/jotta-gui tests/FILE1.rpm",
                "/sync/jotta-gui tests/FILE4.rpm",
                "/sync/jotta-gui tests/FILE3.rpm",
            ]
        );
        let totals = ev.iter().find_map(|e| match e {
            ObserveTuiEvent::Totals { .. } => Some(e),
            _ => None,
        });
        assert!(totals.is_some(), "totals line must be parsed: {ev:?}");
    }

    #[test]
    fn tui_download_totals() {
        let raw = "\u{1b}[3;1HObserving download-totals :: 2 / 9 files complete :: 100.00MiB / 500.00MiB @ 12.50MiB/s";
        let ev = parse_observe_tui_chunk(raw);
        match &ev[0] {
            ObserveTuiEvent::Totals { upload, done_files, total_files, done_bytes, total_bytes, rate_bps } => {
                assert!(!upload);
                assert_eq!((*done_files, *total_files), (2, 9));
                assert_eq!(*done_bytes, Some(100 * 1024 * 1024));
                assert_eq!(*total_bytes, Some(500 * 1024 * 1024));
                assert_eq!(*rate_bps, Some((12.50 * 1024.0 * 1024.0) as u64));
            }
            other => panic!("expected Totals, got {other:?}"),
        }
    }
}
