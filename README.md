# JottaGUI

A cross-desktop (KDE / GNOME / COSMIC) client for Jottacloud, built in Rust.
The backend is Jottacloud's official `jottad`/`jotta-cli` - the app is a thin
frontend layer that owns the daemon lifecycle (Model B).

See [HANDOFF.md](HANDOFF.md) for the full project context, verified CLI facts and roadmap.

## Architecture

```
crates/
  jotta-core/   Zero dependencies. jottad supervision, typed CLI wrapper,
                JSON parser and tolerant status model. 100% unit-testable.
  jotta-tray/   StatusNotifierItem tray icon (ksni). KDE/COSMIC natively,
                GNOME via the AppIndicator extension.
  jotta-gui/    GTK4/libadwaita main window (status, backup, sync, ignores).
data/
  icons/hicolor/  App + tray icon theme (5 states + app icon + symbolic).
  *.desktop       Desktop entries (app + tray autostart).
  *.metainfo.xml  AppStream metadata.
```

- **Model B:** the app bundles `jotta-cli`/`jottad` (static Go binaries, no
  system dependencies) and starts the daemon itself - but reuses an existing
  host daemon (systemd user unit) if one is already running.
- The core is deliberately dependency-free: it builds in any sandbox
  (including Flatpak builders) and never fails on system libraries.

## Install (native, from source)

```bash
sudo dnf install cargo rust gtk4-devel libadwaita-devel
cargo build --release -p jotta-gui -p jotta-tray

# Binaries
sudo install -Dm755 target/release/jotta-desktop /usr/local/bin/jotta-desktop
sudo install -Dm755 target/release/jotta-tray    /usr/local/bin/jotta-tray

# Desktop entries
sudo install -Dm644 data/jotta-desktop.desktop \
  /usr/share/applications/jotta-desktop.desktop
sudo install -Dm644 data/autostart/jotta-tray.desktop \
  /etc/xdg/autostart/jotta-tray.desktop

# Icons (hicolor theme, all sizes + scalable + symbolic)
sudo cp -r data/icons/hicolor /usr/share/icons/
sudo gtk-update-icon-cache -f -t /usr/share/icons/hicolor
```

If `jotta-cli`/`jottad` are not on PATH they are looked up next to the binary.

## Build and run (dev)

```bash
cargo run -p jotta-gui      # main window
cargo run -p jotta-tray     # tray icon
cargo test  -p jotta-core   # core unit tests
```

## Tray icon states

The tray icon switches between 5 named icons based on daemon status:

| State       | Icon name                  | When                              |
|-------------|----------------------------|-----------------------------------|
| Idle        | `jotta-desktop-idle`       | Logged in, no active transfers    |
| Syncing     | `jotta-desktop-syncing`    | Uploads or downloads in flight    |
| Paused      | `jotta-desktop-paused`     | User paused backup/sync           |
| Needs login | `jotta-desktop-attention`  | Daemon up but not authenticated   |
| Offline     | `jotta-desktop-offline`    | Daemon/socket not reachable       |

## Status: iteration 6

- [x] Workspace + core (25 unit tests green)
- [x] Tray: status tooltip, pause/resume, open web, quit; 3s polling; ksni 0.3
- [x] GUI: live status page (state, email, usage bar, pause/resume),
      backup list + add folder (FileDialog), sync info + start/stop (confirmed)
- [x] Status parser verified against real `jotta-cli status --json` output
- [x] Icon theme: 5 state icons + app icon + symbolic, all hicolor sizes
- [x] Desktop entries + AppStream metainfo
- [x] GUI: sync selective (checkbox dialog, non-destructive) + backup ignores
      (per-backup "Udeluk…" dialog, destructive warning on add)
- [x] Restore/download flow: remote browser (Backup/Archive/Sync via `ls --json`),
      download dialog (FileDialog -> `download`), downloads list
      (`list downloads --json`, abort/retry), file revisions (`ls <file> --json`),
      trash web-only (no list UI — `trash list` has no `--json`)
- [x] All remote-browsing uses `ls --json` with case-tolerant parsers
      (real fixtures: Size optional, casing inconsistent Folders/revisions)
- [x] UI polish (iteration 6o): dynamic topbar title, sidebar with symbolic icons +
      app title, no H1 headings in pages, PreferencesGroup layout, positive switch
      labels, chevron icons on dialog rows, "Fjern" button per backup row with
      AlertDialog, "Overførsler" section, filename-only download titles,
      no double-scroll on backup page
### Next: iteration 7 — Transfers page (branch `vibe/iteration7-transfers-errors`)

- [ ] `list uploads --json` + `list downloaderrors` + `list uploaderrors`
- [ ] Live feed via `observe` (uploads + downloads)
- [ ] Error dedup (one toast per unique error)

### Later: iteration 8 — Settings/robustness & future features

- [ ] **Launch on system startup** toggle (XDG autostart for `jotta-desktop`)
- [ ] **Error notifications**: desktop notifications + persistent list in GUI
      (from `list downloaderrors`, `list uploaderrors`, `sync log`)
- [ ] **Info page**: device info, jottad/jotta-cli version, subscription, daemon uptime
- [ ] **Advanced settings** (Nextcloud-style): bandwidth limit, scan interval,
      realtime mode, webhook interval (`config set`/`config get`)
- [ ] **Account page**: subscription plan, usage breakdown, "Log ud" (confirm),
      link to jottacloud.com account settings
- [ ] **Page consolidation**: evaluate merging Status+Account or Status+Info
- [ ] Error toasts, snapshot/restore points
- [ ] Daemon-not-responding state ("restart jottad via systemctl --user restart jottad")

### Later: iteration 9 — Flatpak

- [ ] Pin jotta-cli version + sha256, get Jottacloud permission to bundle
- [ ] Flathub submission + AppStream metadata finalization

### Even later

- [ ] i18n (da/en)
- [ ] COPR + AUR packaging
- [ ] Tray icon color (black/white, Nextcloud-style)
- [ ] "Gendan gammel version" UI (revisions parsed, no UI yet)
- [ ] KSNI menu enhancements (device list, activity)

## License
MPL-2.0
