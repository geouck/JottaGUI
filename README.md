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
```

- **Model B:** the app bundles `jotta-cli`/`jottad` (static Go binaries, no
  system dependencies) and starts the daemon itself - but reuses an existing
  host daemon (systemd user unit) if one is already running.
- The core is deliberately dependency-free: it builds in any sandbox
  (including Flatpak builders) and never fails on system libraries.

## Build and run (Fedora)

``bash
sudo dnf install cargo rust gtk4-devel libadwaita-devel
cargo run -p jotta-gui      # main window
cargo run -p jotta-tray     # tray icon
cargo test  -p jotta-core   # core unit tests
``

If `jotta-cli`/`jottad` are not on PATH they are looked up next to the binary.

## Status: iteration 1

- [x] Workspace + core (cargo check clean, unit tests written)
- [x] Tray MVP: status, pause/resume, web link
- [x] GUI MVP skeleton: sidebar + status/backup/sync pages
- [ ] GUI handlers wired to jotta-core (backup list, restore)
- [ ] Flathub manifest finalized + jotta-cli bundling (needs Jottacloud's OK)
- [ ] Icon theme (5 states: idle/syncing/paused/attention/offline)

## License
MPL-2.0