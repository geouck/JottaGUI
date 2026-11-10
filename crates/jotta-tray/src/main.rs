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

mod i18n;
mod pixmaps;

#[derive(Debug)]
struct AppState {
    status: Status,
    err: Option<String>,
    /// Live transfer rates in bytes/s (iteration 14), from Total-Remaining deltas.
    up_rate: u64,
    down_rate: u64,
}

/// Sort/hvidt tray-ikon (iteration 14): læses fra samme settings.conf som GUI'en
/// skriver (tray_icon_mono=true). Standard FRA (farvede tilstandsikoner).
fn tray_mono() -> bool {
    let path = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(".config")
        })
        .join("jotta-desktop")
        .join("settings.conf");
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("tray_icon_mono="))
                .map(|l| l.trim_end() == "tray_icon_mono=true")
        })
        .unwrap_or(false)
}

/// Localized one-line status for menu + tooltip (mirrors Status::tooltip,
/// but translated — jotta-core itself stays Danish/zero-dep).
fn status_line(st: &Status) -> String {
    match st.state {
        State::Offline => i18n::t("tray.state.offline").to_string(),
        State::NeedsLogin => i18n::t("tray.state.needslogin").to_string(),
        State::Paused => i18n::t("tray.state.paused").to_string(),
        State::Syncing => match st.active_transfers {
            Some(n) => i18n::tf("tray.state.syncing.n", &[&n]),
            None => i18n::t("tray.state.syncing").to_string(),
        },
        State::Idle => i18n::t("tray.state.idle").to_string(),
    }
}

struct JottaTray {
    state: Arc<Mutex<AppState>>,
    quit: Arc<AtomicBool>,
    /// Minimize-to-tray: når aktiv, lukker "Afslut" også GUI.
    close_gui_on_quit: Arc<AtomicBool>,
}

impl JottaTray {
    fn icon_for(state: State, mono: bool) -> &'static str {
        // Sort/hvidt (mono) tilvalg (iteration 14): ét symbolic-ikon uanset tilstand.
        if mono {
            return "io.github.eske.JottaDesktop-symbolic";
        }
        // App-id-named icons: flatpak only exports icons whose filename starts
        // with the app-id, and native installs ship both names, so these work
        // everywhere (the embedded pixmap is the ultimate fallback).
        match state {
            State::Idle => "io.github.eske.JottaDesktop-idle",
            State::Syncing => "io.github.eske.JottaDesktop-syncing",
            State::Paused => "io.github.eske.JottaDesktop-paused",
            State::NeedsLogin => "io.github.eske.JottaDesktop-attention",
            State::Offline => "io.github.eske.JottaDesktop-offline",
        }
    }
}

impl Tray for JottaTray {
    fn id(&self) -> String {
        env!("CARGO_PKG_NAME").into()
    }

