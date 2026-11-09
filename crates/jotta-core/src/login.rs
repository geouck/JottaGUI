//! Prompt state machine for `jotta-cli login` (graphical login — SPEC.md §2B).
//!
//! Empirical facts (jotta-cli 0.17.176206, owner-verified on Pop!_OS 2026-11;
//! stdin piping works, also through `flatpak-spawn --host` — NO TTY needed):
//!
//! - Prompts are printed to STDOUT; errors go to STDERR; failures exit with code 1.
//! - Prompt 1: `accept license (yes/no):` — answer `y`.
//! - Prompt 2: `Personal login token: ` — answer: the single-use token from
//!   https://jottacloud.com/web/secure . A bad token → stderr
//!   `ERROR ... not parseable ...`, exit 1.
//! - Prompt 3: `Device name [<hostname>]:` — answer: device name (the bracketed
//!   default is the hostname). PITFALL (observed): at EOF jotta-cli re-prompts
//!   FOREVER, and a fresh buffered reader per prompt can swallow pre-written
//!   lines — so answers must be fed ONE AT A TIME, only after the prompt has
//!   been seen in the output.
//! - Already logged in: NO prompts at all, stderr `ERROR  Already logged in as
//!   <email>`, exit 1 — treat as "nothing to do", not as an error.
//!
//! SECURITY: the token is an ANSWER on stdin — never argv (argv is world-readable
//! in /proc/<pid>/cmdline), never in logs/error strings. This module never sees
//! the token: it only classifies output and tells the caller WHICH prompt is
//! showing. Feeding code lives in jotta-gui; jotta-core stays zero-dependency.

/// Which prompt jotta-cli is currently showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoginStep {
    /// `accept license (yes/no):` — answer `y` (only if the user ticked the box).
    License,
    /// `Personal login token: ` — answer: the token.
    Token,
    /// `Device name [<hostname>]:` — answer: device name (non-empty!).
    DeviceName,
}

/// Prompt markers, copied verbatim from real jotta-cli output (2026-11).
pub const LICENSE_PROMPT: &str = "accept license (yes/no):";
pub const TOKEN_PROMPT: &str = "Personal login token:";
pub const DEVICE_PROMPT: &str = "Device name [";

/// Detect which prompt is showing in the accumulated STDOUT buffer.
/// The markers are distinct and appear in order, so the last marker present wins.
pub fn detect_prompt(stdout: &str) -> Option<LoginStep> {
    if stdout.contains(DEVICE_PROMPT) {
        Some(LoginStep::DeviceName)
    } else if stdout.contains(TOKEN_PROMPT) {
        Some(LoginStep::Token)
    } else if stdout.contains(LICENSE_PROMPT) {
        Some(LoginStep::License)
    } else {
        None
    }
}

/// "Already logged in": jotta-cli prints `ERROR  Already logged in as <email>`
/// on stderr and exits 1 without showing any prompt. Not a failure for us.
pub fn is_already_logged_in(stderr: &str) -> bool {
    stderr.to_lowercase().contains("already logged in")
}

/// True if stderr carries an `ERROR ...` line (bad token, daemon down, ...).
pub fn is_error(stderr: &str) -> bool {
    stderr.lines().any(|l| l.starts_with("ERROR"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real transcript from `printf 'y\n<token>\ndev\n' | jotta-cli login`
    // (Pop!_OS, 2026-11) — prompts have NO trailing newline.
    const PROMPT1: &str = "\nBy logging in with this app you accept the terms and conditions of Jottacloud\n\n    https://jottacloud.com/en/terms.html\n\nYou agree that we have the right to modify the app, remove or disable features,\nand disable the entire app.\n\naccept license (yes/no):";
    const PROMPT2: &str = "\nEnter your personal login token. You can generate a personal login token by visiting the Security tab under account settings. This is typically found at:\n\nhttps://jottacloud.com/web/secure\n\nPersonal login token: ";

    #[test]
    fn detects_each_prompt_in_order() {
        assert_eq!(detect_prompt(""), None);
        assert_eq!(detect_prompt(PROMPT1), Some(LoginStep::License));
        let both = format!("{PROMPT1}{PROMPT2}");
        assert_eq!(detect_prompt(&both), Some(LoginStep::Token));
        let all = format!("{both}Device name [pop-os]: Device name [pop-os]:");
        assert_eq!(detect_prompt(&all), Some(LoginStep::DeviceName));
    }

    #[test]
    fn already_logged_in_is_not_an_error_for_us() {
        assert!(is_already_logged_in("ERROR  Already logged in as eske@tuta.io\n"));
        assert!(!is_already_logged_in(
            "ERROR\nSorry, the data you pasted was not parseable.\n"
        ));
    }

    #[test]
    fn error_lines_detected_on_stderr() {
        assert!(is_error("ERROR\nSorry, the data you pasted was not parseable.\n\nThe parsing error returned was: illegal base64 data at input byte 0\n"));
        assert!(is_error("ERROR  Not logged in\n"));
        assert!(!is_error(""));
        assert!(!is_error("some random stdout text\n"));
    }
}
