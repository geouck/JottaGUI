# HANDOFF - JottaGUI

This document is the complete project context for any AI agent (or human)
taking over development. Read it fully before writing code.

## What this project is

JottaGUI is a cross-desktop Linux client for Jottacloud (Norwegian cloud
storage). The owner (Eske) runs Fedora KDE and has:

- a paid Jottacloud 5 TB plan,
- `jotta-cli`/`jottad` installed from Jottacloud's official RPM repo on desktop + laptop, running as a systemd **user slice** service with `loginctl enable-linger`,
- `~/Documents` synced two-way between the two machines (`jotta-cli sync setup --root ~/Documents` + `sync start`),
- plans to migrate ~800 GB from OneDrive via rclone (not done yet).

The app's job: give Linux a proper desktop client. Jottacloud only ships
official GUI apps for Windows/macOS; on Linux there is only the CLI.

## Verified facts about jotta-cli (learned hands-on, late 2026)

- Daemon: `jottad`, socket at `$XDG_RUNTIME_DIR/jottad/jottad.socket`; the client `jotta-cli` talks to it. `jotta-cli login` FAILS with "Could not connect to jottad" if the daemon is not running.
- Start/restart daemon: `run_jottad` (wrapper) or `systemctl --user enable --now jottad.service`.
- Sync commands (NOT `sync enable`): `sync setup --root <path>`, `sync start`, `sync stop`, `sync move`, `sync reset`, `sync selective`, `sync log -n <n>`, `sync trigger`, `sync configure`, `sync diag`.
- Backup commands: `add <path>`, `rem`, `scan`, `status`, `pause`, `resume`, `observe`, `tail`, `share`, `archive`, `trash`, `download`, `list`.
- Ignores: `jotta-cli ignores add --pattern PAT [--backup NAME]`, gitignore-style patterns, relative to backup root. `ignores test --pattern P --path X` validates before adding. WARNING: adding a pattern REMOVES already-backed-up matching files (they go to Trash).
- `--json` output exists for status; the schema is NOT formally documented - hence the tolerant parser in jotta-core.
- RPM repo: `https://repo.jotta.cloud/redhat` (gpgkey `https://repo.jotta.cloud/jotta.gpg`, binary format - import via `gpg --enarmor` or let dnf import it from the repo file's gpgkey directive).

## UI/UX design guidelines (GNOME/libadwaita HIG, cross-DE)

Established during iteration 6o (UI polish, Gemini Argon review):
- **Topbar**: dynamic title — `NavigationPage::set_title()` updates on `stack.connect_visible_child_notify`. Never hardcode a page name in the headerbar.
- **No H1 headings in page content**: the topbar shows the page name. Page content starts directly with `adw::PreferencesGroup` or a `gtk::Box`.
- **Sidebar**: `gtk::ListBox` with `navigation-sidebar` CSS class. Each item = `gtk::Box` (horizontal) with `gtk::Image` (symbolic icon, freedesktop names) + `gtk::Label` (left-aligned, hexpand). App title label at the top of the sidebar box.
- **Page content**: all content in `adw::PreferencesGroup` (titled groups) or `boxed-list` ListBoxes. Use `set_margin_all(widget, 24)` helper for page padding.
- **Action buttons**: dialog-opening rows use `go-next-symbolic` chevron icon (flat button) as suffix on `adw::ActionRow`. Destructive actions use `user-trash-symbolic` icon with `adw::AlertDialog` confirmation (safety rule 3).
- **Switches**: positive formulation (ON = active/desired state). `adw::ActionRow::set_activatable_widget(Some(&switch))` so clicking the row toggles the switch.
- **Scrolling**: each page wrapped in `scrollable_page()` helper (outer ScrolledWindow). NO inner ScrolledWindow inside a page — causes double scrollbars and vexpand issues.
- **Status page**: account info (state, email) in a "Konto" PreferencesGroup with values as right-aligned suffix labels. Usage in a "Lagerplads" group with LevelBar.
- **Remote browser**: root path label shows "Jottacloud" (not "Backup / Archive / Sync"). Downloads section titled "Overførsler". Download entries show filename only in title, full paths in subtitle.
- **Cross-DE icons**: use freedesktop standard symbolic icon names (`folder-symbolic`, `folder-sync-symbolic`, `document-revert-symbolic`, `emblem-system-symbolic`, `go-next-symbolic`, `go-up-symbolic`, `view-refresh-symbolic`, `user-trash-symbolic`, `process-stop-symbolic`). These exist in all major icon themes (Adwaita, Breeze, Cosmic).

## Hard safety rules (encode in the GUI)

1. NEVER move the sync folder manually; jotta-cli mirrors a move as a mass delete. Only `sync move`.
2. Deleting a file in a sync folder deletes it on ALL machines.
3. Confirm destructive commands (`rem`, `sync stop`, `reset`, trash empty).
4. Never kill a daemon we did not spawn (host systemd unit wins).
5. Never bundle the proprietary jotta-cli binary in a release without written permission from Jottacloud (support@jottacloud.com) - TODO before Flathub submission.

## Architecture decisions (do not casually change)

- `jotta-core` has ZERO external dependencies by design (builds in Flatpak builder sandboxes, unit-testable anywhere). All jotta-cli knowledge lives here.
- `jotta-tray` uses the `ksni` crate (StatusNotifierItem): native on KDE + COSMIC, GNOME needs the AppIndicator extension.
- `jotta-gui` uses gtk4-rs + libadwaita (adw::Leaflet sidebar layout, app id `io.github.eske.JottaDesktop`).
- Model B: `DaemonSupervisor` spawns the bundled `jottad` only if the socket is absent; otherwise it reuses the host daemon (`stop_if_owned` only kills our own child).
- CLI calls go through the `JottaCommand` enum -> `argv()` whitelist. The GUI must NEVER shell out with raw strings.
- Process model: `jotta-tray` autostarts at login (XDG autostart .desktop) and runs persistently as the always-on background process. `jotta-desktop` (GUI) is launched on demand — from the tray menu ("Åbn vindue"), the app menu, or the .desktop file. The GUI is a GApplication single-instance (application_id `io.github.eske.JottaDesktop`): a second launch activates the existing window instead of opening a new one. When the last GUI window closes, `connect_window_removed` sets a shutdown AtomicBool, the 3s status poller checks it and terminates, and `app.quit()` ends the GTK main loop — the process exits cleanly. The tray keeps running independently.

## Current state (iteration 6 + 6o complete; PR #7 merged, PR #9 draft)

- jotta-core: 28 unit tests green. Zero dependencies. JSON parser, CLI wrapper (`JottaCommand` enum + `argv()` whitelist), daemon supervisor (Model B), tolerant status model, remote JSON parsers (`ls --json`, `list downloads --json`), file revisions, download entries.
- jotta-tray: ksni StatusNotifierItem with embedded ARGB32 pixmap icons (5 states × 5 sizes). Menu: status, pause/resume (idempotent), "Åbn vindue", "Åbn jottacloud.com", "Afslut". Single-instance via PID file. Spawns GUI via `find_gui_binary()` + reaper thread. Click on icon (`activate()`) opens GUI. Reads minimize-to-tray setting from `~/.config/jotta-desktop/settings.conf`.
- jotta-gui: GTK4/libadwaita main window. GApplication single-instance (`io.github.eske.JottaDesktop`). Pages: Status (konto-kort, automatisk backup/sync switch, lagerplads-kort), Backup (list, "Tilføj mappe", "Udeluk…" chevron, "Fjern" med AlertDialog), Sync (info, start/stop, selective folder dialog), Gendan/Restore (remote browser "Jottacloud" rod, download dialog, "Overførsler" liste, papirkurv web-knap), Indstillinger (minimize-to-tray switch, default ON). Dynamisk topbar titel. Sidebar med symbolic ikoner + app-titel. Ingen H1-overskrifter i sider. 0 deprecation warnings.
- Data: icon theme (5 states + app icon + symbolic, all hicolor sizes), desktop entries (app + tray autostart), AppStream metainfo, Flatpak manifest (draft).

## Roadmap (priority order)

1. ~~Run `cargo test -p jotta-core`; fix failures~~ ✅ 28/28 green
2. ~~Compile tray+GUI on Fedora~~ ✅ Verified by owner
3. ~~Wire GUI handlers (backup, sync, ignores)~~ ✅ Iterations 3+5
4. ~~Icon theme (5 states + symbolic)~~ ✅ Iteration 4
5. ~~Discover real `status --json` schema; tighten parser~~ ✅ Verified against owner's machine
6. ~~Restore/download flow~~ ✅ Iteration 6 (PR #6)
7. **Transfers page** (iteration 7, branch `vibe/iteration7-transfers-errors`): `list uploads --json` + `list downloaderrors` + `list uploaderrors` + live feed via `observe`. Error dedup (one toast per unique error).
8. **Settings/robustness** (iteration 8): expand Indstillinger page — see "Future features" below for details.
9. **Flatpak**: pin jotta-cli version + sha256, get Jottacloud permission to bundle, Flathub submission + AppStream metadata.

### Future features (owner-requested, roadmap additions 2026-06)

- **Launch on system startup** (Indstillinger): toggle for XDG autostart of `jotta-desktop` (in addition to existing `jotta-tray` autostart). Write/remove `~/.config/autostart/jotta-desktop.desktop`.
- **Error notifications**: parse patterns from `jotta-cli status` / `list downloaderrors` / `list uploaderrors` / `sync log`. Show as desktop notifications (via `gtk::gio::Notification` or `notify-send`) + a persistent list in the GUI. Error dedup — one notification per unique error.
- **Info page**: a new page (or section on Status) showing device info, jottad version, jotta-cli version, account type/subscription, last sync time, daemon uptime.
- **Advanced settings** (Nextcloud-style): bandwidth limit (`config set maxuploadrate`/`maxdownloadrate`), scan interval (`config set scaninterval`), realtime mode toggle (`scaninterval 0`), webhook status interval. Read current values via `config get` or `config list`.
- **Account page**: new page or section — account email, subscription plan, usage breakdown, "Log ud" button (`jotta-cli logout` — DESTRUCTIVE, confirm), "Skift bruger" (logout + login), link to jottacloud.com account settings.
- **Page consolidation**: consider merging Status + Account into one "Konto" page, or Status + Info. Evaluate after Transfers page is done. Current 5 pages (Status, Backup, Sync, Gendan, Indstillinger) may become 5-6 with Account/Info split or stay at 5 with consolidation.
- **Tray icon color** (Nextcloud-style): symbolic black/white variant selectable in Indstillinger.

### Later

- i18n (da/en), COPR + AUR packaging, "gendan gammel version" UI (revisions parsed but no UI yet), KSNI menu enhancements (device list, activity).

### Process rules (established during iterations 6f–6o)
- `cargo check -p jotta-gui` MUST be 0 warnings before every push.
- Shadow-clone rule: `let x = x.clone(); move closure` ONLY for the LAST use of a value. One clone per closure, BEFORE move.
- Threading: worker threads own ONLY plain data (String, PathBuf, CliClient, mpsc::Sender). No GTK widgets or Rc<RefCell<…>> cross thread boundaries. Results via mpsc + `glib::idle_add_local`.
- Switch guards: poller NEVER issues commands. Only user action does. Use `Rc<Cell<bool>>` for programmatic-update guards, `Arc<AtomicBool>` for cross-thread lockouts.
- Spawned children: always spawn in a `std::thread` that calls `child.wait()` (reaper pattern) to prevent zombies.
- `std::process::exit(0)` for guaranteed termination (not `app.quit()` which can hang).
- Tray binary lookup: same directory as current binary first, then PATH.
- UI/UX: see "UI/UX design guidelines" section above for layout, icon, and interaction patterns.

## Working agreement with the owner

- Danish-speaking owner; UI strings so far are Danish - keep UI Danish for now, add proper i18n later.
- Discuss architectural changes before making them.
- One iteration = compiles + tests + a short summary of what changed.

## Handover prompt for the next agent

Use this as the opening message when handing the project to a new agent:

> You are taking over development of JottaGUI (https://github.com/geouck/JottaGUI),
> a Rust cross-desktop client for Jottacloud (KDE/GNOME/COSMIC) with a
> zero-dependency core (jotta-core), a ksni tray icon (jotta-tray) and a
> GTK4/libadwaita GUI (jotta-gui). Begin by reading HANDOFF.md and README.md
> in the repo root in full - they contain verified jotta-cli facts, hard
> safety rules, architecture decisions and a prioritized roadmap.
> Your first milestone is roadmap items 1-2: run `cargo test -p jotta-core`
> and build the tray+GUI on the owner's Fedora KDE machine, then wire the GUI
> handlers (roadmap item 3). Never bypass the JottaCommand whitelist, never
> move sync folders, confirm destructive operations, and keep jotta-core
> dependency-free. The owner speaks Danish - keep UI strings Danish for now.
> Report back with what compiles, what tests pass, and what you changed.