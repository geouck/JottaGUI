//! Tolerant status model.
//!
//! jotta-cli's status --json schema is not formally documented and moves
//! between releases, so nothing here is allowed to panic or hard-fail:
//! every field is an Option, extracted by best effort from the JSON tree.
//! The GUI renders "unknown" for missing fields.

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
    /// Number of backup folders, if reported.
    pub backup_folders: Option<u64>,
    /// Best-effort state machine for the tray icon.
    pub state: State,
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

impl Status {
    /// Extract a Status from jotta-cli status --json output.
    ///
    /// Extraction is deliberately forgiving: unknown schemas produce a
    /// Status with state Offline + whatever fields matched, never an error.
    pub fn from_json(v: &Value) -> Self {
        let mut s = Status::default();
        s.logged_in = v.path(&["user"]).map(|_| true);
        s.account_email = v
            .path(&["user", "email"])
            .or_else(|| v.path(&["account", "email"]))
            .and_then(|x| x.as_str())
            .map(str::to_owned);
        s.paused = v.path(&["paused"]).and_then(|x| x.as_bool());
        s.bytes_used = v
            .path(&["account", "used"])
            .or_else(|| v.path(&["usage", "used"]))
            .and_then(|x| x.as_f64())
            .map(|n| n as u64);
        s.bytes_quota = v
            .path(&["account", "quota"])
            .or_else(|| v.path(&["usage", "quota"]))
            .and_then(|x| x.as_f64())
            .map(|n| n as u64);
        s.active_transfers = v
            .path(&["uploads"])
            .or_else(|| v.path(&["transfers"]))
            .and_then(|x| x.as_array())
            .map(|a| a.len() as u64);
        s.backup_folders = v
            .path(&["backups"])
            .and_then(|x| x.as_array())
            .map(|a| a.len() as u64);

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

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
    fn paused_wins_over_syncing() {
        let v = json::parse(
            r#"{"user":{"email":"e@e"},"paused":true,"uploads":[{"p":1}]}"#,
        )
        .unwrap();
        assert_eq!(Status::from_json(&v).state, State::Paused);
    }
}
