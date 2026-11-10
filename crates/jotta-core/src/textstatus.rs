//! Parser for the plain-text `jotta-cli status` output.
//!
//! While sync is working, the TEXT status shows live working-progress lines
//! that `status --json` only reports as an empty `"WorkingProgress": {}`:
//!
//! ```text
//!   Files     : 31965 (9.34GiB)
//!   > Evaluating: 0 (0bytes) / 0 (0bytes)
//!   > Uploads : 3 (12.00MiB) / 10 (50.00MiB)
//!   > Downloads : 36 (252.55MiB) / 44091 (43.61GiB)
//! ```
//!
//! (verified in the wild: owner's `jotta-cli status` on Fedora 2026-11-10 and
//! jotta/jotta-cli-issues#168). Format per line:
//! `> <Label> : <done_files> (<done_bytes>) / <total_files> (<total_bytes>)`
//! with human bytes like `0bytes`, `252.55MiB`, `43.61GiB`.
//!
//! This is the most reliable CLI-visible signal for live up/down byte
//! counters: delta of `done_bytes` between polls = transfer speed.

/// One `> Label : n (bytes) / n (bytes)` progress line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextProgress {
    /// e.g. "Uploads", "Downloads", "Evaluating" (as printed by jotta-cli).
    pub label: String,
    pub done_files: u64,
    pub done_bytes: u64,
    pub total_files: u64,
    pub total_bytes: u64,
}

impl TextProgress {
    /// Case-insensitive label match, e.g. `label_is(p, "upload")`.
    pub fn label_is(&self, needle: &str) -> bool {
        self.label.to_ascii_lowercase().contains(needle)
    }
}

/// Parse a human byte size as printed by jotta-cli: `0bytes`, `252.55MiB`,
/// `43.61GiB`, `5MB` (decimal), or a plain number (bytes).
/// `KiB/MiB/...` are 1024-based, `KB/MB/...` are 1000-based, `B`/`bytes` = 1.
pub fn parse_human_bytes(s: &str) -> Option<u64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    // Split into numeric prefix and unit suffix.
    let split = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ','))
        .unwrap_or(t.len());
    let (num_raw, unit_raw) = t.split_at(split);
    let num_raw = num_raw.trim().replace(',', ".");
    let unit = unit_raw.trim().to_ascii_lowercase();
    let mult: f64 = match unit.as_str() {
        "" => {
            // Plain number: integer bytes only (a bare "1.5" is ambiguous).
            if num_raw.contains('.') {
                return None;
            }
            1.0
        }
        "b" | "byte" | "bytes" => 1.0,
        "kib" => 1024.0,
        "mib" => 1024.0f64.powi(2),
        "gib" => 1024.0f64.powi(3),
        "tib" => 1024.0f64.powi(4),
        "pib" => 1024.0f64.powi(5),
        "kb" => 1000.0,
        "mb" => 1000.0f64.powi(2),
        "gb" => 1000.0f64.powi(3),
        "tb" => 1000.0f64.powi(4),
        "pb" => 1000.0f64.powi(5),
        _ => return None,
    };
    let n: f64 = num_raw.parse().ok()?;
    if !n.is_finite() || n < 0.0 {
        return None;
    }
    Some((n * mult) as u64)
}

/// Parse one side of the `/`-separated pair: `36 (252.55MiB)` -> (36, bytes).
fn parse_count_bytes(side: &str) -> (u64, u64) {
    let side = side.trim();
    let files = side
        .split('(')
        .next()
        .and_then(|p| p.trim().parse::<u64>().ok())
        .unwrap_or(0);
    let bytes = side
        .find('(')
        .and_then(|i| side[i + 1..].find(')').map(|j| &side[i + 1..i + 1 + j]))
        .and_then(parse_human_bytes)
        .unwrap_or(0);
    (files, bytes)
}

