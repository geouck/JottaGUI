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
    /// Modified timestamp (millis since epoch), if present.
    pub modified: Option<u64>,
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
/// Verified schemas (case-tolerant, from real jotta-cli on Fedora):
/// - Folder listing: `{"Folders": [{"Name","Path","Modified","Size"?}], "Files": [{"Name","Path","Checksum","Size","Created","Modified"}]}`
///   NB: Der er INGEN type/dir-kolonne — fil vs mappe afgøres AF ARRAY-MEDLEMSKAB.
///   `Size` er OPTIONAL på Folders (nogle enheder omitter den).
/// - File revisions: `{"revisions": [{"id","size","checksum",...}]}`
///
/// Also tolerates a bare array of folder objects (no wrapper key).
pub fn parse_ls_json(v: &Value) -> LsResult {
    // Try "revisions" first (file listing — distinct schema).
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

    // Folder listing: læs BÅDE "Folders" og "Files" arrays.
    // Fil vs mappe afgøres af hvilket array elementet står i.
    let folders_arr = get_ci(v, "Folders").and_then(|x| x.as_array());
    let files_arr = get_ci(v, "Files").and_then(|x| x.as_array());

    if folders_arr.is_some() || files_arr.is_some() {
        let mut entries: Vec<RemoteEntry> = Vec::new();
        // Folders først (mapper øverst i browseren).
        if let Some(arr) = folders_arr {
            for item in arr {
                if let Some(name) = as_str_ci(item, "Name") {
                    entries.push(RemoteEntry {
                        name: name.to_string(),
                        is_dir: true,
                        size: as_u64_ci(item, "Size"),
                        remote_path: as_str_ci(item, "Path").map(str::to_owned),
                        modified: as_u64_ci(item, "Modified"),
                    });
                }
            }
        }
        // Files derefter.
        if let Some(arr) = files_arr {
            for item in arr {
                if let Some(name) = as_str_ci(item, "Name") {
                    entries.push(RemoteEntry {
                        name: name.to_string(),
                        is_dir: false,
                        size: as_u64_ci(item, "Size"),
                        remote_path: as_str_ci(item, "Path").map(str::to_owned),
                        modified: as_u64_ci(item, "Modified"),
                    });
                }
            }
        }
        return LsResult::Folders(entries);
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
                    modified: as_u64_ci(item, "Modified"),
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
    /// Error messages from the "Errors" object (empty if no errors).
    /// VERIFICeret (Fedora, 2026): `list downloaderrors` KRÆVER --downloadid,
    /// så vi udleder fejlbeskeder direkte fra hver download-entry.
    pub errors: Vec<String>,
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
            // VERIFICERET real schema (Fedora, 2026): keys are "Id", "Remote", "Local".
            // Fall back to lowercase variants for forward-compat.
            let id = as_str_ci(item, "Id")
                .or_else(|| as_str_ci(item, "id"))
                .or_else(|| as_str_ci(item, "downloadid"))
                .or_else(|| as_str_ci(item, "downloadId"))?
                .to_string();

            // Status: real schema has "CompletedTimeMs" (set = completed) and
            // "Errors" object (non-empty = failed). Also check explicit status/state keys.
            let status = {
                // Explicit status/state string first.
                if let Some(s) = as_str_ci(item, "status")
                    .or_else(|| as_str_ci(item, "state"))
                    .map(|s| s.to_lowercase())
                {
                    match s.as_str() {
                        "failed" | "error" | "errored" => DownloadStatus::Failed,
                        "completed" | "complete" | "done" | "finished" => {
                            DownloadStatus::Completed
                        }
                        _ => DownloadStatus::Ongoing,
                    }
                } else {
                    // Real schema heuristics:
                    // - CompletedTimeMs present → completed
                    // - Errors is a non-empty object/array → failed
                    // - Otherwise → ongoing
                    let completed = get_ci(item, "CompletedTimeMs").is_some()
                        || get_ci(item, "completedtimems").is_some();
                    let errors = get_ci(item, "Errors")
                        .map(|e| match e {
                            Value::Object(m) => !m.is_empty(),
                            Value::Array(a) => !a.is_empty(),
                            _ => false,
                        })
                        .unwrap_or(false);
                    if errors {
                        DownloadStatus::Failed
                    } else if completed {
                        DownloadStatus::Completed
                    } else {
                        DownloadStatus::Ongoing
                    }
                }
            };

            // Remote path: "Remote" (real schema) or fallbacks.
            let remote_path = as_str_ci(item, "Remote")
                .or_else(|| as_str_ci(item, "remote"))
                .or_else(|| as_str_ci(item, "remotepath"))
                .or_else(|| as_str_ci(item, "remotePath"))
                .or_else(|| as_str_ci(item, "source"))
                .unwrap_or("")
                .to_string();

            // Local dest: "Local" (real schema) or fallbacks.
            let local_dest = as_str_ci(item, "Local")
                .or_else(|| as_str_ci(item, "dest"))
                .or_else(|| as_str_ci(item, "destination"))
                .or_else(|| as_str_ci(item, "local"))
                .or_else(|| as_str_ci(item, "localdest"))
                .or_else(|| as_str_ci(item, "localDest"))
                .unwrap_or("")
                .to_string();

            // Errors: extract message strings from the "Errors" object/array.
            // VERIFICeret: `list downloaderrors` kræver --downloadid, så vi
            // udleder fejlbeskeder direkte fra hver entry's "Errors" felt.
            let errors = extract_error_messages(item);

            Some(DownloadEntry {
                id,
                remote_path,
                local_dest,
                status,
                errors,
            })
        })
        .collect()
}

