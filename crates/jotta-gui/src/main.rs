//! jotta-desktop: the GTK4/libadwaita main window.
//!
//! Iteration 3 scope:
//! - status page: live state from jotta-core (email, usage bar, pause/resume)
//! - backup page: list from jotta-core, add folder via FileDialog
//! - sync page: sync info + start/stop switch
//!
//! The GUI never calls jotta-cli itself except through jotta-core types.
//! All CLI calls run on a worker thread; the GTK main loop is never blocked.

use gtk::gdk;
use gtk::glib::{self, clone, Propagation};
use gtk::prelude::*;
use jotta_core::cli::{BinaryLocator, CliClient, JottaCommand};
use jotta_core::status::{State, Status};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[macro_use]
mod i18n;
mod login;

/// Shared app state: latest parsed status + a channel to request refreshes.
struct App {
    client: CliClient,
    status: Arc<Mutex<Status>>,
}

impl App {
    fn new() -> Self {
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
        App {
            client: CliClient::new(locator),
            status: Arc::new(Mutex::new(Status::default())),
        }
    }

    /// Poll `status --json` once; returns the parsed Status.
    /// On error, "Not logged in" maps to NeedsLogin (daemon answered!),
    /// anything else to Offline.
    fn poll(&self) -> Status {
        match self.client.status() {
            Ok(v) => Status::from_json(&v),
            Err(e) => Status::from_cli_error(&e),
        }
    }

    fn client(&self) -> CliClient {
        CliClient::new(self.client.locator().clone())
    }
}

impl Clone for App {
    fn clone(&self) -> Self {
        App {
            client: CliClient::new(self.client.locator().clone()),
            status: Arc::clone(&self.status),
        }
    }
}

fn main() {
    adw::init().expect("failed to init libadwaita");

    // CSS: kompakte window controls (minimer/maximer/luk) + headerbar.
    // Målretter child-elementerne windowcontrols > button og
    // windowcontrols > button > image (Libadwaita sætter cirkel-baggrund
    // og dimensioner på image-noden).
    // Prioritet: USER + 1 så vi vinder over KDE's kde-gtk-config
    // (~/.config/gtk-4.0/gtk.css som ligger på PRIORITY_USER = 800).
    // GTK4 CSS-nodes: windowcontrols > button.minimize/.maximize/.close
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        "headerbar {\n\
         \x20   min-height: 25px;\n\
         \x20   padding-top: 2px;\n\
         \x20   padding-bottom: 2px;\n\
         }\n\
         windowcontrols > button {\n\
         \x20   min-width: 16px;\n\
         \x20   min-height: 16px;\n\
         \x20   padding: 0px;\n\
         \x20   margin: 0px 2px;\n\
         }\n\
         windowcontrols > button > image {\n\
         \x20   min-width: 16px;\n\
         \x20   min-height: 16px;\n\
         \x20   padding: 2px;\n\
         \x20   -gtk-icon-size: 14px;\n\
         \x20   border-radius: 9999px;\n\
         }",
    );
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("no display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_USER + 1,
    );

    // Single-instance: GApplication med application_id sikrer at kun én
    // jotta-desktop kører ad gangen. Hvis en instans allerede kører,
    // aktiveres den eksisterende (vinduet kommer i forgrunden).
    let app = adw::Application::builder()
        .application_id("io.github.eske.JottaDesktop")
        .flags(gtk::gio::ApplicationFlags::empty())
        .build();

    // Shutdown-flag: sættes når vinduet lukkes.
    // Worker threads tjekker dette og terminerer.
    let shutdown = Arc::new(AtomicBool::new(false));

    // Sikr at tray kører (spawn detached hvis ikke allerede aktiv).
    ensure_tray_running();

    {
        let shutdown = shutdown.clone();
        app.connect_activate(move |app| {
            // Single-instance: hvis der allerede er et aktivt vindue,
            // bring det i forgrunden i stedet for at bygge et nyt.
            if let Some(window) = app.active_window() {
                window.present();
                return;
            }
            build_ui(app, shutdown.clone());
        });
    }

    let code: i32 = app.run().into();
    std::process::exit(code);
}

/// Spawn jotta-tray som detached child hvis den ikke allerede kører.
/// Tjekker PID-fil i $XDG_RUNTIME_DIR; tray skriver sin egen PID ved opstart.
/// Spawner i separat tråd der venter på child — undgår zombie-processer.
fn ensure_tray_running() {
    let pid_file = std::env::var("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
        .join("jotta-tray.pid");

    let tray_bin = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("jotta-tray")))
        .filter(|p| p.exists());

    if let Ok(pid_str) = std::fs::read_to_string(&pid_file) {
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            if std::path::Path::new(&format!("/proc/{pid}")).exists() {
                // Tray kører allerede — MEN hvis tray-binæren er nyere end
                // pid-filen (fx efter `cargo build -p jotta-tray`), så dræb den
                // gamle og spawn den nye. Ellers kører vi videre med gammel kode
                // og tror at fixes (fx mono-ikon) ikke virker.
                let bin_newer = tray_bin
                    .as_ref()
                    .and_then(|b| std::fs::metadata(b).and_then(|m| m.modified()).ok())
                    .zip(std::fs::metadata(&pid_file).and_then(|m| m.modified()).ok())
                    .map(|(bin_m, pid_m)| bin_m > pid_m)
                    .unwrap_or(false);
                if !bin_newer {
                    return;
                }
                eprintln!("jotta-gui: jotta-tray binær opdateret — genstarter tray (pid {pid})");
                let _ = std::process::Command::new("kill")
                    .arg("-TERM")
                    .arg(pid.to_string())
                    .status();
                // Vent op til 2 s på at processen dør.
                for _ in 0..20 {
                    if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                let _ = std::fs::remove_file(&pid_file);
                // Fald igennem til spawn nedenfor.
            }
        }
    }

    let bin = tray_bin.unwrap_or_else(|| std::path::PathBuf::from("jotta-tray"));
    std::thread::spawn(move || {
        match std::process::Command::new(&bin)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                let _ = child.wait();
            }
            Err(e) => eprintln!("jotta-gui: kunne ikke starte jotta-tray: {e}"),
        }
    });
}

// ---------------------------------------------------------------------------
// Live overførsels-feed (iteration 14, forbedret efter PR #20-test)
// ---------------------------------------------------------------------------
// `jotta-cli list uploads --json` viser ikke nødvendigvis sync-triggerede
// overførsler, `status --json` viser tomme `{}` mens sync arbejder, og
// `observe` har intet --json på jotta-cli 0.17.x. Derfor FIRE kilder:
//   source 3 = `sync log --watch` stream — NÆVNER filen der synces lige nu
//              (ingen bytes; hastighed kommer fra de andre kilder)
//   source 2 = `observe --json` stream (hvis understøttet — detekteret via --help)
//   source 1 = `status --json` State.Uploading/Downloading + TEKST-status
//              `> Uploads: n (bytes) / n (bytes)` linjer (byte-tællere → hastighed)
//   source 0 = `list uploads/downloads --json` (Total-Remaining)
// Hastighed = max over alle friske kilder; listen = højeste friske kilde med
// overførsler.

/// En igangværende overførsel — ren data der krydser trådgrænsen.
#[derive(Clone)]
struct LiveTransfer {
    path: String,
    bytes_done: Option<u64>,
    total: Option<u64>,
}

/// Ét sample fra én kilde. `source`: 3 = sync log --watch (filnavne),
/// 2 = observe (--json, ellers TUI via script(1)), 1 = status (json + tekst +
/// WorkingProgress), 0 = list uploads/downloads.
#[derive(Clone)]
struct LiveFeed {
    source: u8,
    uploads: Vec<LiveTransfer>,
    downloads: Vec<LiveTransfer>,
    up_rate: u64,
    down_rate: u64,
}

/// Rebuild "I gang nu"-listen på Overførsler-fanen (aktive overførsler + fremskridt).
fn rebuild_live_list(list: &gtk::ListBox, uploads: &[LiveTransfer], downloads: &[LiveTransfer]) {
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }
    if uploads.is_empty() && downloads.is_empty() {
        let row = adw::ActionRow::builder()
            .title(i18n::t("transfers.live.empty.title"))
            .subtitle(i18n::t("transfers.live.empty.subtitle"))
            .build();
        list.append(&row);
        return;
    }
    for (is_up, t) in uploads
        .iter()
        .map(|t| (true, t))
        .chain(downloads.iter().map(|t| (false, t)))
    {
        let file_name = t.path.rsplit('/').next().unwrap_or(&t.path);
        let (icon, arrow) = if is_up {
            ("go-up-symbolic", "↑")
        } else {
            ("go-down-symbolic", "↓")
        };
        let subtitle = match (t.bytes_done, t.total) {
            (Some(d), Some(tot)) if tot > 0 => tf!(
                "transfers.live.progress",
                i18n::format_bytes(d),
                i18n::format_bytes(tot),
                d * 100 / tot
            ),
            (Some(d), _) => i18n::format_bytes(d),
            _ => t.path.clone(),
        };
        let row = adw::ActionRow::builder()
            .title(&format!("{arrow} {file_name}"))
            .subtitle(&subtitle)
            .build();
        row.add_prefix(&gtk::Image::from_icon_name(icon));
        if let (Some(d), Some(tot)) = (t.bytes_done, t.total) {
            if tot > 0 {
                let bar = gtk::ProgressBar::new();
                bar.set_fraction((d as f64 / tot as f64).min(1.0));
                bar.set_valign(gtk::Align::Center);
                bar.set_size_request(80, -1);
                row.add_suffix(&bar);
            }
        }
        list.append(&row);
    }
}

/// Anvend ét live-feed sample på Status-fanen (hastighed + aktuel fil) og
/// "I gang nu"-gruppen på Overførsler-fanen.
fn apply_live_feed(
    speed_label: &gtk::Label,
    current_row: &adw::ActionRow,
    current_label: &gtk::Label,
    live_group: &adw::PreferencesGroup,
    live_list: &gtk::ListBox,
    feed: &LiveFeed,
) {
    // NB: bind tf!()-resultatet først — `&tf!(...)` i en if/else-arm giver E0716.
    let speed_text = if feed.up_rate == 0 && feed.down_rate == 0 {
        "—".to_string()
    } else {
        tf!(
            "status.speed.format",
            i18n::format_rate(feed.down_rate),
            i18n::format_rate(feed.up_rate)
        )
    };
    speed_label.set_text(&speed_text);

    let cur = feed
        .uploads
        .iter()
        .find(|t| !t.path.is_empty())
        .or_else(|| feed.downloads.iter().find(|t| !t.path.is_empty()));
    match cur {
        Some(t) => {
            let name = t.path.rsplit('/').next().unwrap_or(&t.path);
            let up_active = feed.uploads.iter().any(|u| !u.path.is_empty());
            let arrow = if up_active { "↑" } else { "↓" };
            let pct = match (t.bytes_done, t.total) {
                (Some(d), Some(tot)) if tot > 0 => format!(" ({}%)", d * 100 / tot),
                _ => String::new(),
            };
            current_label.set_text(&format!("{arrow} {name}{pct}"));
            current_row.set_visible(true);
        }
        None => current_row.set_visible(false),
    }

    let has = feed.uploads.iter().any(|t| !t.path.is_empty())
        || feed.downloads.iter().any(|t| !t.path.is_empty());
    live_group.set_visible(has);
    if has {
        rebuild_live_list(live_list, &feed.uploads, &feed.downloads);
    }
}

/// Worker-tråd: stream `jotta-cli observe --json` (kun hvis `observe --help`
/// viser `--json`) og send LiveFeed-samples (kilde 2) til main loop ca. 1 gang/s.
/// Child genstartes ved EOF; dræbes ved shutdown. Read-only.
fn run_observe_worker(
    shutdown: Arc<AtomicBool>,
    live_tx: std::sync::mpsc::Sender<LiveFeed>,
    child_slot: Arc<Mutex<Option<std::process::Child>>>,
) {
    let Some(loc) = BinaryLocator::from_path() else {
        return;
    };
    let client = CliClient::new(loc);
    // Detektér --json én gang (ikke alle jotta-cli-versioner har flaget).
    let supported = client
        .run_text(&JottaCommand::ObserveHelp)
        .map(|h| jotta_core::observe::observe_help_supports_json(&h))
        .unwrap_or(false);
    if supported {
        run_observe_json_worker(shutdown, live_tx, child_slot, client);
    } else if CliClient::observe_tui_available() {
        // jotta-cli 0.17.x: `observe` uden --json viser en TUI der kræver en
        // TTY — kør den under script(1) og parse skærmen (filnavne + totals
        // + live hastighed `@ 50.31MiB/s`, verificeret fra rigtig capture).
        run_observe_tui_worker(shutdown, live_tx, child_slot, client);
    }
    // Ellers: kilde 1 (status) og 0 (list) dækker det så godt de kan.
}

/// Kilde 2, JSON-mode: stream `observe --json` linje for linje.
fn run_observe_json_worker(
    shutdown: Arc<AtomicBool>,
    live_tx: std::sync::mpsc::Sender<LiveFeed>,
    child_slot: Arc<Mutex<Option<std::process::Child>>>,
    client: CliClient,
) {
    use std::io::BufRead as _; // read_line på BufReader
    struct PathState {
        last_seen: std::time::Instant,
        bytes_done: Option<u64>,
        total: Option<u64>,
        rate: u64,
        prev: Option<(std::time::Instant, u64)>,
    }
    let mut paths: std::collections::HashMap<(u8, String), PathState> =
        std::collections::HashMap::new();
    let mut last_send = std::time::Instant::now() - Duration::from_secs(2);

    'respawn: loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        let mut child = match client.spawn_stream(&JottaCommand::ObserveJson) {
            Ok(c) => c,
            Err(_) => {
                std::thread::sleep(Duration::from_secs(10));
                continue;
            }
        };
        let stdout = child.stdout.take();
        {
            let mut slot = child_slot.lock().unwrap();
            if let Some(mut old) = slot.take() {
                let _ = old.kill();
                let _ = old.wait();
            }
            *slot = Some(child);
        }
        let Some(stdout) = stdout else {
            continue 'respawn;
        };
        let mut reader = std::io::BufReader::new(stdout);
        let mut line = String::new();
        loop {
            if shutdown.load(Ordering::SeqCst) {
                break 'respawn;
            }
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break, // EOF — child døde; genstart nedenfor
                Ok(_) => {
                    if let Some(ev) = jotta_core::observe::parse_observe_line(&line) {
                        let dir = match ev.direction {
                            jotta_core::observe::ObserveDirection::Upload => 1u8,
                            jotta_core::observe::ObserveDirection::Download => 2u8,
                            jotta_core::observe::ObserveDirection::Unknown => 0u8,
                        };
                        let key = (dir, ev.path.clone().unwrap_or_default());
                        let now = std::time::Instant::now();
                        let entry = paths.entry(key).or_insert(PathState {
                            last_seen: now,
                            bytes_done: None,
                            total: None,
                            rate: 0,
                            prev: None,
                        });
                        entry.last_seen = now;
                        if ev.bytes_done.is_some() {
                            entry.bytes_done = ev.bytes_done;
                        }
                        if ev.total_bytes.is_some() {
                            entry.total = ev.total_bytes;
                        }
                        // Hastighed: direkte fra event, ellers delta på bytes_done.
                        if let Some(b) = ev.bytes_done {
                            if let Some((pt, pb)) = entry.prev {
                                let dt = now.duration_since(pt).as_secs_f64();
                                if dt > 0.0 {
                                    let delta_rate = (b.saturating_sub(pb) as f64 / dt) as u64;
                                    entry.rate = entry.rate.max(delta_rate);
                                }
                            }
                            entry.prev = Some((now, b));
                        }
                        if let Some(r) = ev.rate_bps {
                            entry.rate = entry.rate.max(r);
                        }
                    }
                    // Send højst 1 gang/s; fjern udløbne stier (> 5 s uden events).
                    let now = std::time::Instant::now();
                    if now.duration_since(last_send) >= Duration::from_secs(1) {
                        last_send = now;
                        paths.retain(|_, s| now.duration_since(s.last_seen).as_secs() < 5);
                        let mut uploads = Vec::new();
                        let mut downloads = Vec::new();
                        let (mut up_rate, mut down_rate) = (0u64, 0u64);
                        for ((dir, path), s) in &paths {
                            if !path.is_empty() {
                                let lt = LiveTransfer {
                                    path: path.clone(),
                                    bytes_done: s.bytes_done,
                                    total: s.total,
                                };
                                match dir {
                                    1 => uploads.push(lt),
                                    2 => downloads.push(lt),
                                    _ => {}
                                }
                            }
                            match dir {
                                1 => up_rate = up_rate.saturating_add(s.rate),
                                2 => down_rate = down_rate.saturating_add(s.rate),
                                _ => {}
                            }
                        }
                        let _ = live_tx.send(LiveFeed {
                            source: 2,
                            uploads,
                            downloads,
                            up_rate,
                            down_rate,
                        });
                    }
                }
                Err(_) => break,
            }
        }
        // EOF/fejl: dræb child, ryd slot, vent, genstart.
        {
            let mut slot = child_slot.lock().unwrap();
            if let Some(mut c) = slot.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
        paths.clear();
        std::thread::sleep(Duration::from_secs(5));
    }
    // Shutdown: dræb child hvis den stadig ligger i slotten.
    let mut slot = child_slot.lock().unwrap();
    if let Some(mut c) = slot.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

