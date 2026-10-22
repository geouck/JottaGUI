//! Typed wrapper around the `jotta-cli` binary (Model B: we ship it).
//!
//! Design notes:
//! - Commands are built as an enum, so the GUI can render any command
//!   without string surgery, and we can whitelist exactly what the app does.
//! - All output is captured; `--json` commands return parsed json Values.
//! - The wrapper is transport-agnostic: it spawns the binary the way you
//!   would from a terminal (env inherited), so host-installed and bundled
//!   binaries behave identically.

use crate::error::{Error, Result};
use crate::json::{self, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where to find the jotta-cli/jottad binaries.
#[derive(Debug, Clone)]
pub struct BinaryLocator {
    cli: PathBuf,
    daemon: PathBuf,
}

impl BinaryLocator {
    /// Find binaries on PATH (dev mode / native installs).
    pub fn from_path() -> Option<Self> {
        let cli = which("jotta-cli")?;
        let daemon = which("jottad").or_else(|| which("jotta-cli").map(|p| p.with_file_name("jottad")))?;
        Some(Self { cli, daemon })
    }

    /// Use explicitly provided binaries (Flatpak bundles them in the app dir).
    pub fn new(cli: PathBuf, daemon: PathBuf) -> Self {
        Self { cli, daemon }
    }

    pub fn cli(&self) -> &Path { &self.cli }
    pub fn daemon(&self) -> &Path { &self.daemon }
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// The set of jotta-cli commands the app knows how to issue.
#[derive(Debug, Clone)]
pub enum JottaCommand {
    /// `status --json`: full state of the system right now.
    Status,
    /// `login`: interactive auth (opens a browser on the host).
    Login,
    /// Pause everything.
    Pause,
    /// Resume everything.
    Resume,
    /// Add a folder to backup.
    BackupAdd(PathBuf),
    /// Remove a backup folder by name.
    BackupRemove(String),
    /// List backup folders.
    BackupsList,
    /// `sync setup --root <path>` then `sync start`.
    SyncSetup(PathBuf),
    SyncStart,
    SyncStop,
    /// `sync log -n <n>`
    SyncLog(usize),
    /// `ignores add --pattern <pat>` (optionally scoped to one backup).
    IgnoreAdd { pattern: String, backup: Option<String> },
    /// `ignores rem --pattern <pat>`
    IgnoreRemove { pattern: String, backup: Option<String> },
    /// `ignores list`
    IgnoresList,
    /// `share <path>` -> returns a public URL.
    Share(PathBuf),
    /// `version`
    Version,
}

impl JottaCommand {
    /// The argv (excluding the binary itself) for this command.
    pub fn argv(&self) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        let push = |args: &mut Vec<String>, words: &[&str]| {
            for w in words {
                args.push((*w).to_string());
            }
        };
        match self {
            JottaCommand::Status => push(&mut args, &["status", "--json"]),
            JottaCommand::Login => push(&mut args, &["login"]),
            JottaCommand::Pause => push(&mut args, &["pause"]),
            JottaCommand::Resume => push(&mut args, &["resume"]),
            JottaCommand::BackupAdd(p) => {
                push(&mut args, &["add"]);
                args.push(p.to_string_lossy().into_owned());
            }
            JottaCommand::BackupRemove(name) => {
                push(&mut args, &["rem"]);
                args.push(name.clone());
            }
            JottaCommand::BackupsList => push(&mut args, &["list"]),
            JottaCommand::SyncSetup(p) => {
                push(&mut args, &["sync", "setup", "--root"]);
                args.push(p.to_string_lossy().into_owned());
            }
            JottaCommand::SyncStart => push(&mut args, &["sync", "start"]),
            JottaCommand::SyncStop => push(&mut args, &["sync", "stop"]),
            JottaCommand::SyncLog(n) => {
                push(&mut args, &["sync", "log", "-n"]);
                args.push(n.to_string());
            }
            JottaCommand::IgnoreAdd { pattern, backup } => {
                push(&mut args, &["ignores", "add", "--pattern"]);
                args.push(pattern.clone());
                if let Some(b) = backup {
                    push(&mut args, &["--backup"]);
                    args.push(b.clone());
                }
            }
            JottaCommand::IgnoreRemove { pattern, backup } => {
                push(&mut args, &["ignores", "rem", "--pattern"]);
                args.push(pattern.clone());
                if let Some(b) = backup {
                    push(&mut args, &["--backup"]);
                    args.push(b.clone());
                }
            }
            JottaCommand::IgnoresList => push(&mut args, &["ignores"]),
            JottaCommand::Share(p) => {
                push(&mut args, &["share"]);
                args.push(p.to_string_lossy().into_owned());
            }
            JottaCommand::Version => push(&mut args, &["version"]),
        }
        args
    }

    /// Whether the command's stdout is expected to be JSON.
    pub fn is_json(&self) -> bool {
        matches!(self, JottaCommand::Status | JottaCommand::IgnoresList)
    }
}

/// A thin client that executes JottaCommands via the jotta-cli binary.
pub struct CliClient {
    locator: BinaryLocator,
}

impl CliClient {
    pub fn new(locator: BinaryLocator) -> Self {
        Self { locator }
    }

    pub fn locator(&self) -> &BinaryLocator {
        &self.locator
    }

    /// Run a command and capture stdout+stderr. Non-zero exit -> CommandFailed.
    pub fn run(&self, cmd: &JottaCommand) -> Result<Output> {
        let argv = cmd.argv();
        let out = Command::new(&self.locator.cli)
            .args(&argv)
            .stdin(Stdio::null())
            .output()
            .map_err(|io| Error::Spawn(self.locator.cli.display().to_string(), io))?;

        if !out.status.success() {
            let code = out.status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            return Err(Error::CommandFailed {
                program: self.locator.cli.display().to_string(),
                code,
                stderr,
            });
        }
        Ok(Output {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    /// Run a JSON command and parse its stdout.
    pub fn run_json(&self, cmd: &JottaCommand) -> Result<Value> {
        debug_assert!(cmd.is_json(), "run_json called on a non-JSON command");
        let out = self.run(cmd)?;
        json::parse(out.stdout.trim())
            .map_err(|e| Error::Parse(format!("{}: {e}", cmd_debug(cmd))))
    }

    /// Convenience: `status --json` parsed.
    pub fn status(&self) -> Result<Value> {
        self.run_json(&JottaCommand::Status)
    }
}

fn cmd_debug(cmd: &JottaCommand) -> &'static str {
    match cmd {
        JottaCommand::Status => "status",
        _ => "command",
    }
}

/// Captured output of one command run.
pub struct Output {
    pub stdout: String,
    pub stderr: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_shapes() {
        assert_eq!(JottaCommand::Status.argv(), vec!["status", "--json"]);
        assert_eq!(
            JottaCommand::SyncSetup(PathBuf::from("/home/eske/Documents")).argv(),
            vec!["sync", "setup", "--root", "/home/eske/Documents"]
        );
        assert_eq!(
            JottaCommand::IgnoreAdd { pattern: "**/node_modules".into(), backup: Some("Dev".into()) }.argv(),
            vec!["ignores", "add", "--pattern", "**/node_modules", "--backup", "Dev"]
        );
        assert_eq!(JottaCommand::SyncLog(20).argv(), vec!["sync", "log", "-n", "20"]);
    }

    #[test]
    fn json_flags() {
        assert!(JottaCommand::Status.is_json());
        assert!(!JottaCommand::Pause.is_json());
    }
}
