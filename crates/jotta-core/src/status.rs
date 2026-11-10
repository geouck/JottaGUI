//! Tolerant status model.
//!
//! jotta-cli's status --json schema is not formally documented and moves
//! between releases, so nothing here is allowed to panic or hard-fail:
//! every field is an Option, extracted by best effort from the JSON tree.
//! The GUI renders "unknown" for missing fields.
//!
//! Verified against real output from jotta-cli (Fedora, 2026):
//! - User.Email, User.AccountInfo.Usage / .Capacity
//! - Sync.Enabled / .Automatic / .RootPath / .Count.{Files,Bytes}
//! - Backup.State.Enabled.Backups[] = {Name, Path, Count.{Files,Bytes}}
//! - State.Uploading / .Downloading / .RestoreWorking (objects, empty = idle)

use crate::json::Value;

/// What the tray icon and dashboard need at a glance.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    pub logged_in: Option<bool>,
    pub account_email: Option<String>,
    pub paused: Option<bool>,
    /// Bytes used, if reported.
    pub bytes_used: Option<u64>,
    /// Quota in bytes, if reported (e.g. 5 TB plan).
    pub bytes_quota: Option<u64>,
    /// Number of active transfers, if reported.
    pub active_transfers: Option<u64>,
    /// In-flight uploads with detail (path/bytes) from `State.Uploading`
    /// (iteration 14+: source for "current file" + live speed).
    pub active_uploads: Vec<ActiveTransfer>,
    /// In-flight downloads with detail from `State.Downloading`.
    pub active_downloads: Vec<ActiveTransfer>,
    /// Backup folders configured on this device.
    pub backups: Vec<BackupFolder>,
    /// Sync folder state, if a sync is configured.
    pub sync: Option<SyncInfo>,
    /// Best-effort state machine for the tray icon.
    pub state: State,
    /// Device name (from `User.device.Name`) — used to build remote paths
    /// like `Backup/<device_name>/<backup_name>` for `jotta-cli download`.
    pub device_name: Option<String>,
    /// Whether a restore/download is currently in progress
    /// (from `State.RestoreWorking`).
    pub restore_working: bool,
    /// Subscription/product name (from `User.AccountInfo.SubscriptionNameLocalized`
    /// or `User.AccountInfo.ProductNameLocalized`).
    pub subscription: Option<String>,
    /// Account type (from `User.AccountInfo.AccountType`).
    pub account_type: Option<String>,
}

/// One backup folder entry (from `Backup.State.Enabled.Backups[]`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackupFolder {
    pub name: String,
    pub path: String,
    pub files: Option<u64>,
    pub bytes: Option<u64>,
}

/// Sync folder state (from the `Sync` object).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncInfo {
    pub enabled: bool,
    pub automatic: bool,
    pub root_path: Option<String>,
    pub files: Option<u64>,
    pub bytes: Option<u64>,
}

/// One in-flight transfer as reported inside `State.Uploading` / `State.Downloading`
/// in `status --json` (objects keyed by transfer id; values may carry path and
/// byte counters — schema undocumented, so every field is optional).
///
/// This is the best source for "which file is being transferred right now" and
/// for live speed (delta of `bytes_done` between polls), because `list uploads`
/// does not reliably show sync-triggered transfers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActiveTransfer {
    pub path: Option<String>,
    /// Bytes already transferred, if reported.
    pub bytes_done: Option<u64>,
    /// Total bytes of the transfer, if reported.
    pub total_bytes: Option<u64>,
}

