//! Tolerant parsers for jotta-cli text output (non-JSON commands).
//!
//! `jotta-cli ls`, `jotta-cli list downloads`, and `jotta-cli trash ls`
//! emit human-readable text, not JSON. The schemas are undocumented and
//! vary between versions, so these parsers are deliberately tolerant:
//! they extract what they can and ignore the rest.

/// One entry from `jotta-cli ls <remote_path>`.
///
/// The `ls` output format is not documented. We tolerate several shapes:
/// - `drwxr-xr-x  0 user group  12345 Jan  1 12:00 foldername`
/// - `foldername/` (trailing slash = directory)
/// - `-rw-r--r--  1 user group  12345 Jan  1 12:00 filename.txt`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    pub name: String,
    pub is_dir: bool,
    /// Size in bytes, if parseable.
    pub size: Option<u64>,
}

/// Parse `jotta-cli ls <path>` text output into entries.
///
/// Handles both `ls -l`-style columns and bare names (one per line).
/// Directory entries are detected by trailing `/` or a leading `d` in
/// the permission column.
pub fn parse_ls(text: &str) -> Vec<RemoteEntry> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("total ") {
            continue;
        }
        // Trailing-slash directory: "foldername/"
        if let Some(name) = line.strip_suffix('/') {
            if !name.contains(char::is_whitespace) || line.split_whitespace().count() == 1 {
                entries.push(RemoteEntry {
                    name: name.to_string(),
                    is_dir: true,
                    size: None,
                });
                continue;
            }
        }
        // `ls -l`-style: permissions links owner group size month day time name
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 5 {
            let is_dir = parts[0].starts_with('d');
            let size = parts.get(4).and_then(|s| s.parse::<u64>().ok());
            // Name is everything after the 8th column (permissions, links, owner,
            // group, size, month, day, time/year).
            let name = if parts.len() > 8 {
                parts[8..].join(" ")
            } else if parts.len() > 5 {
                parts[parts.len() - 1].to_string()
            } else {
                continue;
            };
            if !name.is_empty() && name != "." && name != ".." {
                entries.push(RemoteEntry {
                    name,
                    is_dir,
                    size,
                });
                continue;
            }
        }
        // Bare name (one per line).
        if !line.contains(char::is_whitespace) {
            entries.push(RemoteEntry {
                name: line.to_string(),
                is_dir: false,
                size: None,
            });
        }
    }
    entries
}

/// One download from `jotta-cli list downloads`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadEntry {
    /// The download ID (used for --abort / --retry).
    pub id: String,
    /// Remote path being downloaded.
    pub remote_path: String,
    /// Local destination.
    pub local_dest: String,
    /// Whether the download has failed (stays in list) or is ongoing.
    pub status: DownloadStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadStatus {
    Ongoing,
    Failed,
    Completed,
}

/// Parse `jotta-cli list downloads` text output.
///
/// Format is undocumented; tolerate several shapes:
/// - `ID  STATUS  REMOTE_PATH  LOCAL_DEST`
/// - `ID: xxx  Status: failed  Remote: Backup/X  Dest: /tmp`
/// - Table with header row.
pub fn parse_downloads(text: &str) -> Vec<DownloadEntry> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Skip header rows (a header has column names without "key: value" colons).
        let lower = line.to_lowercase();
        let is_header = (lower.starts_with("downloadid") || lower.starts_with("id "))
            && !line.contains(':');
        if is_header {
            continue;
        }
        // Try "key: value" pairs on one line (separated by 2+ spaces or tabs).
        if line.contains(':') && !line.starts_with('/') {
            let mut id = String::new();
            let mut remote = String::new();
            let mut dest = String::new();
            let mut status = DownloadStatus::Ongoing;
            // Split on 2+ spaces or tab to keep "key: value" pairs together.
            let parts: Vec<&str> = line
                .split(|c: char| c == '\t')
                .flat_map(|s| s.split("  "))
                .collect();
            for part in parts {
                let part = part.trim();
                if let Some((k, v)) = part.split_once(':') {
                    let k = k.trim().to_lowercase();
                    let v = v.trim();
                    match k.as_str() {
                        "downloadid" | "id" => id = v.to_string(),
                        "remote" | "remotepath" | "source" => remote = v.to_string(),
                        "dest" | "destination" | "local" | "localdest" => dest = v.to_string(),
                        "status" | "state" => {
                            status = match v.to_lowercase().as_str() {
                                "failed" | "error" | "errored" => DownloadStatus::Failed,
                                "completed" | "complete" | "done" | "finished" => DownloadStatus::Completed,
                                _ => DownloadStatus::Ongoing,
                            };
                        }
                        _ => {}
                    }
                }
            }
            if !id.is_empty() {
                entries.push(DownloadEntry {
                    id,
                    remote_path: remote,
                    local_dest: dest,
                    status,
                });
                continue;
            }
        }
        // Whitespace-separated columns: ID STATUS REMOTE DEST (or similar).
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let id = parts[0].to_string();
            let status = match parts.get(1).map(|s| s.to_lowercase()).as_deref() {
                Some("failed") | Some("error") | Some("errored") => DownloadStatus::Failed,
                Some("completed") | Some("complete") | Some("done") => DownloadStatus::Completed,
                _ => DownloadStatus::Ongoing,
            };
            let remote_path = parts.get(2).unwrap_or(&"").to_string();
            let local_dest = parts.get(3).unwrap_or(&"").to_string();
            entries.push(DownloadEntry {
                id,
                remote_path,
                local_dest,
                status,
            });
        }
    }
    entries
}

