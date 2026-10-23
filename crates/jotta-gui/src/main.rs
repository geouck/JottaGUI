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

    /// Poll `status --json` once; returns the parsed Status (Offline on error).
    fn poll(&self) -> Status {
        match self.client.status() {
            Ok(v) => Status::from_json(&v),
            Err(_) => Status::default(),
        }
    }

    fn client(&self) -> CliClient {
        CliClient::new(self.client.locator().clone())
    }
}

fn main() {
    adw::init().expect("failed to init libadwaita");

    // CSS: kompakte window controls (minimer/maximer/luk) — mindre knapper
    // så headerbaren ikke fylder for meget på KDE/COSMIC hvor knapperne
    // ellers bliver for store sammenlignet med GNOME/Nextcloud.
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        "headerbar button.windowbutton { min-width: 28px; min-height: 28px; padding: 0 4px; }",
    );
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("no display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
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

    if let Ok(pid_str) = std::fs::read_to_string(&pid_file) {
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            if std::path::Path::new(&format!("/proc/{pid}")).exists() {
                return;
            }
        }
    }

    let tray_bin = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("jotta-tray")))
        .filter(|p| p.exists());

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

fn build_ui(app: &adw::Application, shutdown: Arc<AtomicBool>) {
    let core = Arc::new(App::new());

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .default_width(800)
        .default_height(600)
        .title("Jotta Desktop")
        .icon_name("jotta-desktop")
        .build();

    // Toast overlay for command feedback.
    let toast_overlay = adw::ToastOverlay::new();

    // Channel from worker threads -> main loop for toast messages.
    // Only String crosses the thread boundary; the toast is created on the main thread.
    let (toast_tx, toast_rx) = std::sync::mpsc::channel::<String>();
    {
        let toast_overlay = toast_overlay.clone();
        let toast_rx = std::sync::Mutex::new(toast_rx);
        glib::idle_add_local(move || {
            let rx = toast_rx.lock().unwrap();
            while let Ok(msg) = rx.try_recv() {
                toast_overlay.add_toast(adw::Toast::new(&msg));
            }
            glib::ControlFlow::Continue
        });
    }

    let sidebar = gtk::ListBox::new();
    sidebar.add_css_class("navigation-sidebar");
    sidebar.append(&nav_item("Status", "jotta-desktop-idle-symbolic"));
    sidebar.append(&nav_item("Backup", "folder-symbolic"));
    sidebar.append(&nav_item("Sync", "folder-sync-symbolic"));
    sidebar.append(&nav_item("Overførsler", "emblem-downloads-symbolic"));
    sidebar.append(&nav_item("Gendan", "document-revert-symbolic"));
    sidebar.append(&nav_item("Indstillinger", "emblem-system-symbolic"));

    let stack = gtk::Stack::new();

    // --- Status page widgets (kept in a struct so the poller can update them) ---
    let status_state_label = gtk::Label::new(Some("Jottacloud: offline"));
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

    let status_page = build_status_page(
        &status_state_label,
        &status_email_label,
        &status_level,
        &status_usage_label,
        &pause_switch,
    );
    stack.add_titled(&status_page, Some("status"), "Status");

    // --- Backup page ---
    let backup_list = gtk::ListBox::new();
    backup_list.add_css_class("boxed-list");
    let backup_page = build_backup_page(&backup_list);
    stack.add_titled(&backup_page, Some("backup"), "Backup");

    // --- Sync page ---
    // Sync selective is NOT destructive (unlike backup ignores): ignored
    // folders are not deleted locally or in the cloud. No confirmation dialog.
    let sync_title_row = adw::ActionRow::builder()
        .title("Automatisk sync")
        .subtitle("Synkroniserer mappen mellem dine enheder")
        .build();
    let sync_path_row = adw::ActionRow::builder()
        .title("Sync-mappe")
        .subtitle("—")
        .build();
    let sync_count_row = adw::ActionRow::builder()
        .title("Filer i sync")
        .subtitle("—")
        .build();
    let sync_switch = gtk::Switch::new();
    sync_switch.set_valign(gtk::Align::Center);
    sync_title_row.add_suffix(&sync_switch);
    let sync_group = adw::PreferencesGroup::new();
    sync_group.add(&sync_title_row);
    sync_group.add(&sync_path_row);
    sync_group.add(&sync_count_row);
    // "Vælg mapper at synkronisere" — chevron-ikon for tydelighed (åbner dialog).
    let sync_selective_btn = gtk::Button::from_icon_name("go-next-symbolic");
    sync_selective_btn.add_css_class("flat");
    sync_selective_btn.set_tooltip_text(Some("Vælg mapper at synkronisere…"));
    let sync_selective_row = adw::ActionRow::builder()
        .title("Mappevalg")
        .subtitle("Vælg hvilke top-level mapper der skal synkroniseres")
        .build();
    sync_selective_row.add_suffix(&sync_selective_btn);
    sync_group.add(&sync_selective_row);
    let sync_page = build_sync_page(&sync_group);
    stack.add_titled(&sync_page, Some("sync"), "Sync");

    // --- Overførsler (transfers) page: uploads + downloads + errors ---
    // Three sections: active uploads, active downloads, recent errors.
    // Data comes from `list uploads --json`, `list downloads --json`,
    // `list downloaderrors --json`, `list uploaderrors --json`.
    let transfers_uploads_list = gtk::ListBox::new();
    transfers_uploads_list.add_css_class("boxed-list");
    let transfers_uploads_group = adw::PreferencesGroup::builder()
        .title("Aktive uploads")
        .build();
    transfers_uploads_group.add(&transfers_uploads_list);

    let transfers_downloads_list = gtk::ListBox::new();
    transfers_downloads_list.add_css_class("boxed-list");
    let transfers_downloads_group = adw::PreferencesGroup::builder()
        .title("Aktive downloads")
        .build();
    transfers_downloads_group.add(&transfers_downloads_list);

    let transfers_errors_list = gtk::ListBox::new();
    transfers_errors_list.add_css_class("boxed-list");
    let transfers_errors_group = adw::PreferencesGroup::builder()
        .title("Seneste fejl")
        .build();
    transfers_errors_group.add(&transfers_errors_list);

    let transfers_page = build_transfers_page(
        &transfers_uploads_group,
        &transfers_downloads_group,
        &transfers_errors_group,
    );
    stack.add_titled(&transfers_page, Some("transfers"), "Overførsler");

    // --- Gendan (restore/download) page ---
    // Remote browser for Backup/Archive/Sync + download dialog + downloads list.
    // Trash is web-only (trash list has no --json, plain output is unparseable).
    let restore_path_label = gtk::Label::new(Some("Jottacloud"));
    restore_path_label.set_halign(gtk::Align::Start);
    restore_path_label.add_css_class("dim-label");
    restore_path_label.set_wrap(true);
    let restore_path_up_btn = gtk::Button::from_icon_name("go-up-symbolic");
    restore_path_up_btn.add_css_class("flat");
    restore_path_up_btn.set_tooltip_text(Some("En mappe op"));
    restore_path_up_btn.set_valign(gtk::Align::Center);
    let restore_refresh_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
    restore_refresh_btn.add_css_class("flat");
    restore_refresh_btn.set_tooltip_text(Some("Genindlæs"));
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

    let restore_download_btn = gtk::Button::with_label("Download valgte…");
    restore_download_btn.add_css_class("suggested-action");
    let restore_download_row = adw::ActionRow::builder()
        .title("Download")
        .subtitle("Hent valgte filer eller mapper til en lokal mappe")
        .build();
    restore_download_row.add_suffix(&restore_download_btn);
    let restore_group = adw::PreferencesGroup::new();
    restore_group.add(&restore_download_row);

    let restore_downloads_list = gtk::ListBox::new();
    restore_downloads_list.add_css_class("boxed-list");
    let restore_downloads_group = adw::PreferencesGroup::builder()
        .title("Overførsler")
        .build();

    // Trash: web-only (trash list has no --json, plain output unparseable).
    let restore_trash_web_btn = gtk::Button::from_icon_name("go-next-symbolic");
    restore_trash_web_btn.add_css_class("flat");
    restore_trash_web_btn.set_tooltip_text(Some("Åbn papirkurv på jottacloud.com…"));
    let restore_trash_row = adw::ActionRow::builder()
        .title("Papirkurv")
        .subtitle("Gendan eller slet filer permanent på jottacloud.com (åbner i browser)")
        .build();
    restore_trash_row.add_suffix(&restore_trash_web_btn);
    let restore_trash_group = adw::PreferencesGroup::new();
    restore_trash_group.add(&restore_trash_row);

    let restore_page = build_restore_page(
        &restore_path_box,
        &restore_scrolled,
        &restore_group,
        &restore_downloads_group,
        &restore_downloads_list,
        &restore_trash_group,
    );
    stack.add_titled(&restore_page, Some("restore"), "Gendan");

    let settings_page = build_settings_page();
    stack.add_titled(&settings_page, Some("settings"), "Indstillinger");

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
                    3 => Some("transfers"),
                    4 => Some("restore"),
                    5 => Some("settings"),
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
    let content_page = adw::NavigationPage::new(&main_toolbar, "Status");
    split_view.set_content(Some(&content_page));

    // Dynamisk topbar titel: opdater NavigationPage titel når aktiv fane skifter.
    stack.connect_visible_child_notify(clone!(
        #[weak]
        content_page,
        move |stack| {
            let title = stack
                .visible_child_name()
                .and_then(|name| match name.as_str() {
                    "status" => Some("Status"),
                    "backup" => Some("Backup"),
                    "sync" => Some("Sync"),
                    "transfers" => Some("Overførsler"),
                    "restore" => Some("Gendan"),
                    "settings" => Some("Indstillinger"),
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
                        let _ = toast_tx.send(format!("Kunne ikke ændre pause: {e}"));
                    }
                }
            });
            Propagation::Proceed
        });
    }

    // Sync switch: ON = sync running (SyncStart), OFF = stopped (SyncStop).
    // Destructive: stopping sync is confirmed (safety rule 3).
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
                    Some("Stop sync?"),
                    Some("Sync-mappen stopper med at synkronisere. Filer på denne enhed påvirkes ikke, men ændringer synkroniseres ikke længere, før du starter igen."),
                );
                dialog.add_response("cancel", "Annuller");
                dialog.add_response("stop", "Stop sync");
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

    // "Tilføj mappe" button → FileDialog → BackupAdd.
    let add_btn = gtk::Button::with_label("Tilføj mappe…");
    add_btn.add_css_class("suggested-action");
    let toast_tx_add = toast_tx.clone();
    add_btn.connect_clicked(clone!(
        #[weak]
        window,
        move |_| {
            let dialog = gtk::FileDialog::new();
            dialog.set_title("Vælg mappe til backup");
            dialog.set_modal(true);
            dialog.set_accept_label(Some("Tilføj"));
            let toast_tx_add = toast_tx_add.clone();
            dialog.select_folder(Some(&window), None::<&gtk::gio::Cancellable>, move |res| {
                match res {
                    Ok(file) => {
                        if let Some(path) = file.path() {
                            run_cli(JottaCommand::BackupAdd(path));
                            let _ = toast_tx_add.send("Backup-mappe tilføjet".to_string());
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
            ("Backup", "Enheder med backup"),
            ("Archive", "Arkiverede filer"),
            ("Sync", "Synkroniseret mappe"),
        ];
        for (name, desc) in entries {
            let row = adw::ActionRow::builder()
                .title(name)
                .subtitle(&format!("Mappe — {desc}"))
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
                    restore_selected_is_dir.set(subtitle.starts_with("Mappe"));
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
                let is_dir = subtitle.starts_with("Mappe");
                // Ghost device: subtitle starts with "Offline enhed" — not navigable.
                let is_ghost = subtitle.starts_with("Offline enhed");
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
                let _ = toast_tx.send("Vælg en fil eller mappe først".to_string());
                return;
            }
            let remote_path = if current_path.borrow().is_empty() {
                selected.clone()
            } else {
                format!("{}/{}", current_path.borrow(), selected)
            };
            let is_dir = sel_is_dir.get();
            let dialog = gtk::FileDialog::new();
            dialog.set_title("Vælg download-destination");
            dialog.set_modal(true);
            dialog.set_accept_label(Some("Vælg"));
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
                                    Some("Erstat eksisterende fil?"),
                                    Some(&format!(
                                        "Filen „{selected}“ findes allerede i {}.

Vil du erstatte den?",
                                        dest.display()
                                    )),
                                );
                                alert.add_response("cancel", "Annuller");
                                alert.add_response("replace", "Erstat");
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
    // Error dedup: only toast NEW errors (tracked in a HashSet on main thread).
    {
        let uploads_list = transfers_uploads_list.clone();
        let downloads_list = transfers_downloads_list.clone();
        let errors_list = transfers_errors_list.clone();
        let toast_tx = toast_tx.clone();
        // Channel carries (uploads, downloads).
        let (tx, rx) = std::sync::mpsc::channel::<(
            Vec<jotta_core::remote::UploadEntry>,
            Vec<jotta_core::remote::DownloadEntry>,
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
                while let Ok((uploads, downloads)) = rx.try_recv() {
                    rebuild_uploads_list(&uploads_list, &uploads);
                    rebuild_transfers_downloads_list(&downloads_list, &downloads);
                    // Derive errors from entries (no separate CLI call needed).
                    let errors = jotta_core::remote::collect_transfer_errors(&downloads, &uploads);
                    rebuild_transfer_errors_list(&errors_list, &errors);
                    // Error dedup: toast only NEW errors.
                    let mut seen = seen_errors.borrow_mut();
                    for err in &errors {
                        let key = format!("{}:{}", err.path.as_deref().unwrap_or(""), err.message);
                        if seen.insert(key) {
                            let path_str = err.path.as_deref().unwrap_or("(ukendt sti)");
                            let _ = toast_tx.send(format!("Overførselsfejl: {path_str} — {}", err.message));
                        }
                    }
                }
                glib::ControlFlow::Continue
            });
        }
        {
            let shutdown = shutdown.clone();
            std::thread::spawn(move || loop {
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
                    let _ = tx.send((uploads, downloads));
                }
                std::thread::sleep(Duration::from_secs(5));
            });
        }
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
        toast_tx: toast_tx.clone(),
    };
    // Only Status (plain data) crosses the thread boundary via this channel.
    // The main loop applies it to the widgets; widgets never leave this thread.
    let (status_tx, status_rx) = std::sync::mpsc::channel::<Status>();
    {
        let shutdown = shutdown.clone();
        std::thread::spawn(move || loop {
            // Stop poller når app lukker (sidste vindue væk).
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            let status = match poll_client.status() {
                Ok(v) => Status::from_json(&v),
                Err(_) => Status::default(),
            };
            {
                let mut guard = poll_status.lock().unwrap();
                *guard = status.clone();
            }
            let _ = status_tx.send(status);
            std::thread::sleep(Duration::from_secs(3));
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
                // Normal luk: afslut processen.
                shutdown.store(true, Ordering::SeqCst);
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
    toast_tx: std::sync::mpsc::Sender<String>,
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
            toast_tx: self.toast_tx.clone(),
        }
    }
}



impl StatusWidgets {
    fn apply(&self, status: &Status) {
        // State line
        let state_text = match status.state {
            State::Offline => "Jottacloud: offline",
            State::NeedsLogin => "Jottacloud: log ind påkrævet",
            State::Paused => "Jottacloud: pauset",
            State::Syncing => "Jottacloud: synkroniserer…",
            State::Idle => "Jottacloud: oppe at køre",
        };
        self.state_label.set_text(state_text);

        // Email
        match &status.account_email {
            Some(email) => self.email_label.set_text(email),
            None => self.email_label.set_text(""),
        }

        // Usage bar
        if let (Some(used), Some(quota)) = (status.bytes_used, status.bytes_quota) {
            self.level.set_max_value(quota as f64);
            self.level.set_value(used as f64);
            self.usage_label.set_text(&format!(
                "{} af {} brugt",
                Status::format_bytes(used),
                Status::format_bytes(quota)
            ));
        } else {
            self.usage_label.set_text("Forbrug ikke tilgængeligt");
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
        let sync_active = status.sync.as_ref().map(|s| s.enabled).unwrap_or(false);
        if let Some(sync) = &status.sync {
            if self.sync_switch.is_active() != sync_active {
                self.sync_updating.set(true);
                self.sync_switch.set_active(sync_active);
                self.sync_updating.set(false);
            }
            self.sync_path_row.set_subtitle(
                sync.root_path.as_deref().unwrap_or("—"),
            );
            let count_text = match (sync.files, sync.bytes) {
                (Some(f), Some(b)) => format!("{f} filer, {}", Status::format_bytes(b)),
                (Some(f), None) => format!("{f} filer"),
                _ => "—".to_string(),
            };
            self.sync_count_row.set_subtitle(&count_text);
        } else {
            if self.sync_switch.is_active() != sync_active {
                self.sync_updating.set(true);
                self.sync_switch.set_active(sync_active);
                self.sync_updating.set(false);
            }
            self.sync_path_row.set_subtitle("Ingen sync konfigureret");
            self.sync_count_row.set_subtitle("—");
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
                    let _ = toast_tx.send(format!("Download startet: {remote_path}"));
                }
                Err(e) => {
                    // Error::CommandFailed viser nu fuld argv: "`jotta-cli download ...` exited 1: ..."
                    let _ = toast_tx.send(format!("Kunne ikke starte download: {e}"));
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
            .title("Ingen backup-mapper endnu")
            .subtitle("Tryk 'Tilføj mappe' for at begynde")
            .build();
        list.append(&row);
        return;
    }
    for b in backups {
        let subtitle = match (b.files, b.bytes) {
            (Some(f), Some(bytes)) => format!(
                "{} · {} filer · {}",
                b.path,
                f,
                Status::format_bytes(bytes)
            ),
            (Some(f), None) => format!("{} · {} filer", b.path, f),
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
        exclude_btn.set_tooltip_text(Some("Udeluk filer/mønstre fra denne backup…"));
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
        remove_btn.set_tooltip_text(Some("Fjern denne backup-mappe"));
        let backup_name = b.name.clone();
        let toast_tx = toast_tx.clone();
        let window = window.clone();
        remove_btn.connect_clicked(move |_| {
            let dialog = adw::AlertDialog::new(
                Some(&format!("Fjern backup «{}»?", backup_name)),
                Some("Backup-mappen fjernes fra Jottacloud-konfigurationen. Filerne på denne enhed påvirkes ikke, og de fjernes IKKE fra skyen — men de slettes ikke fra andre enheder heller. Vil du fortsætte?"),
            );
            dialog.add_response("cancel", "Annuller");
            dialog.add_response("remove", "Fjern backup");
            dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
            let toast_tx = toast_tx.clone();
            let backup_name = backup_name.clone();
            dialog.choose(&window, None::<&gtk::gio::Cancellable>, move |res| {
                if res.as_str() == "remove" {
                    run_cli(JottaCommand::BackupRemove(backup_name.clone()));
                    let _ = toast_tx.send(format!("Backup «{}» fjernet", backup_name));
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

fn build_status_page(
    state_label: &gtk::Label,
    email_label: &gtk::Label,
    level: &gtk::LevelBar,
    usage_label: &gtk::Label,
    pause_switch: &gtk::Switch,
) -> gtk::Widget {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 12);
    set_margin_all(&box_, 24);

    // Konto & status i et struktureret kort (PreferencesGroup).
    // Ingen H1-overskrift — topbaren viser fanens navn dynamisk.
    let account_group = adw::PreferencesGroup::builder()
        .title("Konto")
        .build();
    let state_row = adw::ActionRow::builder()
        .title("Status")
        .build();
    state_row.add_suffix(state_label);
    state_label.set_halign(gtk::Align::End);
    let email_row = adw::ActionRow::builder()
        .title("E-mail")
        .build();
    email_row.add_suffix(email_label);
    email_label.set_halign(gtk::Align::End);
    email_label.add_css_class("dim-label");
    account_group.add(&state_row);
    account_group.add(&email_row);
    box_.append(&account_group);

    // Pause/resume: positiv formulering — ON = automatisk aktiv.
    let pause_group = adw::PreferencesGroup::builder()
        .title("Aktivitet")
        .build();
    let pause_row = adw::ActionRow::builder()
        .title("Automatisk backup og sync")
        .subtitle("Slå fra for midlertidigt at sætte al aktivitet på pause")
        .build();
    pause_row.add_suffix(pause_switch);
    pause_row.set_activatable_widget(Some(pause_switch));
    pause_group.add(&pause_row);
    box_.append(&pause_group);

    // Lagerplads i egen indrammet gruppe.
    let usage_group = adw::PreferencesGroup::builder()
        .title("Lagerplads")
        .build();
    let usage_row = adw::ActionRow::builder()
        .title("Forbrug")
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

fn build_backup_page(list: &gtk::ListBox) -> gtk::Widget {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 12);
    set_margin_all(&box_, 24);

    // Ingen indre ScrolledWindow — listen ligger direkte i boksen.
    // ydre scrollable_page håndterer overflow. Undgår dobbelt scroll.
    box_.append(list);

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

fn build_transfers_page(
    uploads_group: &adw::PreferencesGroup,
    downloads_group: &adw::PreferencesGroup,
    errors_group: &adw::PreferencesGroup,
) -> gtk::Widget {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 12);
    set_margin_all(&box_, 24);
    // Ingen H1-overskrift — topbaren viser fanens navn dynamisk.
    box_.append(uploads_group);
    box_.append(downloads_group);
    box_.append(errors_group);
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
            .title("Ingen aktive uploads")
            .subtitle("Uploads vises her når de kører")
            .build();
        list.append(&row);
        return;
    }
    for entry in active {
        let status_text = match entry.status {
            jotta_core::remote::UploadStatus::Ongoing => "Uploader…",
            jotta_core::remote::UploadStatus::Failed => "Fejlet",
            jotta_core::remote::UploadStatus::Completed => "Færdig",
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
            .title("Ingen aktive downloads")
            .subtitle("Downloads vises her når de kører")
            .build();
        list.append(&row);
        return;
    }
    for entry in active {
        let status_text = match entry.status {
            jotta_core::remote::DownloadStatus::Ongoing => "Kører…",
            jotta_core::remote::DownloadStatus::Failed => "Fejlet",
            jotta_core::remote::DownloadStatus::Completed => "Færdig",
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
            .title("Ingen fejl")
            .subtitle("Overførselsfejl vises her")
            .build();
        list.append(&row);
        return;
    }
    for err in errors {
        let path_str = err.path.as_deref().unwrap_or("(ukendt sti)");
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

/// Byg Indstillinger-siden med minimize-to-tray switch.
fn build_settings_page() -> gtk::Widget {
    let group = adw::PreferencesGroup::builder()
        .title("Generelt")
        .build();

    // Minimize-to-tray: når aktiv, minimeres vinduet til tray ved luk i stedet
    // for at afslutte. Gemmes i en konfigurationsfil.
    let mtt_row = adw::ActionRow::builder()
        .title("Minimer til systembakke ved luk")
        .subtitle("Vinduet minimeres til systembakken i stedet for at lukke")
        .build();
    let mtt_switch = gtk::Switch::new();
    mtt_switch.set_valign(gtk::Align::Center);
    // Læs nuværende værdi fra konfigurationsfil
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
    let mtt_enabled = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("minimize_to_tray=")).map(|l| l.trim_end() == "minimize_to_tray=true"))
        .unwrap_or(true); // Standard: AKTIV
    mtt_switch.set_active(mtt_enabled);
    mtt_switch.connect_state_set(move |sw, state| {
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
        if let Some(parent) = config_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let content = format!("minimize_to_tray={}\n", state);
        let _ = std::fs::write(&config_path, content);
        sw.set_active(state);
        Propagation::Proceed
    });
    mtt_row.add_suffix(&mtt_switch);
    mtt_row.set_activatable_widget(Some(&mtt_switch));
    group.add(&mtt_row);

    let box_widget = gtk::Box::new(gtk::Orientation::Vertical, 12);
    set_margin_all(&box_widget, 12);
    box_widget.append(&group);
    scrollable_page(&box_widget.upcast()).upcast()
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
                let _ = toast_tx.send("Ingen sync-mappe konfigureret".to_string());
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
    dialog.set_title("Vælg mapper at synkronisere");
    dialog.set_content_width(420);
    dialog.set_content_height(480);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    set_margin_all(&content, 18);

    let intro = gtk::Label::new(Some(&format!(
        "Vælg hvilke mapper i {sync_root} der skal synkroniseres.\nAfkrydsede mapper synkroniseres. Fjernede afkrydsninger stopper sync for den mappe — filerne slettes ikke hverken lokalt eller i skyen."
    )));
    intro.set_wrap(true);
    intro.set_xalign(0.0);
    intro.add_css_class("dim-label");
    content.append(&intro);

    let list_box = gtk::ListBox::new();
    list_box.add_css_class("boxed-list");
    let mut checkboxes: Vec<(String, gtk::CheckButton)> = Vec::new();
    if all_folders.is_empty() {
        let row = adw::ActionRow::builder()
            .title("Ingen mapper i sync-mappen")
            .subtitle("Tilføj mapper til sync-mappen for at kunne vælge dem her")
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
    let cancel_btn = gtk::Button::with_label("Annuller");
    cancel_btn.connect_clicked(move |_| {
        dialog_cancel.close();
    });
    header.pack_start(&cancel_btn);

    let dialog_apply = dialog.clone();
    let toast_tx = toast_tx.clone();
    let currently_ignored = currently_ignored.to_vec();
    let apply_btn = gtk::Button::with_label("Anvend");
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
                        "Ingen ændringer".to_string()
                    } else {
                        format!("{n} mappevalg opdateret")
                    }
                } else {
                    format!("Fejl: {}", errors.join(", "))
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
    dialog.set_title(&format!("Udeluk fra backup: {backup_name}"));
    dialog.set_content_width(480);
    dialog.set_content_height(520);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    set_margin_all(&content, 18);

    let scope_model = gtk::StringList::new(&["Denne backup", "Alle backups"]);
    let scope_row = adw::ComboRow::builder()
        .title("Anvend på")
        .subtitle("Vælg hvilke backups mønsteret gælder for")
        .model(&scope_model)
        .selected(0)
        .build();
    let scope_group = adw::PreferencesGroup::new();
    scope_group.add(&scope_row);
    content.append(&scope_group);

    let warning = gtk::Label::new(Some(
        "Advarsel: Tilføjelse af et mønster fjerner allerede backup'ede filer der matcher mønsteret — de flyttes til papirkurven i Jottacloud.",
    ));
    warning.add_css_class("warning");
    warning.add_css_class("dim-label");
    warning.set_wrap(true);
    warning.set_xalign(0.0);
    content.append(&warning);

    let pattern_entry = adw::EntryRow::builder()
        .title("Nyt mønster")
        .show_apply_button(true)
        .build();
    let entry_group = adw::PreferencesGroup::builder()
        .title("Mønstre")
        .description("Gitignore-syntaks, relativ til backup-roden (stien nedenfor). Eksempler: **.tmp, node_modules/, *.log")
        .build();
    entry_group.add(&pattern_entry);
    content.append(&entry_group);

    let path_label = gtk::Label::new(Some(&format!("Backup-rod: {backup_path}")));
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
            Some("Tilføj udelukkelsesmønster?"),
            Some(&format!(
                "Mønsteret `{pattern}` tilføjes{}. Matcher mønsteret filer der allerede er backup'et, flyttes de til papirkurven i Jottacloud.",
                if backup.is_some() {
                    format!(" for backup'en `{}`", backup_name2)
                } else {
                    " for ALLE backups".to_string()
                }
            )),
        );
        confirm.add_response("cancel", "Annuller");
        confirm.add_response("add", "Tilføj mønster");
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
                                let _ = toast_tx.send(format!("Mønster tilføjet: {pattern}"));
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
                                let _ = toast_tx.send(format!("Kunne ikke tilføje mønster: {e}"));
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
    let close_btn = gtk::Button::with_label("Luk");
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
            .title("Ingen udelukkelsesmønstre")
            .subtitle("Tilføj et mønster ovenfor")
            .build();
        list.append(&row);
        return;
    }
    for pattern in patterns {
        let row = adw::ActionRow::builder().title(pattern).build();
        let remove_btn = gtk::Button::from_icon_name("user-trash-symbolic");
        remove_btn.add_css_class("flat");
        remove_btn.set_valign(gtk::Align::Center);
        remove_btn.set_tooltip_text(Some("Fjern mønster"));
        let pattern = pattern.to_string();
        let backup_name = backup_name.to_string();
        let toast_tx_remove = toast_tx.clone();
        let list_remove = list.clone();
        remove_btn.connect_clicked(move |_| {
            let confirm = adw::AlertDialog::new(
                Some("Fjern udelukkelsesmønster?"),
                Some(&format!(
                    "Mønsteret `{pattern}` fjernes for backup'en `{backup_name}`. Filer der matcher igen bliver backet up ved næste scan."
                )),
            );
            confirm.add_response("cancel", "Annuller");
            confirm.add_response("remove", "Fjern");
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
                            let _ = toast_tx.send("Mønster fjernet".to_string());
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

fn build_restore_page(
    path_box: &gtk::Box,
    remote_scrolled: &gtk::ScrolledWindow,
    download_group: &adw::PreferencesGroup,
    downloads_group: &adw::PreferencesGroup,
    downloads_list: &gtk::ListBox,
    trash_group: &adw::PreferencesGroup,
) -> gtk::Widget {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 12);
    set_margin_all(&box_, 24);

    // Ingen H1-overskrift — topbaren viser fanens navn dynamisk.

    // Remote browser section
    let browser_title = gtk::Label::new(Some("Fjernfiler"));
    browser_title.set_halign(gtk::Align::Start);
    browser_title.add_css_class("heading");
    box_.append(&browser_title);
    box_.append(path_box);
    box_.append(remote_scrolled);

    // Download section
    box_.append(download_group);

    // Downloads section
    let downloads_title = gtk::Label::new(Some("Overførsler"));
    downloads_title.set_halign(gtk::Align::Start);
    downloads_title.add_css_class("heading");
    downloads_group.add(downloads_list);
    box_.append(downloads_group);

    // Trash section (web-only — no list UI).
    let trash_title = gtk::Label::new(Some("Papirkurv"));
    trash_title.set_halign(gtk::Align::Start);
    trash_title.add_css_class("heading");
    box_.append(&trash_title);
    box_.append(trash_group);

    scrollable_page(&box_.upcast()).upcast()
}

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
                ("Backup", "Enheder med backup"),
                ("Archive", "Arkiverede filer"),
                ("Sync", "Synkroniseret mappe"),
            ];
            for (name, desc) in entries {
                let row = adw::ActionRow::builder()
                    .title(name)
                    .subtitle(&format!("Mappe — {desc}"))
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
                    .title("(tom)")
                    .subtitle("Denne mappe er tom")
                    .build();
                list.append(&row);
                return;
            }
            for entry in entries {
                let subtitle = if entry.is_dir {
                    "Mappe — dobbeltklik for at åbne".to_string()
                } else {
                    // Fil: vis størrelse hvis tilgængelig.
                    match entry.size {
                        Some(s) => format!("Fil — {}", Status::format_bytes(s)),
                        None => "Fil".to_string(),
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
                    .title("Ingen revisioner")
                    .subtitle("Filen har ingen tidligere versioner")
                    .build();
                list.append(&row);
                return;
            }
            for rev in revs {
                let subtitle = match rev.size {
                    Some(s) => format!("Revision {} — {}", rev.id, Status::format_bytes(s)),
                    None => format!("Revision {}", rev.id),
                };
                let row = adw::ActionRow::builder()
                    .title(&format!("Version {}", rev.id))
                    .subtitle(&subtitle)
                    .build();
                let img = gtk::Image::from_icon_name("text-x-generic-symbolic");
                row.add_prefix(&img);
                list.append(&row);
            }
        }
        LsNavResult::Empty => {
            let row = adw::ActionRow::builder()
                .title("(tom)")
                .subtitle("Intet at vise")
                .build();
            list.append(&row);
        }
        LsNavResult::Error(msg) => {
            // Graceful error — e.g. ghost device "Cannot find device".
            let row = adw::ActionRow::builder()
                .title("Kunne ikke indlæse")
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
            .title("Ingen overførsler")
            .subtitle("Downloads og uploads vises her")
            .build();
        list.append(&row);
        return;
    }
    for entry in entries {
        let status_text = match entry.status {
            jotta_core::remote::DownloadStatus::Ongoing => "Kører…",
            jotta_core::remote::DownloadStatus::Failed => "Fejlet",
            jotta_core::remote::DownloadStatus::Completed => "Færdig",
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
        abort_btn.set_tooltip_text(Some("Annuller eller fjern fra liste"));
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
            retry_btn.set_tooltip_text(Some("Prøv igen"));
            let id = entry.id.clone();
            retry_btn.connect_clicked(move |_| {
                run_cli(JottaCommand::DownloadRetry(id.clone()));
            });
            row.add_suffix(&retry_btn);
        }

        list.append(&row);
    }
}
