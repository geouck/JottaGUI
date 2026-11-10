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

### Debian / Ubuntu / Pop!_OS (apt, not dnf)

```bash
# Clone into your home dir — NO sudo needed for git:
cd ~ && git clone https://github.com/geouck/JottaGUI && cd JottaGUI

sudo apt update
sudo apt install -y build-essential pkg-config git curl ca-certificates \
  libgtk-4-dev libadwaita-1-dev libglib2.0-dev

# apt's rustc is often too old for gtk4-rs 0.9 — use rustup instead:
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
rustc --version   # needs >= 1.80

cargo build --release -p jotta-gui -p jotta-tray
cargo check -p jotta-gui    # must be 0 warnings
cargo test  -p jotta-core
# then the same sudo install / cp steps as above
```

### jotta-cli/jottad on Debian/Ubuntu/Pop!_OS

```bash
sudo curl -fsSL https://repo.jotta.cloud/jotta.gpg -o /usr/share/keyrings/jotta.gpg
echo "deb [signed-by=/usr/share/keyrings/jotta.gpg] https://repo.jotta.cloud/debian debian main" \
  | sudo tee /etc/apt/sources.list.d/jotta-cli.list
sudo apt update && sudo apt install -y jotta-cli
systemctl --user enable --now jottad.service
```

If `jotta-cli`/`jottad` are not on PATH they are looked up next to the binary
(and, in a Flatpak, under `/run/host/...` / `~/.jottacloud/bin`).

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

## Status: iteration 13 (i18n da/en) in progress — PR #19 draft; iterations 9–12 merged (PR #13, #15–#18)

Product spec + feature-parity matrix: `jottagui-produkt-redesign-&-flathub-specifikation-spec-md.md` (§4 has the parity matrix).

- [x] Workspace + core (28 unit tests green, zero dependencies)
- [x] Tray: ksni StatusNotifierItem with embedded ARGB32 pixmap icons (always visible),
      menu (status, pause/resume idempotent, "Åbn vindue", "Åbn jottacloud.com", "Afslut"),
      single-instance via PID file, spawns GUI on click/"Åbn vindue"
- [x] GUI: GTK4/libadwaita, GApplication single-instance, 0 deprecation warnings
- [x] GUI pages: Status (live state, email, usage bar, pause switch with 15s lockout),
      Backup (list, add folder, per-backup ignores with scope selector),
      Sync (info, start/stop confirmed, selective folder checkbox dialog),
      Gendan/Restore (remote browser via `ls --json`, download dialog, downloads list
      with abort/retry, file revisions parsed),
      Indstillinger (minimize-to-tray switch, default ON)
- [x] Minimize-to-tray: closing window hides it; tray "Afslut" also closes GUI when active
- [x] Clean shutdown: `std::process::exit(0)` on close (no hanging terminal)
- [x] No zombie processes: reaper threads for all spawned children
- [x] Status parser verified against real `jotta-cli status --json` output
      (paused field unreliable — defaults to "running" when absent)
- [x] Icon theme: 5 state icons + app icon + symbolic, all hicolor sizes
- [x] Desktop entries + AppStream metainfo
- [x] All remote-browsing uses `ls --json` with case-tolerant parsers
      (real fixtures: Size optional, Files array, revisions, ghost devices)
- [x] `list downloads --json` parsed (tolerant of empty objects)
- [x] Trash: web-only UI (`trash list` has no `--json`, output unparseable)

- [x] UI polish (iteration 6o): dynamic topbar title, sidebar with symbolic icons +
      app title, no H1 headings in pages, PreferencesGroup layout, positive switch
      labels, chevron icons on dialog rows, "Fjern" button per backup row with
      AlertDialog, "Overførsler" section, filename-only download titles,
      no double-scroll on backup page