/// Kilde 2, TUI-mode: kør `jotta-cli observe` under `script(1)` (pseudo-TTY,
/// fordi observe-TUI'en ellers afviser uden TTY) og læs stdout i rå chunks.
/// Skærmen vedligeholdes af `jotta_core::observe::TuiScreen` (mini terminal-
/// emulator), så cursor-baserede del-opdateringer også parses korrekt.
/// Ud af skærmen læses: fil-rækker (`Checksuming <sti>` / `Uploading <sti>` /
/// `Downloading <sti>`) og totals-rækken (`Observing upload-totals :: A / B
/// files complete :: X / Y @ Z/s`) der bærer den ÆGTE live hastighed.
/// Sender LiveFeed (kilde 2) ca. 1 gang/s; child genstartes ved EOF, dræbes
/// ved shutdown. Read-only.
fn run_observe_tui_worker(
    shutdown: Arc<AtomicBool>,
    live_tx: std::sync::mpsc::Sender<LiveFeed>,
    child_slot: Arc<Mutex<Option<std::process::Child>>>,
    client: CliClient,
) {
    use std::io::Read as _;
    use jotta_core::observe::{ObserveTuiEvent, TuiScreen};

    #[derive(Clone, Default)]
    struct TotalsState {
        seen: Option<std::time::Instant>,
        done_bytes: Option<u64>,
        total_bytes: Option<u64>,
        rate: u64,
    }

    // path -> (upload, sidst set)
    let mut recent: std::collections::HashMap<String, (bool, std::time::Instant)> =
        std::collections::HashMap::new();
    let mut up_totals = TotalsState::default();
    let mut down_totals = TotalsState::default();
    let mut last_send = std::time::Instant::now() - Duration::from_secs(2);

    'respawn: loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        let mut child = match client.spawn_observe_tui() {
            Ok(c) => c,
            Err(_) => {
                std::thread::sleep(Duration::from_secs(10));
                continue;
            }
        };
        let stdout = child.stdout.take();
        {
            let mut slot = child_slot.lock().unwrap();
            if let Some(mut old) = slot.take() {
                let _ = old.kill();
                let _ = old.wait();
            }
            *slot = Some(child);
        }
        let Some(mut stdout) = stdout else {
            continue 'respawn;
        };
        let mut screen = TuiScreen::new();
        let mut buf = [0u8; 8192];
        loop {
            if shutdown.load(Ordering::SeqCst) {
                break 'respawn;
            }
            match stdout.read(&mut buf) {
                Ok(0) => break, // EOF — child døde; genstart nedenfor
                Ok(n) => {
                    screen.feed(&buf[..n]);
                    for ev in screen.parse_events() {
                        let now = std::time::Instant::now();
                        match ev {
                            ObserveTuiEvent::File { upload, path } => {
                                if !path.is_empty() {
                                    recent.insert(path, (upload, now));
                                }
                            }
                            ObserveTuiEvent::Totals {
                                upload,
                                done_bytes,
                                total_bytes,
                                rate_bps,
                                ..
                            } => {
                                let t = if upload {
                                    &mut up_totals
                                } else {
                                    &mut down_totals
                                };
                                t.seen = Some(now);
                                if done_bytes.is_some() {
                                    t.done_bytes = done_bytes;
                                }
                                if total_bytes.is_some() {
                                    t.total_bytes = total_bytes;
                                }
                                if let Some(r) = rate_bps {
                                    t.rate = r;
                                }
                            }
                        }
                    }
                    // Send højst 1 gang/s; fjern udløbne stier/totals (> 10 s).
                    let now = std::time::Instant::now();
                    if now.duration_since(last_send) >= Duration::from_secs(1) {
                        last_send = now;
                        recent.retain(|_, (_, t)| now.duration_since(*t).as_secs() < 10);
                        // Delvise rækker: mens TUI'en skriver en fil-række ser vi
                        // præfikser af den fulde sti — behold kun den længste.
                        {
                            let mut keys: Vec<&String> = recent.keys().collect();
                            keys.sort_by_key(|k| k.len());
                            let mut drop: Vec<String> = Vec::new();
                            for (i, k) in keys.iter().enumerate() {
                                if keys[i + 1..].iter().any(|k2| k2.starts_with(k.as_str())) {
                                    drop.push((*k).clone());
                                }
                            }
                            for k in drop {
                                recent.remove(&k);
                            }
                        }
                        for t in [&mut up_totals, &mut down_totals] {
                            if t.seen.map(|x| now.duration_since(x).as_secs() >= 10).unwrap_or(true) {
                                *t = TotalsState::default();
                            }
                        }
                        let mut uploads = Vec::new();
                        let mut downloads = Vec::new();
                        for (path, (up, _)) in &recent {
                            let lt = LiveTransfer {
                                path: path.clone(),
                                bytes_done: None,
                                total: None,
                            };
                            if *up {
                                uploads.push(lt);
                            } else {
                                downloads.push(lt);
                            }
                        }
                        // Aggregerede totals-rækker EFTER fil-rækkerne, så
                        // "aktuel fil" stadig vælger den første rigtige fil.
                        if let (Some(d), Some(tot)) = (up_totals.done_bytes, up_totals.total_bytes) {
                            if tot > 0 {
                                uploads.push(LiveTransfer {
                                    path: "Uploads".to_string(),
                                    bytes_done: Some(d),
                                    total: Some(tot),
                                });
                            }
                        }
                        if let (Some(d), Some(tot)) = (down_totals.done_bytes, down_totals.total_bytes) {
                            if tot > 0 {
                                downloads.push(LiveTransfer {
                                    path: "Downloads".to_string(),
                                    bytes_done: Some(d),
                                    total: Some(tot),
                                });
                            }
                        }
                        let _ = live_tx.send(LiveFeed {
                            source: 2,
                            uploads,
                            downloads,
                            up_rate: up_totals.rate,
                            down_rate: down_totals.rate,
                        });
                    }
                }
                Err(_) => break,
            }
        }
        // EOF/fejl: dræb child, ryd slot, vent, genstart.
        {
            let mut slot = child_slot.lock().unwrap();
            if let Some(mut c) = slot.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
        recent.clear();
        up_totals = TotalsState::default();
        down_totals = TotalsState::default();
        std::thread::sleep(Duration::from_secs(5));
    }
    // Shutdown: dræb child hvis den stadig ligger i slotten.
    let mut slot = child_slot.lock().unwrap();
    if let Some(mut c) = slot.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

/// Worker-tråd: stream `jotta-cli sync log --watch` (kun hvis `sync --help`
/// viser `log`) og send LiveFeed-samples (kilde 3) med FILNAVNE ca. 1 gang/s.
/// Linjer: `2026-01-20 20:10:22 +0100 CET :: Upload /sti/fil` — parseren i
/// jotta_core::synclog håndterer ANSI-prefikser. Ingen bytes her, så
/// up_rate/down_rate sættes til 0 (hastighed kommer fra kilde 0/1/2).
/// Child genstartes ved EOF; dræbes ved shutdown. Read-only.
fn run_sync_log_worker(
    shutdown: Arc<AtomicBool>,
    live_tx: std::sync::mpsc::Sender<LiveFeed>,
    child_slot: Arc<Mutex<Option<std::process::Child>>>,
) {
    use std::io::BufRead as _;
    let Some(loc) = BinaryLocator::from_path() else {
        return;
    };
    let client = CliClient::new(loc);
    // Detekér `sync log` én gang (ældre jotta-cli har måske ikke subkommandoen).
    let supported = client
        .run_text(&JottaCommand::SyncHelp)
        .map(|h| jotta_core::synclog::sync_help_lists_log(&h))
        .unwrap_or(false);
    if !supported {
        return;
    }
    // path -> (retning, sidst set)
    let mut recent: std::collections::HashMap<String, (bool, std::time::Instant)> =
        std::collections::HashMap::new();
    let mut last_send = std::time::Instant::now() - Duration::from_secs(2);

    'respawn: loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        let mut child = match client.spawn_stream(&JottaCommand::SyncLogWatch) {
            Ok(c) => c,
            Err(_) => {
                std::thread::sleep(Duration::from_secs(10));
                continue;
            }
        };
        let stdout = child.stdout.take();
        {
            let mut slot = child_slot.lock().unwrap();
            if let Some(mut old) = slot.take() {
                let _ = old.kill();
                let _ = old.wait();
            }
            *slot = Some(child);
        }
        let Some(stdout) = stdout else {
            continue 'respawn;
        };
        let mut reader = std::io::BufReader::new(stdout);
        let mut line = String::new();
        loop {
            if shutdown.load(Ordering::SeqCst) {
                break 'respawn;
            }
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if let Some(ev) = jotta_core::synclog::parse_sync_log_line(&line) {
                        use jotta_core::synclog::SyncLogAction;
                        let is_up = match ev.action {
                            SyncLogAction::Upload => Some(true),
                            SyncLogAction::Download => Some(false),
                            _ => None,
                        };
                        if let (Some(up), false) = (is_up, ev.path.is_empty()) {
                            recent.insert(ev.path, (up, std::time::Instant::now()));
                        }
                    }
                    let now = std::time::Instant::now();
                    if now.duration_since(last_send) >= Duration::from_secs(1) {
                        last_send = now;
                        // Fjern stier uden events i > 10 s (færdige overførsler).
                        recent.retain(|_, (_, t)| now.duration_since(*t).as_secs() < 10);
                        let mut uploads = Vec::new();
                        let mut downloads = Vec::new();
                        for (path, (up, _)) in &recent {
                            let lt = LiveTransfer {
                                path: path.clone(),
                                bytes_done: None,
                                total: None,
                            };
                            if *up {
                                uploads.push(lt);
                            } else {
                                downloads.push(lt);
                            }
                        }
                        let _ = live_tx.send(LiveFeed {
                            source: 3,
                            uploads,
                            downloads,
                            up_rate: 0,
                            down_rate: 0,
                        });
                    }
                }
                Err(_) => break,
            }
        }
        {
            let mut slot = child_slot.lock().unwrap();
            if let Some(mut c) = slot.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
        recent.clear();
        std::thread::sleep(Duration::from_secs(5));
    }
    let mut slot = child_slot.lock().unwrap();
    if let Some(mut c) = slot.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

