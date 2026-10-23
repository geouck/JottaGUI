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
use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::StandardItem;
use ksni::{MenuItem, Status as SniStatus, ToolTip, Tray};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod pixmaps;

#[derive(Debug)]
struct AppState {
    status: Status,
    err: Option<String>,
}

struct JottaTray {
    state: Arc<Mutex<AppState>>,
    quit: Arc<AtomicBool>,
    /// Minimize-to-tray: når aktiv, lukker "Afslut" også GUI.
    close_gui_on_quit: Arc<AtomicBool>,
}

impl JottaTray {
    fn icon_for(state: State) -> &'static str {
        match state {
            State::Idle => "jotta-desktop-idle",
            State::Syncing => "jotta-desktop-syncing",
            State::Paused => "jotta-desktop-paused",
            State::NeedsLogin => "jotta-desktop-attention",
            State::Offline => "jotta-desktop-offline",
        }
    }
}

impl Tray for JottaTray {
    fn id(&self) -> String {
        env!("CARGO_PKG_NAME").into()
    }

    fn icon_name(&self) -> String {
        Self::icon_for(self.state.lock().unwrap().status.state).to_string()
    }

    fn icon_theme_path(&self) -> String {
        // Bundled hicolor dir next to the binary (Flatpak: /app/share/icons).
        // Falls back to the system theme if the icons are not found there.
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let bundled = dir.join("../share/icons/hicolor");
                if bundled.is_dir() {
                    return bundled.display().to_string();
                }
            }
        }
        String::new()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        // Embed ARGB32 pixmap data so the tray icon is always visible,
        // even when the icon theme path is not found by the host.
        let state = self.state.lock().unwrap().status.state;
        pixmaps::icon_for_state(state)
    }

    fn title(&self) -> String {
        "Jotta Desktop".into()
    }

    fn status(&self) -> SniStatus {
        // NOTE: State::Offline does NOT set Passive — the tray must stay
        // Active (visible) even when jottad is down, so the user can see
        // the offline icon and open the GUI to read the banner / retry.
        // Only NeedsLogin gets NeedsAttention.
        match self.state.lock().unwrap().status.state {
            State::NeedsLogin => SniStatus::NeedsAttention,
            _ => SniStatus::Active,
        }
    }

    fn tool_tip(&self) -> ToolTip {
        let s = self.state.lock().unwrap();
        let description = match &s.err {
            Some(e) => format!("Fejl: {e}"),
            None => s.status.tooltip(),
        };
        ToolTip {
            icon_name: Self::icon_for(s.status.state).to_string(),
            title: "Jotta Desktop".into(),
            description,
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let s = self.state.lock().unwrap();
        let quit = Arc::clone(&self.quit);
        let close_gui_on_quit = Arc::clone(&self.close_gui_on_quit);
        vec![
            StandardItem {
                label: format!("Status: {}", s.status.tooltip()),
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            match s.status.paused {
                Some(true) => StandardItem {
                    label: "Genoptag".into(),
                    activate: Box::new(|_tray| run_simple_idempotent(JottaCommand::Resume)),
                    ..Default::default()
                }
                .into(),
                _ => StandardItem {
                    label: "Pause".into(),
                    activate: Box::new(|_tray| run_simple(JottaCommand::Pause)),
                    ..Default::default()
                }
                .into(),
            },
            MenuItem::Separator,
            StandardItem {
                label: "Åbn vindue".into(),
                activate: Box::new(|_tray| {
                    spawn_gui();
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Åbn jottacloud.com".into(),
                activate: Box::new(|_tray| {
                    let _ = open_url("https://www.jottacloud.com/web/");
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Afslut".into(),
                activate: Box::new(move |_tray| {
                    // Hvis minimize-to-tray er aktiv, luk også GUI.
                    if close_gui_on_quit.load(Ordering::SeqCst) {
                        let gui_pid_file = std::env::var("XDG_RUNTIME_DIR")
                            .map(PathBuf::from)
                            .unwrap_or_else(|_| PathBuf::from("/tmp"))
                            .join("jotta-gui.pid");
                        if let Ok(pid_str) = std::fs::read_to_string(&gui_pid_file) {
                            if let Ok(pid) = pid_str.trim().parse::<u32>() {
                                let _ = std::process::Command::new("kill")
                                    .arg("-TERM")
                                    .arg(pid.to_string())
                                    .spawn();
                            }
                        }
                    }
                    quit.store(true, Ordering::SeqCst);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        // Klik på tray-ikon → åbn GUI.
        spawn_gui();
    }
}

/// Find jotta-desktop binary: samme mappe som denne tray-binary, ellers PATH.
fn find_gui_binary() -> Option<std::path::PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("jotta-desktop");
            if sibling.exists() {
                return Some(sibling);
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("jotta-desktop");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Spawn jotta-desktop GUI (single-instance via GApplication).
/// Spawner i en separat tråd der venter på child — ellers bliver den
/// en zombie når GUI-processen terminerer (tray lever længere end GUI).
fn spawn_gui() {
    std::thread::spawn(|| {
        if let Some(bin) = find_gui_binary() {
            match std::process::Command::new(&bin)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(mut child) => {
                    // Vent på at GUI-processen terminerer — reaper, undgår zombie.
                    let _ = child.wait();
                }
                Err(e) => eprintln!("jotta-tray: kunne ikke starte jotta-desktop: {e}"),
            }
        } else {
            eprintln!("jotta-tray: jotta-desktop binary ikke fundet");
        }
    });
}

fn run_simple(cmd: JottaCommand) {
    std::thread::spawn(move || {
        if let Some(loc) = BinaryLocator::from_path() {
            let client = CliClient::new(loc);
            if let Err(e) = client.run(&cmd) {
                eprintln!("jotta-tray: {e}");
            }
        }
    });
}

fn run_simple_idempotent(cmd: JottaCommand) {
    std::thread::spawn(move || {
        if let Some(loc) = BinaryLocator::from_path() {
            let client = CliClient::new(loc);
            if let Err(e) = client.run(&cmd) {
                let msg = e.to_string();
                if msg.contains("already running") {
                    return;
                }
                eprintln!("jotta-tray: {e}");
            }
        }
    });
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
    // Single-instance: skriv PID-fil og afslut hvis en anden tray allerede kører.
    let pid_file = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join("jotta-tray.pid");

    if let Ok(pid_str) = std::fs::read_to_string(&pid_file) {
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            if std::path::Path::new(&format!("/proc/{pid}")).exists() {
                eprintln!("jotta-tray: already running (pid {pid}), exiting");
                std::process::exit(0);
            }
        }
    }

    std::fs::write(&pid_file, std::process::id().to_string()).ok();

    let data_dir = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join(".local/share/jotta-desktop")
        });
    std::fs::create_dir_all(&data_dir).ok();

    // Ensure the daemon is up (Model B: we may own it; otherwise reuse the host one).
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
    let quit = Arc::new(AtomicBool::new(false));

    // Minimize-to-tray: læs fra samme konfigurationsfil som GUI'en.
    // Standard AKTIV — luk GUI når tray afsluttes.
    let config_path = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(".config")
        })
        .join("jotta-desktop")
        .join("settings.conf");
    let close_gui_on_quit_val = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("minimize_to_tray="))
                .map(|l| l.trim_end() == "minimize_to_tray=true")
        })
        .unwrap_or(true); // Standard: AKTIV
    let close_gui_on_quit = Arc::new(AtomicBool::new(close_gui_on_quit_val));

    let handle: Handle<JottaTray> = JottaTray {
        state: Arc::clone(&state),
        quit: Arc::clone(&quit),
        close_gui_on_quit: Arc::clone(&close_gui_on_quit),
    }
    .spawn()
    .expect("could not start tray service");

    // Poll status every 3s and refresh the tray.
    // The tray stays alive regardless of jottad state — State::Offline
    // just shows the offline icon. The user can restart jottad from the
    // Status page banner or manually; the poller picks up the new state.
    let poll_state = Arc::clone(&state);
    let poll_handle = handle.clone();
    let poll_client = CliClient::new(locator.clone());
    let poll_quit = Arc::clone(&quit);
    std::thread::spawn(move || loop {
        if poll_quit.load(Ordering::SeqCst) {
            break;
        }
        match poll_client.run(&JottaCommand::Status) {
            Ok(out) => match jotta_core::json::parse(out.stdout.trim()).map(|v| Status::from_json(&v)) {
                Ok(status) => {
                    let mut st = poll_state.lock().unwrap();
                    st.status = status;
                    st.err = None;
                }
                Err(e) => {
                    let mut st = poll_state.lock().unwrap();
                    st.err = Some(e);
                }
            },
            Err(e) => {
                let mut st = poll_state.lock().unwrap();
                st.status = Status::default();
                st.err = Some(e.to_string());
            }
        }
        poll_handle.update(|_tray| {});
        std::thread::sleep(Duration::from_secs(3));
    });

    // Main thread: wait for a quit request from the menu, then shut down cleanly.
    while !quit.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(200));
    }
    handle.shutdown().wait();
    let _ = std::fs::remove_file(&pid_file);
    if let Err(e) = supervisor.stop_if_owned() {
        eprintln!("jotta-tray: {e}");
    }
}