/// Parse the `State.Uploading` / `State.Downloading` value into active transfers.
/// Tolerates object-keyed-by-id, bare arrays, and one level of nesting.
fn parse_active_transfers(v: Option<&Value>) -> Vec<ActiveTransfer> {
    fn from_obj(obj: &Value, depth: usize) -> Option<ActiveTransfer> {
        let path = ["Path", "Remote", "Local", "File", "FilePath", "Name", "Source", "Destination"]
            .iter()
            .find_map(|k| obj.as_str_ci(k))
            .map(str::to_owned);
        // Key names verified against a working third-party widget
        // (lorentzfb/omarchy-jottacloud) reading the ACTIVE schema of
        // State.Uploading / Backups[].Uploading maps:
        //   total = TotalBytes | Bytes | Total.Bytes | Size
        //   done  = CompletedBytes | Completed.Bytes | Progress
        // Extra best-effort guesses are kept below those.
        let nested_bytes = |key: &str| -> Option<u64> {
            let as_u = |v: &Value| v.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64);
            match obj.get_ci(key) {
                Some(Value::Object(inner)) => inner.values().find_map(as_u),
                Some(n) => as_u(n),
                None => None,
            }
        };
        let total = ["TotalBytes", "TotalSize", "FileSize", "Size"]
            .iter()
            .find_map(|k| obj.as_u64_ci(k))
            .or_else(|| nested_bytes("Total"))
            .or_else(|| obj.as_u64_ci("Bytes"));
        let bytes_done = [
            "CompletedBytes", "BytesDone", "Done", "Uploaded", "Downloaded", "Transferred",
            "BytesTransferred", "Progress",
        ]
        .iter()
        .find_map(|k| obj.as_u64_ci(k))
        .or_else(|| nested_bytes("Completed"))
        // "Bytes" as done ONLY when it was not already consumed as total.
        .or_else(|| {
            let b = obj.as_u64_ci("Bytes");
            if total.is_some() && total != b { b } else { None }
        });
        if path.is_some() || bytes_done.is_some() || total.is_some() {
            return Some(ActiveTransfer {
                path,
                bytes_done,
                total_bytes: total,
            });
        }
        // One level of nesting fallback (e.g. {"u1": {"transfer": {...}}}).
        if depth == 0 {
            if let Value::Object(map) = obj {
                for val in map.values() {
                    if let Value::Object(_) = val {
                        if let Some(t) = from_obj(val, 1) {
                            return Some(t);
                        }
                    }
                }
            }
        }
        None
    }
    let Some(v) = v else { return Vec::new() };
    let items: Vec<&Value> = match v {
        Value::Object(map) => map.values().collect(),
        Value::Array(arr) => arr.iter().collect(),
        _ => return Vec::new(),
    };
    items.iter().filter_map(|item| match item {
        Value::Object(_) => from_obj(item, 0),
        _ => None,
    }).collect()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum State {
    /// Daemon not reachable.
    #[default]
    Offline,
    /// Reachable but not logged in.
    NeedsLogin,
    /// Logged in, paused.
    Paused,
    /// Logged in, active transfers.
    Syncing,
    /// Logged in, idle, all good.
    Idle,
}

fn as_u64(v: &Value) -> Option<u64> {
    v.as_f64().map(|n| n as u64)
}