fn build_ui(app: &adw::Application, shutdown: Arc<AtomicBool>) {
    let core = Arc::new(App::new());

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .default_width(800)
        .default_height(600)
        .title("Jotta Desktop")
        .icon_name("io.github.eske.JottaDesktop")
        .build();

    // Toast overlay for command feedback.
    let toast_overlay = adw::ToastOverlay::new();

    // Channel from worker threads -> main loop for toast messages.
    // Only String crosses the thread boundary; the toast is created on the main thread.
    // SEMANTIK (owner-requested): én aktuel toast — en ny besked ERSTATTER teksten
    // i den synlige toast med det samme (ingen kø), og toasten dismisses 4 s efter
    // den seneste besked (timer nulstilles pr. besked).
    let (toast_tx, toast_rx) = std::sync::mpsc::channel::<String>();
    {
        struct ToastState {
            current: Option<adw::Toast>,
            timer: Option<glib::SourceId>,
        }
        let state = std::rc::Rc::new(std::cell::RefCell::new(ToastState {
            current: None,
            timer: None,
        }));
        let toast_overlay = toast_overlay.clone();
        let toast_rx = std::sync::Mutex::new(toast_rx);
        // CRASH-FIX (ABRT/panic_cannot_unwind): `Toast::dismiss()` emitter
        // "dismissed"-signalet SYNKRONT, og vores connect_dismissed-handler
        // kalder derefter borrow_mut() på samme Rc<RefCell>. Hvis vi holder et
        // borrow mens vi kalder dismiss(), panikker RefCell over FFI-grænsen
        // (panic_cannot_unwind -> abort). Derfor: ALDRIG hold et RefCell-borrow
        // på tværs af GTK-kald der kan emitte signaler — kun korte borrows der
        // slippes før selve kaldet.
        glib::idle_add_local(move || {
            let rx = toast_rx.lock().unwrap();
            while let Ok(msg) = rx.try_recv() {
                let state2 = state.clone();
                let toast_overlay2 = toast_overlay.clone();
                let existing = state2.borrow().current.clone();
                match existing {
                    Some(t) => {
                        t.set_title(&msg); // nyeste besked vinder med det samme
                    }
                    None => {
                        let t = adw::Toast::new(&msg);
                        // AdwToast parses som Pango-markup som standard (>= 1.4);
                        // vores beskeder er ren tekst (stier kan indeholde '&').
                        t.set_use_markup(false);
                        let state3 = state2.clone();
                        t.connect_dismissed(move |_| {
                            // try_borrow_mut: forsvar mod re-entry — et abort over
                            // FFI er værre end en enkelt oversprunget oprydning.
                            if let Ok(mut st) = state3.try_borrow_mut() {
                                st.current = None;
                            }
                        });
                        toast_overlay2.add_toast(t.clone());
                        state2.borrow_mut().current = Some(t);
                    }
                }
                // Nulstil dismiss-timer (4 s efter seneste besked).
                {
                    let mut st = state2.borrow_mut();
                    if let Some(id) = st.timer.take() {
                        id.remove();
                    }
                }
                let state4 = state2.clone();
                let id = glib::timeout_add_local(Duration::from_secs(4), move || {
                    // Tag toasten ud og SLIP BORROWET før dismiss() — dismiss()
                    // emitter "dismissed" synkront og ville ellers re-enter
                    // connect_dismissed-handleren mens samme RefCell er låst.
                    let t = {
                        let mut st = state4.borrow_mut();
                        st.timer = None;
                        st.current.take()
                    };
                    if let Some(t) = t {
                        t.dismiss();
                    }
                    glib::ControlFlow::Break
                });
                state2.borrow_mut().timer = Some(id);
            }
            glib::ControlFlow::Continue
        });
    }

    let sidebar = gtk::ListBox::new();
    sidebar.add_css_class("navigation-sidebar");
    // Iteration 14: 5 faner (SPEC §3) — Overførsler & Gendan er smeltet sammen,
    // Info/Konto bor i Indstillinger.
    sidebar.append(&nav_item(i18n::t("nav.status"), "io.github.eske.JottaDesktop-symbolic"));
    sidebar.append(&nav_item(i18n::t("nav.backup"), "folder-symbolic"));
    sidebar.append(&nav_item(i18n::t("nav.sync"), "folder-sync-symbolic"));
    sidebar.append(&nav_item(i18n::t("nav.transfers_restore"), "emblem-downloads-symbolic"));
    sidebar.append(&nav_item(i18n::t("nav.settings"), "emblem-system-symbolic"));

    let stack = gtk::Stack::new();

    // --- Status page widgets (kept in a struct so the poller can update them) ---
    let status_state_label = gtk::Label::new(Some(i18n::t("status.state.offline")));
    let status_email_label = gtk::Label::new(None);
    status_email_label.add_css_class("dim-label");
    let status_level = gtk::LevelBar::new();
    status_level.set_min_value(0.0);
    status_level.set_max_value(1.0);
    let status_usage_label = gtk::Label::new(None);
    status_usage_label.add_css_class("dim-label");
    let pause_switch = gtk::Switch::new();
    pause_switch.set_valign(gtk::Align::Center);
    // Initial state: ON = kører (ikke pauset). jottad kører som standard,
    // og status.paused er ofte None i JSON — så vi antager "kører".
    pause_switch.set_active(true);
    // Daemon-not-responding banner: vises når jottad ikke svarer (State::Offline).
    let daemon_banner = adw::Banner::new(i18n::t("status.daemon.banner"));
    daemon_banner.set_button_label(Some(i18n::t("status.daemon.reload")));
    daemon_banner.set_revealed(false);

    // Login-række (iteration 11): synlig kun ved State::NeedsLogin.
    // Knappen åbner den grafiske login-dialog, der føder jotta-cli login's
    // tre prompts programmatisk (licens, token, enhedsnavn) — tokenet går
    // kun over stdin, aldrig argv eller logs.
    let login_row = adw::ActionRow::builder()
        .title(i18n::t("status.login.title"))
        .subtitle(i18n::t("status.login.subtitle"))
        .build();
    let login_btn = gtk::Button::with_label(i18n::t("status.login.button"));
    login_btn.add_css_class("suggested-action");
    login_btn.set_valign(gtk::Align::Center);
    login_btn.set_visible(false);
    login_row.add_suffix(&login_btn);
    login_row.set_activatable_widget(Some(&login_btn));
    {
        let window = window.clone();
        let toast_tx = toast_tx.clone();
        login_btn.connect_clicked(move |_| {
            if let Some(loc) = BinaryLocator::from_path() {
                login::open_login_dialog(&window, loc, toast_tx.clone());
            }
        });
    }

    // "Log ud" (iteration 12): DESTRUKTIV — altid bekræft før `jotta-cli logout`.
    // Synlig når logget ind; efter logout går status -> NeedsLogin, og
    // "Log ind"-rækken dukker op igen (skift bruger = log ud + log ind igen).
    let logout_row = adw::ActionRow::builder()
        .title(i18n::t("status.logout.title"))
        .subtitle(i18n::t("status.logout.subtitle"))
        .build();
    let logout_btn = gtk::Button::with_label(i18n::t("status.logout.button"));
    logout_btn.add_css_class("destructive-action");
    logout_btn.set_valign(gtk::Align::Center);
    logout_btn.set_visible(false);
    logout_row.add_suffix(&logout_btn);
    logout_row.set_activatable_widget(Some(&logout_btn));
    {
        let window = window.clone();
        let toast_tx = toast_tx.clone();
        logout_btn.connect_clicked(move |_| {
            let dialog = adw::AlertDialog::new(
                Some(i18n::t("status.logout.confirm.title")),
                Some(i18n::t("status.logout.confirm.body")),
            );
            dialog.add_response("cancel", i18n::t("common.cancel"));
            dialog.add_response("logout", i18n::t("status.logout.button"));
            dialog.set_response_appearance("logout", adw::ResponseAppearance::Destructive);
            dialog.set_default_response(Some("cancel"));
            dialog.set_close_response("cancel");
            let toast_tx = toast_tx.clone();
            dialog.choose(&window, None::<&gtk::gio::Cancellable>, move |resp| {
                if resp == "logout" {
                    run_cli(JottaCommand::Logout);
                    let _ = toast_tx.send(i18n::t("status.logout.toast").to_string());
                }
            });
        });
    }

    // Live up/down-hastighed + aktuel overførsel (iteration 14): opdateres af
    // live-feedet (observe --json stream, ellers status --json, ellers list).
    let status_speed_label = gtk::Label::new(Some("—"));
    status_speed_label.add_css_class("dim-label");
    let status_current_label = gtk::Label::new(None);
    status_current_label.add_css_class("dim-label");
    let status_current_row = adw::ActionRow::builder()
        .title(i18n::t("status.current.row"))
        .build();
    status_current_row.add_suffix(&status_current_label);
    status_current_label.set_halign(gtk::Align::End);
    status_current_row.set_visible(false);
    let status_page = build_status_page(
        &status_state_label,
        &status_email_label,
        &status_level,
        &status_usage_label,
        &pause_switch,
        &daemon_banner,
        &status_speed_label,
        &status_current_row,
    );
    stack.add_titled(&status_page, Some("status"), i18n::t("nav.status"));

    // --- Backup page ---
    let backup_list = gtk::ListBox::new();
    backup_list.add_css_class("boxed-list");
    // Planlægning (SPEC §3): scaninterval bor på Backup-fanen; Gem-knappen i
    // Indstillinger > Avanceret læser stadig samme row (passeret videre).
    let scan_row = adw::EntryRow::builder()
        .title(i18n::t("settings.scaninterval"))
        .build();
    let schedule_group = adw::PreferencesGroup::builder()
        .title(i18n::t("backup.schedule.group"))
        .build();
    schedule_group.add(&scan_row);
    let backup_page = build_backup_page(&backup_list, &schedule_group);
    stack.add_titled(&backup_page, Some("backup"), i18n::t("nav.backup"));

    // --- Sync page ---
    // Sync selective is NOT destructive (unlike backup ignores): ignored
    // folders are not deleted locally or in the cloud. No confirmation dialog.
    let sync_title_row = adw::ActionRow::builder()
        .title(i18n::t("sync.switch.title"))
        .subtitle(i18n::t("sync.switch.subtitle"))
        .build();
    let sync_path_row = adw::ActionRow::builder()
        .title(i18n::t("sync.path.title"))
        .subtitle("—")
        .build();
    let sync_count_row = adw::ActionRow::builder()
        .title(i18n::t("sync.files.title"))
        .subtitle("—")
        .build();
    let sync_mode_row = adw::ActionRow::builder()
        .title(i18n::t("sync.mode.title"))
        .subtitle("—")
        .build();
    let sync_switch = gtk::Switch::new();
    sync_switch.set_valign(gtk::Align::Center);
    sync_title_row.add_suffix(&sync_switch);
    let sync_group = adw::PreferencesGroup::new();
    sync_group.add(&sync_title_row);
    sync_group.add(&sync_path_row);
    sync_group.add(&sync_count_row);
    sync_group.add(&sync_mode_row);
    // "Synkroniser nu" — kører én sync nu (`sync trigger`); knappen er kun
    // aktiv i manuel tilstand (kontinuerlig sync kører jo allerede).
    let sync_trigger_row = adw::ActionRow::builder()
        .title(i18n::t("sync.trigger.title"))
        .subtitle(i18n::t("sync.trigger.subtitle"))
        .build();
    let sync_trigger_btn = gtk::Button::with_label(i18n::t("sync.trigger.button"));
    sync_trigger_btn.set_valign(gtk::Align::Center);
    sync_trigger_btn.set_sensitive(false);
    sync_trigger_row.add_suffix(&sync_trigger_btn);
    sync_trigger_row.set_activatable_widget(Some(&sync_trigger_btn));
    sync_group.add(&sync_trigger_row);
    // "Vælg mapper at synkronisere" — chevron-ikon for tydelighed (åbner dialog).
    let sync_selective_btn = gtk::Button::from_icon_name("go-next-symbolic");
    sync_selective_btn.add_css_class("flat");
    sync_selective_btn.set_tooltip_text(Some(i18n::t("sync.selective.tooltip")));
    let sync_selective_row = adw::ActionRow::builder()
        .title(i18n::t("sync.selective.page.title"))
        .subtitle(i18n::t("sync.selective.page.subtitle"))
        .build();
    sync_selective_row.add_suffix(&sync_selective_btn);
    sync_group.add(&sync_selective_row);
    let sync_page = build_sync_page(&sync_group);
    stack.add_titled(&sync_page, Some("sync"), i18n::t("nav.sync"));

    // --- Overførsler (transfers) page: uploads + downloads + errors ---
    // Three sections: active uploads, active downloads, recent errors.
    // Data comes from `list uploads --json`, `list downloads --json`,
    // `list downloaderrors --json`, `list uploaderrors --json`.
    let transfers_uploads_list = gtk::ListBox::new();
    transfers_uploads_list.add_css_class("boxed-list");
    let transfers_uploads_group = adw::PreferencesGroup::builder()
        .title(i18n::t("transfers.uploads.title"))
        .build();
    transfers_uploads_group.add(&transfers_uploads_list);

    let transfers_downloads_list = gtk::ListBox::new();
    transfers_downloads_list.add_css_class("boxed-list");
    let transfers_downloads_group = adw::PreferencesGroup::builder()
        .title(i18n::t("transfers.downloads.title"))
        .build();
    transfers_downloads_group.add(&transfers_downloads_list);

    let transfers_errors_list = gtk::ListBox::new();
    transfers_errors_list.add_css_class("boxed-list");
    let transfers_errors_group = adw::PreferencesGroup::builder()
        .title(i18n::t("transfers.errors.title"))
        .build();
    transfers_errors_group.add(&transfers_errors_list);

    // "I gang nu" (live, iteration 14): aktuelle overførsler med fremskridt —
    // fyldes af live-feedet (observe --json / status / list), skjult når idle.
    let live_list = gtk::ListBox::new();
    live_list.add_css_class("boxed-list");
    let live_group = adw::PreferencesGroup::builder()
        .title(i18n::t("transfers.live.group"))
        .build();
    live_group.add(&live_list);
    live_group.set_visible(false);


    // --- Gendan (restore/download) page ---
    // Remote browser for Backup/Archive/Sync + download dialog + downloads list.
    // Trash is web-only (trash list has no --json, plain output is unparseable).
    let restore_path_label = gtk::Label::new(Some("Jottacloud"));
    restore_path_label.set_halign(gtk::Align::Start);
    restore_path_label.add_css_class("dim-label");
    restore_path_label.set_wrap(true);
    let restore_path_up_btn = gtk::Button::from_icon_name("go-up-symbolic");
    restore_path_up_btn.add_css_class("flat");
    restore_path_up_btn.set_tooltip_text(Some(i18n::t("restore.up.tooltip")));
    restore_path_up_btn.set_valign(gtk::Align::Center);
    let restore_refresh_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
    restore_refresh_btn.add_css_class("flat");
    restore_refresh_btn.set_tooltip_text(Some(i18n::t("restore.refresh.tooltip")));
    restore_refresh_btn.set_valign(gtk::Align::Center);
    let restore_path_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    restore_path_box.append(&restore_path_up_btn);
    restore_path_box.append(&restore_path_label);
    restore_path_box.append(&restore_refresh_btn);

    let restore_list = gtk::ListBox::new();
    restore_list.add_css_class("boxed-list");
    // BUG 5 fix: enkeltklik = markér, dobbeltklik = åbn (standard-UX).
    restore_list.set_activate_on_single_click(false);
    let restore_scrolled = gtk::ScrolledWindow::new();
    restore_scrolled.set_child(Some(&restore_list));
    restore_scrolled.set_vexpand(true);
    restore_scrolled.set_min_content_height(200);

    let restore_download_btn = gtk::Button::with_label(i18n::t("restore.download.button"));
    restore_download_btn.add_css_class("suggested-action");
    let restore_download_row = adw::ActionRow::builder()
        .title(i18n::t("restore.download.dialog.title"))
        .subtitle(i18n::t("restore.download.dialog.subtitle"))
        .build();
    restore_download_row.add_suffix(&restore_download_btn);
    let restore_group = adw::PreferencesGroup::new();
    restore_group.add(&restore_download_row);

    let restore_downloads_list = gtk::ListBox::new();
    restore_downloads_list.add_css_class("boxed-list");
    let restore_downloads_group = adw::PreferencesGroup::builder()
        .title(i18n::t("nav.transfers"))
        .build();

    // Trash: web-only (trash list has no --json, plain output unparseable).
    let restore_trash_web_btn = gtk::Button::from_icon_name("go-next-symbolic");
    restore_trash_web_btn.add_css_class("flat");
    restore_trash_web_btn.set_tooltip_text(Some(i18n::t("restore.trash.tooltip")));
    let restore_trash_row = adw::ActionRow::builder()
        .title(i18n::t("restore.trash.title"))
        .subtitle(i18n::t("restore.trash.subtitle"))
        .build();
    restore_trash_row.add_suffix(&restore_trash_web_btn);
    let restore_trash_group = adw::PreferencesGroup::new();
    restore_trash_group.add(&restore_trash_row);


    // --- Info page: device, versions, subscription, account ---
    // Data from status --json (device, email, subscription) +
    // `jotta-cli version` on a worker thread (covers both jottad + jotta-cli
    // since they share the same version number in practice).
    let info_device_row = adw::ActionRow::builder().title(i18n::t("info.device")).build();
    let info_email_row = adw::ActionRow::builder().title(i18n::t("status.account.email")).build();
    let info_sub_row = adw::ActionRow::builder().title(i18n::t("info.subscription")).build();
    let info_jotta_ver_row = adw::ActionRow::builder().title(i18n::t("info.jotta.version")).build();
    let info_app_ver_row = adw::ActionRow::builder().title(i18n::t("info.app.version")).build();
    // Iteration 14: Overførsler & Gendan som ÉN fane (SPEC §3).
    let transfers_restore_page = build_transfers_restore_page(
        &live_group,
        &transfers_uploads_group,
        &transfers_downloads_group,
        &transfers_errors_group,
        &restore_path_box,
        &restore_scrolled,
        &restore_group,
        &restore_downloads_group,
        &restore_downloads_list,
        &restore_trash_group,
    );
    stack.add_titled(
        &transfers_restore_page,
        Some("transfers_restore"),
        i18n::t("nav.transfers_restore"),
    );

    // Tidligere Info-fane bliver til "Konto & enhed" + "Om"-grupper i Indstillinger.
    // Log ind/Log ud-rækkerne flytter også hertil (stadig styret af status-polleren).
    let account_group = adw::PreferencesGroup::builder()
        .title(i18n::t("settings.account.group"))
        .build();
    account_group.add(&info_device_row);
    account_group.add(&info_email_row);
    account_group.add(&info_sub_row);
    account_group.add(&login_row);
    account_group.add(&logout_row);
    let about_group = adw::PreferencesGroup::builder()
        .title(i18n::t("settings.about.group"))
        .build();
    about_group.add(&info_jotta_ver_row);
    about_group.add(&info_app_ver_row);

    let settings_page = build_settings_page(toast_tx.clone(), &account_group, &about_group, &scan_row);
    stack.add_titled(&settings_page, Some("settings"), i18n::t("nav.settings"));

    sidebar.connect_row_selected(clone!(
        #[strong]
        stack,
        move |_, row| {
            if let Some(row) = row {
                let idx = row.index();
                let page = match idx {
                    0 => Some("status"),
                    1 => Some("backup"),
                    2 => Some("sync"),
                    3 => Some("transfers_restore"),
                    4 => Some("settings"),
                    _ => None,
                };
                if let Some(p) = page {
                    stack.set_visible_child_name(p);
                }
            }
        },
    ));

    let sidebar_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    // App-titel i toppen af sidebar for visuel balance.
    let app_title = gtk::Label::new(None);
    app_title.set_markup("<span size='large' weight='bold'>Jotta Desktop</span>");
    app_title.set_halign(gtk::Align::Start);
    app_title.set_margin_start(12);
    app_title.set_margin_top(18);
    app_title.set_margin_bottom(6);
    sidebar_box.append(&app_title);
    sidebar.set_margin_top(6);
    sidebar.set_margin_bottom(12);
    sidebar.set_margin_start(12);
    sidebar.set_margin_end(12);
    sidebar_box.append(&sidebar);

    let sidebar_page = adw::NavigationPage::new(&sidebar_box, "Jotta Desktop");

    let split_view = adw::NavigationSplitView::new();
    split_view.set_sidebar(Some(&sidebar_page));

    // FEATURE: Sider skal kunne scrolles når indholdet overflower.
    // gtk::Stack er IKKE scrollable — vi wrapper hver side individuelt
    // i en ScrolledWindow ved oprettelsen (se build_*_page funktioner).
    // Her wrapper vi hele stack i en ydre ScrolledWindow IKKE —
    // i stedet sikrer vi at hver side har sin egen scroll.

    // FEATURE: Hovedvindue med HeaderBar (minimer/maximer/luk knapper).
    // ToolbarView + HeaderBar giver standard window controls.
    let main_toolbar = adw::ToolbarView::new();
    let main_header = adw::HeaderBar::new();
    // Vis standard window title buttons (minimer, maksimer, luk).
    main_header.set_show_start_title_buttons(true);
    main_header.set_show_end_title_buttons(true);
    main_toolbar.add_top_bar(&main_header);
    main_toolbar.set_content(Some(&stack));

    // NavigationPage wrapper så split_view stadig virker.
    let content_page = adw::NavigationPage::new(&main_toolbar, i18n::t("nav.status"));
    split_view.set_content(Some(&content_page));

    // Dynamisk topbar titel: opdater NavigationPage titel når aktiv fane skifter.
    stack.connect_visible_child_notify(clone!(
        #[weak]
        content_page,
        move |stack| {
            let title = stack
                .visible_child_name()
                .and_then(|name| match name.as_str() {
                    "status" => Some(i18n::t("nav.status")),
                    "backup" => Some(i18n::t("nav.backup")),
                    "sync" => Some(i18n::t("nav.sync")),
                    "transfers_restore" => Some(i18n::t("nav.transfers_restore")),
                    "settings" => Some(i18n::t("nav.settings")),
                    _ => None,
                })
                .unwrap_or("Jotta Desktop");
            content_page.set_title(title);
        },
    ));

    toast_overlay.set_child(Some(&split_view));
    window.set_content(Some(&toast_overlay));

    // --- Wire handlers ---
    // Guards: når polleren opdaterer switches programmatisk, sættes guard=true
    // så connect_state_set-handleren returnerer tidligt uden at udstede kommandoer.
    // REGEL: kun brugerhandling udsteder kommandoer. Polleren skriver ALDRIG kommandoer.
    let pause_updating = std::rc::Rc::new(Cell::new(false));
    let sync_updating = std::rc::Rc::new(Cell::new(false));
    // Lockout: true i 5s efter bruger-klik på pause-switch, så polleren
    // ikke reverterer switchen før daemonen når at opdatere status.
    let pause_lockout = Arc::new(AtomicBool::new(false));

    // Pause switch: bruger-toggle → pause/resume (global jottad pause).
    // pause/resume pauser HELE jottad (backup + sync) — matcher UI-label.
    // Idempotent: "already running" ved resume = allerede aktiv = success.
    // Lockout: efter bruger-klik sættes pause_lockout=true i 5s så polleren
    // ikke reverterer switchen før daemonen når at opdatere status.
    {
        let pause_updating = pause_updating.clone();
        let pause_switch = pause_switch.clone();
        let toast_tx = toast_tx.clone();
        let pause_lockout = pause_lockout.clone();
        pause_switch.connect_state_set(move |_sw, state| {
            // Guard: programmatisk opdatering fra polleren — gør ingenting.
            if pause_updating.get() {
                return Propagation::Proceed;
            }
            // state=true → resume (unpause), state=false → pause
            let cmd = if state {
                JottaCommand::Resume
            } else {
                JottaCommand::Pause
            };
            // Sæt lockout så polleren ikke reverterer før status opdateres.
            // 15s: jottad kan være langsom til at opdatere status efter pause.
            pause_lockout.store(true, Ordering::SeqCst);
            {
                let lockout = pause_lockout.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(15));
                    lockout.store(false, Ordering::SeqCst);
                });
            }
            let toast_tx = toast_tx.clone();
            std::thread::spawn(move || {
                if let Some(loc) = BinaryLocator::from_path() {
                    let client = CliClient::new(loc);
                    if let Err(e) = client.run(&cmd) {
                        let msg = e.to_string();
                        // "already running" ved resume = idempotent success.
                        if msg.contains("already running") {
                            return;
                        }
                        let _ = toast_tx.send(tf!("sync.pause.error", e));
                    }
                }
            });
            Propagation::Proceed
        });
    }

    // Sync switch: ON = kontinuerlig sync (SyncStart), OFF = manuel tilstand
    // (SyncStop). "Stop" bekræftes (safety rule 3) — det ændrer tilstanden,
    // men sletter ikke filer.
    {
        let window = window.clone();
        let sync_switch = sync_switch.clone();
        let sync_updating = sync_updating.clone();
        sync_switch.connect_state_set(move |sw, state| {
            // Guard: programmatisk opdatering fra polleren — gør ingenting.
            if sync_updating.get() {
                return Propagation::Proceed;
            }
            if !state {
                // Turning OFF: confirm before stopping sync.
                let dialog = adw::AlertDialog::new(
                    Some(i18n::t("sync.stop.confirm.title")),
                    Some(i18n::t("sync.stop.confirm.body")),
                );
                dialog.add_response("cancel", i18n::t("common.cancel"));
                dialog.add_response("stop", i18n::t("sync.stop.confirm.action"));
                dialog.set_response_appearance("stop", adw::ResponseAppearance::Destructive);
                dialog.set_default_response(Some("cancel"));
                dialog.set_close_response("cancel");
                let sw2 = sw.clone();
                let window = window.clone();
                let sync_updating = sync_updating.clone();
                dialog.choose(&window, None::<&gtk::gio::Cancellable>, move |resp| {
                    if resp == "stop" {
                        run_cli(JottaCommand::SyncStop);
                    } else {
                        // Revert the switch (blocked handler — ingen kommando).
                        sync_updating.set(true);
                        sw2.set_active(true);
                        sync_updating.set(false);
                    }
                });
                return Propagation::Stop;
            }
            run_cli(JottaCommand::SyncStart);
            Propagation::Proceed
        });
    }

    // "Synkroniser nu": én manuel sync-kørsel (`sync trigger`). Ikke-destruktiv.
    {
        let toast_tx = toast_tx.clone();
        sync_trigger_btn.connect_clicked(move |_| {
            run_cli(JottaCommand::SyncTrigger);
            let _ = toast_tx.send(i18n::t("sync.trigger.toast").to_string());
        });
    }

    // "Tilføj mappe" button → FileDialog → BackupAdd.
    let add_btn = gtk::Button::with_label(i18n::t("backup.add.button"));
    add_btn.add_css_class("suggested-action");
    let toast_tx_add = toast_tx.clone();
    add_btn.connect_clicked(clone!(
        #[weak]
        window,
        move |_| {
            let dialog = gtk::FileDialog::new();
            dialog.set_title(i18n::t("backup.dialog.title"));
            dialog.set_modal(true);
            dialog.set_accept_label(Some(i18n::t("backup.dialog.accept")));
            let toast_tx_add = toast_tx_add.clone();
            dialog.select_folder(Some(&window), None::<&gtk::gio::Cancellable>, move |res| {
                match res {
                    Ok(file) => {
                        if let Some(path) = file.path() {
                            run_cli(JottaCommand::BackupAdd(path));
                            let _ = toast_tx_add.send(i18n::t("backup.added.toast").to_string());
                        }
                    }
                    Err(_) => {} // user cancelled
                }
            });
        },
    ));
    // Replace the placeholder button on the backup page with the wired one.
    replace_add_button(&backup_page, &add_btn);

    // --- Sync selective: "Vælg mapper at synkronisere" checkbox dialog ---
    // NOT destructive (unlike backup ignores) — no confirmation dialog.
    // Matches the official Jottacloud app's "Choose folders to synchronize".
    let toast_tx_sel = toast_tx.clone();
    sync_selective_btn.connect_clicked(clone!(
        #[weak]
        window,
        move |_| {
            open_sync_selective_dialog(&window, &toast_tx_sel);
        },
    ));

    // --- Gendan page: remote browser state + handlers ---
    // Current remote path being browsed (main thread only, never crosses threads).
    let restore_current_path = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
    // Selected entry in the remote list (main thread only).
    let restore_selected = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
    // Whether the selected entry is a directory (main thread only).
    let restore_selected_is_dir = std::rc::Rc::new(std::cell::Cell::new(true));

    // Initial load: show static root (Backup / Archive / Sync).
    // No worker thread needed — this is static data applied directly.
    {
        let entries = [
            ("Backup", i18n::t("restore.devices")),
            ("Archive", i18n::t("restore.archived")),
            ("Sync", i18n::t("restore.synced")),
        ];
        for (name, desc) in entries {
            let row = adw::ActionRow::builder()
                .title(name)
                .subtitle(&tf!("restore.folder.desc", desc))
                .build();
            let img = gtk::Image::from_icon_name("folder-symbolic");
            row.add_prefix(&img);
            row.set_activatable(true);
            restore_list.append(&row);
        }
        restore_path_label.set_text("Backup / Archive / Sync");
    }

    // Row selected in remote list -> update selection (main thread only).
    restore_list.connect_row_selected(clone!(
        #[strong]
        restore_selected,
        #[strong]
        restore_selected_is_dir,
        move |_, row| {
            if let Some(row) = row {
                if let Some(action_row) = row.downcast_ref::<adw::ActionRow>() {
                    let name = action_row.title().to_string();
                    let subtitle = action_row.subtitle().unwrap_or_default();
                    *restore_selected.borrow_mut() = name;
                    // "Mappe —" prefix means directory; "Fil —" means file.
                    restore_selected_is_dir.set(subtitle.starts_with(i18n::t("restore.kind.folder")));
                }
            } else {
                restore_selected.borrow_mut().clear();
            }
        },
    ));

    // Double-click / activate row -> navigate into directory or show file revisions.
    // Worker thread fetches JSON listing; result via mpsc + idle_add_local.
    // Channel carries LsResult (plain data: Folders / Revisions / Empty + error string).
    restore_list.connect_row_activated(clone!(
        #[strong]
        restore_current_path,
        #[strong]
        restore_list,
        #[strong]
        restore_path_label,
        #[strong]
        restore_selected,
        move |_, row| {
            if let Some(action_row) = row.downcast_ref::<adw::ActionRow>() {
                let name = action_row.title().to_string();
                let subtitle = action_row.subtitle().unwrap_or_default();
                let is_dir = subtitle.starts_with(i18n::t("restore.kind.folder"));
                // Ghost device: subtitle starts with "Offline enhed" — not navigable.
                let is_ghost = subtitle.starts_with(i18n::t("restore.kind.offline"));
                if is_ghost {
                    return; // graceful: no navigation into dead devices
                }
                if is_dir {
                    let new_path = if restore_current_path.borrow().is_empty() {
                        name.clone()
                    } else {
                        format!("{}/{}", restore_current_path.borrow(), name)
                    };
                    *restore_current_path.borrow_mut() = new_path.clone();
                    restore_path_label.set_text(&new_path);
                    *restore_selected.borrow_mut() = String::new();

                    let (data_tx, data_rx) = std::sync::mpsc::channel::<LsNavResult>();
                    let data_rx = Arc::new(Mutex::new(data_rx));
                    let list = restore_list.clone();
                    let sel = restore_selected.clone();
                    glib::idle_add_local(move || {
                        let rx = data_rx.lock().unwrap();
                        match rx.try_recv() {
                            Ok(result) => {
                                rebuild_remote_list_result(&list, &result, &sel);
                                glib::ControlFlow::Break
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
                        }
                    });

                    let path = new_path.clone();
                    std::thread::spawn(move || {
                        if let Some(loc) = BinaryLocator::from_path() {
                            let client = CliClient::new(loc);
                            let result = match client.run_json(&JottaCommand::LsRemote(path)) {
                                Ok(v) => LsNavResult::from(jotta_core::remote::parse_ls_json(&v)),
                                Err(e) => LsNavResult::Error(format!("{e}")),
                            };
                            let _ = data_tx.send(result);
                        }
                    });
                } else {
                    // File: fetch revisions (for future "gendan gammel version" feature).
                    let file_path = if restore_current_path.borrow().is_empty() {
                        name.clone()
                    } else {
                        format!("{}/{}", restore_current_path.borrow(), name)
                    };
                    let (data_tx, data_rx) = std::sync::mpsc::channel::<LsNavResult>();
                    let data_rx = Arc::new(Mutex::new(data_rx));
                    let list = restore_list.clone();
                    let sel = restore_selected.clone();
                    let path_label = restore_path_label.clone();
                    glib::idle_add_local(move || {
                        let rx = data_rx.lock().unwrap();
                        match rx.try_recv() {
                            Ok(result) => {
                                rebuild_remote_list_result(&list, &result, &sel);
                                // Show path in label so user sees where revisions are.
                                glib::ControlFlow::Break
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
                        }
                    });
                    let path = file_path.clone();
                    std::thread::spawn(move || {
                        if let Some(loc) = BinaryLocator::from_path() {
                            let client = CliClient::new(loc);
                            let result = match client.run_json(&JottaCommand::LsRemote(path)) {
                                Ok(v) => LsNavResult::from(jotta_core::remote::parse_ls_json(&v)),
                                Err(e) => LsNavResult::Error(format!("{e}")),
                            };
                            let _ = data_tx.send(result);
                        }
                    });
                    let _ = path_label;
                }
            }
        },
    ));

    // "En mappe op" button.
    {
        let current_path = restore_current_path.clone();
        let list = restore_list.clone();
        let path_label = restore_path_label.clone();
        let sel = restore_selected.clone();
        restore_path_up_btn.connect_clicked(move |_| {
            let cur = current_path.borrow().clone();
            if cur.is_empty() {
                return; // already at root
            }
            let parent = match cur.rfind('/') {
                Some(idx) => cur[..idx].to_string(),
                None => String::new(),
            };
            *current_path.borrow_mut() = parent.clone();
            if parent.is_empty() {
                path_label.set_text("Backup / Archive / Sync");
            } else {
                path_label.set_text(&parent);
            }
            *sel.borrow_mut() = String::new();

            let (data_tx, data_rx) = std::sync::mpsc::channel::<LsNavResult>();
            let data_rx = Arc::new(Mutex::new(data_rx));
            let list_r = list.clone();
            let sel_r = sel.clone();
            glib::idle_add_local(move || {
                let rx = data_rx.lock().unwrap();
                match rx.try_recv() {
                    Ok(result) => {
                        rebuild_remote_list_result(&list_r, &result, &sel_r);
                        glib::ControlFlow::Break
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
                }
            });

            let path = parent.clone();
            std::thread::spawn(move || {
                if let Some(loc) = BinaryLocator::from_path() {
                    let client = CliClient::new(loc);
                    let result = if path.is_empty() {
                        // Root: static entries.
                        LsNavResult::Root
                    } else {
                        match client.run_json(&JottaCommand::LsRemote(path)) {
                            Ok(v) => LsNavResult::from(jotta_core::remote::parse_ls_json(&v)),
                            Err(e) => LsNavResult::Error(format!("{e}")),
                        }
                    };
                    let _ = data_tx.send(result);
                }
            });
        });
    }

    // Refresh button.
    {
        let current_path = restore_current_path.clone();
        let list = restore_list.clone();
        let sel = restore_selected.clone();
        restore_refresh_btn.connect_clicked(move |_| {
            let path = current_path.borrow().clone();

            let (data_tx, data_rx) = std::sync::mpsc::channel::<LsNavResult>();
            let data_rx = Arc::new(Mutex::new(data_rx));
            let list_r = list.clone();
            let sel_r = sel.clone();
            glib::idle_add_local(move || {
                let rx = data_rx.lock().unwrap();
                match rx.try_recv() {
                    Ok(result) => {
                        rebuild_remote_list_result(&list_r, &result, &sel_r);
                        glib::ControlFlow::Break
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
                }
            });

            let path = path.clone();
            std::thread::spawn(move || {
                if let Some(loc) = BinaryLocator::from_path() {
                    let client = CliClient::new(loc);
                    let result = if path.is_empty() {
                        LsNavResult::Root
                    } else {
                        match client.run_json(&JottaCommand::LsRemote(path)) {
                            Ok(v) => LsNavResult::from(jotta_core::remote::parse_ls_json(&v)),
                            Err(e) => LsNavResult::Error(format!("{e}")),
                        }
                    };
                    let _ = data_tx.send(result);
                }
            });
        });
    }

    // Download selected -> FileDialog for destination -> Download command.
    // BUG 4 fix: --merge er KUN gyldigt for mappe-kilder. Fil-kilde + --merge
    // giver "ERROR PathIsFile: not found". GUI sætter merge=false altid (bruger
    // kan ikke vælge --merge i denne iteration).
    // NY FAKTA 1: CLI spørger interaktivt ved eksisterende destinationsfil.
    // GUI-spawn har ingen TTY -> vi tjekker selv og viser egen bekræftelse.
    {
        let window = window.clone();
        let current_path = restore_current_path.clone();
        let sel = restore_selected.clone();
        let sel_is_dir = restore_selected_is_dir.clone();
        let toast_tx = toast_tx.clone();
        restore_download_btn.connect_clicked(move |_| {
            let selected = sel.borrow().clone();
            if selected.is_empty() {
                let _ = toast_tx.send(i18n::t("restore.select.first").to_string());
                return;
            }
            let remote_path = if current_path.borrow().is_empty() {
                selected.clone()
            } else {
                format!("{}/{}", current_path.borrow(), selected)
            };
            let is_dir = sel_is_dir.get();
            let dialog = gtk::FileDialog::new();
            dialog.set_title(i18n::t("restore.dest.dialog.title"));
            dialog.set_modal(true);
            dialog.set_accept_label(Some(i18n::t("restore.dest.accept")));
            let window_cb = window.clone();
            let window_alert = window.clone();
            let toast_tx = toast_tx.clone();
            dialog.select_folder(Some(&window_cb), None::<&gtk::gio::Cancellable>, move |res| {
                match res {
                    Ok(folder) => {
                        if let Some(dest) = folder.path() {
                            // NY FAKTA 1: Tjek om destinationsfilen findes.
                            // CLI'en ville spørge interaktivt — GUI har ingen TTY.
                            let dest_file = dest.join(&selected);
                            if !is_dir && dest_file.exists() {
                                // Vis egen bekræftelsesdialog før kommandoen.
                                let alert = adw::AlertDialog::new(
                                    Some(i18n::t("restore.replace.confirm.title")),
                                    Some(&format!(
                                        "Filen „{selected}“ findes allerede i {}.

Vil du erstatte den?",
                                        dest.display()
                                    )),
                                );
                                alert.add_response("cancel", i18n::t("common.cancel"));
                                alert.add_response("replace", i18n::t("restore.replace.action"));
                                alert.set_response_appearance(
                                    "replace",
                                    adw::ResponseAppearance::Destructive,
                                );
                                alert.set_default_response(Some("cancel"));
                                alert.set_close_response("cancel");
                                let window = window_alert.clone();
                                let remote_path = remote_path.clone();
                                let dest = dest.clone();
                                let toast_tx = toast_tx.clone();
                                alert.choose(&window, None::<&gtk::gio::Cancellable>, move |resp| {
                                    if resp == "replace" {
                                        start_download(remote_path, dest, toast_tx);
                                    }
                                    // cancel: gør ingenting
                                });
                            } else {
                                start_download(remote_path, dest, toast_tx);
                            }
                        }
                    }
                    Err(_) => {} // user cancelled
                }
            });
        });
    }

    // Downloads list: load on startup (worker sends plain data, main thread rebuilds).
    {
        let list = restore_downloads_list.clone();
        let (data_tx, data_rx) = std::sync::mpsc::channel::<Vec<jotta_core::remote::DownloadEntry>>();
        let data_rx = Arc::new(Mutex::new(data_rx));
        glib::idle_add_local(move || {
            let rx = data_rx.lock().unwrap();
            match rx.try_recv() {
                Ok(entries) => {
                    rebuild_downloads_list(&list, &entries);
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        });
        std::thread::spawn(move || {
            if let Some(loc) = BinaryLocator::from_path() {
                let client = CliClient::new(loc);
                if let Ok(v) = client.run_json(&JottaCommand::ListDownloads) {
                    let entries = jotta_core::remote::parse_downloads_json(&v);
                    let _ = data_tx.send(entries);
                }
            }
        });
    }

    // Trash: web-only button — opens jottacloud.com trash in browser.
    // `trash list` has no --json and plain output is unparseable (single
    // space-separated line with filenames containing spaces).
    restore_trash_web_btn.connect_clicked(move |_| {
        // gtk::UriLauncher (GTK 4.10+) — replacement for deprecated gtk::show_uri.
        let launcher = gtk::UriLauncher::new("https://www.jottacloud.com/web/trash");
        launcher.launch(None::<&gtk::Window>, None::<&gtk::gio::Cancellable>, |res| {
            if let Err(e) = res {
                eprintln!("jotta-gui: could not open trash URL: {e}");
            }
        });
    });

    // --- Transfers poller: refresh uploads/downloads/errors every 5s ---
    // Worker thread fetches plain data (Vec<UploadEntry>, Vec<DownloadEntry>)
    // via mpsc; main thread rebuilds the lists.
    // VERIFICeret (Fedora, 2026): `list downloaderrors`/`list uploaderrors`
    // KRÆVER --downloadid/--uploadid — der er ingen "list alle"-mode.
    // Fejlbeskeder udledes därfor fra hver entry's "Errors"-objekt via
    // jotta_core::remote::collect_transfer_errors().
    // --- Live overførsels-feed: én kanal, fire kilder (se LiveFeed ovenfor) ---
    // source 3 = sync log --watch, 2 = observe (json/TUI), 1 = status, 0 = list.
    let (live_tx, live_rx) = std::sync::mpsc::channel::<LiveFeed>();
    // Child-handles til evt. kørende streams — dræbes ved luk.
    let observe_child: Arc<Mutex<Option<std::process::Child>>> = Arc::new(Mutex::new(None));
    let synclog_child: Arc<Mutex<Option<std::process::Child>>> = Arc::new(Mutex::new(None));

    // Error dedup: only toast NEW errors (tracked in a HashSet on main thread).
    {
        let uploads_list = transfers_uploads_list.clone();
        let downloads_list = transfers_downloads_list.clone();
        let errors_list = transfers_errors_list.clone();
        let toast_tx = toast_tx.clone();
        // Channel carries (uploads, downloads).
        // Kanal bærer også live hastigheder (up_rate, down_rate i bytes/s),
        // udledt som delta af Total−Remaining mellem polls (iteration 14).
        let (tx, rx) = std::sync::mpsc::channel::<(
            Vec<jotta_core::remote::UploadEntry>,
            Vec<jotta_core::remote::DownloadEntry>,
            u64,
            u64,
        )>();
        let rx = Arc::new(Mutex::new(rx));
        // Seen errors for dedup (main thread only — Rc<RefCell<HashSet>>).
        let seen_errors: std::rc::Rc<std::cell::RefCell<std::collections::HashSet<String>>> =
            std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashSet::new()));
        {
            let uploads_list = uploads_list.clone();
            let downloads_list = downloads_list.clone();
            let errors_list = errors_list.clone();
            let toast_tx = toast_tx.clone();
            let seen_errors = seen_errors.clone();
            glib::idle_add_local(move || {
                let rx = rx.lock().unwrap();
                while let Ok((uploads, downloads, up_rate, down_rate)) = rx.try_recv() {
                    rebuild_uploads_list(&uploads_list, &uploads);
                    rebuild_transfers_downloads_list(&downloads_list, &downloads);
                    // Hastighed/aktuel fil håndteres nu af live-feed-forbrugeren
                    // (denne poller er kun kilde 0 i live-feedet).
                    let _ = (up_rate, down_rate);
                    // Derive errors from entries (no separate CLI call needed).
                    let errors = jotta_core::remote::collect_transfer_errors(&downloads, &uploads);
                    rebuild_transfer_errors_list(&errors_list, &errors);
                    // Error dedup: toast only NEW errors.
                    let mut seen = seen_errors.borrow_mut();
                    for err in &errors {
                        let key = format!("{}:{}", err.path.as_deref().unwrap_or(""), err.message);
                        if seen.insert(key) {
                            let path_str = err.path.as_deref().unwrap_or(i18n::t("transfer.unknown.path"));
                            let _ = toast_tx.send(tf!("transfer.error.toast", path_str, err.message));
                        }
                    }
                }
                glib::ControlFlow::Continue
            });
        }
        {
            let shutdown = shutdown.clone();
            let live_tx_t = live_tx.clone();
            std::thread::spawn(move || {
                // Sidste sample til hastighedsberegning (kun denne tråd rører den).
                let mut last: Option<(std::time::Instant, u64, u64)> = None;
                loop {
                    if shutdown.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Some(loc) = BinaryLocator::from_path() {
                        let client = CliClient::new(loc);
                        // VERIFICeret: `list uploads --json` returnerer `{}` ved ingen uploads.
                        let uploads = client
                            .run_json(&JottaCommand::ListUploads)
                            .map(|v| jotta_core::remote::parse_uploads_json(&v))
                            .unwrap_or_default();
                        let downloads = client
                            .run_json(&JottaCommand::ListDownloads)
                            .map(|v| jotta_core::remote::parse_downloads_json(&v))
                            .unwrap_or_default();
                        // Live hastighed = delta(overførte bytes) / delta(tid).
                        let up_done = jotta_core::remote::total_bytes_done_uploads(&uploads);
                        let down_done = jotta_core::remote::total_bytes_done_downloads(&downloads);
                        let now = std::time::Instant::now();
                        let (up_rate, down_rate) = match last {
                            Some((prev_t, prev_up, prev_down)) => {
                                let dt = now.duration_since(prev_t).as_secs_f64();
                                if dt > 0.0 {
                                    (
                                        (up_done.saturating_sub(prev_up) as f64 / dt) as u64,
                                        (down_done.saturating_sub(prev_down) as f64 / dt) as u64,
                                    )
                                } else {
                                    (0, 0)
                                }
                            }
                            None => (0, 0),
                        };
                        last = Some((now, up_done, down_done));
                        // Live-feed kilde 0 (list uploads/downloads) til hastighed + "I gang nu".
                        let feed = LiveFeed {
                            source: 0,
                            uploads: uploads
                                .iter()
                                .map(|e| LiveTransfer {
                                    path: if e.local_path.is_empty() {
                                        e.remote_path.clone()
                                    } else {
                                        e.local_path.clone()
                                    },
                                    bytes_done: e.bytes_done(),
                                    total: e.total_bytes,
                                })
                                .collect(),
                            downloads: downloads
                                .iter()
                                .map(|e| LiveTransfer {
                                    path: e.remote_path.clone(),
                                    bytes_done: e.bytes_done(),
                                    total: e.total_bytes,
                                })
                                .collect(),
                            up_rate,
                            down_rate,
                        };
                        let _ = live_tx_t.send(feed);
                        let _ = tx.send((uploads, downloads, up_rate, down_rate));
                    }
                    std::thread::sleep(Duration::from_secs(5));
                }
            });
        }
    }

    // --- Live-feed forbruger på main loop + observe/sync-log-workers ---
    {
        let live_rx = Arc::new(Mutex::new(live_rx));
        struct LiveUiState {
            latest: [Option<LiveFeed>; 4],
            seen: [Option<std::time::Instant>; 4],
        }
        let state = std::rc::Rc::new(std::cell::RefCell::new(LiveUiState {
            latest: [None, None, None, None],
            seen: [None, None, None, None],
        }));
        let speed_label = status_speed_label.clone();
        let current_row = status_current_row.clone();
        let current_label = status_current_label.clone();
        let live_group = live_group.clone();
        let live_list = live_list.clone();
        glib::idle_add_local(move || {
            let now = std::time::Instant::now();
            {
                let rx = live_rx.lock().unwrap();
                let mut st = state.borrow_mut();
                while let Ok(feed) = rx.try_recv() {
                    let s = (feed.source as usize).min(3);
                    st.latest[s] = Some(feed);
                    st.seen[s] = Some(now);
                }
            }
            // Friske kilder (< 6 s). Borrow slippes før GTK-kald.
            // Hastighed = max over alle friske kilder (kilde 3 sync-log har
            // filnavne men ingen bytes, så dens rate er 0).
            // Listen = højeste friske kilde DER HAR overførsler.
            let feed = {
                let st = state.borrow();
                let fresh: Vec<usize> = (0..=3usize)
                    .filter(|&s| {
                        st.seen[s]
                            .map(|t| now.duration_since(t).as_secs() < 6)
                            .unwrap_or(false)
                    })
                    .collect();
                if fresh.is_empty() {
                    None
                } else {
                    let (mut up_rate, mut down_rate) = (0u64, 0u64);
                    for &s in &fresh {
                        if let Some(f) = &st.latest[s] {
                            up_rate = up_rate.max(f.up_rate);
                            down_rate = down_rate.max(f.down_rate);
                        }
                    }
                    let has_transfers = |f: &LiveFeed| {
                        f.uploads.iter().any(|t| !t.path.is_empty())
                            || f.downloads.iter().any(|t| !t.path.is_empty())
                    };
                    fresh
                        .iter()
                        .rev()
                        .find(|&&s| st.latest[s].as_ref().map(|f| has_transfers(f)).unwrap_or(false))
                        .or_else(|| fresh.iter().rev().next())
                        .and_then(|&s| st.latest[s].clone())
                        .map(|mut f| {
                            f.up_rate = up_rate;
                            f.down_rate = down_rate;
                            f
                        })
                }
            };
            if let Some(feed) = feed {
                apply_live_feed(
                    &speed_label,
                    &current_row,
                    &current_label,
                    &live_group,
                    &live_list,
                    &feed,
                );
            }
            glib::ControlFlow::Continue
        });
    }
    {
        let shutdown = shutdown.clone();
        let live_tx = live_tx.clone();
        let observe_child = observe_child.clone();
        std::thread::spawn(move || run_observe_worker(shutdown, live_tx, observe_child));
    }
    {
        let shutdown = shutdown.clone();
        let live_tx = live_tx.clone();
        let synclog_child = synclog_child.clone();
        std::thread::spawn(move || run_sync_log_worker(shutdown, live_tx, synclog_child));
    }

    // Daemon banner: "Genindlæs" knap trigger et nyt status-poll.
    // Polleren kører hver 3s og opdaterer banner'en automatisk.
    {
        let toast_tx = toast_tx.clone();
        daemon_banner.connect_button_clicked(move |_| {
            let _ = toast_tx.send(i18n::t("status.reload.toast").to_string());
        });
    }

    // --- Poller: refresh status every 3s on a worker thread, update UI on main loop ---
    let poll_status = Arc::clone(&core.status);
    let poll_client = core.client();
    let ui = StatusWidgets {
        window: window.clone(),
        state_label: status_state_label.clone(),
        email_label: status_email_label.clone(),
        level: status_level.clone(),
        usage_label: status_usage_label.clone(),
        pause_switch: pause_switch.clone(),
        pause_updating: pause_updating.clone(),
        pause_lockout: pause_lockout.clone(),
        backup_list: backup_list.clone(),
        sync_switch: sync_switch.clone(),
        sync_updating: sync_updating.clone(),
        sync_path_row: sync_path_row.clone(),
        sync_count_row: sync_count_row.clone(),
        sync_mode_row: sync_mode_row.clone(),
        sync_trigger_btn: sync_trigger_btn.clone(),
        toast_tx: toast_tx.clone(),
        daemon_banner: daemon_banner.clone(),
        login_row: login_row.clone(),
        logout_row: logout_row.clone(),
    };

    // --- Info page: fetch Jottacloud version once on a worker thread ---
    // `jotta-cli version` output contains both jottad and jotta-cli version
    // (they share the same version number). We parse the version line.
    // App version is a compile-time constant (env! CARGO_PKG_VERSION).
    {
        let info_jotta_ver_row = info_jotta_ver_row.clone();
        let info_app_ver_row = info_app_ver_row.clone();
        let (ver_tx, ver_rx) = std::sync::mpsc::channel::<Option<String>>();
        let ver_rx = Arc::new(Mutex::new(ver_rx));
        glib::idle_add_local(move || {
            let rx = ver_rx.lock().unwrap();
            match rx.try_recv() {
                Ok(jotta_ver) => {
                    set_row_value(&info_jotta_ver_row, jotta_ver.as_deref().unwrap_or("—"));
                    set_row_value(&info_app_ver_row, env!("CARGO_PKG_VERSION"));
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        });
        std::thread::spawn(move || {
            if let Some(loc) = BinaryLocator::from_path() {
                let client = CliClient::new(loc);
                // `jotta-cli version` output:
                //   jottad version :0.17.176206
                //   jotta-cli version : 0.17.176206
                // Parse the version number from the jotta-cli line.
                let ver = client.run_text(&JottaCommand::Version).ok()
                    .and_then(|s| {
                        for line in s.lines() {
                            let lower = line.to_lowercase();
                            if lower.contains("jotta-cli version") {
                                // Extract version after the colon.
                                if let Some(idx) = line.find(':') {
                                    let v = line[idx + 1..].trim();
                                    if !v.is_empty() {
                                        return Some(v.to_string());
                                    }
                                }
                            }
                        }
                        None
                    });
                let _ = ver_tx.send(ver);
            }
        });
    }

    // Info page: update device/email/subscription from poller's shared status.
    {
        let info_device_row = info_device_row.clone();
        let info_email_row = info_email_row.clone();
        let info_sub_row = info_sub_row.clone();
        let status_shared = Arc::clone(&core.status);
        glib::timeout_add_local(Duration::from_secs(3), move || {
            let status = status_shared.lock().unwrap().clone();
            set_row_value(&info_device_row, status.device_name.as_deref().unwrap_or("—"));
            set_row_value(&info_email_row, status.account_email.as_deref().unwrap_or("—"));
            set_row_value(&info_sub_row, status.subscription.as_deref().unwrap_or("—"));
            glib::ControlFlow::Continue
        });
    }

    // Only Status (plain data) crosses the thread boundary via this channel.
    // The main loop applies it to the widgets; widgets never leave this thread.
    let (status_tx, status_rx) = std::sync::mpsc::channel::<Status>();
    {
        let shutdown = shutdown.clone();
        let live_tx_s = live_tx.clone();
        std::thread::spawn(move || {
            // Sidste sample til live-hastighed fra status (kun denne tråd rører den).
            let mut last_live: Option<(std::time::Instant, u64, u64)> = None;
            loop {
            // Stop poller når app lukker (sidste vindue væk).
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            let status = match poll_client.status() {
                Ok(v) => Status::from_json(&v),
                // PR #14-mapping også i tråd-polleren: "Not logged in" -> NeedsLogin
                Err(e) => Status::from_cli_error(&e),
            };
            // Live-feed kilde 1 (status): State.Uploading/Downloading i JSON giver
            // filsti + byte-tællere, MEN mens sync arbejder står der tomme `{}` i
            // JSON — så supplerer vi med TEKST-status `> Uploads: n (bytes) / n (bytes)`
            // linjer (verificeret i det vilde), der viser byte-fremskridt også for
            // sync-triggerede overførsler. Hastighed = delta mellem polls.
            {
                let text_prog = poll_client
                    .run_text(&JottaCommand::StatusText)
                    .map(|t| jotta_core::textstatus::parse_status_text_progress(&t))
                    .unwrap_or_default();
                let text_up = jotta_core::textstatus::done_bytes_for(&text_prog, "upload");
                let text_down = jotta_core::textstatus::done_bytes_for(&text_prog, "download");
                // Sync.WorkingProgress (verificeret mid-sync: CompletedUpload/
                // TotalUpload {Files,Bytes}) er den ENESTE byte-tæller i JSON
                // for sync-triggerede overførsler mens State.Uploading står {}.
                let wp_up = status.sync_upload_bytes_done().unwrap_or(0);
                let wp_down = status.sync_download_bytes_done().unwrap_or(0);
                let up_done = status
                    .active_upload_bytes_done()
                    .max(text_up)
                    .max(wp_up);
                let down_done = status
                    .active_download_bytes_done()
                    .max(text_down)
                    .max(wp_down);
                let now = std::time::Instant::now();
                let (up_rate, down_rate) = match last_live {
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
                last_live = Some((now, up_done, down_done));
                // Fil-liste: JSON-transfers hvis de findes, ellers aggregerede
                // tekst-status-rækker (label som "navn", f.eks. "Uploads").
                let mut up_list: Vec<LiveTransfer> = status
                    .active_uploads
                    .iter()
                    .map(|t| LiveTransfer {
                        path: t.path.clone().unwrap_or_default(),
                        bytes_done: t.bytes_done,
                        total: t.total_bytes,
                    })
                    .collect();
                if up_list.is_empty() {
                    up_list = text_prog
                        .iter()
                        .filter(|p| p.label_is("upload") && (p.total_bytes > 0 || p.done_bytes > 0))
                        .map(|p| LiveTransfer {
                            path: p.label.clone(),
                            bytes_done: Some(p.done_bytes),
                            total: Some(p.total_bytes),
                        })
                        .collect();
                }
                if up_list.is_empty() {
                    // Sidste fallback: Sync.WorkingProgress-aggregat (ingen filnavne).
                    if let (Some(d), Some(tot)) =
                        (status.sync_upload_bytes_done(), status.sync_upload_total_bytes())
                    {
                        if tot > 0 {
                            up_list.push(LiveTransfer {
                                path: "Uploads".to_string(),
                                bytes_done: Some(d),
                                total: Some(tot),
                            });
                        }
                    }
                }
                let mut down_list: Vec<LiveTransfer> = status
                    .active_downloads
                    .iter()
                    .map(|t| LiveTransfer {
                        path: t.path.clone().unwrap_or_default(),
                        bytes_done: t.bytes_done,
                        total: t.total_bytes,
                    })
                    .collect();
                if down_list.is_empty() {
                    down_list = text_prog
                        .iter()
                        .filter(|p| p.label_is("download") && (p.total_bytes > 0 || p.done_bytes > 0))
                        .map(|p| LiveTransfer {
                            path: p.label.clone(),
                            bytes_done: Some(p.done_bytes),
                            total: Some(p.total_bytes),
                        })
                        .collect();
                }
                if down_list.is_empty() {
                    if let (Some(d), Some(tot)) =
                        (status.sync_download_bytes_done(), status.sync_download_total_bytes())
                    {
                        if tot > 0 {
                            down_list.push(LiveTransfer {
                                path: "Downloads".to_string(),
                                bytes_done: Some(d),
                                total: Some(tot),
                            });
                        }
                    }
                }
                let feed = LiveFeed {
                    source: 1,
                    uploads: up_list,
                    downloads: down_list,
                    up_rate,
                    down_rate,
                };
                let _ = live_tx_s.send(feed);
            }
            // Adaptivt interval (ressourcer): 3s mens aktiv (sync/restore),
            // ellers 10s når idle — ét status-kald pr. interval.
            let active = matches!(status.state, State::Syncing)
                || status.restore_working
                || status.sync_working
                || status.sync_has_progress();
            {
                let mut guard = poll_status.lock().unwrap();
                *guard = status.clone();
            }
            let _ = status_tx.send(status);
            std::thread::sleep(Duration::from_secs(if active { 3 } else { 10 }));
            }
        });
    }
    {
        let status_rx = std::sync::Mutex::new(status_rx);
        let ui = ui.clone();
        glib::idle_add_local(move || {
            let rx = status_rx.lock().unwrap();
            while let Ok(status) = rx.try_recv() {
                ui.apply(&status);
            }
            glib::ControlFlow::Continue
        });
    }

    // Initial population.
    let initial = core.poll();
    {
        let mut guard = core.status.lock().unwrap();
        *guard = initial.clone();
    }
    ui.apply(&initial);

    // Skriv GUI PID-fil så tray kan lukke GUI ved "Afslut".
    let gui_pid_file = std::env::var("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
        .join("jotta-gui.pid");
    let _ = std::fs::write(&gui_pid_file, std::process::id().to_string());

    // Når vinduet lukkes: tjek minimize-to-tray indstilling.
    // Hvis AKTIV: skjul vinduet (minimer til tray) i stedet for at afslutte.
    // Hvis INAKTIV: afslut processen som normalt.
    {
        let shutdown = shutdown.clone();
        let gui_pid_file = gui_pid_file.clone();
        let window = window.clone();
        let observe_child = observe_child.clone();
        let synclog_child = synclog_child.clone();
        window.connect_close_request(move |window| {
            // Læs minimize-to-tray indstilling
            let config_path = std::env::var("XDG_CONFIG_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| {
                    std::env::var("HOME")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|_| std::path::PathBuf::from("."))
                        .join(".config")
                })
                .join("jotta-desktop")
                .join("settings.conf");
            let mtt = std::fs::read_to_string(&config_path)
                .ok()
                .and_then(|s| {
                    s.lines()
                        .find(|l| l.starts_with("minimize_to_tray="))
                        .map(|l| l.trim_end() == "minimize_to_tray=true")
                })
                .unwrap_or(true); // Standard: AKTIV

            if mtt {
                // Minimer til tray: skjul vinduet, behold processen kørende.
                window.set_visible(false);
                glib::Propagation::Stop
            } else {
                // Normal luk: afslut processen (dræb evt. observe-stream først).
                shutdown.store(true, Ordering::SeqCst);
                if let Some(mut c) = observe_child.lock().unwrap().take() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                if let Some(mut c) = synclog_child.lock().unwrap().take() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                let _ = std::fs::remove_file(&gui_pid_file);
                std::process::exit(0);
            }
        });
    }

    window.present();
}

struct StatusWidgets {
    window: adw::ApplicationWindow,
    state_label: gtk::Label,
    email_label: gtk::Label,
    level: gtk::LevelBar,
    usage_label: gtk::Label,
    pause_switch: gtk::Switch,
    /// Guard: true mens polleren opdaterer switchen programmatisk.
    /// connect_state_set-handleren returnerer tidligt når denne er true.
    pause_updating: std::rc::Rc<Cell<bool>>,
    /// Lockout: true i 5s efter bruger-klik; polleren må ikke opdatere pause-switch.
    pause_lockout: Arc<AtomicBool>,
    backup_list: gtk::ListBox,
    sync_switch: gtk::Switch,
    /// Guard: true mens polleren opdaterer switchen programmatisk.
    sync_updating: std::rc::Rc<Cell<bool>>,
    sync_path_row: adw::ActionRow,
    sync_count_row: adw::ActionRow,
    sync_mode_row: adw::ActionRow,
    sync_trigger_btn: gtk::Button,
    toast_tx: std::sync::mpsc::Sender<String>,
    /// Banner vises når daemon ikke svarer (State::Offline).
    daemon_banner: adw::Banner,
    /// Login-række vises kun ved State::NeedsLogin.
    login_row: adw::ActionRow,
    /// Log ud-række vises kun når logget ind.
    logout_row: adw::ActionRow,
}

impl Clone for StatusWidgets {
    fn clone(&self) -> Self {
        StatusWidgets {
            window: self.window.clone(),
            state_label: self.state_label.clone(),
            email_label: self.email_label.clone(),
            level: self.level.clone(),
            usage_label: self.usage_label.clone(),
            pause_switch: self.pause_switch.clone(),
            pause_updating: self.pause_updating.clone(),
            pause_lockout: self.pause_lockout.clone(),
            backup_list: self.backup_list.clone(),
            sync_switch: self.sync_switch.clone(),
            sync_updating: self.sync_updating.clone(),
            sync_path_row: self.sync_path_row.clone(),
            sync_count_row: self.sync_count_row.clone(),
            sync_mode_row: self.sync_mode_row.clone(),
            sync_trigger_btn: self.sync_trigger_btn.clone(),
            toast_tx: self.toast_tx.clone(),
            daemon_banner: self.daemon_banner.clone(),
            login_row: self.login_row.clone(),
            logout_row: self.logout_row.clone(),
        }
    }
}



impl StatusWidgets {
    fn apply(&self, status: &Status) {
        // State line
        let state_text = match status.state {
            State::Offline => i18n::t("status.state.offline"),
            State::NeedsLogin => i18n::t("status.state.needslogin"),
            State::Paused => i18n::t("status.state.paused"),
            State::Syncing => i18n::t("status.state.syncing"),
            State::Idle => i18n::t("status.state.idle"),
        };
        self.state_label.set_text(state_text);

        // Login-række: kun ved NeedsLogin (polleren styrer synligheden).
        self.login_row
            .set_visible(matches!(status.state, State::NeedsLogin));
        // Log ud-række: kun når logget ind (ikke NeedsLogin/Offline).
        self.logout_row.set_visible(matches!(
            status.state,
            State::Idle | State::Syncing | State::Paused
        ));

        // Daemon banner: vis når offline (jottad svarer ikke).
        // Én fejlmeddelelse — polleren viser ikke banner igen før næste offline.
        let banner_visible = matches!(status.state, State::Offline);
        if self.daemon_banner.is_revealed() != banner_visible {
            self.daemon_banner.set_revealed(banner_visible);
        }

        // Email
        match &status.account_email {
            Some(email) => self.email_label.set_text(email),
            None => self.email_label.set_text(""),
        }

        // Usage bar
        if let (Some(used), Some(quota)) = (status.bytes_used, status.bytes_quota) {
            self.level.set_max_value(quota as f64);
            self.level.set_value(used as f64);
            self.usage_label.set_text(&tf!("status.usage.of", i18n::format_bytes(used), i18n::format_bytes(quota)));
        } else {
            self.usage_label.set_text(i18n::t("status.usage.unavailable"));
        }

        // Pause switch — guard forhindrer feedback-loop:
        // programmatisk set_active trigger IKKE connect_state_set-handleren.
        // REGEL: kun brugerhandling udsteder kommandoer. Polleren skriver ALDRIG kommandoer.
        // Lockout: efter bruger-klik venter vi 15s før poller må opdatere switchen.
        // paused == None betyder "ukendt" — jottad kører, så vi sætter ON (= kører).
        // paused == Some(true) → OFF (= pauset). paused == Some(false) → ON (= kører).
        let pause_active = status.paused != Some(true);
        if self.pause_switch.is_active() != pause_active
            && !self.pause_lockout.load(Ordering::SeqCst)
        {
            self.pause_updating.set(true);
            self.pause_switch.set_active(pause_active);
            self.pause_updating.set(false);
        }

        // Backup list
        rebuild_backup_list(&self.backup_list, &status.backups, &self.toast_tx, &self.window);

        // Sync section — samme guard-mønster som pause switch.
        // Switchen = KONTINUERLIG tilstand (Sync.Automatic i JSON, svarer til
        // "Mode" i tekst-status): ON = `sync start`, OFF = manuel tilstand.
        let sync = status.sync.as_ref();
        let sync_continuous = sync.map(|s| s.automatic).unwrap_or(false);
        if self.sync_switch.is_active() != sync_continuous {
            self.sync_updating.set(true);
            self.sync_switch.set_active(sync_continuous);
            self.sync_updating.set(false);
        }
        match sync {
            Some(sync) => {
                self.sync_path_row
                    .set_subtitle(sync.root_path.as_deref().unwrap_or("—"));
                let count_text = match (sync.files, sync.bytes) {
                    (Some(f), Some(b)) => tf!("backup.files.count", f, i18n::format_bytes(b)),
                    (Some(f), None) => tf!("backup.files.only", f),
                    _ => "—".to_string(),
                };
                self.sync_count_row.set_subtitle(&count_text);
                self.sync_mode_row.set_subtitle(if sync.automatic {
                    i18n::t("sync.mode.continuous")
                } else {
                    i18n::t("sync.mode.manual")
                });
                // `sync trigger` virker kun når kontinuerlig sync er FRA.
                self.sync_trigger_btn
                    .set_sensitive(sync.enabled && !sync.automatic);
            }
            None => {
                self.sync_path_row.set_subtitle(i18n::t("sync.none"));
                self.sync_count_row.set_subtitle("—");
                self.sync_mode_row.set_subtitle("—");
                self.sync_trigger_btn.set_sensitive(false);
            }
        }
    }
}

fn run_cli(cmd: JottaCommand) {
    std::thread::spawn(move || {
        if let Some(loc) = BinaryLocator::from_path() {
            let client = CliClient::new(loc);
            if let Err(e) = client.run(&cmd) {
                eprintln!("jotta-gui: {e}");
            }
        }
    });
}

/// Start a download on a worker thread. merge er altid false (BUG 4:
/// --merge er kun gyldigt for mappe-kilder; fil + --merge fejler).
/// Fejlbeskeder inkluderer fuld argv (Error::CommandFailed viser argv).
fn start_download(
    remote_path: String,
    dest: std::path::PathBuf,
    toast_tx: std::sync::mpsc::Sender<String>,
) {
    std::thread::spawn(move || {
        if let Some(loc) = BinaryLocator::from_path() {
            let client = CliClient::new(loc);
            match client.run(&JottaCommand::Download {
                remote_path: remote_path.clone(),
                local_dest: dest,
                merge: false,
            }) {
                Ok(_) => {
                    let _ = toast_tx.send(tf!("download.started.toast", remote_path));
                }
                Err(e) => {
                    // Error::CommandFailed viser nu fuld argv: "`jotta-cli download ...` exited 1: ..."
                    let _ = toast_tx.send(tf!("download.start.error", e));
                }
            }
        }
    });
}

fn rebuild_backup_list(
    list: &gtk::ListBox,
    backups: &[jotta_core::status::BackupFolder],
    toast_tx: &std::sync::mpsc::Sender<String>,
    window: &adw::ApplicationWindow,
) {
    // Remove all existing rows.
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }
    if backups.is_empty() {
        let row = adw::ActionRow::builder()
            .title(i18n::t("backup.empty.title"))
            .subtitle(i18n::t("backup.empty.subtitle"))
            .build();
        list.append(&row);
        return;
    }
    for b in backups {
        let subtitle = match (b.files, b.bytes) {
            (Some(f), Some(bytes)) => tf!("backup.summary", b.path, f, i18n::format_bytes(bytes)),
            (Some(f), None) => tf!("backup.summary.files", b.path, f),
            _ => b.path.clone(),
        };
        let row = adw::ActionRow::builder()
            .title(&b.name)
            .subtitle(&subtitle)
            .build();

        // "Udeluk…" — chevron-ikon for tydelighed (åbner dialog).
        let exclude_btn = gtk::Button::from_icon_name("go-next-symbolic");
        exclude_btn.add_css_class("flat");
        exclude_btn.set_valign(gtk::Align::Center);
        exclude_btn.set_tooltip_text(Some(i18n::t("backup.exclude.tooltip")));
        let backup_name_ex = b.name.clone();
        let backup_path = b.path.clone();
        let toast_tx_ex = toast_tx.clone();
        let window_ex = window.clone();
        exclude_btn.connect_clicked(move |_| {
            open_backup_ignores_dialog(&window_ex, &backup_name_ex, &backup_path, &toast_tx_ex);
        });
        row.add_suffix(&exclude_btn);

        // "Fjern" — sletter backup-mappen (DESTRUKTIV — safety rule 3: altid bekræft).
        let remove_btn = gtk::Button::from_icon_name("user-trash-symbolic");
        remove_btn.add_css_class("flat");
        remove_btn.set_valign(gtk::Align::Center);
        remove_btn.set_tooltip_text(Some(i18n::t("backup.remove.tooltip")));
        let backup_name = b.name.clone();
        let toast_tx = toast_tx.clone();
        let window = window.clone();
        remove_btn.connect_clicked(move |_| {
            let dialog = adw::AlertDialog::new(
                Some(&tf!("backup.remove.confirm.title", backup_name)),
                Some(i18n::t("backup.remove.confirm.body")),
            );
            dialog.add_response("cancel", i18n::t("common.cancel"));
            dialog.add_response("remove", i18n::t("backup.remove.action"));
            dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
            let toast_tx = toast_tx.clone();
            let backup_name = backup_name.clone();
            dialog.choose(&window, None::<&gtk::gio::Cancellable>, move |res| {
                if res.as_str() == "remove" {
                    run_cli(JottaCommand::BackupRemove(backup_name.clone()));
                    let _ = toast_tx.send(tf!("backup.removed.toast", backup_name));
                }
            });
        });
        row.add_suffix(&remove_btn);

        list.append(&row);
    }
}


/// Wrap a page widget in a ScrolledWindow so it can scroll when content overflows.
/// gtk::Stack itself is not scrollable; each page must handle its own scrolling.
fn scrollable_page(page: &gtk::Widget) -> gtk::ScrolledWindow {
    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_child(Some(page));
    scrolled.set_vexpand(true);
    scrolled.set_hscrollbar_policy(gtk::PolicyType::Never);
    scrolled.set_vscrollbar_policy(gtk::PolicyType::Automatic);
    scrolled
}

#[allow(clippy::too_many_arguments)]
fn build_status_page(
    state_label: &gtk::Label,
    email_label: &gtk::Label,
    level: &gtk::LevelBar,
    usage_label: &gtk::Label,
    pause_switch: &gtk::Switch,
    daemon_banner: &adw::Banner,
    speed_label: &gtk::Label,
    current_row: &adw::ActionRow,
) -> gtk::Widget {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 12);
    set_margin_all(&box_, 24);

    // Daemon-not-responding banner (iteration 8): vises øverst når jottad
    // ikke svarer. "Genindlæs"-knap trigger et nyt status-poll.
    box_.append(daemon_banner);

    // Konto & status i et struktureret kort (PreferencesGroup).
    // Ingen H1-overskrift — topbaren viser fanens navn dynamisk.
    let account_group = adw::PreferencesGroup::builder()
        .title(i18n::t("status.account.group"))
        .build();
    let state_row = adw::ActionRow::builder()
        .title(i18n::t("status.account.state"))
        .build();
    state_row.add_suffix(state_label);
    state_label.set_halign(gtk::Align::End);
    let email_row = adw::ActionRow::builder()
        .title(i18n::t("status.account.email"))
        .build();
    email_row.add_suffix(email_label);
    email_label.set_halign(gtk::Align::End);
    email_label.add_css_class("dim-label");
    account_group.add(&state_row);
    account_group.add(&email_row);
    // Live hastighed (iteration 14): ↓ ned / ↑ op — "—" når idle.
    let speed_row = adw::ActionRow::builder()
        .title(i18n::t("status.speed.row"))
        .build();
    speed_row.add_suffix(speed_label);
    speed_label.set_halign(gtk::Align::End);
    account_group.add(&speed_row);
    // Aktuel overførsel (live) — række skjules når idle (live-feed sætter synlighed).
    account_group.add(current_row);
    box_.append(&account_group);

    // Pause/resume: positiv formulering — ON = automatisk aktiv.
    let pause_group = adw::PreferencesGroup::builder()
        .title(i18n::t("status.activity.group"))
        .build();
    let pause_row = adw::ActionRow::builder()
        .title(i18n::t("status.pause.title"))
        .subtitle(i18n::t("status.pause.subtitle"))
        .build();
    pause_row.add_suffix(pause_switch);
    pause_row.set_activatable_widget(Some(pause_switch));
    pause_group.add(&pause_row);
    box_.append(&pause_group);

    // Lagerplads i egen indrammet gruppe.
    let usage_group = adw::PreferencesGroup::builder()
        .title(i18n::t("status.storage.group"))
        .build();
    let usage_row = adw::ActionRow::builder()
        .title(i18n::t("status.usage.row"))
        .build();
    usage_row.add_suffix(usage_label);
    usage_label.set_halign(gtk::Align::End);
    usage_label.add_css_class("dim-label");
    usage_group.add(&usage_row);
    // LevelBar skal fylde bredden — pak i en række uden suffix.
    let level_row = adw::ActionRow::builder()
        .title("")
        .build();
    level.set_hexpand(true);
    level.set_margin_top(6);
    level.set_margin_bottom(6);
    level.set_margin_start(12);
    level.set_margin_end(12);
    level_row.add_suffix(level);
    usage_group.add(&level_row);
    box_.append(&usage_group);

    scrollable_page(&box_.upcast()).upcast()
}

fn build_backup_page(list: &gtk::ListBox, schedule_group: &adw::PreferencesGroup) -> gtk::Widget {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 12);
    set_margin_all(&box_, 24);

    // Ingen indre ScrolledWindow — listen ligger direkte i boksen.
    // ydre scrollable_page håndterer overflow. Undgår dobbelt scroll.
    box_.append(list);
    box_.append(schedule_group);

    scrollable_page(&box_.upcast()).upcast()
}

fn build_sync_page(group: &adw::PreferencesGroup) -> gtk::Widget {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 12);
    set_margin_all(&box_, 24);
    box_.append(group);
    scrollable_page(&box_.upcast()).upcast()
}

// ---------------------------------------------------------------------------
// Overførsler (transfers) page
// ---------------------------------------------------------------------------

/// Iteration 14 (SPEC §3): Overførsler & Gendan smeltet sammen til én fane.
/// Øverst: aktive uploads/downloads + fejl. Nederst: remote browser + downloads + papirkurv.
#[allow(clippy::too_many_arguments)]
fn build_transfers_restore_page(
    live_group: &adw::PreferencesGroup,
    uploads_group: &adw::PreferencesGroup,
    downloads_group: &adw::PreferencesGroup,
    errors_group: &adw::PreferencesGroup,
    path_box: &gtk::Box,
    remote_scrolled: &gtk::ScrolledWindow,
    download_group: &adw::PreferencesGroup,
    rest_downloads_group: &adw::PreferencesGroup,
    rest_downloads_list: &gtk::ListBox,
    trash_group: &adw::PreferencesGroup,
) -> gtk::Widget {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 12);
    set_margin_all(&box_, 24);

    // --- Overførsler-sektion ---
    let transfers_title = gtk::Label::new(Some(i18n::t("page.transfers.heading")));
    transfers_title.set_halign(gtk::Align::Start);
    transfers_title.add_css_class("heading");
    box_.append(&transfers_title);
    // "I gang nu" øverst — live overførsler med fremskridt (skjult når idle).
    box_.append(live_group);
    box_.append(uploads_group);
    box_.append(downloads_group);
    box_.append(errors_group);

    // --- Gendan-sektion (remote browser) ---
    let browser_title = gtk::Label::new(Some(i18n::t("restore.browser.title")));
    browser_title.set_halign(gtk::Align::Start);
    browser_title.add_css_class("heading");
    box_.append(&browser_title);
    box_.append(path_box);
    box_.append(remote_scrolled);
    box_.append(download_group);

    let downloads_title = gtk::Label::new(Some(i18n::t("nav.transfers")));
    downloads_title.set_halign(gtk::Align::Start);
    downloads_title.add_css_class("heading");
    rest_downloads_group.add(rest_downloads_list);
    box_.append(rest_downloads_group);

    let trash_title = gtk::Label::new(Some(i18n::t("restore.trash.title")));
    trash_title.set_halign(gtk::Align::Start);
    trash_title.add_css_class("heading");
    box_.append(&trash_title);
    box_.append(trash_group);

    scrollable_page(&box_.upcast()).upcast()
}

