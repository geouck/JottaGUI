//! Tolerant parsers for jotta-cli JSON output.
//!
//! All remote-browsing commands (`ls`, `list downloads`) now use `--json`.
//! The schemas are verified against real jotta-cli output (Fedora, 2026) but
//! casing is inconsistent ("Folders" vs "revisions"), so parsers are
//! case-tolerant and never assume a fixed key casing.

use crate::json::Value;

/// One entry from `jotta-cli ls <remote_path> --json`.
///
/// Verified schema for a folder listing:
/// ```json
/// {"Folders": [{"Name": "Laptop", "Path": "/backup/Laptop", "Modified": 1792743158517, "Size": 7867524812}]}
/// ```
/// `Size` is OPTIONAL (some devices/folders omit it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    pub name: String,
    pub is_dir: bool,
    /// Size in bytes, if present in the JSON.
    pub size: Option<u64>,
    /// Remote path (e.g. `/backup/Laptop`), if present.
    pub remote_path: Option<String>,
}

/// One revision of a remote file (from `ls <file> --json`).
///
/// Verified schema:
/// ```json
/// {"revisions": [{"id": "1", "size": 3778, "checksum": "f232...", "createdAtMillis": ..., "modifiedAtMillis": ...}]}
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRevision {
    pub id: String,
    pub size: Option<u64>,
    pub checksum: Option<String>,
}

/// Result of `ls --json`: either a folder listing or file revisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LsResult {
    Folders(Vec<RemoteEntry>),
    Revisions(Vec<FileRevision>),
    Empty,
}

/// Case-tolerant key lookup: tries exact, lowercase, and capitalized variants.
fn get_ci<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    if let Value::Object(map) = v {
        // Try exact match first.
        if let Some(val) = map.get(key) {
            return Some(val);
        }
        // Try case-insensitive match.
        let lower = key.to_lowercase();
        for (k, val) in map {
            if k.to_lowercase() == lower {
                return Some(val);
            }
        }
    }
    None
}

fn as_u64_ci(v: &Value, key: &str) -> Option<u64> {
    get_ci(v, key).and_then(|x| x.as_f64()).map(|n| n as u64)
}

fn as_str_ci<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    get_ci(v, key).and_then(|x| x.as_str())
}

/// Parse `jotta-cli ls <path> --json` output.
///
/// Handles two verified schemas (case-tolerant):
/// - Folder: `{"Folders": [{"Name", "Path", "Modified", "Size"?}]}`
/// - File:   `{"revisions": [{"id", "size", "checksum", ...}]}`
///
/// Also tolerates a bare array of folder objects (no wrapper key).
pub fn parse_ls_json(v: &Value) -> LsResult {
    // Try "Folders" (case-tolerant).
    if let Some(arr) = get_ci(v, "Folders").and_then(|x| x.as_array()) {
        let entries: Vec<RemoteEntry> = arr
            .iter()
            .filter_map(|item| {
                let name = as_str_ci(item, "Name")?.to_string();
                Some(RemoteEntry {
                    name,
                    is_dir: true,
                    size: as_u64_ci(item, "Size"),
                    remote_path: as_str_ci(item, "Path").map(str::to_owned),
                })
            })
            .collect();
        return LsResult::Folders(entries);
    }
    // Try "revisions" (case-tolerant).
    if let Some(arr) = get_ci(v, "revisions").and_then(|x| x.as_array()) {
        let revisions: Vec<FileRevision> = arr
            .iter()
            .filter_map(|item| {
                let id = as_str_ci(item, "id")?.to_string();
                Some(FileRevision {
                    id,
                    size: as_u64_ci(item, "size"),
                    checksum: as_str_ci(item, "checksum").map(str::to_owned),
                })
            })
            .collect();
        return LsResult::Revisions(revisions);
    }
    // Try a bare array of folder objects.
    if let Value::Array(arr) = v {
        let entries: Vec<RemoteEntry> = arr
            .iter()
            .filter_map(|item| {
                let name = as_str_ci(item, "Name")?.to_string();
                Some(RemoteEntry {
                    name,
                    is_dir: true,
                    size: as_u64_ci(item, "Size"),
                    remote_path: as_str_ci(item, "Path").map(str::to_owned),
                })
            })
            .collect();
        if !entries.is_empty() {
            return LsResult::Folders(entries);
        }
    }
    LsResult::Empty
}

/// One download from `jotta-cli list downloads --json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadEntry {
    /// The download ID (used for --abort / --retry / --clear).
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

