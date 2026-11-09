//! Graphical login dialog + stdin-feeding worker for `jotta-cli login`.
//!
//! SPEC.md §2B (decision change owner-approved 23.10.2026): the GUI feeds the
//! 3 interactive prompts of `jotta-cli login` programmatically. Empirically
//! verified (jotta-cli 0.17.176206): prompts on stdout, errors on stderr,
//! stdin piping works (also via `flatpak-spawn --host`) — no PTY needed.
//!
//! SECURITY INVARIANTS:
//! - the token goes over STDIN only — never argv (world-readable in
//!   /proc/<pid>/cmdline), never in logs/toasts/error strings;
//! - the token field is an `adw::PasswordEntryRow` (masked);
//! - the token is held transiently in memory (this function's locals) and is
//!   never stored; jotta-cli owns all credential storage;
//! - answers are fed ONE AT A TIME, only after the expected prompt text was
//!   verified in the output (jotta-core::login::detect_prompt) — jotta-cli
//!   re-prompts forever at EOF, and a fresh buffered reader per prompt can
//!   swallow pre-written lines;
//! - any mismatch/error aborts and the NeedsLogin state + this dialog's
//!   fallback instructions remain.
//!
//! Threading rule: the worker thread owns only plain data (Child, String,
//! channels). Widgets never cross the boundary; events come back via mpsc +
//! glib::idle_add_local.

use gtk::glib;
use gtk::prelude::*;
use jotta_core::cli::{BinaryLocator, CliClient, JottaCommand};
use jotta_core::login::{self, LoginStep};
use libadwaita as adw;
use std::io::{Read, Write};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Events from the login worker thread back to the GTK main loop.
enum LoginEvent {
    /// A prompt was verified in the output; we answered it.
    Prompt(LoginStep),
    /// jotta-cli exited 0 — logged in.
    Done,
    /// `ERROR  Already logged in as …` — nothing to do, re-poll status.
    AlreadyLoggedIn,
    /// Login failed; `String` is jotta-cli's stderr (never contains the token).
    Failed(String),
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "jotta-enhed".to_string())
}