/// One upload from `jotta-cli list uploads --json`.
///
/// Schema is expected to mirror `list downloads --json` (same CLI family):
/// array of objects with Id/Remote/Local/Total/Remaining/StartedTimeMs/etc.
/// Parser is case-tolerant and tolerant of empty objects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadEntry {
    /// The upload ID.
    pub id: String,
    /// Remote path being uploaded (e.g. local file → remote).
    pub remote_path: String,
    /// Local source path.
    pub local_path: String,
    /// Whether the upload has failed or is ongoing.
    pub status: UploadStatus,
    /// Error messages from the "Errors" object (empty if no errors).
    /// VERIFICeret (Fedora, 2026): `list uploaderrors` KRÆVER --uploadid,
    /// så vi udleder fejlbeskeder direkte fra hver upload-entry.
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadStatus {
    Ongoing,
    Failed,
    Completed,
}

/// Parse `jotta-cli list uploads --json` output.
///
/// Expected schema mirrors `list downloads --json`:
/// - Array of objects with Id/Remote/Local/Total/Remaining keys (case-tolerant).
/// - Object with "uploads" or "Uploads" array wrapper.
/// - Empty objects tolerated (Remaining/Errors can be `{}`).
pub fn parse_uploads_json(v: &Value) -> Vec<UploadEntry> {
    let arr = get_ci(v, "uploads")
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
            let id = as_str_ci(item, "Id")
                .or_else(|| as_str_ci(item, "id"))
                .or_else(|| as_str_ci(item, "uploadid"))
                .or_else(|| as_str_ci(item, "uploadId"))?
                .to_string();

            let status = {
                if let Some(s) = as_str_ci(item, "status")
                    .or_else(|| as_str_ci(item, "state"))
                    .map(|s| s.to_lowercase())
                {
                    match s.as_str() {
                        "failed" | "error" | "errored" => UploadStatus::Failed,
                        "completed" | "complete" | "done" | "finished" => {
                            UploadStatus::Completed
                        }
                        _ => UploadStatus::Ongoing,
                    }
                } else {
                    let completed = get_ci(item, "CompletedTimeMs").is_some()
                        || get_ci(item, "completedtimems").is_some();
                    let errors = get_ci(item, "Errors")
                        .map(|e| match e {
                            Value::Object(m) => !m.is_empty(),
                            Value::Array(a) => !a.is_empty(),
                            _ => false,
                        })
                        .unwrap_or(false);
                    if errors {
                        UploadStatus::Failed
                    } else if completed {
                        UploadStatus::Completed
                    } else {
                        UploadStatus::Ongoing
                    }
                }
            };

            let remote_path = as_str_ci(item, "Remote")
                .or_else(|| as_str_ci(item, "remote"))
                .or_else(|| as_str_ci(item, "remotepath"))
                .or_else(|| as_str_ci(item, "remotePath"))
                .unwrap_or("")
                .to_string();

            let local_path = as_str_ci(item, "Local")
                .or_else(|| as_str_ci(item, "local"))
                .or_else(|| as_str_ci(item, "source"))
                .or_else(|| as_str_ci(item, "localsource"))
                .or_else(|| as_str_ci(item, "localSource"))
                .unwrap_or("")
                .to_string();

            let errors = extract_error_messages(item);

            Some(UploadEntry {
                id,
                remote_path,
                local_path,
                status,
                errors,
            })
        })
        .collect()
}