    fn icon_name(&self) -> String {
        Self::icon_for(self.state.lock().unwrap().status.state, tray_mono()).to_string()
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
        // NB (iteration 14 fix): KDE/Plasma prefers the embedded IconPixmap over
        // IconName, so the mono toggle (tray_icon_mono=true) must swap the PIXMAP
        // too — changing only icon_name() has no visible effect. In mono mode we
        // therefore return a desaturated (grayscale) variant of the state pixmap.
        let state = self.state.lock().unwrap().status.state;
        if tray_mono() {
            pixmaps::icon_for_state_mono(state)
        } else {
            pixmaps::icon_for_state(state)
        }
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
            Some(e) => i18n::tf("tray.error", &[&e]),
            None => status_line(&s.status),
        };
        ToolTip {
            icon_name: Self::icon_for(s.status.state, tray_mono()).to_string(),
            title: "Jotta Desktop".into(),
            description,
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let s = self.state.lock().unwrap();
        let quit = Arc::clone(&self.quit);
        let close_gui_on_quit = Arc::clone(&self.close_gui_on_quit);
        let mut items: Vec<MenuItem<Self>> = vec![StandardItem {
            label: i18n::tf("tray.status", &[&status_line(&s.status)]),
            enabled: false,
            ..Default::default()
        }
        .into()];
        // Live up/down-hastighed (iteration 14) — vises kun når der overføres noget.
        if s.up_rate > 0 || s.down_rate > 0 {
            items.push(
                StandardItem {
                    label: i18n::tf(
                        "tray.speed",
                        &[&i18n::format_rate(s.down_rate), &i18n::format_rate(s.up_rate)],
                    ),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
        }
        items.push(MenuItem::Separator);
        items.extend(vec![
            match s.status.paused {
                Some(true) => StandardItem {
                    label: i18n::t("tray.resume").into(),
                    activate: Box::new(|_tray| run_simple_idempotent(JottaCommand::Resume)),
                    ..Default::default()
                }
                .into(),
                _ => StandardItem {
                    label: i18n::t("tray.pause").into(),
                    activate: Box::new(|_tray| run_simple(JottaCommand::Pause)),
                    ..Default::default()
                }
                .into(),
            },
            MenuItem::Separator,
            StandardItem {
                label: i18n::t("tray.open_window").into(),
                activate: Box::new(|_tray| {
                    spawn_gui();
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: i18n::t("tray.open_web").into(),
                activate: Box::new(|_tray| {
                    let _ = open_url("https://www.jottacloud.com/web/");
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: i18n::t("tray.quit").into(),
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
        ]);
        items
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


/// Er <pid> en levende (ikke-zombie) jotta-tray-proces? /proc/<pid>/stat har
/// state lige efter sidste ')'; 'Z' = zombie = død for vores formål (en zombie
/// holder /proc/<pid> i live og narrede tidligere single-instance-checket).
fn pid_alive_and_is_tray(pid: u32) -> bool {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => s,
        Err(_) => return false,
    };
    match stat.rsplit(')').next().and_then(|r| r.trim_start().chars().next()) {
        Some('Z') | None => false,
        _ => std::fs::read_to_string(format!("/proc/{pid}/cmdline"))
            .map(|c| c.contains("jotta-tray"))
            .unwrap_or(false),
    }
}

fn main() {
    // Single-instance: skriv PID-fil og afslut hvis en anden tray allerede kører.
    let pid_file = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join("jotta-tray.pid");

    if let Ok(pid_str) = std::fs::read_to_string(&pid_file) {
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            if pid_alive_and_is_tray(pid) {
                eprintln!("jotta-tray: already running (pid {pid}), exiting");
                std::process::exit(0);
            }
            // Stale pid-fil (død/zombie/ikke-tray): fjern og fortsæt.
            let _ = std::fs::remove_file(&pid_file);
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

    let state = Arc::new(Mutex::new(AppState { status: Status::default(), err: None, up_rate: 0, down_rate: 0 }));
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
    std::thread::spawn(move || {
        // Sidste sample til hastighedsberegning (kun denne tråd rører den).
        let mut last: Option<(std::time::Instant, u64, u64)> = None;
        loop {
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
                // "Not logged in" on stderr = NeedsLogin (daemon answered!),
                // not Offline — otherwise the tray shows the wrong icon on
                // fresh installs.
                st.status = Status::from_cli_error(&e);
                st.err = Some(e.to_string());
            }
        }
        // Live up/down-hastighed (iteration 14): to ekstra CLI-kald (jottad cacher
        // svarene); hastighed = delta(overførte bytes) / delta(tid) mellem polls.
        let up_done = poll_client
            .run_json(&JottaCommand::ListUploads)
            .ok()
            .map(|v| jotta_core::remote::total_bytes_done_uploads(&jotta_core::remote::parse_uploads_json(&v)));
        let down_done = poll_client
            .run_json(&JottaCommand::ListDownloads)
            .ok()
            .map(|v| jotta_core::remote::total_bytes_done_downloads(&jotta_core::remote::parse_downloads_json(&v)));
        if let (Some(up_done), Some(down_done)) = (up_done, down_done) {
            let now = std::time::Instant::now();
            let (up_rate, down_rate) = match last {
                Some((t, pu, pd)) => {
                    let dt = now.duration_since(t).as_secs_f64();
                    if dt > 0.0 {
                        (
                            (up_done.saturating_sub(pu) as f64 / dt) as u64,
                            (down_done.saturating_sub(pd) as f64 / dt) as u64,
                        )
                    } else {
                        (0, 0)
                    }
                }
                None => (0, 0),
            };
            last = Some((now, up_done, down_done));
            let mut st = poll_state.lock().unwrap();
            st.up_rate = up_rate;
            st.down_rate = down_rate;
        }
        poll_handle.update(|_tray| {});
        std::thread::sleep(Duration::from_secs(3));
        }
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
