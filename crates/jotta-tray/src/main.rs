//! jotta-tray: a StatusNotifierItem tray icon backed by jotta-core.
//!
//! Runs a background thread that polls jotta-cli status --json and updates
//! the tray icon + menu. Polling is deliberately simple: jottad already
//! caches status, and the CLI call is cheap.
//!
//! Menu: status line (disabled), pause/resume, open web, quit.

use jotta_core::cli::{BinaryLocator, CliClient, JottaCommand};
use jotta_core::daemon::DaemonSupervisor;
use jotta_core::status::{State, Status};
use ksni::Tray;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug)]
struct AppState {
    status: Status,
    err: Option<String>,
}

struct JottaTray {
    state: Arc<Mutex<AppState>>,
}

impl ksni::Tray for JottaTray {
    fn icon_name(&self) -> String {
        match self.state.lock().unwrap().status.state {
            State::Idle => "jotta-desktop-idle",
            State::Syncing => "jotta-desktop-syncing",
            State::Paused => "jotta-desktop-paused",
            State::NeedsLogin => "jotta-desktop-attention",
            State::Offline => "jotta-desktop-offline",
        }
        .to_string()
    }

    fn title(&self) -> String {
        "Jotta Desktop".into()
    }

    fn tool_tip(&self) -> String {
        self.state.lock().unwrap().status.tooltip()
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        let s = self.state.lock().unwrap();
        vec![
            ksni::StandardItem {
                label: format!("Status: {}", s.status.tooltip()),
                enabled: false,
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            match s.status.paused {
                Some(true) => ksni::MenuItem::Standard(ksni::StandardItem {
                    label: "Genoptag".into(),
                    activate: Box::new(|_tray, _app| do_simple(JottaCommand::Resume)),
                    ..Default::default()
                }),
                _ => ksni::MenuItem::Standard(ksni::StandardItem {
                    label: "Pause".into(),
                    activate: Box::new(|_tray, _app| do_simple(JottaCommand::Pause)),
                    ..Default::default()
                }),
            },
            ksni::MenuItem::Separator,
            ksni::StandardItem {
                label: "Åbn jottacloud.com".into(),
                activate: Box::new(|_tray, _app| {
                    let _ = open_url("https://www.jottacloud.com/web/");
                }),
                ..Default::default()
            }
            .into(),
            ksni::StandardItem {
                label: "Afslut".into(),
                activate: Box::new(|tray: &mut Self, _app| {
                    tray.state.lock().unwrap().err = Some("quit".into());
                    ksni::spawn(|| std::process::exit(0)).ok();
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

fn do_simple(cmd: JottaCommand) {
    ksni::spawn(move || {
        if let Some(loc) = BinaryLocator::from_path() {
            let client = CliClient::new(loc);
            if let Err(e) = client.run(&cmd) {
                eprintln!("jotta-tray: {e}");
            }
        }
    })
    .ok();
}

fn open_url(url: &str) -> std::io::Result<()> {
    for prog in ["xdg-open", "gio", "kde-open"] {
        if std::process::Command::new(prog).arg(url).spawn().is_ok() {
            return Ok(());
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no opener found",
    ))
}

fn main() {
    let data_dir = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join(".local/share/jotta-desktop")
        });
    std::fs::create_dir_all(&data_dir).ok();

    // Ensure the daemon is up (Model B: we may own it; Model A: reuse host one).
    let locator = BinaryLocator::from_path()
        .or_else(|| {
            let exe = std::env::current_exe().ok()?;
            let dir = exe.parent()?;
            Some(BinaryLocator::new(
                dir.join("jotta-cli"),
                dir.join("jottad"),
            ))
        })
        .expect("jotta-cli/jottad not found on PATH or next to this binary");

    let mut supervisor = DaemonSupervisor::new(locator.daemon().to_path_buf(), data_dir);
    match supervisor.ensure_running() {
        Ok(ours) => {
            if ours {
                eprintln!("jotta-tray: started jottad ourselves");
            } else {
                eprintln!("jotta-tray: reusing existing jottad");
            }
        }
        Err(e) => eprintln!("jotta-tray: {e}"),
    }

    let state = Arc::new(Mutex::new(AppState { status: Status::default(), err: None }));
    let poll_state = Arc::clone(&state);
    let client = CliClient::new(locator.clone());

    let service = ksni::TrayService::new(JottaTray { state: Arc::clone(&state) });
    let handle = service.handle();
    service.spawn();

    std::thread::spawn(move || loop {
        match client.run(&JottaCommand::Status) {
            Ok(out) => {
                let parsed = jotta_core::json::parse(out.stdout.trim())
                    .and_then(|v| Ok(Status::from_json(&v)));
                match parsed {
                    Ok(status) => {
                        let mut st = poll_state.lock().unwrap();
                        st.status = status;
                        st.err = None;
                    }
                    Err(e) => {
                        let mut st = poll_state.lock().unwrap();
                        st.err = Some(e.to_string());
                    }
                }
            }
            Err(e) => {
                let mut st = poll_state.lock().unwrap();
                st.status = Status::default();
                st.err = Some(e.to_string());
            }
        }
        handle.update(|_tray: &mut JottaTray| {});
        std::thread::sleep(Duration::from_secs(3));
    });

    // Park main thread; ksni runs its own event loop.
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}