/// Extract all `> Label : ... / ...` working-progress lines from plain
/// `jotta-cli status` output. Lines that don't match are skipped.
pub fn parse_status_text_progress(text: &str) -> Vec<TextProgress> {
    let mut out = Vec::new();
    for line in text.lines() {
        let l = line.trim_start();
        if !l.starts_with('>') {
            continue;
        }
        let rest = l[1..].trim_start();
        let Some(colon) = rest.find(':') else {
            continue;
        };
        let label = rest[..colon].trim().to_string();
        if label.is_empty() {
            continue;
        }
        let body = rest[colon + 1..].trim();
        let (left, right) = match body.split_once('/') {
            Some((a, b)) => (a, b),
            None => (body, ""),
        };
        let (done_files, done_bytes) = parse_count_bytes(left);
        let (total_files, total_bytes) = parse_count_bytes(right);
        out.push(TextProgress {
            label,
            done_files,
            done_bytes,
            total_files,
            total_bytes,
        });
    }
    out
}

/// Sum of `done_bytes` over all progress lines whose label contains `needle`
/// (e.g. "upload" / "download", case-insensitive).
pub fn done_bytes_for(progress: &[TextProgress], needle: &str) -> u64 {
    progress
        .iter()
        .filter(|p| p.label_is(needle))
        .map(|p| p.done_bytes)
        .fold(0u64, u64::saturating_add)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_units() {
        assert_eq!(parse_human_bytes("0bytes"), Some(0));
        assert_eq!(parse_human_bytes("512B"), Some(512));
        assert_eq!(parse_human_bytes("1KiB"), Some(1024));
        assert_eq!(parse_human_bytes("252.55MiB"), Some((252.55 * 1024.0 * 1024.0) as u64));
        assert_eq!(parse_human_bytes("43.61GiB"), Some((43.61 * 1024.0f64.powi(3)) as u64));
        assert_eq!(parse_human_bytes("5MB"), Some(5_000_000));
        assert_eq!(parse_human_bytes("2GB"), Some(2_000_000_000));
        assert_eq!(parse_human_bytes("9343841855"), Some(9_343_841_855));
        assert_eq!(parse_human_bytes("abc"), None);
        assert_eq!(parse_human_bytes("1.5"), None); // ambiguous bare float
        assert_eq!(parse_human_bytes(""), None);
    }

    #[test]
    fn parses_real_status_lines() {
        // Owner's real line (Fedora, 2026-11-10) + issue #168's Downloads line.
        let text = "------------------------------------------------------------------------------\n\
                    Sync      :\n\
                    ------------------------------------------------------------------------------\n\
                      Path      : /home/eske/Documents\n\
                      Files     : 31931 (8.50GiB)\n\
                    > Evaluating: 0 (0bytes) / 0 (0bytes)\n\
                    > Uploads : 3 (12.00MiB) / 10 (50.00MiB)\n\
                    > Downloads : 36 (252.55MiB) / 44091 (43.61GiB)\n\
                      Mode      : listening to events\n";
        let p = parse_status_text_progress(text);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].label, "Evaluating");
        assert_eq!((p[0].done_files, p[0].done_bytes, p[0].total_files, p[0].total_bytes), (0, 0, 0, 0));
        assert_eq!(p[1].label, "Uploads");
        assert_eq!(p[1].done_files, 3);
        assert_eq!(p[1].done_bytes, 12 * 1024 * 1024);
        assert_eq!(p[1].total_files, 10);
        assert_eq!(p[1].total_bytes, 50 * 1024 * 1024);
        assert_eq!(p[2].label, "Downloads");
        assert_eq!(p[2].done_files, 36);
        assert_eq!(p[2].done_bytes, (252.55 * 1024.0 * 1024.0) as u64);
        assert_eq!(p[2].total_files, 44091);
        assert_eq!(p[2].total_bytes, (43.61 * 1024.0f64.powi(3)) as u64);
        assert_eq!(done_bytes_for(&p, "upload"), 12 * 1024 * 1024);
        assert_eq!(done_bytes_for(&p, "download"), (252.55 * 1024.0 * 1024.0) as u64);
        assert_eq!(done_bytes_for(&p, "evaluat"), 0);
    }

    #[test]
    fn tolerates_garbage() {
        assert!(parse_status_text_progress("").is_empty());
        // Non-progress lines, missing colon, and empty label are all skipped.
        assert!(parse_status_text_progress("hello world\n> no colon here\n> : 1 (2B)\n").is_empty());
        assert!(parse_status_text_progress("> Uploads: x / y").len() == 1); // zeros, no panic
        let p = parse_status_text_progress("> Uploads: x / y");
        assert_eq!((p[0].done_bytes, p[0].total_bytes), (0, 0));
    }
}