/// Rebuild the uploads list on the Overførsler page.
fn rebuild_uploads_list(
    list: &gtk::ListBox,
    entries: &[jotta_core::remote::UploadEntry],
) {
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }
    // Only show ongoing + failed uploads (completed are less interesting here).
    let active: Vec<_> = entries
        .iter()
        .filter(|e| e.status != jotta_core::remote::UploadStatus::Completed)
        .collect();
    if active.is_empty() {
        let row = adw::ActionRow::builder()
            .title(i18n::t("transfers.uploads.empty.title"))
            .subtitle(i18n::t("transfers.uploads.empty.subtitle"))
            .build();
        list.append(&row);
        return;
    }
    for entry in active {
        let status_text = match entry.status {
            jotta_core::remote::UploadStatus::Ongoing => i18n::t("transfer.status.uploading"),
            jotta_core::remote::UploadStatus::Failed => i18n::t("transfer.status.failed"),
            jotta_core::remote::UploadStatus::Completed => i18n::t("transfer.status.done"),
        };
        // Kun filnavn i titel, fulde stier i undertitel.
        let file_name = entry
            .local_path
            .rsplit('/')
            .next()
            .unwrap_or(&entry.local_path);
        let subtitle = format!("{} → {}", entry.local_path, entry.remote_path);
        let row = adw::ActionRow::builder()
            .title(&format!("{} ({})", file_name, status_text))
            .subtitle(&subtitle)
            .build();
        let img = gtk::Image::from_icon_name("go-up-symbolic");
        row.add_prefix(&img);
        list.append(&row);
    }
}

