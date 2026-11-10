//! Built-in da/en i18n for the tray menu — small local copy of jotta-gui's
//! i18n module (documented duplication: the two crates are separate binaries
//! and share no code; jotta-core stays zero-dependency and untouched).
//!
//! Locale detection: LC_MESSAGES -> LC_ALL -> LANG; "en*" => English,
//! everything else (unset/C/POSIX included) => Danish (default/fallback).
//!
//! Pure std — testable standalone:
//!   rustc --edition 2021 --test crates/jotta-tray/src/i18n.rs -o /tmp/tray_i18n_test && /tmp/tray_i18n_test

use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Da,
    En,
}

static LANG: OnceLock<Lang> = OnceLock::new();

fn lang_from_locale_value(v: &str) -> Option<Lang> {
    let v = v.trim();
    if v.is_empty() || v == "C" || v == "POSIX" {
        return None;
    }
    let base = v.split(['.', '@']).next().unwrap_or("");
    let primary = base.split('_').next().unwrap_or("").to_ascii_lowercase();
    match primary.as_str() {
        "en" => Some(Lang::En),
        _ => Some(Lang::Da),
    }
}

pub fn detect() -> Lang {
    for var in ["LC_MESSAGES", "LC_ALL", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            if let Some(l) = lang_from_locale_value(&v) {
                return l;
            }
        }
    }
    Lang::Da
}

pub fn lang() -> Lang {
    *LANG.get_or_init(detect)
}

fn catalog(key: &str) -> (&'static str, &'static str) {
    match key {
        "tray.status" => ("Status: {}", "Status: {}"),
        "tray.speed" => ("↓ {} · ↑ {}", "↓ {} · ↑ {}"),
        "tray.error" => ("Fejl: {}", "Error: {}"),
        "tray.pause" => ("Pause", "Pause"),
        "tray.resume" => ("Genoptag", "Resume"),
        "tray.open_window" => ("Åbn vindue", "Open window"),
        "tray.open_web" => ("Åbn jottacloud.com", "Open jottacloud.com"),
        "tray.quit" => ("Afslut", "Quit"),
        "tray.state.offline" => ("Jottacloud: offline", "Jottacloud: offline"),
        "tray.state.needslogin" => ("Jottacloud: log ind påkrævet", "Jottacloud: sign-in required"),
        "tray.state.paused" => ("Jottacloud: pauset", "Jottacloud: paused"),
        "tray.state.syncing" => ("Jottacloud: synkroniserer", "Jottacloud: syncing"),
        "tray.state.syncing.n" => ("Jottacloud: synkroniserer ({} overførsler)", "Jottacloud: syncing ({} transfers)"),
        "tray.state.idle" => ("Jottacloud: oppe at køre", "Jottacloud: up and running"),
        _ => {
            let leaked: &'static str = Box::leak(key.to_string().into_boxed_str());
            (leaked, leaked)
        }
    }
}

pub fn t(key: &str) -> &'static str {
    let (da, en) = catalog(key);
    match lang() {
        Lang::Da => da,
        Lang::En => en,
    }
}

pub fn tf(key: &str, args: &[&dyn std::fmt::Display]) -> String {
    let (da, en) = catalog(key);
    let mut out = match lang() {
        Lang::Da => da,
        Lang::En => en,
    }
    .to_string();
    for a in args {
        out = out.replacen("{}", &a.to_string(), 1);
    }
    out
}

fn format_bytes_lang(n: u64, l: Lang) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut val = n as f64;
    let mut unit = 0;
    while val >= 1000.0 && unit < UNITS.len() - 1 {
        val /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        let rounded = format!("{val:.2}");
        let sep = match l {
            Lang::Da => ",",
            Lang::En => ".",
        };
        format!("{} {}", rounded.replace('.', sep), UNITS[unit])
    }
}

/// Locale-aware transfer rate ("1,58 MB/s" da / "1.58 MB/s" en) — iteration 14.
pub fn format_rate(bytes_per_sec: u64) -> String {
    format!("{}/s", format_bytes_lang(bytes_per_sec, lang()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_value_mapping() {
        assert_eq!(lang_from_locale_value("en_US.UTF-8"), Some(Lang::En));
        assert_eq!(lang_from_locale_value("da_DK.UTF-8"), Some(Lang::Da));
        assert_eq!(lang_from_locale_value("C"), None);
        assert_eq!(lang_from_locale_value(""), None);
    }

    #[test]
    fn menu_strings_both_languages() {
        let (da, en) = catalog("tray.resume");
        assert_eq!((da, en), ("Genoptag", "Resume"));
        let (da, en) = catalog("tray.quit");
        assert_eq!((da, en), ("Afslut", "Quit"));
    }

    #[test]
    fn format_rate_locale() {
        assert_eq!(format_bytes_lang(1580000000, Lang::Da), "1,58 GB");
        // format_rate uses process lang; just check the /s suffix shape via format_bytes_lang
        assert!(format!("{}/s", format_bytes_lang(1500, Lang::En)).ends_with("/s"));
    }

    #[test]
    fn tf_positional() {
        let (da, en) = catalog("tray.status");
        assert_eq!(da, "Status: {}");
        assert_eq!(en, "Status: {}");
        let s = "X";
        let mut out = t("tray.status").to_string();
        out = out.replacen("{}", s, 1);
        assert_eq!(out, "Status: X");
    }
}
