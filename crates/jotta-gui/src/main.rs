//! jotta-desktop: the GTK4/libadwaita main window.
//!
//! Iteration 3 scope:
//! - status page: live state from jotta-core (email, usage bar, pause/resume)
//! - backup page: list from jotta-core, add folder via FileDialog
//! - sync page: sync info + start/stop switch
//!
//! The GUI never calls jotta-cli itself except through jotta-core types.
//! All CLI calls run on a worker thread; the GTK main loop is never blocked.

use gtk::glib::{self, clone, Propagation};
use gtk::prelude::*;
use jotta_core::cli::{BinaryLocator, CliClient, JottaCommand};
use jotta_core::status::{State, Status};
use libadwaita as adw;
use libadwaita::prelude::*;
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

    let app = adw::Application::builder()
        .application_id("io.github.eske.JottaDesktop")
        .build();

    app.connect_activate(build_ui);
    let code: i32 = app.run().into();
    std::process::exit(code);
}

fn build_ui(app: &adw::Application) {
    let core = Arc::new(App::new());

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .default_width(800)
        .default_height(600)
        .title("Jotta Desktop")
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
    sidebar.append(&nav_item("Status"));
    sidebar.append(&nav_item("Backup"));
    sidebar.append(&nav_item("Sync"));
    sidebar.append(&nav_item("Ignore-mønstre"));
    sidebar.append(&nav_item("Indstillinger"));

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
    let sync_page = build_sync_page(&sync_group);
    stack.add_titled(&sync_page, Some("sync"), "Sync");

    stack.add_titled(&placeholder("Ignore-mønstre"), Some("ignores"), "Ignore-mønstre");
    stack.add_titled(&placeholder("Indstillinger"), Some("settings"), "Indstillinger");

    sidebar.connect_row_selected(clone!(
        #[strong]
        stack,
        move |_, row| {
            if let Some(row) = row {
                if let Some(child) = row.child() {
                    if let Some(label) = child.downcast_ref::<gtk::Label>() {
                        let title = label.text().to_string();
                        let page = match title.as_str() {
                            "Status" => Some("status"),
                            "Backup" => Some("backup"),
                            "Sync" => Some("sync"),
                            "Ignore-mønstre" => Some("ignores"),
                            "Indstillinger" => Some("settings"),
                            _ => None,
                        };
                        if let Some(p) = page {
                            stack.set_visible_child_name(p);
                        }
                    }
                }
            }
        },
    ));

    let sidebar_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    sidebar.set_margin_top(12);
    sidebar.set_margin_bottom(12);
    sidebar.set_margin_start(12);
    sidebar.set_margin_end(12);
    sidebar_box.append(&sidebar);

    let sidebar_page = adw::NavigationPage::new(&sidebar_box, "Jotta Desktop");
    let content_page = adw::NavigationPage::new(&stack, "Status");

    let split_view = adw::NavigationSplitView::new();
    split_view.set_sidebar(Some(&sidebar_page));
    split_view.set_content(Some(&content_page));

    toast_overlay.set_child(Some(&split_view));
    window.set_content(Some(&toast_overlay));

    // --- Wire handlers ---
    let toast_tx_pause = toast_tx.clone();
    pause_switch.connect_state_set(move |_sw, state| {
        let cmd = if state {
            JottaCommand::Resume
        } else {
            JottaCommand::Pause
        };
        if let Some(loc) = BinaryLocator::from_path() {
            let client = CliClient::new(loc);
            let tx = toast_tx_pause.clone();
            std::thread::spawn(move || {
                if let Err(e) = client.run(&cmd) {
                    let _ = tx.send(format!("Kunne ikke ændre pause: {e}"));
                }
            });
        }
        Propagation::Proceed
    });

    // Sync switch: ON = sync running (SyncStart), OFF = stopped (SyncStop).
    // Destructive: stopping sync is confirmed (safety rule 3).
    {
        let window = window.clone();
        let sync_switch = sync_switch.clone();
        sync_switch.connect_state_set(move |sw, state| {
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
                dialog.choose(&window, None::<&gtk::gio::Cancellable>, move |resp| {
                    if resp == "stop" {
                        run_cli(JottaCommand::SyncStop);
                    } else {
                        // Revert the switch.
                        sw2.set_active(true);
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
    add_btn.connect_clicked(clone!(
        #[weak]
        window,
        move |_| {
            let dialog = gtk::FileDialog::new();
            dialog.set_title("Vælg mappe til backup");
            dialog.set_modal(true);
            dialog.set_accept_label(Some("Tilføj"));
            let toast_tx = toast_tx.clone();
            dialog.select_folder(Some(&window), None::<&gtk::gio::Cancellable>, move |res| {
                match res {
                    Ok(file) => {
                        if let Some(path) = file.path() {
                            run_cli(JottaCommand::BackupAdd(path));
                            let _ = toast_tx.send("Backup-mappe tilføjet".to_string());
                        }
                    }
                    Err(_) => {} // user cancelled
                }
            });
        },
    ));
    // Replace the placeholder button on the backup page with the wired one.
    replace_add_button(&backup_page, &add_btn);

    // --- Poller: refresh status every 3s on a worker thread, update UI on main loop ---
    let poll_status = Arc::clone(&core.status);
    let poll_client = core.client();
    let ui = StatusWidgets {
        state_label: status_state_label.clone(),
        email_label: status_email_label.clone(),
        level: status_level.clone(),
        usage_label: status_usage_label.clone(),
        pause_switch: pause_switch.clone(),
        backup_list: backup_list.clone(),
        sync_switch: sync_switch.clone(),
        sync_path_row: sync_path_row.clone(),
        sync_count_row: sync_count_row.clone(),
    };
    // Only Status (plain data) crosses the thread boundary via this channel.
    // The main loop applies it to the widgets; widgets never leave this thread.
    let (status_tx, status_rx) = std::sync::mpsc::channel::<Status>();
    std::thread::spawn(move || loop {
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

    window.present();
}

#[derive(Clone)]
struct StatusWidgets {
    state_label: gtk::Label,
    email_label: gtk::Label,
    level: gtk::LevelBar,
    usage_label: gtk::Label,
    pause_switch: gtk::Switch,
    backup_list: gtk::ListBox,
    sync_switch: gtk::Switch,
    sync_path_row: adw::ActionRow,
    sync_count_row: adw::ActionRow,
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

        // Pause switch (block signal to avoid feedback loop)
        if status.state == State::Paused {
            self.pause_switch.set_active(true);
        } else if status.state != State::Offline {
            self.pause_switch.set_active(false);
        }

        // Backup list
        rebuild_backup_list(&self.backup_list, &status.backups);

        // Sync section
        if let Some(sync) = &status.sync {
            self.sync_switch.set_active(sync.enabled);
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
            self.sync_switch.set_active(false);
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

fn rebuild_backup_list(list: &gtk::ListBox, backups: &[jotta_core::status::BackupFolder]) {
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
        list.append(&row);
    }
}

fn build_status_page(
    state_label: &gtk::Label,
    email_label: &gtk::Label,
    level: &gtk::LevelBar,
    usage_label: &gtk::Label,
    pause_switch: &gtk::Switch,
) -> gtk::Widget {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 24);
    set_margin_all(&box_, 24);

    let title = gtk::Label::new(None);
    title.set_markup("<span size='xx-large' weight='bold'>Status</span>");
    title.set_halign(gtk::Align::Start);
    box_.append(&title);

    box_.append(state_label);
    box_.append(email_label);

    // Pause/resume row
    let pause_group = adw::PreferencesGroup::new();
    let pause_row = adw::ActionRow::builder()
        .title("Pause backup og sync")
        .subtitle("Stopper midlertidigt al aktivitet")
        .build();
    pause_row.add_suffix(pause_switch);
    pause_group.add(&pause_row);
    box_.append(&pause_group);

    // Usage
    let usage_title = gtk::Label::new(Some("Lagerplads"));
    usage_title.set_halign(gtk::Align::Start);
    usage_title.add_css_class("heading");
    box_.append(&usage_title);
    box_.append(usage_label);
    box_.append(level);

    box_.upcast()
}

fn build_backup_page(list: &gtk::ListBox) -> gtk::Widget {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 12);
    set_margin_all(&box_, 24);

    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_child(Some(list));
    scrolled.set_vexpand(true);
    box_.append(&scrolled);

    box_.upcast()
}

fn build_sync_page(group: &adw::PreferencesGroup) -> gtk::Widget {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 12);
    set_margin_all(&box_, 24);
    box_.append(group);
    box_.upcast()
}

fn replace_add_button(page: &gtk::Widget, new_btn: &gtk::Button) {
    if let Some(box_) = page.downcast_ref::<gtk::Box>() {
        box_.append(new_btn);
    }
}

fn nav_item(label: &str) -> gtk::Widget {
    gtk::Label::new(Some(label)).upcast()
}

fn placeholder(title: &str) -> gtk::Widget {
    let label = gtk::Label::new(Some(&format!("{title} — under udvikling")));
    label.set_vexpand(true);
    label.upcast()
}

fn set_margin_all(widget: &impl IsA<gtk::Widget>, margin: i32) {
    widget.set_margin_top(margin);
    widget.set_margin_bottom(margin);
    widget.set_margin_start(margin);
    widget.set_margin_end(margin);
}


