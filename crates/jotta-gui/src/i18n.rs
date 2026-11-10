//! Built-in da/en i18n catalog — no external crates (jotta-core stays zero-dependency,
//! and we avoid regenerating flatpak cargo-sources.json for a gettext crate).
//!
//! Locale detection follows the system locale: LC_MESSAGES -> LC_ALL -> LANG.
//! A primary language of "en" selects English; everything else (including unset,
//! "C" and "POSIX") falls back to Danish, which is the default.
//!
//! Keys are stable (e.g. "nav.status") so the later 7->5 tab redesign does not
//! invalidate translations. Unknown keys degrade to showing the key itself.
//!
//! This module is pure std and can be tested standalone:
//!   rustc --edition 2021 --test crates/jotta-gui/src/i18n.rs -o /tmp/i18n_test && /tmp/i18n_test
//!
//! NOTE: jotta-tray has its own small copy of this module (documented duplication —
//! the crates are separate binaries and cannot share code without a new crate).

use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Da,
    En,
}

static LANG: OnceLock<Lang> = OnceLock::new();

/// Map one locale env value ("en_US.UTF-8", "da_DK.UTF-8", "C", ...) to a Lang.
/// Returns None for empty/"C"/"POSIX" so the next env var can be tried.
fn lang_from_locale_value(v: &str) -> Option<Lang> {
    let v = v.trim();
    if v.is_empty() || v == "C" || v == "POSIX" {
        return None;
    }
    // Strip codeset/modifier: "en_US.UTF-8@euro" -> "en_US" -> "en".
    let base = v.split(['.', '@']).next().unwrap_or("");
    let primary = base.split('_').next().unwrap_or("").to_ascii_lowercase();
    match primary.as_str() {
        "en" => Some(Lang::En),
        _ => Some(Lang::Da), // Danish is the default/fallback
    }
}

/// Detect the UI language from the environment (LC_MESSAGES -> LC_ALL -> LANG).
pub fn detect() -> Lang {
    for var in ["LC_MESSAGES", "LC_ALL", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            if let Some(l) = lang_from_locale_value(&v) {
                return l;
            }
        }
    }
    Lang::Da
}

/// The process-wide language, detected once.
pub fn lang() -> Lang {
    *LANG.get_or_init(detect)
}

