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

        // Backup folders (verified schema: Backup.State.Enabled.Backups[]).
        if let Some(arr) = v
            .path(&["Backup", "State", "Enabled", "Backups"])
            .and_then(|x| x.as_array())
        {
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
}