/// One transfer error from `jotta-cli list downloaderrors --json` or
/// `jotta-cli list uploaderrors --json`.
///
/// Schema is undocumented; tolerant parsing of common shapes:
/// - Array of objects with error/message/path keys.
/// - Object with "errors"/"Errors" array wrapper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferError {
    /// The transfer ID (download or upload ID) this error belongs to.
    pub transfer_id: Option<String>,
    /// The file/path that caused the error.
    pub path: Option<String>,
    /// The error message.
    pub message: String,
}

/// Collect transfer errors from download + upload entries.
///
/// VERIFICeret (Fedora, 2026): `list downloaderrors` og `list uploaderrors`
/// KRÆVER hhv. --downloadid / --uploadid — der er ingen "list alle"-mode.
/// Fejlbeskeder udledes derfor direkte fra hver entry's "Errors"-objekt.
///
/// Returns a combined Vec<TransferError> with download errors first,
/// then upload errors.
pub fn collect_transfer_errors(
    downloads: &[DownloadEntry],
    uploads: &[UploadEntry],
) -> Vec<TransferError> {
    let mut errors = Vec::new();
    for d in downloads {
        for msg in &d.errors {
            errors.push(TransferError {
                transfer_id: Some(d.id.clone()),
                path: Some(d.remote_path.clone()),
                message: msg.clone(),
            });
        }
    }
    for u in uploads {
        for msg in &u.errors {
            errors.push(TransferError {
                transfer_id: Some(u.id.clone()),
                path: Some(u.local_path.clone()),
                message: msg.clone(),
            });
        }
    }
    errors
}

/// Parse `jotta-cli list downloaderrors --json` or `list uploaderrors --json`.
///
/// Tolerant of several shapes:
/// - `[{"id": "...", "path": "...", "message": "..."}]`
/// - `{"errors": [{"message": "..."}]}`
/// - `[{"error": "..."}]` (bare string in "error" key)
pub fn parse_transfer_errors_json(v: &Value) -> Vec<TransferError> {
    let arr = get_ci(v, "errors")
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
            let transfer_id = as_str_ci(item, "id")
                .or_else(|| as_str_ci(item, "downloadid"))
                .or_else(|| as_str_ci(item, "downloadId"))
                .or_else(|| as_str_ci(item, "uploadid"))
                .or_else(|| as_str_ci(item, "uploadId"))
                .map(str::to_owned);
            let path = as_str_ci(item, "path")
                .or_else(|| as_str_ci(item, "file"))
                .or_else(|| as_str_ci(item, "remote"))
                .map(str::to_owned);
            let message = as_str_ci(item, "message")
                .or_else(|| as_str_ci(item, "error"))
                .or_else(|| as_str_ci(item, "reason"))
                .or_else(|| as_str_ci(item, "description"))
                .unwrap_or("Ukendt fejl")
                .to_string();
            Some(TransferError {
                transfer_id,
                path,
                message,
            })
        })
        .collect()
}