/// Open the login dialog. `locator` finds jotta-cli; `toast_tx` reports the result.
pub fn open_login_dialog(
    window: &adw::ApplicationWindow,
    locator: BinaryLocator,
    toast_tx: mpsc::Sender<String>,
) {
    let dialog = adw::Dialog::new();
    dialog.set_title("Log ind på Jottacloud");
    dialog.set_content_width(520);

    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let cancel_btn = gtk::Button::with_label("Annuller");
    header.pack_start(&cancel_btn);
    let login_btn = gtk::Button::with_label("Log ind");
    login_btn.add_css_class("suggested-action");
    header.pack_end(&login_btn);
    toolbar.add_top_bar(&header);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_all(18);

    let info = gtk::Label::new(Some(
        "Log ind med et personligt login-token. Generer tokenet under Security-fanen på jottacloud.com — tokenet er engangs og vises kun én gang.",
    ));
    info.set_wrap(true);
    info.set_xalign(0.0);
    info.add_css_class("dim-label");
    content.append(&info);

    // Vilkår (checkbox skal være afkrydset før "Log ind" aktiveres).
    let terms_group = adw::PreferencesGroup::builder().title("Vilkår").build();
    let license_row = adw::ActionRow::builder()
        .title("Jeg accepterer Jottaclouds betingelser")
        .build();
    let license_cb = gtk::CheckButton::new();
    license_row.add_suffix(&license_cb);
    license_row.set_activatable_widget(Some(&license_cb));
    terms_group.add(&license_row);
    let terms_link_row = adw::ActionRow::builder().title("Vilkår").build();
    let terms_link = gtk::LinkButton::with_label(
        "https://jottacloud.com/en/terms.html",
        "jottacloud.com/en/terms.html",
    );
    terms_link_row.add_suffix(&terms_link);
    terms_group.add(&terms_link_row);
    content.append(&terms_group);

    // Token (maskeret felt).
    let token_group = adw::PreferencesGroup::builder()
        .title("Login-token")
        .description("Personligt login-token — engangs, genereres på jottacloud.com")
        .build();
    let token_row = adw::PasswordEntryRow::builder()
        .title("Personal login token")
        .build();
    token_group.add(&token_row);
    let secure_row = adw::ActionRow::builder().title("Generer token").build();
    let secure_link =
        gtk::LinkButton::with_label("https://www.jottacloud.com/web/secure", "jottacloud.com/web/secure");
    secure_row.add_suffix(&secure_link);
    token_group.add(&secure_row);
    content.append(&token_group);

    // Enhedsnavn (pre-udfyldt med hostname — må aldrig være tomt, ellers
    // re-prompter jotta-cli i en uendelig løkke ved EOF).
    let device_group = adw::PreferencesGroup::builder().title("Denne enhed").build();
    let device_row = adw::EntryRow::builder()
        .title("Enhedsnavn")
        .text(&hostname())
        .build();
    device_group.add(&device_row);
    content.append(&device_group);

    let status_label = gtk::Label::new(None);
    status_label.add_css_class("dim-label");
    status_label.set_wrap(true);
    status_label.set_xalign(0.0);
    content.append(&status_label);

    toolbar.set_content(Some(&content));
    dialog.set_child(Some(&toolbar));
    dialog.present(Some(window));

    cancel_btn.connect_clicked({
        let dialog = dialog.clone();
        move |_| dialog.close()
    });

    // "Log ind" kun aktiv når vilkår er accepteret OG token er udfyldt.
    login_btn.set_sensitive(false);
    fn update_btn(btn: &gtk::Button, cb: &gtk::CheckButton, row: &adw::PasswordEntryRow) {
        btn.set_sensitive(cb.is_active() && !row.text().trim().is_empty());
    }
    {
        let b = login_btn.clone();
        let c = license_cb.clone();
        let t = token_row.clone();
        license_cb.connect_toggled(move |_| update_btn(&b, &c, &t));
    }
    {
        let b = login_btn.clone();
        let c = license_cb.clone();
        let t = token_row.clone();
        token_row.connect_changed(move |_| update_btn(&b, &c, &t));
    }

    login_btn.connect_clicked(move |_| {
        // Token lever kun transient i denne closure — logges aldrig.
        let token = token_row.text().to_string();
        let device_raw = device_row.text().to_string();
        let device = {
            let t = device_raw.trim();
            if t.is_empty() {
                hostname()
            } else {
                t.to_string()
            }
        };
        if !license_cb.is_active() || token.trim().is_empty() {
            return;
        }
        login_btn.set_sensitive(false);
        status_label.set_text("Kontakter jottad og logger ind…");

        let (ev_tx, ev_rx) = mpsc::channel::<LoginEvent>();
        let ev_rx = Arc::new(Mutex::new(ev_rx));
        std::thread::spawn(move || run_login(locator, token, device, ev_tx));

        let dialog2 = dialog.clone();
        let status_label2 = status_label.clone();
        let login_btn2 = login_btn.clone();
        let toast_tx2 = toast_tx.clone();
        glib::idle_add_local(move || {
            let rx = ev_rx.lock().unwrap();
            loop {
                match rx.try_recv() {
                    Ok(LoginEvent::Prompt(step)) => {
                        status_label2.set_text(match step {
                            LoginStep::License => "Accepterer vilkår…",
                            LoginStep::Token => "Sender login-token…",
                            LoginStep::DeviceName => "Registrerer enhed…",
                        });
                    }
                    Ok(LoginEvent::Done) => {
                        dialog2.close();
                        let _ = toast_tx2.send("Logget ind på Jottacloud".to_string());
                        return glib::ControlFlow::Break;
                    }
                    Ok(LoginEvent::AlreadyLoggedIn) => {
                        dialog2.close();
                        let _ = toast_tx2.send("Allerede logget ind — opdaterer status".to_string());
                        return glib::ControlFlow::Break;
                    }
                    Ok(LoginEvent::Failed(msg)) => {
                        // Fallback: NeedsLogin-tilstanden forbliver, og
                        // dialogen viser fejlen + instruktionerne ovenfor.
                        status_label2.set_text(&format!("Login fejlede: {msg}"));
                        login_btn2.set_sensitive(true);
                        return glib::ControlFlow::Break;
                    }
                    Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => return glib::ControlFlow::Break,
                }
            }
        });
    });
}

