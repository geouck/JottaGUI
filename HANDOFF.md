# HANDOFF - JottaGUI

This document is the complete project context for any AI agent (or human)
taking over development. Read it fully before writing code — and also read
`jottagui-produkt-redesign-&-flathub-specifikation-spec-md.md` (the owner's
SPEC.md: Flathub/extra-data packaging strategy, the graphical-login decision
change, the 7→5 tab redesign, the feature-parity matrix in §4, and the
Flathub-beta Definition of Done).

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
- Start/restart daemon: `run_jottad` (wrapper) or `systemctl --user enable --now jottad.service` (Fedora runs jottad as a systemd user unit).
- Sync commands (NOT `sync enable`): `sync setup --root <path>`, `sync start`, `sync stop`, `sync move`, `sync reset`, `sync selective add|rem|list <folder>`, `sync log -n <n>`, `sync trigger`, `sync configure`, `sync diag`. MODES (docs.jottacloud.com, 2026-11): `sync start` = kontinuerlig/aktiv sync (JSON `Sync.Automatic=true`; tekst-status `Mode:` viser fx "manually triggered" når FRA); `sync stop` = stop aktiv sync → manuel tilstand (`Sync.Automatic=false`); `sync trigger` = én manuel sync-kørsel — kun gyldig når aktiv sync er FRA. `Sync.Enabled` = sync-mappe konfigureret (root sat) uafhængigt af mode. GUI: switch = kontinuerlig tilstand (Automatic), "Synkroniser nu"-knap = trigger (aktiv kun i manuel tilstand).
- Sync pause: `config set syncpaused true|false` (per docs.jottacloud.com). The global `pause`/`resume` commands pause jottad entirely (backup + sync).
- Backup commands: `add <path>`, `rem`, `scan`, `status`, `pause`, `resume`, `observe`, `tail`, `share`, `archive`, `trash`, `download`, `list`.
- `pause` REQUIRES a duration argument: `jotta-cli pause 87600h` (10 years = effectively indefinite). `jotta-cli pause` without duration fails: "ERROR You need to provide one or more additional arguments".
- `resume` is idempotent-ish: calling it when already running fails with "ERROR already running" (exit code 1) — treat this as success in the GUI.
- Ignores: `jotta-cli ignores add --pattern PAT [--backup NAME]`, `ignores rem --pattern PAT [--backup NAME]`, `ignores list [--backup NAME]`. Gitignore-style patterns, relative to backup root. `ignores test --pattern P --path X` validates before adding. WARNING: adding a pattern REMOVES already-backed-up matching files (they go to Trash).
- `--json` output exists for: `status`, `ls <path>`, `ignores list`, `sync selective list`, `list downloads`. The schema is NOT formally documented — hence the tolerant parser in jotta-core.
- `ls <path> --json` schemas (verified): folder → `{"Folders": [{"Name","Path","Modified","Size"?}]}` + optional `{"Files": [{"Name","Path","Checksum","Size","Created","Modified"}]}`; file → `{"revisions": [{"id","size","checksum","createdAtMillis","modifiedAtMillis"}]}`. Casing is inconsistent ("Folders" vs "revisions") — parser must be case-tolerant. `Size` is optional on folders.
- `list downloads --json` schema (verified): array of `{"Id","Remote","Local","Total":{"Files","Bytes"},"Remaining":{},"CompletedTimeMs","StartedTimeMs","SelectionCount":{},"Errors":{}}`. Empty objects are common — parser must tolerate.
- `trash list` has NO `--json` flag. Plain output is a single space-separated line (unparseable). Trash UI is web-only.
- `ls Backup --json` lists ALL devices including dead/ghost ones ("Cannot find device") — browser must handle gracefully.
- `download <remote> <dest> [--merge]`: `--merge` is only valid for folder sources (not files). The CLI prompts interactively if the destination file exists — the GUI must check and confirm BEFORE issuing the command (no TTY in spawned process).
- `list` subcommands: `downloads` (has `--json`), `uploads` (has `--json` — VERIFIED, returns `{}` when no uploads), `downloaderrors --downloadid=<id>` (REQUIRES `--downloadid` — no "list all" mode), `uploaderrors --uploadid=<id>` (REQUIRES `--uploadid` — no "list all" mode).
- Transfer errors are derived from the `Errors` object inside each `list downloads --json` / `list uploads --json` entry (not from a separate `list downloaderrors`/`list uploaderrors` call without ID).
- `login` has NO flags (only `-h`/`--help`). Login is 3 interactive prompts: (1) accept T&C, (2) Personal Login Token (from jottacloud.com/web/secure Security tab — single-use, short-lived), (3) device name. Login REQUIRES a running jottad ("Could not connect to jottad" otherwise). DECISION CHANGE (owner-approved 23.10.2026, supersedes the earlier "no token login" rule — SPEC.md §2B): **graphical login is GO**. The GUI will feed the 3 prompts via stdin-pipe or PTY; feeding code lives in jotta-gui ONLY (jotta-core stays zero-dependency); `JottaCommand::Login` with argv `["login"]` is already whitelisted. Token NEVER in argv/logs (argv is readable in /proc/<pid>/cmdline), field is AdwPasswordEntryRow, token held transiently in memory only — jotta-cli owns all credential storage. Prompt-matching verifies expected prompt text before each answer; on mismatch abort and fall back to the existing NeedsLogin dialog (kept as fallback). Implementation waits for the owner's empirical transport test (`printf 'y\n<TOKEN>\n<DEVICE>\n' | jotta-cli login` and `... | flatpak-spawn --host -- jotta-cli login`) run in a VM/container, NEVER on the work laptop (login mutates device credentials). Until implemented, NeedsLogin → dialog with UriLauncher to jottacloud.com/web/secure + terminal instructions + "Prøv igen" retry. EMPIRISK VERIFICERET (ejer-testet på Pop!_OS, jotta-cli 0.17.176206, 2026-11): stdin-piping VIRKER, også gennem `flatpak-spawn --host` — INTET PTY nødvendigt. Prompts skrives til STDOUT uden trailing newline; fejl til STDERR; fejl exit 1. P1 `accept license (yes/no):` → `y`. P2 `Personal login token: ` → token (forkert token → stderr `ERROR ... not parseable ...`, exit 1; token med `<>`-klammer fejler base64 ved byte 0). P3 `Device name [<hostname>]:` → enhedsnavn. PITFALL: ved EOF re-prompter jotta-cli i en UENDELIG løkke, og en frisk buffered reader pr. prompt kan sluge forud-skrevne linjer → fød ÉT svar ad gangen, først når prompt-markøren er set i outputtet (jotta-core::login::detect_prompt). Allerede logget ind → INGEN prompts, stderr `ERROR  Already logged in as <email>`, exit 1 (ikke en fejl for os). `status --json` uden login: exit 1, stderr `ERROR  Not logged in` (→ State::NeedsLogin, PR #14). Iteration 11 (branch vibe/iteration11-login-9475a0) implementerer dette i jotta-gui.
- `status --json` does NOT reliably include a `paused` field. When paused is unknown (`None`), assume "running" (not paused).
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
- **Window controls (CSD)**: GTK4/libadwaita always draws its own headerbar (client-side decorations) — the DE's native titlebar cannot be used. On KDE, window control buttons (minimize/maximize/close) appear larger than on GNOME. CSS overrides via `windowcontrols button { min-width/min-height }` and `headerbar { min-height }` have been tried (iteration 7) but did NOT change button size on KDE — likely cause: KDE's GTK theme integration (kde-gtk-config) or HiDPI scaling. **TODO (debug later)**: investigate `GTK_THEME` env var, `GDK_SCALE`, KDE's `kde-gtk-config` settings, or try `-gtk-icon-size` CSS property on `windowcontrols button image`.

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
- Process model: `jotta-tray` autostarts at login (XDG autostart .desktop) and runs persistently as the always-on background process. `jotta-desktop` (GUI) is launched on demand — from the tray menu ("Åbn vindue"), tray icon click (`activate()`), the app menu, or the .desktop file. The GUI is a GApplication single-instance (application_id `io.github.eske.JottaDesktop`): `connect_activate` checks `app.active_window()` and presents the existing window instead of building a new one.
- Minimize-to-tray (default ON): closing the GUI window hides it (`window.set_visible(false)` + `Propagation::Stop`) instead of terminating. The setting lives in `~/.config/jotta-desktop/settings.conf` (`minimize_to_tray=true|false`). The tray reads the same file: "Afslut" in the tray menu sends SIGTERM to the GUI (via `$XDG_RUNTIME_DIR/jotta-gui.pid`) when minimize-to-tray is active.
- Shutdown: when minimize-to-tray is OFF, `connect_close_request` calls `std::process::exit(0)` directly (not `app.quit()` — that can hang the GApplication main loop).
- Single-instance for tray: PID file at `$XDG_RUNTIME_DIR/jotta-tray.pid`. Both GUI and tray write PID files; tray exits if another live tray is found. GUI's `ensure_tray_running()` spawns tray as a detached child if the PID file is stale/missing.
- Zombie prevention: both `spawn_gui()` (tray) and `ensure_tray_running()` (GUI) spawn children inside a `std::thread` that calls `child.wait()` — acts as reaper, prevents `<defunct>` zombies.
- Tray icon: uses embedded ARGB32 pixmap data (`icon_pixmap()` in ksni) generated from the PNG icons — visible even when the icon theme path is not resolved by the host DE. Source: `crates/jotta-tray/src/pixmaps.rs` (auto-generated, do not hand-edit).
- Tray binary lookup: `find_gui_binary()` looks for `jotta-desktop` next to the tray binary first, then falls back to PATH. Same pattern for `ensure_tray_running()` finding `jotta-tray`.
- Switch guards: `pause_lockout` (AtomicBool, 15s after user click) prevents the poller from reverting the pause switch before jottad updates status. `pause_updating`/`sync_updating` (Rc<Cell<bool>>) prevent feedback loops from programmatic `set_active()`. RULE: only user action issues commands; the poller NEVER issues commands.
- Pause switch truth: `status.paused != Some(true)` → ON (running). `None` (unknown) is treated as "running". Initial switch state is ON.