/// One item from `jotta-cli trash ls`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashEntry {
    /// Remote path of the trashed item (used for restore/purge).
    pub path: String,
    /// Display name (last path component).
    pub name: String,
    /// Whether it's a folder.
    pub is_dir: bool,
    /// Size in bytes, if parseable.
    pub size: Option<u64>,
    /// When it was deleted, if parseable.
    pub deleted_at: Option<String>,
}

/// Parse `jotta-cli trash ls` text output.
///
/// Format is undocumented; tolerate `ls -l`-style and bare-path shapes.
pub fn parse_trash(text: &str) -> Vec<TrashEntry> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("total ") {
            continue;
        }
        // `ls -l`-style: permissions links owner group size month day time path
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 5 && parts[0].starts_with(['d', '-']) {
            let is_dir = parts[0].starts_with('d');
            let size = parts.get(4).and_then(|s| s.parse::<u64>().ok());
            let path = if parts.len() > 8 {
                parts[8..].join(" ")
            } else {
                continue;
            };
            let name = path.rsplit('/').next().unwrap_or(&path).to_string();
            entries.push(TrashEntry {
                path,
                name,
                is_dir,
                size,
                deleted_at: None,
            });
            continue;
        }
        // Bare path (one per line).
        if line.starts_with('/') || line.contains('/') {
            let trimmed = line.trim_end_matches('/');
            let name = trimmed.rsplit('/').next().unwrap_or(trimmed).to_string();
            entries.push(TrashEntry {
                path: line.to_string(),
                name,
                is_dir: line.ends_with('/'),
                size: None,
                deleted_at: None,
            });
        }
    }
    entries
}

/// Build the remote path prefix for a backup folder.
///
/// `jotta-cli download` expects paths like `Backup/<DeviceName>/<BackupName>`.
/// The device name comes from `status --json` (`User.device.Name`).
pub fn backup_remote_path(device_name: &str, backup_name: &str) -> String {
    format!("Backup/{device_name}/{backup_name}")
}

/// Build the remote path for the sync root.
pub fn sync_remote_path() -> String {
    "Sync".to_string()
}

/// Build the remote path for the archive root.
pub fn archive_remote_path() -> String {
    "Archive".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ls_long_format() {
        let text = "total 42\ndrwxr-xr-x  0 user group   4096 Jan  1 12:00 Documents\n-rw-r--r--  1 user group 123456 Jan  2 09:30 report.pdf\n";
        let entries = parse_ls(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "Documents");
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].name, "report.pdf");
        assert!(!entries[1].is_dir);
        assert_eq!(entries[1].size, Some(123456));
    }

    #[test]
    fn parses_ls_bare_names() {
        let text = "Documents\nPhotos\nfile.txt\n";
        let entries = parse_ls(text);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "Documents");
        assert!(!entries[0].is_dir);
    }

    #[test]
    fn parses_ls_trailing_slash_dirs() {
        let text = "Documents/\nPhotos/\nfile.txt\n";
        let entries = parse_ls(text);
        assert_eq!(entries.len(), 3);
        assert!(entries[0].is_dir);
        assert!(entries[1].is_dir);
        assert!(!entries[2].is_dir);
    }

    #[test]
    fn parses_downloads_columns() {
        let text = "abc123  ongoing  Backup/Laptop/Documents  /home/eske/Downloads\ndef456  failed   Archive/file.txt         /tmp\n";
        let entries = parse_downloads(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "abc123");
        assert_eq!(entries[0].status, DownloadStatus::Ongoing);
        assert_eq!(entries[0].remote_path, "Backup/Laptop/Documents");
        assert_eq!(entries[1].status, DownloadStatus::Failed);
    }

    #[test]
    fn parses_downloads_keyvalue() {
        let text = "DownloadID: abc123  Status: failed  Remote: Backup/X  Dest: /tmp\n";
        let entries = parse_downloads(text);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "abc123");
        assert_eq!(entries[0].status, DownloadStatus::Failed);
        assert_eq!(entries[0].remote_path, "Backup/X");
    }

    #[test]
    fn parses_trash_bare_paths() {
        let text = "Backup/Laptop/old_file.txt\nArchive/deleted_folder/\n";
        let entries = parse_trash(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "old_file.txt");
        assert!(!entries[0].is_dir);
        assert_eq!(entries[1].name, "deleted_folder");
        assert!(entries[1].is_dir);
    }

    #[test]
    fn parses_trash_long_format() {
        let text = "-rw-r--r--  1 user group  1024 Jan  1 12:00 Backup/Laptop/file.txt\ndrwxr-xr-x  0 user group   4096 Jan  2 09:30 Archive/folder\n";
        let entries = parse_trash(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "Backup/Laptop/file.txt");
        assert_eq!(entries[0].size, Some(1024));
        assert!(!entries[0].is_dir);
        assert!(entries[1].is_dir);
    }

    #[test]
    fn builds_remote_paths() {
        assert_eq!(backup_remote_path("Laptop", "Documents"), "Backup/Laptop/Documents");
        assert_eq!(sync_remote_path(), "Sync");
        assert_eq!(archive_remote_path(), "Archive");
    }

    #[test]
    fn empty_output_yields_empty_vec() {
        assert!(parse_ls("").is_empty());
        assert!(parse_downloads("").is_empty());
        assert!(parse_trash("").is_empty());
    }
}