/// Worker: spawn `jotta-cli login` (via the argv whitelist + flatpak-spawn
/// prefix), read stdout/stderr on reader threads, and feed each answer only
/// after its prompt marker was verified in the output.
fn run_login(locator: BinaryLocator, token: String, device: String, tx: mpsc::Sender<LoginEvent>) {
    let client = CliClient::new(locator);
    let mut child = match client.spawn_piped(&JottaCommand::Login) {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(LoginEvent::Failed(e.to_string()));
            return;
        }
    };
    let mut stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let (bytes_tx, bytes_rx) = mpsc::channel::<(bool, Vec<u8>)>();
    if let Some(mut so) = stdout {
        let t = bytes_tx.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match so.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if t.send((true, buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }
    if let Some(mut se) = stderr {
        let t = bytes_tx.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match se.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if t.send((false, buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }
    drop(bytes_tx);

    let mut out = String::new();
    let mut err = String::new();
    let mut answered: Vec<LoginStep> = Vec::new();
    let start = Instant::now();

    let event = loop {
        if let Ok(Some(status)) = child.try_wait() {
            // Tøm det som læsertrådene stadig har i luften (f.eks. stderr
            // med "Already logged in" lige før exit), før vi klassificerer.
            while let Ok((is_out, bytes)) = bytes_rx.recv_timeout(Duration::from_millis(50)) {
                if is_out {
                    out.push_str(&String::from_utf8_lossy(&bytes));
                } else {
                    err.push_str(&String::from_utf8_lossy(&bytes));
                }
            }
            break classify(status, &err);
        }
        if start.elapsed() > Duration::from_secs(90) {
            let _ = child.kill();
            break LoginEvent::Failed("Login tog over 90 sekunder (timeout)".to_string());
        }
        match bytes_rx.recv_timeout(Duration::from_millis(100)) {
            Ok((true, bytes)) => {
                out.push_str(&String::from_utf8_lossy(&bytes));
                if let Some(step) = login::detect_prompt(&out) {
                    if !answered.contains(&step) {
                        answered.push(step);
                        let _ = tx.send(LoginEvent::Prompt(step));
                        // ene svar ad gangen, først når prompten er verifieret
                        let answer = match step {
                            LoginStep::License => "y".to_string(),
                            LoginStep::Token => token.clone(),
                            LoginStep::DeviceName => device.clone(),
                        };
                        if let Some(sin) = stdin.as_mut() {
                            let _ = sin.write_all(answer.as_bytes());
                            let _ = sin.write_all(b"\n");
                            let _ = sin.flush();
                        }
                    }
                }
            }
            Ok((false, bytes)) => {
                err.push_str(&String::from_utf8_lossy(&bytes));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => match child.wait() {
                Ok(status) => break classify(status, &err),
                Err(_) => {
                    break LoginEvent::Failed("kunne ikke vente på jotta-cli login".to_string())
                }
            },
        }
    };
    let _ = child.wait(); // reaper — ingen zombier
    let _ = tx.send(event);
}

fn classify(status: std::process::ExitStatus, err: &str) -> LoginEvent {
    if login::is_already_logged_in(err) {
        return LoginEvent::AlreadyLoggedIn;
    }
    if status.success() {
        return LoginEvent::Done;
    }
    if login::is_error(err) {
        // jotta-cli echoer ikke stdin, så stderr indeholder aldrig tokenet.
        return LoginEvent::Failed(err.trim().to_string());
    }
    LoginEvent::Failed(format!(
        "jotta-cli login afsluttede med kode {}",
        status.code().unwrap_or(-1)
    ))
}
