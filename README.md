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

## Status: iteration 4

- [x] Workspace + core (15 unit tests green)
- [x] Tray: status tooltip, pause/resume, open web, quit; 3s polling; ksni 0.3
- [x] GUI: live status page (state, email, usage bar, pause/resume),
      backup list + add folder (FileDialog), sync info + start/stop (confirmed)
- [x] Status parser verified against real `jotta-cli status --json` output
- [x] Icon theme: 5 state icons + app icon + symbolic, all hicolor sizes
- [x] Desktop entries + AppStream metainfo
- [ ] GUI: ignore-pattern editor (roadmap item 3 remainder)
- [ ] Restore/download flow (`jotta-cli download`)
- [ ] Flathub manifest finalized + jotta-cli bundling (needs Jottacloud's OK)

## License
MPL-2.0