## Current state (iteration 9 complete, PR #13 merged as e749604)

- jotta-core: 41 unit tests green (incl. 2 Flatpak tests: `host_view`/`host_spawn_prefix`). Zero dependencies. `BinaryLocator::from_path()` now also searches `$JOTTA_CLI`/`$JOTTAD` env overrides, the exe dir, `~/.jottacloud/bin`, `/run/host/usr/local/bin`, `/run/host/usr/bin`, `/usr/local/bin`, `/usr/bin` (jottad falls back to the sibling of jotta-cli). Inside a Flatpak, host binaries are executed ON THE HOST via `flatpak-spawn --host -- <path>` (`host_spawn_prefix()` wraps every `CliClient::run`), and `DaemonSupervisor::ensure_running()` spawns jottad detached on the host (`nohup … &`) — a sandbox-spawned daemon would die with the sandbox; the host systemd daemon is still reused via the socket check and never killed by us. JSON parser, CLI wrapper (`JottaCommand` enum + `argv()` whitelist), daemon supervisor (Model B), tolerant status model (incl. subscription/account_type), remote JSON parsers (`ls --json`, `list downloads --json`, `list uploads --json`), file revisions, download/upload entries, transfer errors (`collect_transfer_errors`).
- jotta-tray: ksni StatusNotifierItem with embedded ARGB32 pixmap icons (5 states × 5 sizes). Menu: status, pause/resume (idempotent), "Åbn vindue", "Åbn jottacloud.com", "Afslut". Single-instance via PID file. Spawns GUI via `find_gui_binary()` + reaper thread. Click on icon (`activate()`) opens GUI. Reads minimize-to-tray setting from `~/.config/jotta-desktop/settings.conf`. **Stays alive when jottad stops** (Offline → Active, not Passive — iteration 8 fix).
- jotta-gui: GTK4/libadwaita main window. GApplication single-instance (`io.github.eske.JottaDesktop`). 7 pages: Status (konto-kort, automatisk backup/sync switch, lagerplads-kort, **daemon-not-responding banner**), Backup (list, "Tilføj mappe", "Udeluk…" chevron, "Fjern" med AlertDialog), Sync (info, start/stop, selective folder dialog), **Overførsler** (aktive uploads/downloads, seneste fejl, 5s poller, error dedup toasts), Gendan/Restore (remote browser "Jottacloud" rod, download dialog, downloads list, papirkurv web-knap), **Info** (enhed, e-mail, abonnement, Jottacloud version, Jotta Desktop version), Indstillinger (minimize-to-tray switch + **start ved login** switch, default ON). Dynamisk topbar titel. Sidebar med symbolic ikoner + app-titel. Ingen H1-overskrifter i sider. 0 deprecation warnings. Kompakt headerbar/windowcontrols via CSS på PRIORITY_USER+1.
- Data: icon theme (5 states + app icon + symbolic, all hicolor sizes incl. 512px + app-id aliases `io.github.eske.JottaDesktop.*` required by flatpak export), desktop entries (app + tray autostart), AppStream metainfo, D-Bus service file.
- Flatpak (`flatpak/`, verified build+install+run on Fedora KDE by owner): manifest `io.github.eske.JottaDesktop.json` on GNOME 50 runtime (FDSK 25.08 + `rust-stable//25.08`), `cargo-sources.json` (262 entries generated from the committed `Cargo.lock`), icons installed under `/app/share/icons/hicolor` (with `mkdir -p` first — cp drops the hicolor level otherwise), desktop file installed as `<app-id>.desktop`, `--device=dri`, `--filesystem=host` + `--filesystem=host-os`, `--talk-name=org.freedesktop.Flatpak` (for flatpak-spawn), OOM-safe build caps (`CARGO_BUILD_JOBS=2`, `CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1`). Remaining Flathub TODOs live in `flatpak/README.md` — most importantly **extra-data instead of host-CLI** (SPEC.md §2A: Flathub rejects host-dependent apps), screenshots, rsvg-convert icon re-render, GNOME/KDE/COSMIC regression tests, and autostart `Exec=flatpak run …` when `/.flatpak-info` exists.