/// Rebuild the downloads list on the Overførsler page.
fn rebuild_transfers_downloads_list(
    list: &gtk::ListBox,
    entries: &[jotta_core::remote::DownloadEntry],
) {
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }
    let active: Vec<_> = entries
        .iter()
        .filter(|e| e.status != jotta_core::remote::DownloadStatus::Completed)
        .collect();
    if active.is_empty() {
        let row = adw::ActionRow::builder()
            .title(i18n::t("transfers.downloads.empty.title"))
            .subtitle(i18n::t("transfers.downloads.empty.subtitle"))
            .build();
        list.append(&row);
        return;
    }
    for entry in active {
        let status_text = match entry.status {
            jotta_core::remote::DownloadStatus::Ongoing => i18n::t("transfer.status.downloading"),
            jotta_core::remote::DownloadStatus::Failed => i18n::t("transfer.status.failed"),
            jotta_core::remote::DownloadStatus::Completed => i18n::t("transfer.status.done"),
        };
        let file_name = entry
            .remote_path
            .rsplit('/')
            .next()
            .unwrap_or(&entry.remote_path);
        let subtitle = format!("{} → {}", entry.remote_path, entry.local_dest);
        let row = adw::ActionRow::builder()
            .title(&format!("{} ({})", file_name, status_text))
            .subtitle(&subtitle)
            .build();
        let img = gtk::Image::from_icon_name("go-down-symbolic");
        row.add_prefix(&img);
        list.append(&row);
    }
}