/// The catalog: key -> (danish, english). Danish strings are the original UI
/// literals; `{}` placeholders are positional and replaced in order by `tf`.
fn catalog(key: &str) -> (&'static str, &'static str) {
    match key {
        // --- Navigation / pages ---
        "nav.status" => ("Status", "Status"),
        "nav.backup" => ("Backup", "Backup"),
        "nav.sync" => ("Sync", "Sync"),
        "nav.transfers" => ("Overførsler", "Transfers"),
        "nav.restore" => ("Gendan", "Restore"),
        "nav.info" => ("Info", "Info"),
        "nav.settings" => ("Indstillinger", "Settings"),
        // --- Common ---
        "common.cancel" => ("Annuller", "Cancel"),
        // --- Status page ---
        "status.state.offline" => ("Jottacloud: offline", "Jottacloud: offline"),
        "status.state.needslogin" => ("Jottacloud: log ind påkrævet", "Jottacloud: sign-in required"),
        "status.state.paused" => ("Jottacloud: pauset", "Jottacloud: paused"),
        "status.state.syncing" => ("Jottacloud: synkroniserer…", "Jottacloud: syncing…"),
        "status.state.idle" => ("Jottacloud: oppe at køre", "Jottacloud: up and running"),
        "status.daemon.banner" => ("Jottacloud-daemonen svarer ikke", "The Jottacloud daemon is not responding"),
        "status.daemon.reload" => ("Genindlæs", "Reload"),
        "status.account.group" => ("Konto", "Account"),
        "status.account.state" => ("Status", "Status"),
        "status.account.email" => ("E-mail", "Email"),
        "status.login.title" => ("Log ind", "Sign in"),
        "status.login.subtitle" => ("Forbind denne enhed med din Jottacloud-konto", "Connect this device to your Jottacloud account"),
        "status.login.button" => ("Log ind…", "Sign in…"),
        "status.logout.title" => ("Log ud", "Sign out"),
        "status.logout.subtitle" => ("Fjerner denne enheds login fra Jottacloud", "Removes this device's login from Jottacloud"),
        "status.logout.button" => ("Log ud", "Sign out"),
        "status.logout.confirm.title" => ("Log ud?", "Sign out?"),
        "status.logout.confirm.body" => (
            "Login fjernes fra denne enhed og backup/sync stopper. Filer i skyen påvirkes ikke. Du kan logge ind igen med et nyt token.",
            "Login is removed from this device and backup/sync stops. Files in the cloud are not affected. You can sign in again with a new token.",
        ),
        "status.logout.toast" => ("Logger ud…", "Signing out…"),
        "status.activity.group" => ("Aktivitet", "Activity"),
        "status.pause.title" => ("Automatisk backup og sync", "Automatic backup and sync"),
        "status.pause.subtitle" => ("Slå fra for midlertidigt at sætte al aktivitet på pause", "Turn off to temporarily pause all activity"),
        "status.storage.group" => ("Lagerplads", "Storage"),
        "status.usage.row" => ("Forbrug", "Usage"),
        "status.usage.of" => ("{} af {} brugt", "{} of {} used"),
        "status.usage.unavailable" => ("Forbrug ikke tilgængeligt", "Usage not available"),
        "status.reload.toast" => ("Genindlæser status…", "Reloading status…"),
        // --- Sync page ---
        "sync.switch.title" => ("Kontinuerlig sync", "Continuous sync"),
        "sync.switch.subtitle" => (
            "ON = synkroniserer løbende mellem dine enheder; OFF = manuel tilstand",
            "ON = continuously syncs between your devices; OFF = manual mode",
        ),
        "sync.path.title" => ("Sync-mappe", "Sync folder"),
        "sync.files.title" => ("Filer i sync", "Files in sync"),
        "sync.mode.title" => ("Tilstand", "Mode"),
        "sync.mode.continuous" => ("Kontinuerlig", "Continuous"),
        "sync.mode.manual" => ("Manuel (trigges med \"Synkroniser nu\")", "Manual (triggered by \"Sync now\")"),
        "sync.none" => ("Ingen sync konfigureret", "No sync configured"),
        "sync.trigger.title" => ("Synkroniser nu", "Sync now"),
        "sync.trigger.subtitle" => ("Kører én sync-kørsel med det samme (manuel tilstand)", "Runs a single sync pass right away (manual mode)"),
        "sync.trigger.button" => ("Synkroniser nu", "Sync now"),
        "sync.trigger.toast" => ("Synkronisering trigget…", "Sync triggered…"),
        "sync.stop.confirm.title" => ("Slå kontinuerlig sync fra?", "Turn off continuous sync?"),
        "sync.stop.confirm.body" => (
            "Sync går over i manuel tilstand: ændringer synkroniseres først igen, når du trykker \"Synkroniser nu\" eller slår kontinuerlig sync til igen. Filer på denne enhed påvirkes ikke.",
            "Sync switches to manual mode: changes are synced again only when you press \"Sync now\" or turn continuous sync back on. Files on this device are not affected.",
        ),
        "sync.stop.confirm.action" => ("Slå fra", "Turn off"),
        "sync.pause.error" => ("Kunne ikke ændre pause: {}", "Could not change pause state: {}"),
        "sync.selective.tooltip" => ("Vælg mapper at synkronisere…", "Choose folders to sync…"),
        "sync.selective.page.title" => ("Mappevalg", "Folder selection"),
        "sync.selective.page.subtitle" => ("Vælg hvilke top-level mapper der skal synkroniseres", "Choose which top-level folders to sync"),
        "sync.selective.dialog.title" => ("Vælg mapper at synkronisere", "Choose folders to sync"),
        "sync.selective.dialog.body" => (
            "Vælg hvilke mapper i {} der skal synkroniseres.\nAfkrydsede mapper synkroniseres. Fjernede afkrydsninger stopper sync for den mappe — filerne slettes ikke hverken lokalt eller i skyen.",
            "Choose which folders in {} to sync.\nChecked folders are synced. Unchecking a folder stops syncing it — the files are not deleted, neither locally nor in the cloud.",
        ),
        "sync.selective.empty.title" => ("Ingen mapper i sync-mappen", "No folders in the sync folder"),
        "sync.selective.empty.subtitle" => ("Tilføj mapper til sync-mappen for at kunne vælge dem her", "Add folders to the sync folder to be able to select them here"),
        "sync.selective.nofolder.toast" => ("Ingen sync-mappe konfigureret", "No sync folder configured"),
        "sync.selective.apply" => ("Anvend", "Apply"),
        "sync.selective.unchanged" => ("Ingen ændringer", "No changes"),
        "sync.selective.updated" => ("{} mappevalg opdateret", "{} folder selections updated"),
        "sync.selective.error" => ("Fejl: {}", "Error: {}"),
        // --- Backup page ---
        "backup.add.button" => ("Tilføj mappe…", "Add folder…"),
        "backup.dialog.title" => ("Vælg mappe til backup", "Choose folder to back up"),
        "backup.dialog.accept" => ("Tilføj", "Add"),
        "backup.added.toast" => ("Backup-mappe tilføjet", "Backup folder added"),
        "backup.empty.title" => ("Ingen backup-mapper endnu", "No backup folders yet"),
        "backup.empty.subtitle" => ("Tryk 'Tilføj mappe' for at begynde", "Press 'Add folder' to get started"),
        "backup.summary" => ("{} · {} filer · {}", "{} · {} files · {}"),
        "backup.summary.files" => ("{} · {} filer", "{} · {} files"),
        "backup.files.count" => ("{} filer, {}", "{} files, {}"),
        "backup.files.only" => ("{} filer", "{} files"),
        "backup.exclude.tooltip" => ("Udeluk filer/mønstre fra denne backup…", "Exclude files/patterns from this backup…"),
        "backup.remove.tooltip" => ("Fjern denne backup-mappe", "Remove this backup folder"),
        "backup.remove.confirm.title" => ("Fjern backup «{}»?", "Remove backup \"{}\"?"),
        "backup.remove.confirm.body" => (
            "Backup-mappen fjernes fra Jottacloud-konfigurationen. Filerne på denne enhed påvirkes ikke, og de fjernes IKKE fra skyen — men de slettes ikke fra andre enheder heller. Vil du fortsætte?",
            "The backup folder is removed from the Jottacloud configuration. Files on this device are not affected, and they are NOT removed from the cloud — nor are they deleted from other devices. Continue?",
        ),
        "backup.remove.action" => ("Fjern backup", "Remove backup"),
        "backup.removed.toast" => ("Backup «{}» fjernet", "Backup \"{}\" removed"),
        // --- Backup ignore (exclusion patterns) ---
        "backup.ignore.dialog.title" => ("Udeluk fra backup: {}", "Exclude from backup: {}"),
        "backup.ignore.scope.label" => ("Anvend på", "Apply to"),
        "backup.ignore.scope.subtitle" => ("Vælg hvilke backups mønsteret gælder for", "Choose which backups the pattern applies to"),
        "backup.ignore.scope.this" => ("Denne backup", "This backup"),
        "backup.ignore.scope.all" => ("Alle backups", "All backups"),
        "backup.ignore.warning" => (
            "Advarsel: Tilføjelse af et mønster fjerner allerede backup'ede filer der matcher mønsteret — de flyttes til papirkurven i Jottacloud.",
            "Warning: Adding a pattern removes already backed-up files matching the pattern — they are moved to the trash in Jottacloud.",
        ),
        "backup.ignore.new.title" => ("Nyt mønster", "New pattern"),
        "backup.ignore.patterns.title" => ("Mønstre", "Patterns"),
        "backup.ignore.patterns.desc" => (
            "Gitignore-syntaks, relativ til backup-roden (stien nedenfor). Eksempler: **.tmp, node_modules/, *.log",
            "Gitignore syntax, relative to the backup root (path below). Examples: **.tmp, node_modules/, *.log",
        ),
        "backup.ignore.root" => ("Backup-rod: {}", "Backup root: {}"),
        "backup.ignore.add.confirm.title" => ("Tilføj udelukkelsesmønster?", "Add exclusion pattern?"),
        "backup.ignore.add.confirm.body" => (
            "Mønsteret `{}` tilføjes{}. Matcher mønsteret filer der allerede er backup'et, flyttes de til papirkurven i Jottacloud.",
            "The pattern `{}` will be added{}. If it matches files that are already backed up, they are moved to the trash in Jottacloud.",
        ),
        "backup.ignore.suffix.backup" => (" for backup'en `{}`", " for backup `{}`"),
        "backup.ignore.suffix.all" => (" for ALLE backups", " for ALL backups"),
        "backup.ignore.add.action" => ("Tilføj mønster", "Add pattern"),
        "backup.ignore.added.toast" => ("Mønster tilføjet: {}", "Pattern added: {}"),
        "backup.ignore.add.error" => ("Kunne ikke tilføje mønster: {}", "Could not add pattern: {}"),
        "backup.ignore.close" => ("Luk", "Close"),
        "backup.ignore.empty.title" => ("Ingen udelukkelsesmønstre", "No exclusion patterns"),
        "backup.ignore.empty.subtitle" => ("Tilføj et mønster ovenfor", "Add a pattern above"),
        "backup.ignore.remove.tooltip" => ("Fjern mønster", "Remove pattern"),
        "backup.ignore.remove.confirm.title" => ("Fjern udelukkelsesmønster?", "Remove exclusion pattern?"),
        "backup.ignore.remove.confirm.body" => (
            "Mønsteret `{}` fjernes for backup'en `{}`. Filer der matcher igen bliver backet up ved næste scan.",
            "The pattern `{}` is removed from backup `{}`. Files matching it again will be backed up at the next scan.",
        ),
        "backup.ignore.remove.action" => ("Fjern", "Remove"),
        "backup.ignore.removed.toast" => ("Mønster fjernet", "Pattern removed"),
        // --- Restore (remote browser) ---
        "restore.kind.folder" => ("Mappe", "Folder"),
        "restore.kind.offline" => ("Offline enhed", "Offline device"),
        "restore.folder.desc" => ("Mappe — {}", "Folder — {}"),
        "restore.folder.open.tooltip" => ("Mappe — dobbeltklik for at åbne", "Folder — double-click to open"),
        "restore.file.size" => ("Fil — {}", "File — {}"),
        "restore.file.plain" => ("Fil", "File"),
        "restore.browser.title" => ("Fjernfiler", "Remote files"),
        "restore.up.tooltip" => ("En mappe op", "One folder up"),
        "restore.refresh.tooltip" => ("Genindlæs", "Reload"),
        "restore.download.button" => ("Download valgte…", "Download selected…"),
        "restore.download.dialog.title" => ("Download", "Download"),
        "restore.download.dialog.subtitle" => ("Hent valgte filer eller mapper til en lokal mappe", "Download the selected files or folders to a local folder"),
        "restore.select.first" => ("Vælg en fil eller mappe først", "Select a file or folder first"),
        "restore.dest.dialog.title" => ("Vælg download-destination", "Choose download destination"),
        "restore.dest.accept" => ("Vælg", "Choose"),
        "restore.replace.confirm.title" => ("Erstat eksisterende fil?", "Replace existing file?"),
        "restore.replace.action" => ("Erstat", "Replace"),
        "restore.trash.tooltip" => ("Åbn papirkurv på jottacloud.com…", "Open the trash on jottacloud.com…"),
        "restore.trash.title" => ("Papirkurv", "Trash"),
        "restore.trash.subtitle" => ("Gendan eller slet filer permanent på jottacloud.com (åbner i browser)", "Restore or permanently delete files on jottacloud.com (opens in browser)"),
        "restore.devices" => ("Enheder med backup", "Devices with backup"),
        "restore.archived" => ("Arkiverede filer", "Archived files"),
        "restore.synced" => ("Synkroniseret mappe", "Synced folder"),
        "restore.empty.title" => ("(tom)", "(empty)"),
        "restore.empty.subtitle" => ("Denne mappe er tom", "This folder is empty"),
        "restore.no.revisions.title" => ("Ingen revisioner", "No revisions"),
        "restore.no.revisions.subtitle" => ("Filen har ingen tidligere versioner", "The file has no previous versions"),
        "restore.revision.size" => ("Revision {} — {}", "Revision {} — {}"),
        "restore.revision" => ("Revision {}", "Revision {}"),
        "restore.version" => ("Version {}", "Version {}"),
        "restore.empty.list.subtitle" => ("Intet at vise", "Nothing to show"),
        "restore.load.error" => ("Kunne ikke indlæse", "Could not load"),
        "restore.no.transfers.title" => ("Ingen overførsler", "No transfers"),
        "restore.no.transfers.subtitle" => ("Downloads og uploads vises her", "Downloads and uploads appear here"),
        "restore.abort.tooltip" => ("Annuller eller fjern fra liste", "Cancel or remove from list"),
        "restore.retry.tooltip" => ("Prøv igen", "Retry"),
        // --- Transfers page ---
        "transfers.uploads.title" => ("Aktive uploads", "Active uploads"),
        "transfers.uploads.empty.title" => ("Ingen aktive uploads", "No active uploads"),
        "transfers.uploads.empty.subtitle" => ("Uploads vises her når de kører", "Uploads appear here while they run"),
        "transfers.downloads.title" => ("Aktive downloads", "Active downloads"),
        "transfers.downloads.empty.title" => ("Ingen aktive downloads", "No active downloads"),
        "transfers.downloads.empty.subtitle" => ("Downloads vises her når de kører", "Downloads appear here while they run"),
        "transfers.errors.title" => ("Seneste fejl", "Recent errors"),
        "transfers.errors.empty.title" => ("Ingen fejl", "No errors"),
        "transfers.errors.empty.subtitle" => ("Overførselsfejl vises her", "Transfer errors appear here"),
        "transfer.status.uploading" => ("Uploader…", "Uploading…"),
        "transfer.status.downloading" => ("Kører…", "Downloading…"),
        "transfer.status.failed" => ("Fejlet", "Failed"),
        "transfer.status.done" => ("Færdig", "Done"),
        "transfer.error.toast" => ("Overførselsfejl: {} — {}", "Transfer error: {} — {}"),
        "transfer.unknown.path" => ("(ukendt sti)", "(unknown path)"),
        "download.started.toast" => ("Download startet: {}", "Download started: {}"),
        "download.start.error" => ("Kunne ikke starte download: {}", "Could not start download: {}"),
        // --- Info page ---
        "info.device" => ("Enhed", "Device"),
        "info.subscription" => ("Abonnement", "Subscription"),
        "info.jotta.version" => ("Jottacloud version", "Jottacloud version"),
        "info.app.version" => ("Jotta Desktop version", "Jotta Desktop version"),
        "info.system.group" => ("System", "System"),
        // --- Settings page ---
        "settings.general.group" => ("Generelt", "General"),
        "settings.mtt.title" => ("Minimer til systembakke ved luk", "Minimize to system tray on close"),
        "settings.mtt.subtitle" => ("Vinduet minimeres til systembakken i stedet for at lukke", "The window minimizes to the system tray instead of closing"),
        "settings.autostart.title" => ("Start ved login", "Start at login"),
        "settings.autostart.subtitle" => ("Åbn Jotta Desktop automatisk når du logger ind", "Open Jotta Desktop automatically when you sign in"),
        "settings.tray.title" => ("Tray-ikon", "Tray icon"),
        "settings.tray.subtitle" => ("Vis ikonet i systembakken — slå fra for at skjule det", "Show the icon in the system tray — turn off to hide it"),
        "settings.advanced.group" => ("Avanceret", "Advanced"),
        "settings.advanced.desc" => ("jottad-indstillinger — tomme felter ændres ikke ved gem", "jottad settings — empty fields are left unchanged when saving"),
        "settings.bandwidth.up" => ("Båndbredde op (uploadrate)", "Bandwidth up (uploadrate)"),
        "settings.bandwidth.down" => ("Båndbredde ned (downloadrate)", "Bandwidth down (downloadrate)"),
        "settings.scaninterval" => ("Scan-interval (scaninterval — 0 = realtime)", "Scan interval (scaninterval — 0 = realtime)"),
        "settings.save.advanced.title" => ("Gem avancerede indstillinger", "Save advanced settings"),
        "settings.save.button" => ("Gem", "Save"),
        "settings.saved.toast" => ("Avancerede indstillinger gemt", "Advanced settings saved"),
        "settings.save.error.toast" => ("Nogle indstillinger kunne ikke gemmes (se log)", "Some settings could not be saved (see log)"),
        "tray.already.running" => ("Tray-ikonet kører allerede (pid {})", "Tray icon already running (pid {})"),
        "tray.hidden.toast" => ("Tray-ikon skjult", "Tray icon hidden"),
        "tray.already.hidden.toast" => ("Tray-ikon var allerede skjult", "Tray icon was already hidden"),
        "tray.restarted.toast" => ("jotta-tray startet igen ({})", "jotta-tray restarted ({})"),
        "tray.start.error" => ("Kunne ikke starte jotta-tray: {}", "Could not start jotta-tray: {}"),
        "tray.not.found.toast" => ("jotta-tray-binaren blev ikke fundet (tjek PATH)", "The jotta-tray binary was not found (check PATH)"),
        // --- Login dialog ---
        "login.dialog.title" => ("Log ind på Jottacloud", "Sign in to Jottacloud"),
        "login.button" => ("Log ind", "Sign in"),
        "login.info" => (
            "Log ind med et personligt login-token. Generer tokenet under Security-fanen på jottacloud.com — tokenet er engangs og vises kun én gang.",
            "Sign in with a personal login token. Generate the token in the Security tab on jottacloud.com — the token is one-time and is only shown once.",
        ),
        "login.terms.group" => ("Vilkår", "Terms"),
        "login.terms.accept" => ("Jeg accepterer Jottaclouds betingelser", "I accept Jottacloud's terms of service"),
        "login.token.group" => ("Login-token", "Login token"),
        "login.token.desc" => ("Personligt login-token — engangs, genereres på jottacloud.com", "Personal login token — one-time, generated on jottacloud.com"),
        "login.token.placeholder" => ("Personligt login-token", "Personal login token"),
        "login.secure.row" => ("Generer token", "Generate token"),
        "login.device.group" => ("Denne enhed", "This device"),
        "login.device.row" => ("Enhedsnavn", "Device name"),
        "login.contacting" => ("Kontakter jottad og logger ind…", "Contacting jottad and signing in…"),
        "login.step.license" => ("Accepterer vilkår…", "Accepting terms…"),
        "login.step.token" => ("Sender login-token…", "Sending login token…"),
        "login.step.device" => ("Registrerer enhed…", "Registering device…"),
        "login.done.toast" => ("Logget ind på Jottacloud", "Signed in to Jottacloud"),
        "login.already.toast" => ("Allerede logget ind — opdaterer status", "Already signed in — refreshing status"),
        "login.failed" => ("Login fejlede: {}", "Login failed: {}"),
        "login.timeout" => ("Login tog over 90 sekunder (timeout)", "Login took over 90 seconds (timeout)"),
        "login.wait.error" => ("kunne ikke vente på jotta-cli login", "could not wait for jotta-cli login"),
        "login.exit.error" => ("jotta-cli login afsluttede med kode {}", "jotta-cli login exited with code {}"),
        // Unknown key: leak so the key itself shows in the UI (visible bug, no silent wrong text).
        _ => {
            let leaked: &'static str = Box::leak(key.to_string().into_boxed_str());
            (leaked, leaked)
        }
    }
}