impl Status {
    /// Extract a Status from jotta-cli status --json output.
    ///
    /// Extraction is deliberately forgiving: unknown schemas produce a
    /// Status with state Offline + whatever fields matched, never an error.
    pub fn from_json(v: &Value) -> Self {
        let mut s = Status::default();

        // Account (verified schema: User.Email, User.AccountInfo.{Usage,Capacity}).
        // Tolerant fallback to the older lowercase keys.
        s.logged_in = v
            .path(&["User", "Email"])
            .or_else(|| v.path(&["user"]))
            .map(|_| true);
        s.account_email = v
            .path(&["User", "Email"])
            .or_else(|| v.path(&["user", "email"]))
            .or_else(|| v.path(&["account", "email"]))
            .and_then(|x| x.as_str())
            .map(str::to_owned);
        s.bytes_used = v
            .path(&["User", "AccountInfo", "Usage"])
            .or_else(|| v.path(&["account", "used"]))
            .or_else(|| v.path(&["usage", "used"]))
            .and_then(as_u64);
        s.bytes_quota = v
            .path(&["User", "AccountInfo", "Capacity"])
            .or_else(|| v.path(&["account", "quota"]))
            .or_else(|| v.path(&["usage", "quota"]))
            .and_then(as_u64);

        // Paused: jotta-cli status --json rapporterer ikke altid et "paused" felt.
        // Vi kigger efter flere mulige nøgler (case-tolerant via get_ci).
        // Hvis ingen findes, forbliver paused = None (ukendt — ikke "ikke pauset").
        s.paused = v
            .path(&["paused"])
            .or_else(|| v.path(&["Paused"]))
            .or_else(|| v.path(&["State", "Paused"]))
            .or_else(|| v.path(&["state", "paused"]))
            .and_then(|x| x.as_bool());

        // Active transfers (verified schema: State.Uploading / .Downloading are
        // objects; non-empty means something is in flight). Tolerant fallback
        // to the older "uploads"/"transfers" arrays.
        s.active_transfers = v
            .path(&["State", "Uploading"])
            .or_else(|| v.path(&["State", "Downloading"]))
            .and_then(|x| match x {
                Value::Object(map) => Some(map.len() as u64),
                _ => None,
            })
            .or_else(|| {
                v.path(&["uploads"])
                    .or_else(|| v.path(&["transfers"]))
                    .and_then(|x| x.as_array())
                    .map(|a| a.len() as u64)
            });

        // Active transfer detail (iteration 14+): State.Uploading / State.Downloading
        // objects, keyed by transfer id. Case-tolerant lookup; empty = idle.
        let state_ci = v.get_ci("State");
        s.active_uploads = parse_active_transfers(
            state_ci
                .and_then(|st| st.get_ci("Uploading"))
                .or_else(|| v.get_ci("Uploading"))
                .or_else(|| v.get_ci("uploads")),
        );
        s.active_downloads = parse_active_transfers(
            state_ci
                .and_then(|st| st.get_ci("Downloading"))
                .or_else(|| v.get_ci("Downloading"))
                .or_else(|| v.get_ci("downloads")),
        );

        // Backup folders (verified schema: Backup.State.Enabled.Backups[]).
        // Each backup entry ALSO carries its own "Uploading" object (verified in
        // real status --json: "Uploading": {} inside Backups[]) — active BACKUP
        // uploads show up there, while State.Uploading covers sync. Merge both.
        if let Some(arr) = v
            .path(&["Backup", "State", "Enabled", "Backups"])
            .and_then(|x| x.as_array())
        {
            for b in arr {
                if let Some(up) = b.get_ci("Uploading") {
                    s.active_uploads.extend(parse_active_transfers(Some(up)));
                }
            }
            s.backups = arr
                .iter()
                .map(|b| BackupFolder {
                    name: b
                        .path(&["Name"])
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_owned(),
                    path: b
                        .path(&["Path"])
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_owned(),
                    files: b.path(&["Count", "Files"]).and_then(as_u64),
                    bytes: b.path(&["Count", "Bytes"]).and_then(as_u64),
                })
                .collect();
        }

        // Sync (verified schema: Sync.{Enabled,Automatic,RootPath,Count}).
        if let Some(sync_v) = v.path(&["Sync"]) {
            s.sync = Some(SyncInfo {
                enabled: sync_v
                    .path(&["Enabled"])
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false),
                automatic: sync_v
                    .path(&["Automatic"])
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false),
                root_path: sync_v
                    .path(&["RootPath"])
                    .and_then(|x| x.as_str())
                    .map(str::to_owned),
                files: sync_v.path(&["Count", "Files"]).and_then(as_u64),
                bytes: sync_v.path(&["Count", "Bytes"]).and_then(as_u64),
            });
        }

        // Device name (verified schema: User.device.Name) — used for remote paths.
        s.device_name = v
            .path(&["User", "device", "Name"])
            .or_else(|| v.path(&["User", "Device", "Name"]))
            .or_else(|| v.path(&["device", "name"]))
            .and_then(|x| x.as_str())
            .map(str::to_owned);

        // RestoreWorking (verified schema: State.RestoreWorking is a bool).
        s.restore_working = v
            .path(&["State", "RestoreWorking"])
            .and_then(|x| x.as_bool())
            .unwrap_or(false);

        // Subscription / account type (iteration 8: Info page).
        // Verified schema: User.AccountInfo.SubscriptionNameLocalized,
        // User.AccountInfo.ProductNameLocalized, User.AccountInfo.AccountType.
        s.subscription = v
            .path(&["User", "AccountInfo", "SubscriptionNameLocalized"])
            .or_else(|| v.path(&["User", "AccountInfo", "ProductNameLocalized"]))
            .or_else(|| v.path(&["User", "AccountInfo", "SubscriptionName"]))
            .or_else(|| v.path(&["User", "AccountInfo", "ProductName"]))
            .and_then(|x| x.as_str())
            .map(str::to_owned);
        s.account_type = v
            .path(&["User", "AccountInfo", "AccountType"])
            .or_else(|| v.path(&["User", "AccountInfo", "accountType"]))
            .and_then(|x| x.as_str())
            .map(str::to_owned);

        s.state = s.compute_state();
        s
    }

    fn compute_state(&self) -> State {
        if self.logged_in != Some(true) {
            if self.logged_in.is_none() && self.account_email.is_none() {
                return State::Offline;
            }
            return State::NeedsLogin;
        }
        if self.paused == Some(true) {
            return State::Paused;
        }
        if self.active_transfers.unwrap_or(0) > 0 {
            return State::Syncing;
        }
        State::Idle
    }

    /// Sum of bytes_done across active uploads (0 when counters are absent).
    pub fn active_upload_bytes_done(&self) -> u64 {
        self.active_uploads.iter().filter_map(|t| t.bytes_done).sum()
    }

    /// Sum of bytes_done across active downloads (0 when counters are absent).
    pub fn active_download_bytes_done(&self) -> u64 {
        self.active_downloads.iter().filter_map(|t| t.bytes_done).sum()
    }

    /// True when status reports in-flight transfers at all.
    pub fn has_active_transfers(&self) -> bool {
        !self.active_uploads.is_empty() || !self.active_downloads.is_empty()
    }

    /// True when at least one active transfer carries byte counters — then
    /// active_*_bytes_done() is a usable speed source (delta between polls).
    pub fn active_transfers_have_byte_counters(&self) -> bool {
        self.active_uploads.iter().any(|t| t.bytes_done.is_some())
            || self.active_downloads.iter().any(|t| t.bytes_done.is_some())
    }

    /// Build a Status from a failed `status --json` run.
    ///
    /// When jottad is reachable but unauthenticated, jotta-cli exits 1 with
    /// "ERROR  Not logged in" on stderr — that is **NeedsLogin, not Offline**
    /// (the daemon answered!). Verified empirically (Pop!_OS, jotta-cli
    /// 0.17.176206, 2026-11): without this, the GUI showed the
    /// "daemon svarer ikke" banner on fresh installs.
    pub fn from_cli_error(err: &crate::error::Error) -> Status {
        use crate::error::Error;
        let needs_login = match err {
            Error::CommandFailed { stderr, .. } => {
                stderr.to_lowercase().contains("not logged in")
            }
            _ => false,
        };
        let mut s = Status::default();
        if needs_login {
            s.logged_in = Some(false);
            s.state = State::NeedsLogin;
        }
        s
    }

    /// Human-readable one-liner for the tray tooltip.
    pub fn tooltip(&self) -> String {
        match self.state {
            State::Offline => "Jottacloud: offline".into(),
            State::NeedsLogin => "Jottacloud: log ind påkrævet".into(),
            State::Paused => "Jottacloud: pauset".into(),
            State::Syncing => format!(
                "Jottacloud: synkroniserer{}",
                self.active_transfers
                    .map(|n| format!(" ({n} overførsler)"))
                    .unwrap_or_default()
            ),
            State::Idle => "Jottacloud: oppe at køre".into(),
        }
    }

    /// Format a byte count as a human-readable Danish string (e.g. "1,58 GB").
    pub fn format_bytes(n: u64) -> String {
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
            format!("{} {}", rounded.replace('.', ","), UNITS[unit])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

    /// Real captured output from `jotta-cli status --json` (Fedora, 2026),
    /// trimmed to the fields the parser reads.
    const REAL_SCHEMA: &str = r#"{
        "User": {
            "Email": "eske@tuta.io",
            "device": {
                "Name": "Laptop",
                "Type": 12
            },
            "AccountInfo": {
                "Capacity": 5000000000000,
                "Usage": 16993271698
            }
        },
        "Sync": {
            "Enabled": true,
            "Automatic": true,
            "RootPath": "/home/eske/Documents",
            "Count": {"Files": 31927, "Bytes": 9126400809}
        },
        "State": {
            "RestoreWorking": true,
            "Uploading": {},
            "Downloading": {}
        },
        "Backup": {
            "State": {
                "Enabled": {
                    "deviceName": "Laptop",
                    "Backups": [
                        {
                            "Name": "Documents",
                            "Path": "/home/eske/Documents",
                            "Count": {"Files": 6379, "Bytes": 7840405242}
                        }
                    ]
                }
            }
        }
    }"#;

    #[test]
    fn parses_real_schema() {
        let v = json::parse(REAL_SCHEMA).unwrap();
        let s = Status::from_json(&v);
        assert_eq!(s.logged_in, Some(true));
        assert_eq!(s.account_email.as_deref(), Some("eske@tuta.io"));
        assert_eq!(s.bytes_used, Some(16_993_271_698));
        assert_eq!(s.bytes_quota, Some(5_000_000_000_000));
        assert_eq!(s.state, State::Idle);
    }

    #[test]
    fn parses_backup_folders_from_real_schema() {
        let v = json::parse(REAL_SCHEMA).unwrap();
        let s = Status::from_json(&v);
        assert_eq!(s.backups.len(), 1);
        let b = &s.backups[0];
        assert_eq!(b.name, "Documents");
        assert_eq!(b.path, "/home/eske/Documents");
        assert_eq!(b.files, Some(6379));
        assert_eq!(b.bytes, Some(7_840_405_242));
    }

    #[test]
    fn parses_sync_info_from_real_schema() {
        let v = json::parse(REAL_SCHEMA).unwrap();
        let s = Status::from_json(&v);
        let sync = s.sync.expect("sync info present");
        assert!(sync.enabled);
        assert!(sync.automatic);
        assert_eq!(sync.root_path.as_deref(), Some("/home/eske/Documents"));
        assert_eq!(sync.files, Some(31927));
        assert_eq!(sync.bytes, Some(9_126_400_809));
    }

    #[test]
    fn parses_device_name_and_restore_working_from_real_schema() {
        let v = json::parse(REAL_SCHEMA).unwrap();
        let s = Status::from_json(&v);
        assert_eq!(s.device_name.as_deref(), Some("Laptop"));
        assert!(s.restore_working);
    }

    #[test]
    fn syncing_when_uploads_nonempty() {
        let v = json::parse(
            r#"{"User":{"Email":"e@e"},"State":{"Uploading":{"f1":{}},"Downloading":{}}}"#,
        )
        .unwrap();
        let s = Status::from_json(&v);
        assert_eq!(s.active_transfers, Some(1));
        assert_eq!(s.state, State::Syncing);
    }

    #[test]
    fn full_status_parses() {
        let v = json::parse(
            r#"{"user":{"email":"eske@example.com"},"paused":false,
                "account":{"used":850000000000,"quota":5000000000000},
                "uploads":[{"path":"a"},{"path":"b"}]}"#,
        )
        .unwrap();
        let s = Status::from_json(&v);
        assert_eq!(s.state, State::Syncing);
        assert_eq!(s.account_email.as_deref(), Some("eske@example.com"));
        assert_eq!(s.bytes_quota, Some(5_000_000_000_000));
    }

    #[test]
    fn empty_schema_is_offline() {
        let v = json::parse("{}").unwrap();
        assert_eq!(Status::from_json(&v).state, State::Offline);
    }

    #[test]
    fn parses_subscription_and_account_type() {
        // Verified schema: User.AccountInfo.SubscriptionNameLocalized,
        // ProductNameLocalized, AccountType.
        let v = json::parse(
            r#"{"User":{"Email":"e@e","AccountInfo":{
                "SubscriptionNameLocalized":"Home subscription 5 TB",
                "ProductNameLocalized":"Home subscription 5 TB",
                "AccountType":"premium",
                "Capacity":5000000000000,
                "Usage":16993271698
            }}}"#,
        )
        .unwrap();
        let s = Status::from_json(&v);
        assert_eq!(
            s.subscription.as_deref(),
            Some("Home subscription 5 TB")
        );
        assert_eq!(s.account_type.as_deref(), Some("premium"));
    }

    #[test]
    fn subscription_falls_back_to_product_name() {
        // Hvis kun ProductNameLocalized findes (ikke SubscriptionNameLocalized).
        let v = json::parse(
            r#"{"User":{"AccountInfo":{"ProductNameLocalized":"Jottacloud Free"}}}"#,
        )
        .unwrap();
        let s = Status::from_json(&v);
        assert_eq!(s.subscription.as_deref(), Some("Jottacloud Free"));
        assert!(s.account_type.is_none());
    }

    #[test]
    fn paused_wins_over_syncing() {
        let v = json::parse(
            r#"{"user":{"email":"e@e"},"paused":true,"uploads":[{"p":1}]}"#,
        )
        .unwrap();
        assert_eq!(Status::from_json(&v).state, State::Paused);
    }

    #[test]
    fn format_bytes_human_readable() {
        assert_eq!(Status::format_bytes(0), "0 B");
        assert_eq!(Status::format_bytes(500), "500 B");
        assert_eq!(Status::format_bytes(1_500), "1,50 KB");
        assert_eq!(Status::format_bytes(16_993_271_698), "16,99 GB");
        assert_eq!(Status::format_bytes(5_000_000_000_000), "5,00 TB");
    }

    #[test]
    fn cli_error_not_logged_in_maps_to_needs_login_not_offline() {
        // jotta-cli status --json when jottad runs but user is not logged in:
        // exit 1, "ERROR  Not logged in" on stderr (verified on Pop!_OS).
        let err = crate::error::Error::CommandFailed {
            program: "jotta-cli".into(),
            argv: vec!["status".into(), "--json".into()],
            code: 1,
            stderr: "ERROR  Not logged in\n".into(),
        };
        let s = Status::from_cli_error(&err);
        assert_eq!(s.state, State::NeedsLogin);
        assert_eq!(s.logged_in, Some(false));

        // Spawn failure (binary missing) stays Offline.
        let err2 = crate::error::Error::Spawn(
            "jotta-cli".into(),
            std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"),
        );
        assert_eq!(Status::from_cli_error(&err2).state, State::Offline);

        // Other command failures (e.g. daemon socket gone) stay Offline.
        let err3 = crate::error::Error::CommandFailed {
            program: "jotta-cli".into(),
            argv: vec!["status".into(), "--json".into()],
            code: 1,
            stderr: "ERROR  Could not connect to jottad\n".into(),
        };
        assert_eq!(Status::from_cli_error(&err3).state, State::Offline);
    }

    #[test]
    fn parses_active_transfers_from_state_uploading_downloading() {
        // Object keyed by transfer id, values carry path + byte counters.
        let v = json::parse(
            r#"{
                "State": {
                    "Uploading": {"u-1": {"Path": "/sync/jotta-gui tests/a.deb", "Bytes": 34, "Total": 142}},
                    "Downloading": {"d-1": {"remote": "/Sync/b.txt", "bytes_done": 5, "total_bytes": 10}}
                }
            }"#,
        )
        .unwrap();
        let s = Status::from_json(&v);
        assert_eq!(s.active_uploads.len(), 1);
        assert_eq!(s.active_uploads[0].path.as_deref(), Some("/sync/jotta-gui tests/a.deb"));
        assert_eq!(s.active_uploads[0].bytes_done, Some(34));
        assert_eq!(s.active_uploads[0].total_bytes, Some(142));
        assert_eq!(s.active_downloads.len(), 1);
        assert_eq!(s.active_downloads[0].path.as_deref(), Some("/Sync/b.txt"));
        assert_eq!(s.active_downloads[0].bytes_done, Some(5));
        assert_eq!(s.active_downloads[0].total_bytes, Some(10));
        assert!(s.has_active_transfers());
        assert!(s.active_transfers_have_byte_counters());
        assert_eq!(s.active_upload_bytes_done(), 34);
        assert_eq!(s.active_download_bytes_done(), 5);
        // Count still derived from the same objects.
        assert_eq!(s.active_transfers, Some(1));
        assert_eq!(s.state, State::Offline); // not logged in in this sample
    }

    #[test]
    fn parses_per_backup_uploading_objects() {
        // Real schema shape (captured from jotta-cli status --json, Fedora 2026):
        // each Backups[] entry has its own "Uploading" object alongside State.Uploading.
        let v = json::parse(
            r#"{
                "State": {"Uploading": {}, "Downloading": {}},
                "Backup": {"State": {"Enabled": {"Backups": [
                    {"Name": "Documents", "Path": "/home/eske/Documents",
                     "Uploading": {"u-7": {"Path": "/home/eske/Documents/big.iso", "Bytes": 100, "Total": 5000}},
                     "Errors": {}}
                ]}}}
            }"#,
        )
        .unwrap();
        let s = Status::from_json(&v);
        assert!(s.active_uploads.iter().any(|t| t.path.as_deref() == Some("/home/eske/Documents/big.iso")));
        assert!(s.has_active_transfers());
        assert_eq!(s.active_upload_bytes_done(), 100);
        // Backup folder list still parsed as before.
        assert_eq!(s.backups.len(), 1);
        assert_eq!(s.backups[0].name, "Documents");
    }

    #[test]
    fn active_transfers_match_widget_verified_keys() {
        // Active-schema keys as used by lorentzfb/omarchy-jottacloud:
        // total = TotalBytes|Bytes|Total.Bytes|Size, done = CompletedBytes|Completed.Bytes|Progress.
        let v = json::parse(
            r#"{"State": {"Uploading": {
                "u-1": {"Path": "/sync/a.bin", "TotalBytes": 5000, "CompletedBytes": 1200},
                "u-2": {"Path": "/sync/b.bin", "Bytes": 8000, "Progress": 2000},
                "u-3": {"Path": "/sync/c.bin", "Size": 900, "Completed": {"Bytes": 300}}
            }, "Downloading": {}}}"#,
        )
        .unwrap();
        let s = Status::from_json(&v);
        assert_eq!(s.active_uploads.len(), 3);
        let a = s.active_uploads.iter().find(|t| t.path.as_deref() == Some("/sync/a.bin")).unwrap();
        assert_eq!(a.bytes_done, Some(1200));
        assert_eq!(a.total_bytes, Some(5000));
        let b = s.active_uploads.iter().find(|t| t.path.as_deref() == Some("/sync/b.bin")).unwrap();
        assert_eq!(b.bytes_done, Some(2000));
        assert_eq!(b.total_bytes, Some(8000)); // "Bytes" is the TOTAL in this schema
        let c = s.active_uploads.iter().find(|t| t.path.as_deref() == Some("/sync/c.bin")).unwrap();
        assert_eq!(c.bytes_done, Some(300));
        assert_eq!(c.total_bytes, Some(900));
        assert!(s.active_transfers_have_byte_counters());
        assert_eq!(s.active_upload_bytes_done(), 3500);
    }

    #[test]
    fn active_transfers_tolerate_arrays_scalars_and_empty() {
        // Array shape.
        let v = json::parse(r#"{"State": {"Uploading": [{"Path": "/x", "Bytes": 1, "Total": 2}]}}"#).unwrap();
        let s = Status::from_json(&v);
        assert_eq!(s.active_uploads.len(), 1);
        assert_eq!(s.active_uploads[0].path.as_deref(), Some("/x"));

        // Scalar values (id -> true) carry no detail: skipped, but count remains.
        let v = json::parse(r#"{"State": {"Uploading": {"u-9": true}, "Downloading": {}}}"#).unwrap();
        let s = Status::from_json(&v);
        assert!(s.active_uploads.is_empty());
        assert!(s.active_downloads.is_empty());
        assert!(!s.has_active_transfers());
        assert!(!s.active_transfers_have_byte_counters());
        assert_eq!(s.active_transfers, Some(1));

        // Empty objects = idle.
        let v = json::parse(r#"{"State": {"Uploading": {}, "Downloading": {}}}"#).unwrap();
        let s = Status::from_json(&v);
        assert!(!s.has_active_transfers());
        assert_eq!(s.active_transfers, Some(0));

        // One level of nesting.
        let v = json::parse(
            r#"{"State": {"Downloading": {"d-1": {"transfer": {"Local": "/tmp/f", "Done": 7, "Total": 9}}}}}"#,
        )
        .unwrap();
        let s = Status::from_json(&v);
        assert_eq!(s.active_downloads.len(), 1);
        assert_eq!(s.active_downloads[0].path.as_deref(), Some("/tmp/f"));
        assert_eq!(s.active_downloads[0].bytes_done, Some(7));
    }
}