/// Rebuild the transfer errors list on the Overførsler page.
/// Errors are derived from download/upload entries (collect_transfer_errors).
fn rebuild_transfer_errors_list(
    list: &gtk::ListBox,
    errors: &[jotta_core::remote::TransferError],
) {
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }
    if errors.is_empty() {
        let row = adw::ActionRow::builder()
            .title(i18n::t("transfers.errors.empty.title"))
            .subtitle(i18n::t("transfers.errors.empty.subtitle"))
            .build();
        list.append(&row);
        return;
    }
    for err in errors {
        let path_str = err.path.as_deref().unwrap_or(i18n::t("transfer.unknown.path"));
        let file_name = path_str.rsplit('/').next().unwrap_or(path_str);
        let row = adw::ActionRow::builder()
            .title(file_name)
            .subtitle(&format!("{path_str} — {}", err.message))
            .build();
        row.add_css_class("error");
        let img = gtk::Image::from_icon_name("dialog-warning-symbolic");
        row.add_prefix(&img);
        list.append(&row);
    }
}

fn replace_add_button(page: &gtk::Widget, new_btn: &gtk::Button) {
    if let Some(box_) = page.downcast_ref::<gtk::Box>() {
        box_.append(new_btn);
    }
}

fn nav_item(label: &str, icon_name: &str) -> gtk::Widget {
    let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row_box.set_margin_start(6);
    row_box.set_margin_end(6);
    row_box.set_margin_top(6);
    row_box.set_margin_bottom(6);
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_icon_size(gtk::IconSize::Normal);
    let lbl = gtk::Label::new(Some(label));
    lbl.set_halign(gtk::Align::Start);
    lbl.set_hexpand(true);
    row_box.append(&icon);
    row_box.append(&lbl);
    row_box.upcast()
}

/// Sæt en værdi som subtitle på en ActionRow (Info-page værdier).
/// Første kald tilføjer en label; efterfølgende kald opdaterer samme label
/// ved at sætte teksten direkte på row's subtitle (simpel og robust).
fn set_row_value(row: &adw::ActionRow, value: &str) {
    row.set_subtitle(value);
}

/// Sti til GUI'ens settings.conf (~/.config/jotta-desktop/settings.conf).
fn settings_conf_path() -> std::path::PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
                .join(".config")
        })
        .join("jotta-desktop")
        .join("settings.conf")
}

/// Læs en `key=true|false`-værdi fra settings.conf.
fn read_bool_setting(path: &std::path::Path, key: &str) -> Option<bool> {
    let want_true = format!("{key}=true");
    let want_false = format!("{key}=false");
    std::fs::read_to_string(path)
        .ok()?
        .lines()
        .map(str::trim_end)
        .find(|l| *l == want_true || *l == want_false)
        .map(|l| l == want_true)
}

/// Skriv en `key=true|false`-værdi — read-modify-write, bevarer andre nøgler.
fn write_bool_setting(path: &std::path::Path, key: &str, value: bool) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let prefix = format!("{key}=");
    let mut lines: Vec<String> = std::fs::read_to_string(path)
        .ok()
        .map(|s| {
            s.lines()
                .filter(|l| !l.trim_end().starts_with(&prefix))
                .map(|l| l.to_string())
                .collect()
        })
        .unwrap_or_default();
    lines.push(format!("{prefix}{value}"));
    let _ = std::fs::write(path, format!("{}\n", lines.join("\n")));
}

/// Er <pid> vores egen jotta-tray-proces? Sikkerhed før kill — aldrig en
/// fremmed proces, og aldrig jottad.
fn tray_pid_alive(pid: u32) -> bool {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => s,
        Err(_) => return false,
    };
    // state står efter sidste ')' i /proc/<pid>/stat; 'Z' = zombie = død for os.
    !matches!(
        stat.rsplit(')').next().and_then(|r| r.trim_start().chars().next()),
        Some('Z') | None
    )
}

fn tray_pid_matches(pid: u32) -> bool {
    tray_pid_alive(pid)
        && std::fs::read_to_string(format!("/proc/{pid}/cmdline"))
            .map(|c| c.contains("jotta-tray"))
            .unwrap_or(false)
}

/// Fjern pid-filen hvis den peger på en død/zombie/ikke-tray proces — ellers
/// tror næste jotta-tray-start at en gammel instans kører og exit'er.
fn clear_stale_tray_pid() {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        let pid_file = std::path::PathBuf::from(dir).join("jotta-tray.pid");
        if let Ok(s) = std::fs::read_to_string(&pid_file) {
            if let Ok(pid) = s.trim().parse::<u32>() {
                if !tray_pid_matches(pid) {
                    let _ = std::fs::remove_file(&pid_file);
                }
            }
        }
    }
}

