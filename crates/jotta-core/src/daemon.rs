//! Supervision of the bundled `jottad` daemon (Model B).
//!
//! The app owns the daemon lifecycle:
//! - spawn jottad on first use (its socket lands in $XDG_RUNTIME_DIR/jottad),
//! - wait for the socket to appear (that is jottad's "ready" signal),
//! - stop it with SIGTERM on logout/quit.
//!
//! In native installs a host systemd user unit may already run jottad;
//! ensure_running detects that and reuses it instead of spawning a second
//! daemon (two daemons = double backups).

use crate::error::{Error, Result};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct DaemonSupervisor {
    /// Absolute path to the jottad binary.
    binary: PathBuf,
    /// The socket jottad will create once ready.
    socket: PathBuf,
    /// Log file for daemon stdout/stderr.
    log_file: PathBuf,
    /// Our spawned child, if we started the daemon ourselves.
    child: Option<Child>,
}

impl DaemonSupervisor {
    pub fn new(binary: PathBuf, data_dir: PathBuf) -> Self {
        let socket = socket_path();
        Self {
            binary,
            socket,
            log_file: data_dir.join("jottad.log"),
            child: None,
        }
    }

    /// Standard jottad socket path: $XDG_RUNTIME_DIR/jottad/jottad.socket
    pub fn socket_path(&self) -> &std::path::Path {
        &self.socket
    }

    /// True if the socket exists.
    pub fn is_running(&self) -> bool {
        self.socket.exists()
    }

    /// Start jottad if it is not already running (e.g. host systemd unit).
    /// Idempotent. Returns whether *we* spawned it.
    pub fn ensure_running(&mut self) -> Result<bool> {
        if self.is_running() {
            return Ok(false);
        }
        std::fs::create_dir_all(self.socket.parent().unwrap())
            .map_err(|io| Error::Spawn("mkdir jottad run dir".into(), io))?;

        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_file)
            .map_err(|io| Error::Spawn("open jottad.log".into(), io))?;

        let child = Command::new(&self.binary)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|io| Error::Spawn(self.binary.display().to_string(), io))?;
        let we_spawned = true;
        self.child = Some(child);
        self.wait_for_socket(Duration::from_secs(10))?;
        Ok(we_spawned)
    }

    /// Block until the socket shows up, or fail with Timeout.
    pub fn wait_for_socket(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.is_running() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(Error::Timeout(format!(
            "jottad did not create socket {} within {:?}",
            self.socket.display(),
            timeout
        )))
    }

    /// Stop the daemon, but only if we started it (never kill a host daemon).
    pub fn stop_if_owned(&mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            child.kill().map_err(|io| Error::Spawn("kill jottad".into(), io))?;
            let _ = child.wait();
        }
        Ok(())
    }
}

impl Drop for DaemonSupervisor {
    fn drop(&mut self) {
        let _ = self.stop_if_owned();
    }
}

pub fn socket_path() -> PathBuf {
    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/run/user").join(uid_string()));
    runtime.join("jottad").join("jottad.socket")
}

#[cfg(target_os = "linux")]
fn uid_string() -> String {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(1).map(|x| x.to_string()))
        })
        .unwrap_or_else(|| "1000".to_string())
}

#[cfg(not(target_os = "linux"))]
fn uid_string() -> String {
    "1000".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_lives_in_xdg_runtime_dir() {
        // In the test env XDG_RUNTIME_DIR may be unset; either way the path
        // must end with jottad/jottad.socket.
        let p = socket_path();
        assert!(p.ends_with("jottad/jottad.socket"));
    }
}