/// Extract error message strings from an "Errors" object or array.
///
/// Real schema (Fedora, 2026): `"Errors": {"some_error": "file not found"}`
/// or `"Errors": {}` (empty = no errors). Also tolerates arrays of strings
/// or objects with message/error keys.
fn extract_error_messages(item: &Value) -> Vec<String> {
    let Some(errors_val) = get_ci(item, "Errors") else {
        return Vec::new();
    };
    match errors_val {
        Value::Object(map) => map
            .values()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        Value::Array(arr) => arr
            .iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s.clone()),
                Value::Object(_) => as_str_ci(v, "message")
                    .or_else(|| as_str_ci(v, "error"))
                    .map(str::to_owned),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
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

    /// VERIFICERET schema fra `ls "Backup/Laptop/Documents" --json` (Fedora-test, iteration 6d).
    /// NB: INGEN type/dir-kolonne — fil vs mappe afgøres AF ARRAY-MEDLEMSKAB.
    /// Filnavne rummer mellemrum, æøå, em-streger og "_".
    const REAL_LS_MIXED: &str = r#"{
        "Folders": [
            {"Name": "Undermappe — test", "Path": "/backup/Laptop/Documents/Undermappe — test", "Modified": 1792743158517}
        ],
        "Files": [
            {"Name": "CV Eske Aller — 2024.pdf", "Path": "/backup/Laptop/Documents/CV Eske Aller — 2024.pdf", "Checksum": "abc123", "Size": 245760, "Created": 1791366225783, "Modified": 1792743158517},
            {"Name": "noter_æøå.txt", "Path": "/backup/Laptop/Documents/noter_æøå.txt", "Checksum": "def456", "Size": 1024, "Created": 1791366225783, "Modified": 1792743158517}
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
    fn parses_ls_mixed_folders_and_files() {
        // VERIFICERET: fil vs mappe afgøres af array-medlemskab (ingen type-kolonne).
        let v = json::parse(REAL_LS_MIXED).unwrap();
        match parse_ls_json(&v) {
            LsResult::Folders(entries) => {
                assert_eq!(entries.len(), 3);
                // Først: mappen
                assert_eq!(entries[0].name, "Undermappe — test");
                assert!(entries[0].is_dir);
                assert_eq!(entries[0].size, None); // Folders har ikke altid Size
                // Så: filerne
                assert_eq!(entries[1].name, "CV Eske Aller — 2024.pdf");
                assert!(!entries[1].is_dir);
                assert_eq!(entries[1].size, Some(245760));
                assert_eq!(entries[1].modified, Some(1792743158517));
                assert_eq!(entries[2].name, "noter_æøå.txt");
                assert!(!entries[2].is_dir);
                assert_eq!(entries[2].size, Some(1024));
            }
            _ => panic!("expected Folders (mixed)"),
        }
    }

    #[test]
    fn parses_ls_files_only() {
        // Kun Files array, ingen Folders — skal stadig virke.
        let v = json::parse(r#"{"Files": [{"Name": "a.txt", "Size": 100, "Modified": 1234}]}"#).unwrap();
        match parse_ls_json(&v) {
            LsResult::Folders(entries) => {
                assert_eq!(entries.len(), 1);
                assert!(!entries[0].is_dir);
                assert_eq!(entries[0].name, "a.txt");
            }
            _ => panic!("expected Folders (files only)"),
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

    /// REAL fixture fra `jotta-cli list downloads --json` (Fedora, 2026).
    /// NB: Remaining/SelectionCount/Errors kan være TOMME objekter.
    /// Keys er PascalCase: Id, Remote, Local, Total, Remaining, CompletedTimeMs, etc.
    const REAL_DOWNLOADS_JSON: &str = r#"[
        {
            "Id": "d1",
            "Remote": "Backup/Laptop/Documents/CV Eske Aller — 2024.pdf",
            "Local": "/home/eske/Downloads/CV Eske Aller — 2024.pdf",
            "Total": {"Files": 1, "Bytes": 245760},
            "Remaining": {"Files": 1, "Bytes": 245760},
            "StartedTimeMs": 1792743158517,
            "SelectionCount": {},
            "Errors": {}
        },
        {
            "Id": "d2",
            "Remote": "Archive/old_folder",
            "Local": "/home/eske/Downloads/old_folder",
            "Total": {"Files": 5, "Bytes": 1024000},
            "Remaining": {},
            "StartedTimeMs": 1792743000000,
            "CompletedTimeMs": 1792743100000,
            "SelectionCount": {},
            "Errors": {}
        },
        {
            "Id": "d3",
            "Remote": "Sync/broken_file.txt",
            "Local": "/home/eske/Downloads/broken_file.txt",
            "Total": {"Files": 1, "Bytes": 100},
            "Remaining": {"Files": 1, "Bytes": 100},
            "StartedTimeMs": 1792742000000,
            "SelectionCount": {},
            "Errors": {"some_error": "file not found"}
        }
    ]"#;

    #[test]
    fn parses_downloads_real_schema() {
        let v = json::parse(REAL_DOWNLOADS_JSON).unwrap();
        let entries = parse_downloads_json(&v);
        assert_eq!(entries.len(), 3);

        // d1: ongoing (Remaining non-empty, no CompletedTimeMs, Errors empty)
        assert_eq!(entries[0].id, "d1");
        assert_eq!(entries[0].remote_path, "Backup/Laptop/Documents/CV Eske Aller — 2024.pdf");
        assert_eq!(entries[0].local_dest, "/home/eske/Downloads/CV Eske Aller — 2024.pdf");
        assert_eq!(entries[0].status, DownloadStatus::Ongoing);

        // d2: completed (CompletedTimeMs present, Errors empty)
        assert_eq!(entries[1].id, "d2");
        assert_eq!(entries[1].status, DownloadStatus::Completed);

        // d3: failed (Errors non-empty)
        assert_eq!(entries[2].id, "d3");
        assert_eq!(entries[2].status, DownloadStatus::Failed);
    }

    #[test]
    fn builds_remote_paths() {
        assert_eq!(backup_remote_path("Laptop", "Documents"), "Backup/Laptop/Documents");
        assert_eq!(sync_remote_path(), "Sync");
        assert_eq!(archive_remote_path(), "Archive");
    }

    // --- Iteration 7: uploads + transfer errors ---

    /// Placeholder fixture for `list uploads --json` (mirrors downloads schema).
    /// Owner will provide real JSON later.
    const PLACEHOLDER_UPLOADS_JSON: &str = r#"[
        {
            "Id": "u1",
            "Remote": "Sync/Documents/photo.jpg",
            "Local": "/home/eske/Documents/photo.jpg",
            "Total": {"Files": 1, "Bytes": 1048576},
            "Remaining": {"Files": 1, "Bytes": 524288},
            "StartedTimeMs": 1792743158517,
            "SelectionCount": {},
            "Errors": {}
        },
        {
            "Id": "u2",
            "Remote": "Backup/Laptop/CV.pdf",
            "Local": "/home/eske/Documents/CV.pdf",
            "Total": {"Files": 1, "Bytes": 245760},
            "Remaining": {},
            "StartedTimeMs": 1792743000000,
            "CompletedTimeMs": 1792743100000,
            "SelectionCount": {},
            "Errors": {}
        },
        {
            "Id": "u3",
            "Remote": "Sync/broken.txt",
            "Local": "/home/eske/Documents/broken.txt",
            "Total": {"Files": 1, "Bytes": 100},
            "Remaining": {"Files": 1, "Bytes": 100},
            "StartedTimeMs": 1792742000000,
            "SelectionCount": {},
            "Errors": {"io_error": "permission denied"}
        }
    ]"#;

    #[test]
    fn parses_uploads_placeholder_schema() {
        let v = json::parse(PLACEHOLDER_UPLOADS_JSON).unwrap();
        let entries = parse_uploads_json(&v);
        assert_eq!(entries.len(), 3);

        // u1: ongoing
        assert_eq!(entries[0].id, "u1");
        assert_eq!(entries[0].remote_path, "Sync/Documents/photo.jpg");
        assert_eq!(entries[0].local_path, "/home/eske/Documents/photo.jpg");
        assert_eq!(entries[0].status, UploadStatus::Ongoing);

        // u2: completed
        assert_eq!(entries[1].id, "u2");
        assert_eq!(entries[1].status, UploadStatus::Completed);

        // u3: failed
        assert_eq!(entries[2].id, "u3");
        assert_eq!(entries[2].status, UploadStatus::Failed);
    }

    #[test]
    fn parses_uploads_wrapped_and_empty() {
        // Wrapped in {"uploads": [...]}.
        let v = json::parse(r#"{"uploads": [{"id": "x1", "status": "ongoing", "remote": "Sync/a.txt", "local": "/tmp/a.txt"}]}"#).unwrap();
        let entries = parse_uploads_json(&v);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "x1");
        assert_eq!(entries[0].status, UploadStatus::Ongoing);

        // Empty.
        let v = json::parse(r#"[]"#).unwrap();
        assert!(parse_uploads_json(&v).is_empty());
        let v = json::parse(r#"{}"#).unwrap();
        assert!(parse_uploads_json(&v).is_empty());
    }

    #[test]
    fn parses_transfer_errors_array() {
        let v = json::parse(r#"[
            {"id": "d1", "path": "Backup/Laptop/file.pdf", "message": "Network timeout"},
            {"downloadid": "d2", "file": "Sync/broken.txt", "error": "Permission denied"}
        ]"#).unwrap();
        let errors = parse_transfer_errors_json(&v);
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].transfer_id.as_deref(), Some("d1"));
        assert_eq!(errors[0].path.as_deref(), Some("Backup/Laptop/file.pdf"));
        assert_eq!(errors[0].message, "Network timeout");
        assert_eq!(errors[1].transfer_id.as_deref(), Some("d2"));
        assert_eq!(errors[1].path.as_deref(), Some("Sync/broken.txt"));
        assert_eq!(errors[1].message, "Permission denied");
    }

    #[test]
    fn parses_transfer_errors_wrapped_and_empty() {
        // Wrapped in {"errors": [...]}.
        let v = json::parse(r#"{"errors": [{"message": "Disk full"}]}"#).unwrap();
        let errors = parse_transfer_errors_json(&v);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].message, "Disk full");
        assert!(errors[0].transfer_id.is_none());
        assert!(errors[0].path.is_none());

        // Empty.
        let v = json::parse(r#"[]"#).unwrap();
        assert!(parse_transfer_errors_json(&v).is_empty());
        let v = json::parse(r#"{}"#).unwrap();
        assert!(parse_transfer_errors_json(&v).is_empty());
    }

    #[test]
    fn extracts_errors_from_download_entry() {
        // Real schema: Errors is an object with key→message.
        let v = json::parse(r#"[
            {"Id": "d1", "Remote": "Backup/x.pdf", "Local": "/tmp/x.pdf", "Errors": {"io_error": "file not found"}},
            {"Id": "d2", "Remote": "Sync/y.txt", "Local": "/tmp/y.txt", "Errors": {}}
        ]"#).unwrap();
        let entries = parse_downloads_json(&v);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].errors, vec!["file not found".to_string()]);
        assert!(entries[1].errors.is_empty());
    }

    #[test]
    fn extracts_errors_from_upload_entry() {
        let v = json::parse(r#"[
            {"Id": "u1", "Remote": "Sync/a.txt", "Local": "/tmp/a.txt", "Errors": {"net_err": "timeout"}},
            {"Id": "u2", "Remote": "Backup/b.txt", "Local": "/tmp/b.txt", "Errors": {}}
        ]"#).unwrap();
        let entries = parse_uploads_json(&v);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].errors, vec!["timeout".to_string()]);
        assert!(entries[1].errors.is_empty());
    }

    #[test]
    fn collects_transfer_errors_from_entries() {
        let v_dl = json::parse(r#"[
            {"Id": "d1", "Remote": "Backup/x.pdf", "Local": "/tmp/x.pdf", "Errors": {"io": "not found"}}
        ]"#).unwrap();
        let downloads = parse_downloads_json(&v_dl);

        let v_ul = json::parse(r#"[
            {"Id": "u1", "Remote": "Sync/a.txt", "Local": "/home/a.txt", "Errors": {"net": "timeout"}}
        ]"#).unwrap();
        let uploads = parse_uploads_json(&v_ul);

        let errors = collect_transfer_errors(&downloads, &uploads);
        assert_eq!(errors.len(), 2);
        // Download error first.
        assert_eq!(errors[0].transfer_id.as_deref(), Some("d1"));
        assert_eq!(errors[0].path.as_deref(), Some("Backup/x.pdf"));
        assert_eq!(errors[0].message, "not found");
        // Upload error second.
        assert_eq!(errors[1].transfer_id.as_deref(), Some("u1"));
        assert_eq!(errors[1].path.as_deref(), Some("/home/a.txt"));
        assert_eq!(errors[1].message, "timeout");
    }

    #[test]
    fn parses_uploads_empty_object() {
        // VERIFICeret (Fedora, 2026): `list uploads --json` returnerer `{}`
        // når der ikke er nogen uploads.
        let v = json::parse(r#"{}"#).unwrap();
        assert!(parse_uploads_json(&v).is_empty());
    }

    #[test]
    fn parses_downloads_empty_object() {
        // Samme for downloads.
        let v = json::parse(r#"{}"#).unwrap();
        assert!(parse_downloads_json(&v).is_empty());
    }
}