/// Look up a translation for the current language.
pub fn t(key: &str) -> &'static str {
    let (da, en) = catalog(key);
    match lang() {
        Lang::Da => da,
        Lang::En => en,
    }
}

fn tf_lang(key: &str, l: Lang, args: &[&dyn std::fmt::Display]) -> String {
    let (da, en) = catalog(key);
    let mut out = match l {
        Lang::Da => da,
        Lang::En => en,
    }
    .to_string();
    for a in args {
        out = out.replacen("{}", &a.to_string(), 1);
    }
    out
}

/// Translate a key with positional `{}` placeholders, e.g. `tf("status.usage.of", &[&used, &quota])`.
pub fn tf(key: &str, args: &[&dyn std::fmt::Display]) -> String {
    tf_lang(key, lang(), args)
}

/// Convenience macro so call sites read like `format!`:
/// `tf!("backup.removed.toast", backup_name)` — args must impl Display.
#[macro_export]
macro_rules! tf {
    ($key:expr $(, $arg:expr)* $(,)?) => {
        $crate::i18n::tf($key, &[$(&($arg) as &dyn ::std::fmt::Display),*])
    };
}

fn format_bytes_lang(n: u64, l: Lang) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut val = n as f64;
    let mut unit = 0;
    while val >= 1000.0 && unit < UNITS.len() - 1 {
        val /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        let rounded = format!("{val:.2}");
        // Danish uses comma as decimal separator, English a point.
        let sep = match l {
            Lang::Da => ",",
            Lang::En => ".",
        };
        format!("{} {}", rounded.replace('.', sep), UNITS[unit])
    }
}