/// Slå tray-ikon til/fra NU og ved fremtidige login (XDG user-override med
/// Hidden=true — overskriver /etc/xdg/autostart uden at røre den).
/// Returnerer besked til toast.
fn set_tray_visibility(visible: bool) -> String {
    let autostart_dir = std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
                .join(".config")
        })
        .join("autostart");
    let override_file = autostart_dir.join("jotta-tray.desktop");
    if visible {
        let _ = std::fs::remove_file(&override_file);
    } else if let Some(parent) = override_file.parent() {
        let _ = std::fs::create_dir_all(parent);
        let _ = std::fs::write(
            &override_file,
            "[Desktop Entry]\nType=Application\nHidden=true\n",
        );
    }
    let pid_file = std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .map(|d| std::path::PathBuf::from(d).join("jotta-tray.pid"));
    let running = pid_file
        .as_ref()
        .and_then(|pf| std::fs::read_to_string(pf).ok())
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|pid| tray_pid_matches(*pid));
    match (visible, running) {
        (true, Some(pid)) => tf!("tray.already.running", pid),
        (true, None) => respawn_tray(),
        (false, Some(pid)) => {
            let _ = std::process::Command::new("kill")
                .arg("-TERM")
                .arg(pid.to_string())
                .status();
            // Vent til /proc/<pid> er væk (maks ~2s), så pid-filen ikke bliver stale.
            for _ in 0..20 {
                if !tray_pid_alive(pid) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            clear_stale_tray_pid();
            i18n::t("tray.hidden.toast").to_string()
        }
        (false, None) => i18n::t("tray.already.hidden.toast").to_string(),
    }
}

/// Byg Indstillinger-siden: minimize-to-tray, start ved login, vis tray-ikon,
/// avancerede jottad-indstillinger (uploadrate/downloadrate/scaninterval).
#[allow(clippy::too_many_arguments)]
fn build_settings_page(
    toast_tx: std::sync::mpsc::Sender<String>,
    account_group: &adw::PreferencesGroup,
    about_group: &adw::PreferencesGroup,
    scan_row: &adw::EntryRow,
) -> gtk::Widget {
    let scan_row = scan_row.clone(); // owned — bruges af closures længere nede
    let group = adw::PreferencesGroup::builder()
        .title(i18n::t("settings.general.group"))
        .build();

    // Minimize-to-tray: når aktiv, minimeres vinduet til tray ved luk i stedet
    // for at afslutte. Gemmes i en konfigurationsfil.
    let mtt_row = adw::ActionRow::builder()
        .title(i18n::t("settings.mtt.title"))
        .subtitle(i18n::t("settings.mtt.subtitle"))
        .build();
    let mtt_switch = gtk::Switch::new();
    mtt_switch.set_valign(gtk::Align::Center);
    // Læs nuværende værdi fra konfigurationsfil
    let config_path = settings_conf_path();
    let mtt_enabled = read_bool_setting(&config_path, "minimize_to_tray").unwrap_or(true); // Standard: AKTIV
    mtt_switch.set_active(mtt_enabled);
    mtt_switch.connect_state_set(move |sw, state| {
        write_bool_setting(&settings_conf_path(), "minimize_to_tray", state);
        sw.set_active(state);
        Propagation::Proceed
    });
    mtt_row.add_suffix(&mtt_switch);
    mtt_row.set_activatable_widget(Some(&mtt_switch));
    group.add(&mtt_row);

    // Start ved login: skriver/fjerner XDG autostart .desktop for jotta-desktop.
    // (jotta-tray har allerede sin egen autostart — denne er for GUI'en.)
    let autostart_row = adw::ActionRow::builder()
        .title(i18n::t("settings.autostart.title"))
        .subtitle(i18n::t("settings.autostart.subtitle"))
        .build();
    let autostart_switch = gtk::Switch::new();
    autostart_switch.set_valign(gtk::Align::Center);
    let autostart_dir = std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
                .join(".config")
        })
        .join("autostart");
    let autostart_file = autostart_dir.join("jotta-desktop.desktop");
    autostart_switch.set_active(autostart_file.exists());
    autostart_switch.connect_state_set(move |sw, state| {
        let autostart_dir = std::env::var("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::var("HOME")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| std::path::PathBuf::from("."))
                    .join(".config")
            })
            .join("autostart");
        let autostart_file = autostart_dir.join("jotta-desktop.desktop");
        if state {
            if let Some(parent) = autostart_dir.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            // Find the current binary path for Exec=.
            let exe = std::env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "jotta-desktop".to_string());
            let content = format!(
                "[Desktop Entry]\n\
                 Type=Application\n\
                 Name=Jotta Desktop\n\
                 Exec={exe}\n\
                 Icon=jotta-desktop\n\
                 Terminal=false\n\
                 X-GNOME-Autostart-enabled=true\n"
            );
            let _ = std::fs::write(&autostart_file, content);
        } else {
            let _ = std::fs::remove_file(&autostart_file);
        }
        sw.set_active(state);
        Propagation::Proceed
    });
    autostart_row.add_suffix(&autostart_switch);
    autostart_row.set_activatable_widget(Some(&autostart_switch));
    group.add(&autostart_row);

    // Tray-ikon som TOGGLE (owner-requested 2026-11, iteration 12): ON = kører
    // (genstart hvis nede); OFF = skjules nu OG ved fremtidige login
    // (XDG user-override Hidden=true + tray_visible i settings.conf).
    // Kun vores egen proces dræbes (verificeret via /proc/<pid>/cmdline).
    let tray_row = adw::ActionRow::builder()
        .title(i18n::t("settings.tray.title"))
        .subtitle(i18n::t("settings.tray.subtitle"))
        .build();
    let tray_switch = gtk::Switch::new();
    tray_switch.set_valign(gtk::Align::Center);
    let tray_visible = read_bool_setting(&config_path, "tray_visible").unwrap_or(true);
    tray_switch.set_active(tray_visible);
    tray_row.add_suffix(&tray_switch);
    tray_row.set_activatable_widget(Some(&tray_switch));
    group.add(&tray_row);

    // Sort/hvidt tray-ikon (iteration 14, Nextcloud-stil): skriver tray_icon_mono
    // til samme settings.conf, som jotta-tray læser ved hvert poll.
    let mono_row = adw::ActionRow::builder()
        .title(i18n::t("settings.tray.mono.title"))
        .subtitle(i18n::t("settings.tray.mono.subtitle"))
        .build();
    let mono_switch = gtk::Switch::new();
    mono_switch.set_valign(gtk::Align::Center);
    mono_switch.set_active(read_bool_setting(&config_path, "tray_icon_mono").unwrap_or(false));
    mono_switch.connect_state_set(move |sw, state| {
        write_bool_setting(&settings_conf_path(), "tray_icon_mono", state);
        sw.set_active(state);
        Propagation::Proceed
    });
    mono_row.add_suffix(&mono_switch);
    mono_row.set_activatable_widget(Some(&mono_switch));
    group.add(&mono_row);

    // Ved opstart: hvis pref er FRA, sørg for at tray heller ikke kører nu.
    if !tray_visible {
        std::thread::spawn(|| {
            let _ = set_tray_visibility(false);
        });
    }
    {
        let toast_tx = toast_tx.clone();
        tray_switch.connect_state_set(move |sw, state| {
            write_bool_setting(&settings_conf_path(), "tray_visible", state);
            let toast_tx = toast_tx.clone();
            std::thread::spawn(move || {
                let msg = set_tray_visibility(state);
                let _ = toast_tx.send(msg);
            });
            sw.set_active(state);
            Propagation::Proceed
        });
    }

    // Avanceret (iteration 12): jottad config-værdier. Læses via `jotta-cli config`
    // (key : value-linjer) på worker-tråd; gemmes via `config set <key> <value>`.
    let adv_group = adw::PreferencesGroup::builder()
        .title(i18n::t("settings.advanced.group"))
        .description(i18n::t("settings.advanced.desc"))
        .build();
    let up_row = adw::EntryRow::builder().title(i18n::t("settings.bandwidth.up")).build();
    let down_row = adw::EntryRow::builder().title(i18n::t("settings.bandwidth.down")).build();
    adv_group.add(&up_row);
    adv_group.add(&down_row);
    // scan_row bor på Backup-fanen (Planlægning) — tilføjes ikke her.
    let save_row = adw::ActionRow::builder()
        .title(i18n::t("settings.save.advanced.title"))
        .build();
    let save_btn = gtk::Button::with_label(i18n::t("settings.save.button"));
    save_btn.add_css_class("suggested-action");
    save_btn.set_valign(gtk::Align::Center);
    save_row.add_suffix(&save_btn);
    save_row.set_activatable_widget(Some(&save_btn));
    adv_group.add(&save_row);

    // Hent nuværende værdier én gang (worker -> kanal -> main loop).
    {
        let (cfg_tx, cfg_rx) = std::sync::mpsc::channel::<Vec<(String, String)>>();
        let cfg_rx = Arc::new(Mutex::new(cfg_rx));
        std::thread::spawn(move || {
            let v = BinaryLocator::from_path()
                .and_then(|loc| CliClient::new(loc).run_text(&JottaCommand::ConfigList).ok())
                .map(|t| jotta_core::cli::parse_config_lines(&t))
                .unwrap_or_default();
            let _ = cfg_tx.send(v);
        });
        let up2 = up_row.clone();
        let down2 = down_row.clone();
        let scan2 = scan_row.clone();
        glib::idle_add_local(move || {
            let rx = cfg_rx.lock().unwrap();
            match rx.try_recv() {
                Ok(v) => {
                    let get = |k: &str| v.iter().find(|(key, _)| key == k).map(|(_, val)| val.clone());
                    if let Some(val) = get("uploadrate") {
                        up2.set_text(&val);
                    }
                    if let Some(val) = get("downloadrate") {
                        down2.set_text(&val);
                    }
                    if let Some(val) = get("scaninterval") {
                        scan2.set_text(&val);
                    }
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        });
    }

    // Gem: kør `config set` pr. ikke-tomt felt på worker-tråd. Ikke-destruktiv.
    {
        let toast_tx = toast_tx.clone();
        save_btn.connect_clicked(move |_| {
            let up = up_row.text().trim().to_string();
            let down = down_row.text().trim().to_string();
            let scan = scan_row.text().trim().to_string();
            let toast_tx = toast_tx.clone();
            std::thread::spawn(move || {
                if let Some(loc) = BinaryLocator::from_path() {
                    let client = CliClient::new(loc);
                    let mut ok = true;
                    for (k, v) in [("uploadrate", up), ("downloadrate", down), ("scaninterval", scan)] {
                        if !v.is_empty() {
                            if let Err(e) = client.run(&JottaCommand::ConfigSet { key: k.into(), value: v }) {
                                eprintln!("jotta-gui: config set {k}: {e}");
                                ok = false;
                            }
                        }
                    }
                    let _ = toast_tx.send(if ok {
                        i18n::t("settings.saved.toast").to_string()
                    } else {
                        i18n::t("settings.save.error.toast").to_string()
                    });
                }
            });
        });
    }

    let box_widget = gtk::Box::new(gtk::Orientation::Vertical, 12);
    set_margin_all(&box_widget, 12);
    box_widget.append(&group);
    box_widget.append(account_group);
    box_widget.append(&adv_group);
    box_widget.append(about_group);
    scrollable_page(&box_widget.upcast()).upcast()
}

/// Genstart jotta-tray: led efter binaren ved siden af eget exe, så i PATH.
/// Returnerer en besked til toast. Kun vores egen binær, ingen shell-strenge.
fn respawn_tray() -> String {
    // Ryd evt. stale pid-fil først — ellers tror den nye proces at en gammel kører.
    clear_stale_tray_pid();
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("jotta-tray"));
        }
    }
    if let Ok(paths) = std::env::var("PATH") {
        for p in std::env::split_paths(&paths) {
            candidates.push(p.join("jotta-tray"));
        }
    }
    match candidates.into_iter().find(|p| p.is_file()) {
        Some(bin) => match std::process::Command::new(&bin).spawn() {
            Ok(mut child) => {
                // Reaper: ellers bliver den nye tray til zombie når den exit'er,
                // og /proc/<pid> lever videre -> næste start fejler igen.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                tf!("tray.restarted.toast", bin.display())
            }
            Err(e) => tf!("tray.start.error", e),
        },
        None => i18n::t("tray.not.found.toast").to_string(),
    }
}

/// Parse `jotta-cli ignores list --json` output (backup ignores).
/// Schema is undocumented; tolerate several shapes:
/// - ["p1", "p2"] | {"patterns": [...]} | [{"pattern": "p1"}]
fn parse_ignores(v: &jotta_core::json::Value) -> Vec<String> {
    use jotta_core::json::Value;
    fn strs_from_array(arr: &[Value]) -> Vec<String> {
        arr.iter()
            .filter_map(|item| match item {
                Value::String(s) => Some(s.clone()),
                Value::Object(_) => item
                    .path(&["pattern"])
                    .or_else(|| item.path(&["Pattern"]))
                    .and_then(|x| x.as_str())
                    .map(str::to_owned),
                _ => None,
            })
            .collect()
    }
    match v {
        Value::Array(arr) => strs_from_array(arr),
        Value::Object(_) => v
            .path(&["patterns"])
            .or_else(|| v.path(&["Patterns"]))
            .and_then(|x| x.as_array())
            .map(strs_from_array)
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Parse `jotta-cli sync selective list --json` output.
/// Returns the list of selectively-ignored top-level folder names.
/// Schema is undocumented; tolerate array-of-strings and object shapes.
fn parse_sync_selective(v: &jotta_core::json::Value) -> Vec<String> {
    parse_ignores(v)
}

/// List top-level directories in the sync root (for the checkbox dialog).
/// Returns folder names only (not files), sorted.
fn list_sync_root_folders(root: &str) -> Vec<String> {
    let mut folders = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    folders.push(name.to_string());
                }
            }
        }
    }
    folders.sort();
    folders
}

/// Open the sync-selective checkbox dialog.
///
/// NOT destructive (unlike backup ignores): ignored folders are not deleted
/// locally or in the cloud. Matches the official Jottacloud app's
/// "Choose folders to synchronize" dialog from the Sync tab.
///
/// Shows all top-level folders in the sync root with checkboxes.
/// Checked = will be synced. Unchecked = selectively ignored.
/// On "Anvend": computes the diff and issues SyncSelectiveAdd/Remove.
fn open_sync_selective_dialog(
    parent: &adw::ApplicationWindow,
    toast_tx: &std::sync::mpsc::Sender<String>,
) {
    // Channel for dialog data: (sync_root, all_folders, currently_ignored).
    // Only plain data crosses the thread boundary; the parent window and all
    // widgets stay on the main thread via idle_add_local (no Send required).
    let (data_tx, data_rx) = std::sync::mpsc::channel::<(String, Vec<String>, Vec<String>)>();
    let data_rx = Arc::new(Mutex::new(data_rx));
    let parent = parent.clone();
    let toast_tx_dlg = toast_tx.clone();
    glib::idle_add_local(move || {
        let rx = data_rx.lock().unwrap();
        match rx.try_recv() {
            Ok((sync_root, all_folders, ignored)) => {
                build_sync_selective_dialog(&parent, &sync_root, &all_folders, &ignored, &toast_tx_dlg);
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });

    let toast_tx = toast_tx.clone();
    std::thread::spawn(move || {
        let client = match BinaryLocator::from_path() {
            Some(loc) => CliClient::new(loc),
            None => return,
        };
        let sync_root = match client.status() {
            Ok(v) => jotta_core::status::Status::from_json(&v)
                .sync
                .and_then(|s| s.root_path),
            Err(_) => None,
        };
        let sync_root = match sync_root {
            Some(r) => r,
            None => {
                let _ = toast_tx.send(i18n::t("sync.selective.nofolder.toast").to_string());
                return;
            }
        };
        let ignored = match client.run_json(&JottaCommand::SyncSelectiveList) {
            Ok(v) => parse_sync_selective(&v),
            Err(_) => Vec::new(),
        };
        let all_folders = list_sync_root_folders(&sync_root);
        let _ = data_tx.send((sync_root, all_folders, ignored));
    });
}

#[allow(clippy::too_many_arguments)]
fn build_sync_selective_dialog(
    parent: &adw::ApplicationWindow,
    sync_root: &str,
    all_folders: &[String],
    currently_ignored: &[String],
    toast_tx: &std::sync::mpsc::Sender<String>,
) {
    let dialog = adw::Dialog::new();
    dialog.set_title(i18n::t("sync.selective.dialog.title"));
    dialog.set_content_width(420);
    dialog.set_content_height(480);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    set_margin_all(&content, 18);

    let intro = gtk::Label::new(Some(&tf!("sync.selective.dialog.body", sync_root)));
    intro.set_wrap(true);
    intro.set_xalign(0.0);
    intro.add_css_class("dim-label");
    content.append(&intro);

    let list_box = gtk::ListBox::new();
    list_box.add_css_class("boxed-list");
    let mut checkboxes: Vec<(String, gtk::CheckButton)> = Vec::new();
    if all_folders.is_empty() {
        let row = adw::ActionRow::builder()
            .title(i18n::t("sync.selective.empty.title"))
            .subtitle(i18n::t("sync.selective.empty.subtitle"))
            .build();
        list_box.append(&row);
    }
    for folder in all_folders {
        let is_ignored = currently_ignored.iter().any(|i| i == folder);
        let cb = gtk::CheckButton::with_label(folder);
        cb.set_active(!is_ignored);
        let row = adw::ActionRow::builder().title(folder).build();
        row.add_suffix(&cb);
        row.set_activatable_widget(Some(&cb));
        list_box.append(&row);
        checkboxes.push((folder.clone(), cb));
    }
    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_child(Some(&list_box));
    scrolled.set_vexpand(true);
    scrolled.set_min_content_height(200);
    content.append(&scrolled);

    // ToolbarView with header bar: Cancel (start) + Apply (end) + window controls.
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    // FEATURE: Dialog skal kunne lukkes og flyttes — vis window controls.
    header.set_show_start_title_buttons(true);
    header.set_show_end_title_buttons(true);

    let dialog_cancel = dialog.clone();
    let cancel_btn = gtk::Button::with_label(i18n::t("common.cancel"));
    cancel_btn.connect_clicked(move |_| {
        dialog_cancel.close();
    });
    header.pack_start(&cancel_btn);

    let dialog_apply = dialog.clone();
    let toast_tx = toast_tx.clone();
    let currently_ignored = currently_ignored.to_vec();
    let apply_btn = gtk::Button::with_label(i18n::t("sync.selective.apply"));
    apply_btn.add_css_class("suggested-action");
    apply_btn.connect_clicked(move |_| {
        let mut to_add: Vec<String> = Vec::new();
        let mut to_remove: Vec<String> = Vec::new();
        for (folder, cb) in &checkboxes {
            let was_ignored = currently_ignored.iter().any(|i| i == folder);
            let is_checked = cb.is_active();
            if was_ignored && is_checked {
                to_remove.push(folder.clone());
            } else if !was_ignored && !is_checked {
                to_add.push(folder.clone());
            }
        }
        let toast_tx = toast_tx.clone();
        std::thread::spawn(move || {
            if let Some(loc) = BinaryLocator::from_path() {
                let client = CliClient::new(loc);
                let mut errors = Vec::new();
                // FIX: sync selective kræver at sync kører. Hvis add/rem fejler
                // med "syncapp not...", prøv at starte sync først og retry én gang.
                let mut needs_sync_start = false;
                for name in &to_add {
                    match client.run(&JottaCommand::SyncSelectiveAdd(name.clone())) {
                        Ok(_) => {}
                        Err(e) => {
                            let msg = format!("{e}");
                            if msg.contains("syncapp") || msg.contains("not running") || msg.contains("not enabled") {
                                needs_sync_start = true;
                            }
                            errors.push(format!("{name}: {e}"));
                        }
                    }
                }
                for name in &to_remove {
                    match client.run(&JottaCommand::SyncSelectiveRemove(name.clone())) {
                        Ok(_) => {}
                        Err(e) => {
                            let msg = format!("{e}");
                            if msg.contains("syncapp") || msg.contains("not running") || msg.contains("not enabled") {
                                needs_sync_start = true;
                            }
                            errors.push(format!("{name}: {e}"));
                        }
                    }
                }
                // Retry én gang efter sync start hvis nødvendigt.
                if needs_sync_start && !errors.is_empty() {
                    let _ = client.run(&JottaCommand::SyncStart);
                    errors.clear();
                    for name in &to_add {
                        if let Err(e) = client.run(&JottaCommand::SyncSelectiveAdd(name.clone())) {
                            errors.push(format!("{name}: {e}"));
                        }
                    }
                    for name in &to_remove {
                        if let Err(e) = client.run(&JottaCommand::SyncSelectiveRemove(name.clone())) {
                            errors.push(format!("{name}: {e}"));
                        }
                    }
                }
                let msg = if errors.is_empty() {
                    let n = to_add.len() + to_remove.len();
                    if n == 0 {
                        i18n::t("sync.selective.unchanged").to_string()
                    } else {
                        tf!("sync.selective.updated", n)
                    }
                } else {
                    tf!("sync.selective.error", errors.join(", "))
                };
                let _ = toast_tx.send(msg);
            }
        });
        dialog_apply.close();
    });
    header.pack_end(&apply_btn);

    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content));
    dialog.set_child(Some(&toolbar));

    dialog.present(Some(parent));
}

