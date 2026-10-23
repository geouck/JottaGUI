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
    /// Pause everything. `jotta-cli pause <duration>` — duration is required.
    /// We use 87600h (10 years) = effectively indefinite until manual resume.
    Pause,
    /// Resume everything.
    Resume,
    /// `config set <key> <value>` — sæt en config-værdi (fx syncpaused).
    ConfigSet { key: String, value: String },
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
    /// `ignores add --pattern <pat> [--backup NAME]` — BACKUP ignores only.
    /// `backup: None` means "all backups" (NOT the default — UI must ask).
    /// WARNING: adding a pattern removes already-backed-up matching files
    /// (they go to Trash). Destructive — always confirm in the UI.
    IgnoreAdd { pattern: String, backup: Option<String> },
    /// `ignores rem --pattern <pat> [--backup NAME]` — BACKUP ignores only.
    IgnoreRemove { pattern: String, backup: Option<String> },
    /// `ignores list [--backup NAME]` — BACKUP ignores only.
    /// `backup: None` lists patterns for ALL backups.
    IgnoresList { backup: Option<String> },
    /// `ignores test --pattern <pat> --path <path>` — validate a pattern
    /// against a path before adding (BACKUP ignores).
    IgnoreTest { pattern: String, path: String },
    /// `sync selective add <folder>` — SYNC selective (NOT backup ignores).
    /// Only works with top-level folders in the sync root. NOT destructive:
    /// contents are not deleted locally or in the cloud.
    SyncSelectiveAdd(String),
    /// `sync selective rem <folder>` — SYNC selective. NOT destructive:
    /// local and remote contents are merged on removal.
    SyncSelectiveRemove(String),
    /// `sync selective list` — SYNC selective. Returns list of ignored
    /// top-level folder names.
    SyncSelectiveList,
    /// `share <path>` -> returns a public URL.
    Share(PathBuf),
    /// `version`
    Version,
    /// `download <remote_path> <local_dest> [--merge]` — download a file or
    /// folder from Jottacloud (Backup/Archive/Sync/Photos/Trash) to a local
    /// destination. Remote path is like `Backup/MyDevice/Folder` or
    /// `Archive/file.txt` or `Sync`. NOT destructive (read-only on remote).
    Download { remote_path: String, local_dest: PathBuf, merge: bool },
    /// `download --abort=<downloadid>` — cancel an ongoing download or clear
    /// a finished/failed download from the list. Partial files are deleted;
    /// already-completed files are kept.
    DownloadAbort(String),
    /// `download --retry=<downloadid>` — retry failed files in a download.
    DownloadRetry(String),
    /// `list downloads --json` — list ongoing/failed downloads as JSON.
    /// Verified: `jotta-cli list downloads --help` shows `--json` flag.
    ListDownloads,
    /// `list uploads --json` — list ongoing/failed uploads as JSON.
    /// Expected to have `--json` (same pattern as `list downloads --json`).
    /// Verified: `jotta-cli list --help` shows `uploads` subcommand.
    ListUploads,
    /// `list downloaderrors [--downloadid=<id>] [--json]` — error details
    /// for failed downloads. With no filter, lists all download errors.
    ListDownloadErrors { download_id: Option<String>, json: bool },
    /// `list uploaderrors [--uploadid=<id>] [--json]` — error details
    /// for failed uploads. With no filter, lists all upload errors.
    ListUploadErrors { upload_id: Option<String>, json: bool },
    /// `list downloadinfo --downloadid=<id> [--json]` — error details for a
    /// failed download. JSON when `json` is true.
    /// NOTE: newer jotta-cli renamed this to `list downloaderrors`
    /// (verified in `jotta-cli download --help` output). Both work.
    ListDownloadInfo { download_id: String, json: bool },
    /// `ls <remote_path> --json` — browse remote filesystem (Sync/Backup/Archive)
    /// as JSON. Verified schemas:
    /// - Folder: `{"Folders": [{"Name","Path","Modified","Size"?}]}` (Size optional)
    /// - File:   `{"revisions": [{"id","size","checksum","createdAtMillis","modifiedAtMillis"}]}`
    /// Casing is inconsistent in jotta-cli — parser must be case-tolerant.
    LsRemote(String),
    /// `trash restore <path>` — restore an item from trash to its original
    /// location. NOT destructive (reverses a delete).
    /// NOTE: `trash list` has NO --json flag and its plain output is unparseable
    /// (single space-separated line). Trash UI is web-only for now.
    TrashRestore(String),
    /// `trash purge <path>` — permanently delete an item from trash.
    /// DESTRUCTIVE — always confirm in the UI (safety rule 3).
    /// WARNING: `trash purge` WITHOUT a path empties the ENTIRE trash.
    /// The GUI must always pass an explicit path + show a confirmation dialog.
    TrashPurge(String),
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
            JottaCommand::Pause => push(&mut args, &["pause", "87600h"]),
            JottaCommand::Resume => push(&mut args, &["resume"]),
            JottaCommand::ConfigSet { key, value } => {
                push(&mut args, &["config", "set"]);
                args.push(key.clone());
                args.push(value.clone());
            }
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
            JottaCommand::IgnoresList { backup } => {
                push(&mut args, &["ignores", "list"]);
                if let Some(b) = backup {
                    push(&mut args, &["--backup"]);
                    args.push(b.clone());
                }
            }
            JottaCommand::IgnoreTest { pattern, path } => {
                push(&mut args, &["ignores", "test", "--pattern"]);
                args.push(pattern.clone());
                push(&mut args, &["--path"]);
                args.push(path.clone());
            }
            JottaCommand::SyncSelectiveAdd(name) => {
                push(&mut args, &["sync", "selective", "add"]);
                args.push(name.clone());
            }
            JottaCommand::SyncSelectiveRemove(name) => {
                push(&mut args, &["sync", "selective", "rem"]);
                args.push(name.clone());
            }
            JottaCommand::SyncSelectiveList => push(&mut args, &["sync", "selective", "list"]),
            JottaCommand::Share(p) => {
                push(&mut args, &["share"]);
                args.push(p.to_string_lossy().into_owned());
            }
            JottaCommand::Version => push(&mut args, &["version"]),
            JottaCommand::Download { remote_path, local_dest, merge } => {
                push(&mut args, &["download"]);
                args.push(remote_path.clone());
                args.push(local_dest.to_string_lossy().into_owned());
                if *merge {
                    push(&mut args, &["--merge"]);
                }
            }
            JottaCommand::DownloadAbort(id) => {
                push(&mut args, &["download"]);
                args.push(format!("--abort={id}"));
            }
            JottaCommand::DownloadRetry(id) => {
                push(&mut args, &["download"]);
                args.push(format!("--retry={id}"));
            }
            JottaCommand::ListDownloads => push(&mut args, &["list", "downloads", "--json"]),
            JottaCommand::ListUploads => push(&mut args, &["list", "uploads", "--json"]),
            JottaCommand::ListDownloadInfo { download_id, json } => {
                push(&mut args, &["list", "downloadinfo"]);
                args.push(format!("--downloadid={download_id}"));
                if *json {
                    push(&mut args, &["--json"]);
                }
            }
            JottaCommand::ListDownloadErrors { download_id, json } => {
                push(&mut args, &["list", "downloaderrors"]);
                if let Some(id) = download_id {
                    args.push(format!("--downloadid={id}"));
                }
                if *json {
                    push(&mut args, &["--json"]);
                }
            }
            JottaCommand::ListUploadErrors { upload_id, json } => {
                push(&mut args, &["list", "uploaderrors"]);
                if let Some(id) = upload_id {
                    args.push(format!("--uploadid={id}"));
                }
                if *json {
                    push(&mut args, &["--json"]);
                }
            }
            JottaCommand::LsRemote(path) => {
                push(&mut args, &["ls"]);
                args.push(path.clone());
                push(&mut args, &["--json"]);
            }
            JottaCommand::TrashRestore(path) => {
                push(&mut args, &["trash", "restore"]);
                args.push(path.clone());
            }
            JottaCommand::TrashPurge(path) => {
                push(&mut args, &["trash", "purge"]);
                args.push(path.clone());
            }
        }
        args
    }

    /// Whether the command's stdout is expected to be JSON.
    pub fn is_json(&self) -> bool {
        matches!(
            self,
            JottaCommand::Status
                | JottaCommand::IgnoresList { .. }
                | JottaCommand::SyncSelectiveList
                | JottaCommand::ListDownloadInfo { json: true, .. }
                | JottaCommand::ListDownloads
                | JottaCommand::ListUploads
                | JottaCommand::ListDownloadErrors { json: true, .. }
                | JottaCommand::ListUploadErrors { json: true, .. }
                | JottaCommand::LsRemote(_)
        )
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
                argv: argv.clone(),
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

    /// Run a command and return stdout as text (for non-JSON commands like
    /// `ls`, `list downloads`, `trash ls`).
    pub fn run_text(&self, cmd: &JottaCommand) -> Result<String> {
        let out = self.run(cmd)?;
        Ok(out.stdout)
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
        // Backup ignores: "ignores list" (NOT bare "ignores") — verified against
        // docs.jottacloud.com "Ignoring files and folders from backup".
        assert_eq!(
            JottaCommand::IgnoresList { backup: None }.argv(),
            vec!["ignores", "list"]
        );
        assert_eq!(
            JottaCommand::IgnoresList { backup: Some("Documents".into()) }.argv(),
            vec!["ignores", "list", "--backup", "Documents"]
        );
        // Backup ignores without --backup means "all backups" — UI must not
        // default to this silently.
        assert_eq!(
            JottaCommand::IgnoreAdd { pattern: "**.tmp".into(), backup: None }.argv(),
            vec!["ignores", "add", "--pattern", "**.tmp"]
        );
        // Sync selective: top-level folder names, NOT gitignore globs.
        assert_eq!(
            JottaCommand::SyncSelectiveAdd("Photos".into()).argv(),
            vec!["sync", "selective", "add", "Photos"]
        );
        assert_eq!(
            JottaCommand::SyncSelectiveRemove("Photos".into()).argv(),
            vec!["sync", "selective", "rem", "Photos"]
        );
        assert_eq!(
            JottaCommand::SyncSelectiveList.argv(),
            vec!["sync", "selective", "list"]
        );
        // ignores test validates a pattern before adding.
        assert_eq!(
            JottaCommand::IgnoreTest { pattern: "**.png".into(), path: "/home/eske/Documents/foto.png".into() }.argv(),
            vec!["ignores", "test", "--pattern", "**.png", "--path", "/home/eske/Documents/foto.png"]
        );
        // Download: remote path + local destination, optional --merge.
        // Verified against docs.jottacloud.com "Downloading using Jottacloud CLI".
        assert_eq!(
            JottaCommand::Download {
                remote_path: "Backup/Laptop/Documents".into(),
                local_dest: PathBuf::from("/home/eske/Downloads"),
                merge: false,
            }.argv(),
            vec!["download", "Backup/Laptop/Documents", "/home/eske/Downloads"]
        );
        assert_eq!(
            JottaCommand::Download {
                remote_path: "Archive/file.txt".into(),
                local_dest: PathBuf::from("/tmp"),
                merge: true,
            }.argv(),
            vec!["download", "Archive/file.txt", "/tmp", "--merge"]
        );
        // Download abort/retry use --abort=ID / --retry=ID.
        assert_eq!(
            JottaCommand::DownloadAbort("abc123".into()).argv(),
            vec!["download", "--abort=abc123"]
        );
        assert_eq!(
            JottaCommand::DownloadRetry("abc123".into()).argv(),
            vec!["download", "--retry=abc123"]
        );
        // List downloads --json (verified: `jotta-cli list downloads --help` shows --json).
        assert_eq!(
            JottaCommand::ListDownloads.argv(),
            vec!["list", "downloads", "--json"]
        );
        assert_eq!(
            JottaCommand::ListDownloadInfo { download_id: "d1".into(), json: true }.argv(),
            vec!["list", "downloadinfo", "--downloadid=d1", "--json"]
        );
        assert_eq!(
            JottaCommand::ListDownloadInfo { download_id: "d1".into(), json: false }.argv(),
            vec!["list", "downloadinfo", "--downloadid=d1"]
        );
        // Remote ls --json (verified schema: {"Folders":[...]} or {"revisions":[...]}).
        assert_eq!(
            JottaCommand::LsRemote("Backup/Laptop".into()).argv(),
            vec!["ls", "Backup/Laptop", "--json"]
        );
        // Trash: restore (non-destructive), purge (DESTRUCTIVE — always pass path).
        // NOTE: TrashList removed — `trash list` has no --json and plain output is unparseable.
        assert_eq!(
            JottaCommand::TrashRestore("Backup/Laptop/file.txt".into()).argv(),
            vec!["trash", "restore", "Backup/Laptop/file.txt"]
        );
        assert_eq!(
            JottaCommand::TrashPurge("Backup/Laptop/file.txt".into()).argv(),
            vec!["trash", "purge", "Backup/Laptop/file.txt"]
        );
        // ConfigSet: config set <key> <value>
        assert_eq!(
            JottaCommand::ConfigSet { key: "syncpaused".into(), value: "true".into() }.argv(),
            vec!["config", "set", "syncpaused", "true"]
        );
        // List uploads --json (iteration 7: transfers page).
        assert_eq!(
            JottaCommand::ListUploads.argv(),
            vec!["list", "uploads", "--json"]
        );
        // List download errors: all errors with --json.
        assert_eq!(
            JottaCommand::ListDownloadErrors { download_id: None, json: true }.argv(),
            vec!["list", "downloaderrors", "--json"]
        );
        // List download errors: single download with --json.
        assert_eq!(
            JottaCommand::ListDownloadErrors { download_id: Some("d1".into()), json: true }.argv(),
            vec!["list", "downloaderrors", "--downloadid=d1", "--json"]
        );
        // List download errors: single download without --json.
        assert_eq!(
            JottaCommand::ListDownloadErrors { download_id: Some("d1".into()), json: false }.argv(),
            vec!["list", "downloaderrors", "--downloadid=d1"]
        );
        // List upload errors: all errors with --json.
        assert_eq!(
            JottaCommand::ListUploadErrors { upload_id: None, json: true }.argv(),
            vec!["list", "uploaderrors", "--json"]
        );
        // List upload errors: single upload with --json.
        assert_eq!(
            JottaCommand::ListUploadErrors { upload_id: Some("u1".into()), json: true }.argv(),
            vec!["list", "uploaderrors", "--uploadid=u1", "--json"]
        );
    }

    #[test]
    fn json_flags() {
        assert!(JottaCommand::Status.is_json());
        assert!(JottaCommand::IgnoresList { backup: None }.is_json());
        assert!(JottaCommand::SyncSelectiveList.is_json());
        assert!(!JottaCommand::Pause.is_json());
        assert_eq!(JottaCommand::Pause.argv(), vec!["pause", "87600h"]);
        assert!(!JottaCommand::IgnoreAdd { pattern: "x".into(), backup: None }.is_json());
        assert!(!JottaCommand::SyncSelectiveAdd("x".into()).is_json());
        // ListDownloadInfo is JSON only when json=true.
        assert!(JottaCommand::ListDownloadInfo { download_id: "x".into(), json: true }.is_json());
        assert!(!JottaCommand::ListDownloadInfo { download_id: "x".into(), json: false }.is_json());
        // Download is NOT JSON. LsRemote and ListDownloads ARE JSON (always --json).
        assert!(!JottaCommand::Download {
            remote_path: "Backup/X".into(),
            local_dest: PathBuf::from("/tmp"),
            merge: false,
        }.is_json());
        assert!(JottaCommand::LsRemote("Backup".into()).is_json());
        assert!(JottaCommand::ListDownloads.is_json());
        // Iteration 7: uploads and error lists.
        assert!(JottaCommand::ListUploads.is_json());
        assert!(JottaCommand::ListDownloadErrors { download_id: None, json: true }.is_json());
        assert!(!JottaCommand::ListDownloadErrors { download_id: None, json: false }.is_json());
        assert!(JottaCommand::ListUploadErrors { upload_id: Some("u1".into()), json: true }.is_json());
        assert!(!JottaCommand::ListUploadErrors { upload_id: None, json: false }.is_json());
        // Trash restore/purge are never JSON.
        assert!(!JottaCommand::TrashRestore("x".into()).is_json());
        assert!(!JottaCommand::TrashPurge("x".into()).is_json());
    }
}