/// Parse `jotta-cli list downloads --json` output.
///
/// Schema is undocumented; tolerate several shapes:
/// - Array of objects with id/status/remote/dest keys (case-tolerant).
/// - Object with "downloads" or "Downloads" array wrapper.
pub fn parse_downloads_json(v: &Value) -> Vec<DownloadEntry> {
    // Unwrap if there's a "downloads"/"Downloads" array.
    let arr = get_ci(v, "downloads")
        .and_then(|x| x.as_array())
        .or_else(|| match v {
            Value::Array(a) => Some(a.as_slice()),
            _ => None,
        });
    let Some(arr) = arr else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| {
            let id = as_str_ci(item, "id")
                .or_else(|| as_str_ci(item, "downloadid"))
                .or_else(|| as_str_ci(item, "downloadId"))?
                .to_string();
            let status = match as_str_ci(item, "status")
                .or_else(|| as_str_ci(item, "state"))
                .map(|s| s.to_lowercase())
                .as_deref()
            {
                Some("failed") | Some("error") | Some("errored") => DownloadStatus::Failed,
                Some("completed") | Some("complete") | Some("done") | Some("finished") => {
                    DownloadStatus::Completed
                }
                _ => DownloadStatus::Ongoing,
            };
            let remote_path = as_str_ci(item, "remote")
                .or_else(|| as_str_ci(item, "remotepath"))
                .or_else(|| as_str_ci(item, "remotePath"))
                .or_else(|| as_str_ci(item, "source"))
                .unwrap_or("")
                .to_string();
            let local_dest = as_str_ci(item, "dest")
                .or_else(|| as_str_ci(item, "destination"))
                .or_else(|| as_str_ci(item, "local"))
                .or_else(|| as_str_ci(item, "localdest"))
                .or_else(|| as_str_ci(item, "localDest"))
                .unwrap_or("")
                .to_string();
            Some(DownloadEntry {
                id,
                remote_path,
                local_dest,
                status,
            })
        })
        .collect()
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
    use crate::json;

    /// Real `jotta-cli ls Backup --json` output (Fedora, 2026).
    /// Note: "Desk" has NO Size field, "Laptop" does — parser must tolerate both.
    const REAL_LS_BACKUP: &str = r#"{
        "Folders": [
            {"Name": "Desk", "Path": "/backup/Desk", "Modified": 1792662822641},
            {"Name": "Laptop", "Path": "/backup/Laptop", "Modified": 1792743158517, "Size": 7867524812}
        ]
    }"#;

    /// Real `jotta-cli ls <file> --json` output (revisions).
    const REAL_LS_FILE: &str = r#"{
        "revisions": [
            {
                "id": "1",
                "size": 3778,
                "checksum": "f232b050ba20bbe45c1a15771bab281f",
                "createdAtMillis": 1791366225783,
                "modifiedAtMillis": 1761198640000
            }
        ]
    }"#;

    #[test]
    fn parses_ls_folders_real_schema() {
        let v = json::parse(REAL_LS_BACKUP).unwrap();
        match parse_ls_json(&v) {
            LsResult::Folders(entries) => {
                assert_eq!(entries.len(), 2);
                assert_eq!(entries[0].name, "Desk");
                assert!(entries[0].is_dir);
                assert_eq!(entries[0].size, None); // Size optional — Desk omits it
                assert_eq!(entries[0].remote_path.as_deref(), Some("/backup/Desk"));
                assert_eq!(entries[1].name, "Laptop");
                assert_eq!(entries[1].size, Some(7867524812));
            }
            _ => panic!("expected Folders"),
        }
    }

    #[test]
    fn parses_ls_revisions_real_schema() {
        let v = json::parse(REAL_LS_FILE).unwrap();
        match parse_ls_json(&v) {
            LsResult::Revisions(revs) => {
                assert_eq!(revs.len(), 1);
                assert_eq!(revs[0].id, "1");
                assert_eq!(revs[0].size, Some(3778));
                assert_eq!(
                    revs[0].checksum.as_deref(),
                    Some("f232b050ba20bbe45c1a15771bab281f")
                );
            }
            _ => panic!("expected Revisions"),
        }
    }

    #[test]
    fn parses_ls_case_tolerant() {
        // Lowercase "folders" should also work.
        let v = json::parse(r#"{"folders": [{"name": "X", "size": 100}]}"#).unwrap();
        match parse_ls_json(&v) {
            LsResult::Folders(entries) => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].name, "X");
                assert_eq!(entries[0].size, Some(100));
            }
            _ => panic!("expected Folders (case-tolerant)"),
        }
    }

    #[test]
    fn parses_ls_bare_array() {
        let v = json::parse(r#"[{"Name": "A"}, {"Name": "B", "Size": 50}]"#).unwrap();
        match parse_ls_json(&v) {
            LsResult::Folders(entries) => assert_eq!(entries.len(), 2),
            _ => panic!("expected Folders (bare array)"),
        }
    }

    #[test]
    fn parses_ls_empty() {
        let v = json::parse(r#"{"Folders": []}"#).unwrap();
        assert_eq!(parse_ls_json(&v), LsResult::Folders(vec![]));
        let v = json::parse(r#"{}"#).unwrap();
        assert_eq!(parse_ls_json(&v), LsResult::Empty);
    }

    #[test]
    fn parses_downloads_json_array() {
        let v = json::parse(
            r#"[
                {"id": "abc", "status": "ongoing", "remote": "Backup/Laptop/Docs", "dest": "/tmp"},
                {"id": "def", "status": "failed", "remotePath": "Archive/f.txt", "localDest": "/home"}
            ]"#,
        )
        .unwrap();
        let entries = parse_downloads_json(&v);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "abc");
        assert_eq!(entries[0].status, DownloadStatus::Ongoing);
        assert_eq!(entries[0].remote_path, "Backup/Laptop/Docs");
        assert_eq!(entries[1].status, DownloadStatus::Failed);
        assert_eq!(entries[1].remote_path, "Archive/f.txt");
        assert_eq!(entries[1].local_dest, "/home");
    }

    #[test]
    fn parses_downloads_json_wrapped() {
        let v = json::parse(
            r#"{"downloads": [{"id": "x1", "state": "completed", "source": "Sync", "destination": "/dl"}]}"#,
        )
        .unwrap();
        let entries = parse_downloads_json(&v);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "x1");
        assert_eq!(entries[0].status, DownloadStatus::Completed);
    }

    #[test]
    fn parses_downloads_empty() {
        let v = json::parse(r#"[]"#).unwrap();
        assert!(parse_downloads_json(&v).is_empty());
        let v = json::parse(r#"{}"#).unwrap();
        assert!(parse_downloads_json(&v).is_empty());
    }

    #[test]
    fn builds_remote_paths() {
        assert_eq!(backup_remote_path("Laptop", "Documents"), "Backup/Laptop/Documents");
        assert_eq!(sync_remote_path(), "Sync");
        assert_eq!(archive_remote_path(), "Archive");
    }
}
