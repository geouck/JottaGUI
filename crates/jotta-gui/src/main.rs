//! jotta-desktop: the GTK4/libadwaita main window.
//!
//! MVP scope (iteration 1):
//! - status page: state, account, usage bar, active transfers
//! - backup folders: list + add folder + remove (with confirmation)
//! - pause/resume switch
//! - sync section: start/stop
//!
//! The GUI never calls jotta-cli itself except through jotta-core types.

use gtk::glib::clone;
use gtk::prelude::*;
use libadwaita as adw;

fn main() {
    adw::init().expect("failed to init libadwaita");

    let app = adw::Application::builder()
        .application_id("io.github.eske.JottaDesktop")
        .build();

    app.connect_activate(build_ui);
    std::process::exit(app.run().into());
}

fn build_ui(app: &adw::Application) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .default_width(800)
        .default_height(600)
        .title("Jotta Desktop")
        .build();

    let sidebar = gtk::ListBox::new();
    sidebar.add_css_class("navigation-sidebar");
    sidebar.append(&nav_item("Status"));
    sidebar.append(&nav_item("Backup"));
    sidebar.append(&nav_item("Sync"));
    sidebar.append(&nav_item("Ignore-mønstre"));
    sidebar.append(&nav_item("Indstillinger"));

    let stack = gtk::Stack::new();
    stack.add_titled(&status_page(), Some("status"), "Status");
    stack.add_titled(&backup_page(), Some("backup"), "Backup");
    stack.add_titled(&sync_page(), Some("sync"), "Sync");
    stack.add_titled(&placeholder("Ignore-mønstre"), Some("ignores"), "Ignore-mønstre");
    stack.add_titled(&placeholder("Indstillinger"), Some("settings"), "Indstillinger");

    sidebar.connect_row_selected(clone!(@strong stack => move |_, row| {
        if let Some(row) = row {
            if let Some(child) = row.child() {
                if let Some(label) = child.downcast_ref::<gtk::Label>() {
                    if let Some(title) = label.text().as_deref().map(|s| s.to_string()) {
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
        }
    }));

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

    window.set_content(Some(&split_view));
    window.present();
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

fn status_page() -> gtk::Widget {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 24);
    set_margin_all(&box_, 24);

    let title = gtk::Label::new(None);
    title.set_markup("<span size='xx-large' weight='bold'>Status</span>");
    box_.append(&title);

    // Usage bar (5 TB plan).
    let used = 850_000_000_000u64;
    let quota = 5_000_000_000_000u64;
    let level = gtk::LevelBar::builder()
        .min(0.0)
        .max(quota as f64)
        .value(used as f64)
        .valign(gtk::Align::Start)
        .build();
    let usage_label = gtk::Label::new(Some(&format!(
        "{:.2} TB af 5,00 TB brugt",
        used as f64 / 1e12
    )));
    usage_label.add_css_class("dim-label");

    box_.append(&usage_label);
    box_.append(&level);
    box_.upcast()
}

fn backup_page() -> gtk::Widget {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 12);
    set_margin_all(&box_, 24);

    let scrolled = gtk::ScrolledWindow::new();
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    // TODO: populated from jotta-core CliClient::run(BackupsList).
    let row = adw::ActionRow::builder()
        .title("Ingen backup-mapper endnu")
        .subtitle("Tryk 'Tilføj mappe' for at begynde")
        .build();
    list.append(&row);
    scrolled.set_child(Some(&list));
    scrolled.set_vexpand(true);

    let add_btn = gtk::Button::with_label("Tilføj mappe…");
    add_btn.add_css_class("suggested-action");
    // TODO: gtk::FileChooserDialog -> jotta_core BackupAdd + refresh list.

    box_.append(&scrolled);
    box_.append(&add_btn);
    box_.upcast()
}

fn sync_page() -> gtk::Widget {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 12);
    set_margin_all(&box_, 24);

    let group = adw::PreferencesGroup::new();
    let row = adw::ActionRow::builder()
        .title("Automatisk sync")
        .subtitle("Styrer sync-mappen mellem dine enheder")
        .build();
    let switch = gtk::Switch::new();
    row.add_suffix(&switch);
    group.add(&row);

    box_.append(&group);
    box_.upcast()
}
