# Flatpak packaging for Jotta Desktop

## Strategy

Jotta Desktop is a **thin frontend** for Jottacloud's official `jottad`/`jotta-cli`
daemon. The Flatpak bundles only our own Rust code (`jotta-desktop` + `jotta-tray`).
The user must have `jotta-cli` and `jottad` installed on the host (via Jottacloud's
official installer, COPR, AUR, etc.). The app finds them via `PATH`.

**Why not bundle jotta-cli?** Jottacloud's binaries are proprietary. Bundling them
in a Flatpak requires written redistribution permission (Flathub policy). Until
permission is obtained, the app expects host-installed binaries.

## Files

| File | Purpose |
|------|---------|
| `io.github.eske.JottaDesktop.json` | Flatpak manifest |
| `cargo-sources.json` | Vendored crate URLs + sha256 checksums (45 KB, generated from `Cargo.lock`) |
| `setup-cargo-vendor.py` | Downloads crates at build time and creates a cargo vendor directory |
| `cargo-config.toml` | Cargo config for vendored sources (used when building locally with `cargo vendor`) |

## Building

```bash
# Install flatpak-builder and the Rust SDK extension
flatpak install flathub org.freedesktop.Sdk.Extension.rust-stable

# Build and install
flatpak-builder --user --install --force-clean build-dir flatpak/io.github.eske.JottaDesktop.json

# Run
flatpak run io.github.eske.JottaDesktop
```

## Regenerating cargo-sources.json

When dependencies change (after `cargo update`):

```bash
# Option 1: Use flatpak-cargo-generator (recommended)
pip install flatpak-cargo-generator
flatpak-cargo-generator Cargo.lock -o flatpak/cargo-sources.json

# Option 2: Use the manual script
python3 scripts/generate-cargo-sources.py
```

## Prerequisites on the host

The user needs `jotta-cli` and `jottad` installed:

```bash
# Fedora/RHEL (official installer)
curl -s https://repo.jotta.cloud/install.sh | bash

# Or from COPR/AUR on other distros
```

## Finish-args explained

| Arg | Why |
|-----|-----|
| `--share=network` | jotta-cli talks to Jottacloud API + jottad socket |
| `--share=ipc` | D-Bus for StatusNotifierItem tray |
| `--socket=fallback-x11` / `--socket=wayland` | GUI display |
| `--socket=session-bus` | Session D-Bus (notifications, SNI) |
| `--filesystem=host` | Read/write backup source folders anywhere on the host |
| `--filesystem=xdg-run/jottad` | Access jottad's Unix socket |
| `--filesystem=xdg-config/autostart:ro` | Read autostart entries (launch-on-startup toggle) |
| `--talk-name=org.freedesktop.Notifications` | Desktop notifications |
| `--talk-name=org.kde.StatusNotifierWatcher` | KDE tray icon support |

## TODO before Flathub submission

- [ ] Add screenshots to `data/io.github.eske.JottaDesktop.metainfo.xml`
- [ ] Re-render PNG icons with `rsvg-convert` (currently rendered with PIL)
- [ ] Consider `extra-data` for jotta-cli if Jottacloud grants permission
- [ ] Test on GNOME, KDE Plasma, and COSMIC