- [x] Transfers page (iteration 7, PR #11): "Overførsler" sidebar entry with
      active uploads, active downloads, recent errors. 5s poller fetches
      `list uploads --json` + `list downloads --json`; errors derived from
      entries via `collect_transfer_errors()`. Error dedup (one toast per
      unique error). Compact headerbar/windowcontrols via CSS (PRIORITY_USER+1)
- [x] Settings/robustness (iteration 8, PR #12): "Start ved login" toggle
      (XDG autostart), daemon-not-responding banner on Status page,
      Info page (device, email, subscription, Jottacloud version,
      Jotta Desktop version). Tray stays alive when jottad stops.
- [x] Flatpak packaging (iteration 9, PR #13 merged as e749604): GNOME 50 runtime,
      `cargo-sources.json` from committed `Cargo.lock`, app-id icons + D-Bus service
      file, `--device=dri`, host `jotta-cli`/`jottad` executed on the host via
      `flatpak-spawn --host` (Flatpak), OOM-safe build caps. Build + install + run
      verified on Fedora KDE. See `flatpak/README.md`.
- [x] Graphical login (iteration 11, PR #15 + compile-fix PR #16): AdwDialog feeds
      `jotta-cli login`'s 3 prompts via stdin pipe (empirically verified, also via
      `flatpak-spawn --host`); token only in transient memory (AdwPasswordEntryRow);
      NeedsLogin row stays as fallback
- [x] Sync page: mode toggle (kontinuerlig vs manuel) + "Synkroniser nu" (PR #17)
- [x] Polish (iteration 12, PR #18): "Log ud" with confirm dialog, "Vis tray-ikon"
      toggle (`tray_visible`), Avanceret (uploadrate/downloadrate/scaninterval via
      `jotta-cli config`), adaptive status poll (3 s active / 10 s idle)

### Next (per SPEC.md)

- [ ] Iteration 10 — Flathub self-contained via `extra-data`: pin official
      `jotta-cli`/`jottad` (repo.jotta.cloud) + SHA256, unpacked to `/app/extra`
      at install time (Flathub rejects host-dependent apps); mail
      support@jottacloud.com re: extra-data/bundling acceptance + logo rules
- [ ] UI redesign 7→5 sidebar tabs (SPEC.md §3)

### i18n — DONE (iteration 13, PR #19 draft; SPEC.md §4)

- [x] Detect system locale (LC_MESSAGES → LC_ALL → LANG); "en*" → English,
      everything else (incl. unset/C/POSIX) → Danish default/fallback — the UI
      follows the desktop language automatically
- [x] Built-in da/en catalog in pure Rust (no gettext dependency, no
      `cargo-sources.json` churn): `crates/jotta-gui/src/i18n.rs` (~140 keys +
      `tf!` macro for positional `{}` placeholders + locale-aware byte
      formatting "1,58 GB"/"1.58 GB") and a small documented copy in
      `crates/jotta-tray/src/i18n.rs` (menu labels + status line)
- [x] jotta-core stays zero-dependency and untouched — the tray mirrors
      `Status::tooltip()` locally in the detected language instead

### Even later

- [ ] COPR + AUR packaging
- [ ] Advanced settings (bandwidth limit, scan interval, realtime mode)
- [ ] Account page (subscription, usage breakdown, "Log ud")
- [ ] Page consolidation (Status+Account, Status+Info)
- [ ] Tray icon color (black/white, Nextcloud-style)
- [ ] "Gendan gammel version" UI (revisions parsed, no UI yet)
- [ ] KSNI menu enhancements (device list, activity, live up/down speed next to
      the status line — owner request after PR #18; pause/resume is already there)
- [ ] Notifications: stack or dynamically replace each other instead of a fixed
      delay (owner observation after PR #18)
- [ ] "Vis tray-ikon" button in Indstillinger (verify/respawn jotta-tray via PID
      file; AppIndicator hint on GNOME)
- [ ] Snapshot/restore points
- [ ] Desktop notifications for errors

## License
MPL-2.0
