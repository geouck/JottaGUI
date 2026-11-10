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
}