## Roadmap (priority order)

1. ~~Run `cargo test -p jotta-core`; fix failures~~ ✅ 28/28 green
2. ~~Compile tray+GUI on Fedora~~ ✅ Verified by owner
3. ~~Wire GUI handlers (backup, sync, ignores)~~ ✅ Iterations 3+5
4. ~~Icon theme (5 states + symbolic)~~ ✅ Iteration 4
5. ~~Discover real `status --json` schema; tighten parser~~ ✅ Verified against owner's machine
6. ~~Restore/download flow~~ ✅ Iteration 6 (PR #6)
7. ~~**Transfers page**~~ ✅ Iteration 7 (PR #11, branch `vibe/iteration7-transfers-errors`): `list uploads --json` + `list downloads --json` polled every 5s. Error dedup (one toast per unique error). NOTE: `list downloaderrors`/`list uploaderrors` REQUIRE `--downloadid`/`--uploadid` (no "list all" mode) — errors are derived from the `Errors` object inside each download/upload entry via `collect_transfer_errors()`.
8. ~~**Settings/robustness**~~ ✅ Iteration 8 (PR #12, branch `vibe/iteration8-settings-robustness`): launch on startup toggle (XDG autostart), daemon-not-responding banner on Status page, Info page (device, email, subscription, Jottacloud version, Jotta Desktop version). Tray stays alive when jottad stops (Offline → Active, not Passive).
9. ~~**Flatpak**~~ ✅ Iteration 9 (PR #13 merged as e749604, branch `vibe/iteration9-flatpak-9475a0`): builds + installs + runs on Fedora KDE; host `jotta-cli`/`jottad` found via extended `BinaryLocator` and executed on the host via `flatpak-spawn --host`; app-id icons + D-Bus service file; GNOME 50 runtime; OOM-safe build. Host-bundling is NOT the Flathub path — see item 10.
10. **Flathub self-contained via `extra-data`** (SPEC.md §2A — next iteration): Flathub rejects host-dependent apps, so pin the official `jotta-cli`/`jottad` (RPM repo `https://repo.jotta.cloud/redhat`, gpgkey `https://repo.jotta.cloud/jotta.gpg`) with URL + SHA256 as `extra-data`, unpacked to `/app/extra` at install time (Spotify/Discord model), run jottad as a sandbox child (Model B). Also mail support@jottacloud.com re: extra-data/bundling acceptance + logo/brand rules (SPEC.md §5).
11. **Graphical login** — IN PROGRESS, branch `vibe/iteration11-login-9475a0` (empirisk transport-test DONE 2026-11 på Pop!_OS: stdin-pipe virker, også via `flatpak-spawn --host` — intet PTY). own iteration AFTER the owner's empirical transport test in a VM (`printf 'y\n<TOKEN>\n<DEVICE>\n' | jotta-cli login` and the same via `flatpak-spawn --host --` — stdin pipe suffices if it works; a real PTY is only clean after item 10 moves jotta-cli into the sandbox). Prompt state machine with unit tests; feeding code in jotta-gui only (jotta-core zero-dep); `JottaCommand::Login` argv `["login"]` already whitelisted; token never in argv/logs, AdwPasswordEntryRow, transient in memory; prompt-matching with fallback to the existing NeedsLogin dialog; handle "already logged in"; jottad must run first.
12. **UI redesign 7→5 tabs** (SPEC.md §3): Status(Hjem)+live hastighed, Backup+schedule, Sync & Arkiv, Overførsler & Gendan, Indstillinger+Info+Konto+båndbredde.
13. **i18n** (SPEC.md §4: da/en first, post-beta; Danish stays default/fallback): detect via LC_MESSAGES/LANG; translation catalog (gettext .po or fluent/tr!); jotta-core stays zero-dependency — i18n in jotta-gui/jotta-tray only.

### Future features (owner-requested, roadmap additions 2026-06)

- **Launch on system startup** (Indstillinger): toggle for XDG autostart of `jotta-desktop` (in addition to existing `jotta-tray` autostart). Write/remove `~/.config/autostart/jotta-desktop.desktop`.
- **Error notifications**: parse patterns from `jotta-cli status` / `list downloaderrors` / `list uploaderrors` / `sync log`. Show as desktop notifications (via `gtk::gio::Notification` or `notify-send`) + a persistent list in the GUI. Error dedup — one notification per unique error.
- **Info page**: a new page (or section on Status) showing device info, jottad version, jotta-cli version, account type/subscription, last sync time, daemon uptime.
- ~~**Launch on system startup**~~ ✅ Iteration 8 (Indstillinger toggle, writes `~/.config/autostart/jotta-desktop.desktop`)
- ~~**Info page**~~ ✅ Iteration 8 (device, email, subscription, Jottacloud version, Jotta Desktop version)
- **Advanced settings** (Nextcloud-style): bandwidth limit (`config set maxuploadrate`/`maxdownloadrate`), scan interval (`config set scaninterval`), realtime mode toggle (`scaninterval 0`), webhook status interval. Read current values via `config get` or `config list`.
- **Account page**: new page or section — account email, subscription plan, usage breakdown, "Log ud" button (`jotta-cli logout` — DESTRUCTIVE, confirm), "Skift bruger" (logout + login), link to jottacloud.com account settings.
  - **Graphical token login** — DECISION CHANGE owner-approved 23.10.2026 (supersedes the 2026-06 "no token login" rule; SPEC.md §2B): GUI feeds the 3 `jotta-cli login` prompts via stdin-pipe/PTY (see roadmap item 11). Until that lands, the NeedsLogin dialog (UriLauncher to jottacloud.com/web/secure + terminal instructions + retry) stays as the fallback.
- **"Vis tray-ikon" button** (Indstillinger, owner-requested 2026-11): verify `jotta-tray` is running (PID file `$XDG_RUNTIME_DIR/jotta-tray.pid`), respawn it if not; on GNOME show a hint that the tray icon needs the AppIndicator extension when it is not visible.
- **Page consolidation**: consider merging Status + Account into one "Konto" page, or Status + Info. Evaluate after i18n is done. Current 7 pages (Status, Backup, Sync, Overførsler, Gendan, Info, Indstillinger) may become 6-7 with Account split or stay at 7 with consolidation.
- **Tray icon color** (Nextcloud-style): symbolic black/white variant selectable in Indstillinger.

### Later

- **i18n** (iteration 10, owner-requested 2026-06): multilingual support — detect user language via `LC_MESSAGES`/`LANG` env vars. Supported languages: **Norwegian (nb/nn), Swedish (sv), Danish (da), German (de), English (en)**. All UI strings currently hardcoded Danish. jotta-core stays zero-dependency — i18n lives in jotta-gui + jotta-tray only (gettext .po files or Rust `fluent`/`tr!` macro).
- COPR + AUR packaging
- "gendan gammel version" UI (revisions parsed but no UI yet)
- KSNI menu enhancements (device list, activity)
- Advanced settings (Nextcloud-style): bandwidth limit, scan interval, realtime mode
- Account page: subscription plan, usage breakdown, "Log ud" (confirm), link to jottacloud.com
- Page consolidation: evaluate merging Status+Account or Status+Info
- Tray icon color (black/white, Nextcloud-style)
- Snapshot/restore points
- Desktop notifications for errors (in addition to toasts)

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
> in the repo root in full — they contain verified jotta-cli facts, hard
> safety rules, architecture decisions, process rules and a prioritized roadmap.
> Also read `jottagui-produkt-redesign-&-flathub-specifikation-spec-md.md`
> (SPEC.md: Flathub strategy, feature-parity matrix in §4, redesign plan).
> The project is at iteration 9 (complete, PR #13 merged as e749604 — Flatpak
> packaging works: build+install+run verified on Fedora KDE). Next milestone
> is roadmap item 10: Flathub self-contained packaging via extra-data
> (SPEC.md §2A — Flathub rejects host-dependent apps). Then item 11: graphical
> login (owner-approved decision change 23.10.2026; wait for the owner's
> empirical stdin/PTY test result before choosing the transport), item 12:
> 7→5 tab redesign, item 13: i18n (da/en first, Danish stays default).
> Never bypass the JottaCommand whitelist, never move sync folders, confirm
> destructive operations, and keep jotta-core dependency-free.
> `cargo check -p jotta-gui` MUST be 0 warnings before every push.
> The owner speaks Danish — UI strings are Danish until i18n lands (roadmap item 13).
> Report back with what compiles, what tests pass, and what you changed.