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

## Current state (iteration 1, pushed to this repo)

- jotta-core: compiles clean (`cargo check` verified in a sandbox), JSON parser + CLI wrapper + daemon supervisor + status state machine, with unit tests (NOT yet executed - the authoring sandbox cannot link test binaries; run `cargo test -p jotta-core` on a real machine).
- jotta-tray: MVP (status tooltip, pause/resume, open web, quit; 3s polling).
- jotta-gui: skeleton (sidebar, status/backup/sync pages, TODO markers where handlers go).
- flatpak manifest: draft, jotta-cli source URL needs a pinned version + sha256.

## Roadmap (priority order)

1. Run `cargo test -p jotta-core` on a real machine; fix any failures.
2. Compile tray+GUI on Fedora (`dnf install cargo rust gtk4-devel libadwaita-devel`); the gtk4-rs/libadwaita versions in Cargo.toml may need pinning to what Fedora 42+ ships.
3. Wire GUI handlers: backup list (`BackupsList`), FileChooser -> `BackupAdd`, sync switch -> `SyncStart`/`SyncStop`, ignore-pattern editor (`IgnoreAdd`/`IgnoreRemove`/`IgnoresList`).
4. Icon theme: 5 named icons (jotta-desktop-idle/-syncing/-paused/-attention/-offline) referenced by jotta-tray.
5. Discover the real `status --json` schema on Eske's machine (`jotta-cli status --json | jq .`) and tighten `Status::from_json`.
6. Restore/download flow (`jotta-cli download`).
7. Flatpak: pin jotta-cli version + sha256, get Jottacloud permission to bundle, Flathub submission + AppStream metadata.
8. Later: i18n (da/en), COPR + AUR packaging, KSNI menu enhancements (device list, activity).

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