/// Open the backup-ignores dialog for ONE specific backup folder.
///
/// DESTRUCTIVE: adding a pattern removes already-backed-up matching files
/// (they go to Trash). Always shows a confirmation before adding.
///
/// Scope selector: "denne backup" (default, --backup NAME) / "alle backups"
/// (no flag). "Alle backups" is NEVER the silent default.
fn open_backup_ignores_dialog(
    window: &adw::ApplicationWindow,
    backup_name: &str,
    backup_path: &str,
    toast_tx: &std::sync::mpsc::Sender<String>,
) {
    let dialog = adw::Dialog::new();
    dialog.set_title(&tf!("backup.ignore.dialog.title", backup_name));
    dialog.set_content_width(480);
    dialog.set_content_height(520);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    set_margin_all(&content, 18);

    let scope_model = gtk::StringList::new(&[i18n::t("backup.ignore.scope.this"), i18n::t("backup.ignore.scope.all")]);
    let scope_row = adw::ComboRow::builder()
        .title(i18n::t("backup.ignore.scope.label"))
        .subtitle(i18n::t("backup.ignore.scope.subtitle"))
        .model(&scope_model)
        .selected(0)
        .build();
    let scope_group = adw::PreferencesGroup::new();
    scope_group.add(&scope_row);
    content.append(&scope_group);

    let warning = gtk::Label::new(Some(
        i18n::t("backup.ignore.warning"),
    ));
    warning.add_css_class("warning");
    warning.add_css_class("dim-label");
    warning.set_wrap(true);
    warning.set_xalign(0.0);
    content.append(&warning);

    let pattern_entry = adw::EntryRow::builder()
        .title(i18n::t("backup.ignore.new.title"))
        .show_apply_button(true)
        .build();
    let entry_group = adw::PreferencesGroup::builder()
        .title(i18n::t("backup.ignore.patterns.title"))
        .description(i18n::t("backup.ignore.patterns.desc"))
        .build();
    entry_group.add(&pattern_entry);
    content.append(&entry_group);

    let path_label = gtk::Label::new(Some(&tf!("backup.ignore.root", backup_path)));
    path_label.add_css_class("dim-label");
    path_label.set_xalign(0.0);
    path_label.set_wrap(true);
    content.append(&path_label);

    let patterns_list = gtk::ListBox::new();
    patterns_list.add_css_class("boxed-list");
    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_child(Some(&patterns_list));
    scrolled.set_min_content_height(120);
    scrolled.set_vexpand(true);
    content.append(&scrolled);

    // Initial load of patterns: worker thread fetches, main thread rebuilds.
    // Only plain data (Vec<String>) crosses the thread boundary.
    {
        let (data_tx, data_rx) = std::sync::mpsc::channel::<Vec<String>>();
        let data_rx = Arc::new(Mutex::new(data_rx));
        let patterns_list = patterns_list.clone();
        let backup_name_idle = backup_name.to_string();
        let toast_tx = toast_tx.clone();
        glib::idle_add_local(move || {
            let rx = data_rx.lock().unwrap();
            match rx.try_recv() {
                Ok(patterns) => {
                    rebuild_backup_ignores_list(&patterns_list, &patterns, &backup_name_idle, &toast_tx);
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        });
        let backup_name = backup_name.to_string();
        std::thread::spawn(move || {
            if let Some(loc) = BinaryLocator::from_path() {
                let client = CliClient::new(loc);
                if let Ok(v) = client.run_json(&JottaCommand::IgnoresList { backup: Some(backup_name) }) {
                    let patterns = parse_ignores(&v);
                    let _ = data_tx.send(patterns);
                }
            }
        });
    }

    let toast_tx2 = toast_tx.clone();
    let backup_name2 = backup_name.to_string();
    let scope_row2 = scope_row.clone();
    let patterns_list2 = patterns_list.clone();
    let dialog_parent = dialog.clone();
    pattern_entry.connect_apply(move |entry| {
        let pattern = entry.text().to_string();
        if pattern.trim().is_empty() {
            return;
        }
        let pattern = pattern.trim().to_string();
        let backup = if scope_row2.selected() == 0 {
            Some(backup_name2.clone())
        } else {
            None
        };
        let confirm = adw::AlertDialog::new(
            Some(i18n::t("backup.ignore.add.confirm.title")),
            Some(&tf!(
                "backup.ignore.add.confirm.body",
                pattern,
                if backup.is_some() {
                    tf!("backup.ignore.suffix.backup", backup_name2)
                } else {
                    i18n::t("backup.ignore.suffix.all").to_string()
                }
            )),
        );
        confirm.add_response("cancel", i18n::t("common.cancel"));
        confirm.add_response("add", i18n::t("backup.ignore.add.action"));
        confirm.set_response_appearance("add", adw::ResponseAppearance::Destructive);
        confirm.set_default_response(Some("cancel"));
        confirm.set_close_response("cancel");
        let toast_tx = toast_tx2.clone();
        let patterns_list = patterns_list2.clone();
        let backup_name = backup_name2.clone();
        let pattern_clone = pattern.clone();
        let backup_clone = backup.clone();
        confirm.choose(&dialog_parent, None::<&gtk::gio::Cancellable>, move |resp| {
            if resp == "add" {
                // Refresh channel: worker sends Vec<String>, main thread rebuilds.
                // The patterns_list widget never crosses the thread boundary.
                let (refresh_tx, refresh_rx) = std::sync::mpsc::channel::<Vec<String>>();
                let refresh_rx = Arc::new(Mutex::new(refresh_rx));
                let patterns_list = patterns_list.clone();
                let backup_name_r = backup_name.clone();
                let toast_tx_r = toast_tx.clone();
                glib::idle_add_local(move || {
                    let rx = refresh_rx.lock().unwrap();
                    match rx.try_recv() {
                        Ok(patterns) => {
                            rebuild_backup_ignores_list(&patterns_list, &patterns, &backup_name_r, &toast_tx_r);
                            glib::ControlFlow::Break
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
                    }
                });

                let toast_tx = toast_tx.clone();
                let backup_name = backup_name.clone();
                let pattern = pattern_clone.clone();
                let backup = backup_clone.clone();
                std::thread::spawn(move || {
                    if let Some(loc) = BinaryLocator::from_path() {
                        let client = CliClient::new(loc);
                        match client.run(&JottaCommand::IgnoreAdd { pattern: pattern.clone(), backup: backup.clone() }) {
                            Ok(_) => {
                                let _ = toast_tx.send(tf!("backup.ignore.added.toast", pattern));
                                // FIX: Refresh skal følge samme scope som add.
                                // "Denne backup" → Some(name), "Alle backups" → None.
                                let refresh_scope = if backup.is_some() {
                                    Some(backup_name.clone())
                                } else {
                                    None
                                };
                                if let Ok(v) = client.run_json(&JottaCommand::IgnoresList { backup: refresh_scope }) {
                                    let patterns = parse_ignores(&v);
                                    let _ = refresh_tx.send(patterns);
                                }
                            }
                            Err(e) => {
                                let _ = toast_tx.send(tf!("backup.ignore.add.error", e));
                            }
                        }
                    }
                });
            }
        });
        entry.set_text("");
    });

    // ToolbarView with header bar: Luk button + window controls.
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    // FEATURE: Dialog skal kunne lukkes og flyttes — vis window controls.
    header.set_show_start_title_buttons(true);
    header.set_show_end_title_buttons(true);
    let dialog_close = dialog.clone();
    let close_btn = gtk::Button::with_label(i18n::t("backup.ignore.close"));
    close_btn.connect_clicked(move |_| {
        dialog_close.close();
    });
    header.pack_end(&close_btn);
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content));
    dialog.set_child(Some(&toolbar));

    dialog.present(Some(window));
}

fn rebuild_backup_ignores_list(
    list: &gtk::ListBox,
    patterns: &[String],
    backup_name: &str,
    toast_tx: &std::sync::mpsc::Sender<String>,
) {
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }
    if patterns.is_empty() {
        let row = adw::ActionRow::builder()
            .title(i18n::t("backup.ignore.empty.title"))
            .subtitle(i18n::t("backup.ignore.empty.subtitle"))
            .build();
        list.append(&row);
        return;
    }
    for pattern in patterns {
        let row = adw::ActionRow::builder().title(pattern).build();
        let remove_btn = gtk::Button::from_icon_name("user-trash-symbolic");
        remove_btn.add_css_class("flat");
        remove_btn.set_valign(gtk::Align::Center);
        remove_btn.set_tooltip_text(Some(i18n::t("backup.ignore.remove.tooltip")));
        let pattern = pattern.to_string();
        let backup_name = backup_name.to_string();
        let toast_tx_remove = toast_tx.clone();
        let list_remove = list.clone();
        remove_btn.connect_clicked(move |_| {
            let confirm = adw::AlertDialog::new(
                Some(i18n::t("backup.ignore.remove.confirm.title")),
                Some(&tf!("backup.ignore.remove.confirm.body", pattern, backup_name)),
            );
            confirm.add_response("cancel", i18n::t("common.cancel"));
            confirm.add_response("remove", i18n::t("backup.ignore.remove.action"));
            confirm.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
            confirm.set_default_response(Some("cancel"));
            confirm.set_close_response("cancel");
            let pattern = pattern.clone();
            let backup_name = backup_name.clone();
            let toast_tx = toast_tx_remove.clone();
            let list = list_remove.clone();
            let root = list.root();
            confirm.choose(&root.unwrap(), None::<&gtk::gio::Cancellable>, move |resp| {
                if resp == "remove" {
                    // Refresh channel: worker sends Vec<String>, main thread rebuilds.
                    // The list widget never crosses the thread boundary.
                    let (refresh_tx, refresh_rx) = std::sync::mpsc::channel::<Vec<String>>();
                    let refresh_rx = Arc::new(Mutex::new(refresh_rx));
                    let list_r = list.clone();
                    let backup_name_r = backup_name.clone();
                    let toast_tx_r = toast_tx.clone();
                    glib::idle_add_local(move || {
                        let rx = refresh_rx.lock().unwrap();
                        match rx.try_recv() {
                            Ok(patterns) => {
                                rebuild_backup_ignores_list(&list_r, &patterns, &backup_name_r, &toast_tx_r);
                                glib::ControlFlow::Break
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
                        }
                    });

                    let pattern = pattern.clone();
                    let backup_name = backup_name.clone();
                    let toast_tx = toast_tx.clone();
                    std::thread::spawn(move || {
                        if let Some(loc) = BinaryLocator::from_path() {
                            let client = CliClient::new(loc);
                            let _ = client.run(&JottaCommand::IgnoreRemove {
                                pattern: pattern.clone(),
                                backup: Some(backup_name.clone()),
                            });
                            if let Ok(v) = client.run_json(&JottaCommand::IgnoresList {
                                backup: Some(backup_name.clone()),
                            }) {
                                let patterns = parse_ignores(&v);
                                let _ = refresh_tx.send(patterns);
                            }
                            let _ = toast_tx.send(i18n::t("backup.ignore.removed.toast").to_string());
                        }
                    });
                }
            });
        });
        row.add_suffix(&remove_btn);
        list.append(&row);
    }
}

fn set_margin_all(widget: &impl IsA<gtk::Widget>, margin: i32) {
    widget.set_margin_top(margin);
    widget.set_margin_bottom(margin);
    widget.set_margin_start(margin);
    widget.set_margin_end(margin);
}

// ---------------------------------------------------------------------------
// Gendan (restore/download) page
// ---------------------------------------------------------------------------

/// Navigation result from a worker thread: plain data only.
/// Carries the parsed `ls --json` result, the static root entries,
/// or an error string (e.g. ghost device "Cannot find device").
enum LsNavResult {
    Folders(Vec<jotta_core::remote::RemoteEntry>),
    Revisions(Vec<jotta_core::remote::FileRevision>),
    Root,
    Empty,
    Error(String),
}

impl From<jotta_core::remote::LsResult> for LsNavResult {
    fn from(r: jotta_core::remote::LsResult) -> Self {
        match r {
            jotta_core::remote::LsResult::Folders(f) => LsNavResult::Folders(f),
            jotta_core::remote::LsResult::Revisions(r) => LsNavResult::Revisions(r),
            jotta_core::remote::LsResult::Empty => LsNavResult::Empty,
        }
    }
}

/// Rebuild the remote file list from a navigation result.
/// Handles Folders, Revisions, Root (static Backup/Archive/Sync), Empty, Error.
fn rebuild_remote_list_result(
    list: &gtk::ListBox,
    result: &LsNavResult,
    selected: &std::rc::Rc<std::cell::RefCell<String>>,
) {
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }
    selected.borrow_mut().clear();

    match result {
        LsNavResult::Root => {
            let entries = [
                ("Backup", i18n::t("restore.devices")),
                ("Archive", i18n::t("restore.archived")),
                ("Sync", i18n::t("restore.synced")),
            ];
            for (name, desc) in entries {
                let row = adw::ActionRow::builder()
                    .title(name)
                    .subtitle(&tf!("restore.folder.desc", desc))
                    .build();
                let img = gtk::Image::from_icon_name("folder-symbolic");
                row.add_prefix(&img);
                row.set_activatable(true);
                list.append(&row);
            }
        }
        LsNavResult::Folders(entries) => {
            if entries.is_empty() {
                let row = adw::ActionRow::builder()
                    .title(i18n::t("restore.empty.title"))
                    .subtitle(i18n::t("restore.empty.subtitle"))
                    .build();
                list.append(&row);
                return;
            }
            for entry in entries {
                let subtitle = if entry.is_dir {
                    i18n::t("restore.folder.open.tooltip").to_string()
                } else {
                    // Fil: vis størrelse hvis tilgængelig.
                    match entry.size {
                        Some(s) => tf!("restore.file.size", i18n::format_bytes(s)),
                        None => i18n::t("restore.file.plain").to_string(),
                    }
                };
                let icon = if entry.is_dir {
                    "folder-symbolic"
                } else {
                    "text-x-generic-symbolic"
                };
                let row = adw::ActionRow::builder()
                    .title(&entry.name)
                    .subtitle(&subtitle)
                    .build();
                let img = gtk::Image::from_icon_name(icon);
                row.add_prefix(&img);
                row.set_activatable(true);
                list.append(&row);
            }
        }
        LsNavResult::Revisions(revs) => {
            // File revisions — placeholder for future "gendan gammel version".
            if revs.is_empty() {
                let row = adw::ActionRow::builder()
                    .title(i18n::t("restore.no.revisions.title"))
                    .subtitle(i18n::t("restore.no.revisions.subtitle"))
                    .build();
                list.append(&row);
                return;
            }
            for rev in revs {
                let subtitle = match rev.size {
                    Some(s) => tf!("restore.revision.size", rev.id, i18n::format_bytes(s)),
                    None => tf!("restore.revision", rev.id),
                };
                let row = adw::ActionRow::builder()
                    .title(&tf!("restore.version", rev.id))
                    .subtitle(&subtitle)
                    .build();
                let img = gtk::Image::from_icon_name("text-x-generic-symbolic");
                row.add_prefix(&img);
                list.append(&row);
            }
        }
        LsNavResult::Empty => {
            let row = adw::ActionRow::builder()
                .title(i18n::t("restore.empty.title"))
                .subtitle(i18n::t("restore.empty.list.subtitle"))
                .build();
            list.append(&row);
        }
        LsNavResult::Error(msg) => {
            // Graceful error — e.g. ghost device "Cannot find device".
            let row = adw::ActionRow::builder()
                .title(i18n::t("restore.load.error"))
                .subtitle(msg)
                .build();
            row.add_css_class("error");
            let img = gtk::Image::from_icon_name("dialog-warning-symbolic");
            row.add_prefix(&img);
            list.append(&row);
        }
    }
}

/// Rebuild the downloads list.
fn rebuild_downloads_list(
    list: &gtk::ListBox,
    entries: &[jotta_core::remote::DownloadEntry],
) {
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }
    if entries.is_empty() {
        let row = adw::ActionRow::builder()
            .title(i18n::t("restore.no.transfers.title"))
            .subtitle(i18n::t("restore.no.transfers.subtitle"))
            .build();
        list.append(&row);
        return;
    }
    for entry in entries {
        let status_text = match entry.status {
            jotta_core::remote::DownloadStatus::Ongoing => i18n::t("transfer.status.downloading"),
            jotta_core::remote::DownloadStatus::Failed => i18n::t("transfer.status.failed"),
            jotta_core::remote::DownloadStatus::Completed => i18n::t("transfer.status.done"),
        };
        // Kun filnavn i titel, fulde stier i undertitel.
        let file_name = entry
            .remote_path
            .rsplit('/')
            .next()
            .unwrap_or(&entry.remote_path);
        let subtitle = format!("{} → {}", entry.remote_path, entry.local_dest);
        let row = adw::ActionRow::builder()
            .title(&format!("{} ({})", file_name, status_text))
            .subtitle(&subtitle)
            .build();

        // Abort/clear button.
        let abort_btn = gtk::Button::from_icon_name("process-stop-symbolic");
        abort_btn.add_css_class("flat");
        abort_btn.set_valign(gtk::Align::Center);
        abort_btn.set_tooltip_text(Some(i18n::t("restore.abort.tooltip")));
        let id = entry.id.clone();
        abort_btn.connect_clicked(move |_| {
            run_cli(JottaCommand::DownloadAbort(id.clone()));
        });
        row.add_suffix(&abort_btn);

        // Retry button (only for failed).
        if entry.status == jotta_core::remote::DownloadStatus::Failed {
            let retry_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
            retry_btn.add_css_class("flat");
            retry_btn.set_valign(gtk::Align::Center);
            retry_btn.set_tooltip_text(Some(i18n::t("restore.retry.tooltip")));
            let id = entry.id.clone();
            retry_btn.connect_clicked(move |_| {
                run_cli(JottaCommand::DownloadRetry(id.clone()));
            });
            row.add_suffix(&retry_btn);
        }

        list.append(&row);
    }
}