/// Locale-aware byte formatting ("1,58 GB" in Danish, "1.58 GB" in English).
/// Mirrors jotta_core::status::Status::format_bytes but with a locale separator
/// (jotta-core itself stays untouched).
pub fn format_bytes(n: u64) -> String {
    format_bytes_lang(n, lang())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_value_mapping() {
        assert_eq!(lang_from_locale_value("en_US.UTF-8"), Some(Lang::En));
        assert_eq!(lang_from_locale_value("en_GB"), Some(Lang::En));
        assert_eq!(lang_from_locale_value("en"), Some(Lang::En));
        assert_eq!(lang_from_locale_value("da_DK.UTF-8"), Some(Lang::Da));
        assert_eq!(lang_from_locale_value("de_DE.UTF-8"), Some(Lang::Da)); // fallback = Danish
        assert_eq!(lang_from_locale_value("nb_NO.UTF-8"), Some(Lang::Da));
        assert_eq!(lang_from_locale_value("C"), None);
        assert_eq!(lang_from_locale_value("POSIX"), None);
        assert_eq!(lang_from_locale_value(""), None);
        assert_eq!(lang_from_locale_value("  "), None);
    }

    #[test]
    fn catalog_known_keys_nonempty() {
        for key in ["nav.status", "status.state.idle", "login.dialog.title", "backup.remove.confirm.body"] {
            let (da, en) = catalog(key);
            assert!(!da.is_empty() && !en.is_empty(), "empty translation for {key}");
        }
    }

    #[test]
    fn unknown_key_falls_back_to_key() {
        assert_eq!(catalog("no.such.key"), ("no.such.key", "no.such.key"));
    }

    #[test]
    fn tf_replaces_positional_placeholders_in_order() {
        let used = 35u64;
        let quota = 4550u64;
        let da = tf_lang("status.usage.of", Lang::Da, &[&used, &quota]);
        let en = tf_lang("status.usage.of", Lang::En, &[&used, &quota]);
        assert_eq!(da, "35 af 4550 brugt");
        assert_eq!(en, "35 of 4550 used");
    }

    #[test]
    fn tf_handles_no_args_and_extra_braces() {
        assert_eq!(tf_lang("common.cancel", Lang::En, &[]), "Cancel");
        let pid = 129867u32;
        assert_eq!(
            tf_lang("tray.already.running", Lang::En, &[&pid]),
            "Tray icon already running (pid 129867)"
        );
    }

    #[test]
    fn format_bytes_uses_locale_separator() {
        assert_eq!(format_bytes_lang(1580000000, Lang::Da), "1,58 GB");
        assert_eq!(format_bytes_lang(1580000000, Lang::En), "1.58 GB");
        assert_eq!(format_bytes_lang(512, Lang::Da), "512 B");
        assert_eq!(format_bytes_lang(512, Lang::En), "512 B");
    }

    #[test]
    fn danish_strings_keep_original_placeholders() {
        // The Danish text must remain usable with positional {} replacement.
        let (da, _) = catalog("sync.mode.manual");
        assert!(da.contains("Synkroniser nu"));
        let (da, _) = catalog("backup.remove.confirm.title");
        assert_eq!(da.matches("{}").count(), 1);
    }
}
