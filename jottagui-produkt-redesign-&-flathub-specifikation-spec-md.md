# JottaGUI – Produkt-, Redesign- &amp; Flathub-specifikation (`SPEC.md`)

**Baseline:** `main` branch pr. 5. november 2026, commit `e749604` (iteration 1–9 gennemført, PR #1–#13 squash-merged — Flatpak-pakning merged 5.11.2026). Alle "implementeret"-punkter er verificeret mod koden.

> **Versionsnote:** Dette er den korrigerede udgave. Rettelser vs. tidligere udkast: repo-domæne (`repo.jotta.cloud`, ikke `repo.jotta.us`), Flathub-eksempler (Spotify/Discord — Steam er ikke på Flathub), PTY-login er godkendt som beslutningsændring (ejer, 23.10.2026), 16 implementerede features tilføjet, i18n-sektion fuldført.

---

## 1. Strategisk mål: "Flathub-Ready &amp; Officiel Anerkendelse"

Målet med næste fase af **JottaGUI** er at løfte applikationen fra en funktionel `jotta-cli`-wrapper til en komplet, selvkørende desktop-klient, som:

1. Kan installeres med ét klik fra **Flathub** (uden krav om at brugeren manuelt installerer `jotta-cli` på host-systemet eller skriver i en terminal).
2. Har så høj UX-kvalitet og visuel sammenhængskraft, at Jottacloud vil give tilladelse til binær bundling (eller henvise Linux-brugere direkte til Flathub-pakken), eller eventuelt tillade brug af officiel logo.

---

## 2. Flathub &amp; Pakke-strategi

### A. `extra-data` i Flatpak (indtil evt. direkte bundling-tilladelse)

Indtil Jottacloud giver skriftlig tilladelse til at redistribuere `jottad` og `jotta-cli` direkte i Flatpak-repoet, løses pakningen 100 % lovligt på Flathub via Flatpaks **`extra-data`**-mekanisme (samme model som **Spotify** `com.spotify.Client` og **Discord** `com.discordapp.Discord` bruger — Steam er *ikke* på Flathub):

- Flatpak-manifestet specificerer den officielle URL og SHA256-checksum til Jottaclouds officielle pakke på **`repo.jotta.cloud`** (RPM-repo: `https://repo.jotta.cloud/redhat`, gpgkey: `https://repo.jotta.cloud/jotta.gpg`).
- De proprietære binære filer (`jottad`, `jotta-cli`) ligger **ikke** på Flathubs servere, men downloades automatisk på brugerens maskine under `flatpak install` og udpakkes i `/app/extra/bin/`.
- **Daemon-supervision i sandbox (Model B, allerede implementeret i jotta-core):** JottaGUI starter selv `/app/extra/bin/jottad` som child-proces inde i sandboxen, hvis der ikke allerede kører en host-daemon — `stop_if_owned()` dræber kun egne children. Det eliminerer behovet for root/systemd-opsætning på værten.
- **Bemærk:** Flathub afviser "host-dependent applications" — Flatpaken må altså *ikke* forudsætte host-installeret `jotta-cli`. **Det er netop den tilgang PR #13 (merged 5.11.2026) tog:** appen finder host-binaries via `BinaryLocator::from_path()` og kører dem på værten via `flatpak-spawn --host`. Extra-data (evt. sandbox-Model B) er derfor ikke bare et krav for Flathub-submission, men en **ændring af main**, der skal implementeres som næste iteration.

### B. Grafisk login: nuværende løsning og PTY-udvidelse

**Nuværende tilstand (implementeret i PR #11):** Ved `NeedsLogin`-status viser GUI'en allerede en **dialog** med instruktioner om at køre `jotta-cli login` i en terminal (login er 3 interaktive TTY-prompts uden flags; token genereres på jottacloud.com/web/secure og er **engangs og kortlivet**). `JottaCommand`-whitelisten berøres ikke.

**Problem på Flathub:** At skulle åbne en terminal og køre `flatpak run --command=jotta-cli … login` ved første opstart er en showstopper for almindelige brugere.

**Teknisk løsning (Fase 2 — ✅ beslutningsændring ejer-godkendt 23.10.2026):** En pseudo-terminal (`rustix::pty`/`libc::openpty`) — eller blot et stdin-pipe, hvis det viser sig at være nok — besvarer de 3 prompts:

1. Prompt 1 (acceptér vilkår): send `"y\n"` når brugeren har vinget "Jeg accepterer Jottaclouds betingelser" af.
2. Prompt 2 (Personal login token): send `<token>\n` fra et `AdwPasswordEntryRow`-felt (med direkte knap til jottacloud.com/web/secure).
3. Prompt 3 (enhedsnavn): send `<valgt_navn>\n` (pre-udfyldt med `hostname`).

> ✅ **Beslutningsændring godkendt af ejer (23.10.2026)** og erstatter PR #11-beslutningen ("ingen token-login, ingen secret-håndtering"). Begrundelse: tokenen er engangs/kortlivet og holdes kun transient i hukommelsen — ingen persistent credential-håndtering i GUI'en; `jotta-cli` ejer stadig al lagring. PTY-løsningen er desuden *sikrere* end et hypotetisk `--token`-flag (argv er læsbart i `/proc/<pid>/cmdline`).
>
> **Implementeringsregler (ufravigelige):**
>
> - PTY/stdin-koden ligger i **jotta-gui** — `jotta-core` forbliver **zero-dependency** (ingen rustix/libc derinde).
> - `JottaCommand::Login` tilføjes til whitelisten (argv: `["login"]`) — ingen raw strings.
> - **Prompt-matching:** læs output og verificér forventet prompt-tekst før hvert svar; afbryd ved mismatch → fald tilbage til den eksisterende `NeedsLogin`-dialog (som beholdes som fallback, ikke erstattes).
> - Token **aldrig** i argv, **aldrig** i logs/fejlbeskeder; feltet er `AdwPasswordEntryRow` (maskeret).
> - **Empirisk test før implementering (i VM/container, aldrig på ejerens arbejdslaptop — login roder med eksisterende device-credentials):** 1) `printf 'y\n<TOKEN>\n<ENHED>\n' | jotta-cli login` — virker stdin-pipe, behøves PTY slet ikke; 2) ellers PTY-test; 3) først derefter implementering som egen iteration med unit-tests af prompt-state-machine.
> - Rækkefølge runtime: `jottad` skal køre FØR login ("Could not connect to jottad") — i Flatpak skal Model B have startet sin egen daemon først. `Already logged in`-tilstanden håndteres graceful.
> - **⚠️ Opdatering efter PR #13 (merged 5.11.2026):** main kører `jotta-cli` på **værten** via `flatpak-spawn --host`. Det betyder: (a) login-credentials lander i **host-config** (`~/.config/jottacloud`) — præcis som ved terminal-login, ikke i sandbox-hjemmet; (b) `flatpak-spawn` videresender stdin/stdout, men **allokerer ingen TTY** — så den empiriske test skal også køres som `printf 'y\n<TOKEN>\n<ENHED>\n' | flatpak-spawn --host -- jotta-cli login`. Virker det, virker den grafiske løsning direkte oven på main i dag; kræver CLI'en ægte TTY, er PTY kun uproblematisk, når extra-data/Model B-flytningen (afsnit 2A) er gennemført og `jotta-cli` kører som child inde i sandboxen.

---

## 3. UI/UX Redesign: Konsolidering fra 7 til 5 kerne-faner

I `main` har JottaGUI **7 faner** i sidebaren (`Status`, `Backup`, `Sync`, `Overførsler`, `Gendan`, `Info`, `Indstillinger`). Sammenlignet med den officielle klients slanke top-navigation (`Home`, `Files` \[Backups / Sync / Archive\], `Preferences`) virker 7 punkter i sidebaren lidt fragmenteret.

### Forslag til konsolideret sidebar-arkitektur (5 stærke sider)


| Ny Sidebar-struktur                                             | Hvad konsolideres her?                                                                              | Begrundelse                                                                                                                                                                                                               |
| :--------------------------------------------------------------- | :--------------------------------------------------------------------------------------------------- | :------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **1. Status (Hjem)**                                            | Nuværende `Status` + **Live hastighed** + **Seneste aktivitet** (fra *Overførsler*).                | Gør forsiden til et ægte dashboard ligesom `Home` i den officielle klient.                                                                                                                                                |
| **2. Backup**                                                   | Nuværende `Backup` + **Scannings-interval (`Schedule`)** + **Empty State onboarding**.              | Samler alt vedrørende enhedens sikkerhedskopiering ét sted.                                                                                                                                                               |
| **3. Sync &amp; Arkiv**                                         | Nuværende `Sync` + **Åbn mappe / Trigger nu** + simpel **Archive &amp; Share** drop-zone.           | Giver adgang til `jotta-cli archive --share` uden at kræve en 8. fane.                                                                                                                                                    |
| **4. Overførsler &amp; Gendan** (eller beholdes som 2 separate) | Nuværende `Overførsler` (uploads, downloads, fejl) og `Gendan` (remote browser + revisions-vælger). | Ved sammensmeltning: 5 faner; alternativt 6 faner med badge-tæller i sidebaren ved aktive jobs/fejl.                                                                                                                      |
| **5. Indstillinger**                                            | Nuværende `Indstillinger` + **`Info`-siden** + **Konto (Log ud)** + **Båndbredde &amp; Avanceret**. | `Info` (5 statiske rækker) og `Indstillinger` (2 switches) er hver især for små. Én `AdwPreferencesPage` med sektionerne *Konto &amp; Enhed*, *Opførsel*, *Båndbredde* og *Om / Versioner* føles komplet og professionel. |


---

## 4. Feature-paritet Matrix (Status pr. PR #12, verificeret mod `main`)


| Feature-område                | Status i `main` (PR #1–#12)                                                                                                                                                                                                                                                                                                                                                    | Mangler vs. officiel klient / `jotta-cli`                                                                                                              | Prioritet           |
| :----------------------------- | :------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | :------------------------------------------------------------------------------------------------------------------------------------------------------ | :------------------- |
| **Layout &amp; Libadwaita**   | ✅ Dynamisk headerbar-titel, ikon-sidebar, `AdwPreferencesGroup`, ingen H1-dubletter, én scrollbar pr. side (PR #10). ✅ Kompakte window controls via CSS-provider der vinder over KDE's `kde-gtk-config`. ✅ ToastOverlay-feedback på alle CLI-handlinger. ✅ Empty states i remote browser.                                                                                      | Konsolidering af korte faner (`Info` + `Indstillinger`); live netværkshastighed (`↓/↑ MB/s`).                                                          | **Høj**             |
| **Status &amp; Daemon**       | ✅ Konto-kort, positiv pause-switch **med 15s lockout** (poller revertér ikke switchen), lagerbar (`LevelBar`), `AdwBanner` ved offline daemon med Genindlæs (PR #12). ✅ `paused=None → "running"`-tolerance; parser verificeret mod reel `status --json`.                                                                                                                      | `Log ud`-knap (`jotta-cli logout`); live transfer-speed når ikke idle.                                                                                 | **Høj**             |
| **Tray (jotta-tray, ksni)**   | ✅ 5-state ikon med embedded ARGB32-pixmaps (altid synligt). ✅ Menu: status, pause/resume (idempotent), "Åbn vindue", "Åbn jottacloud.com", "Afslut". ✅ Klik på ikon åbner GUI; single-instance via PID-fil; XDG-autostart; minimer-til-tray: vinduesluk skjuler, "Afslut" lukker GUI via SIGTERM.                                                                              | Tray-farvevalg (sort/hvid, Nextcloud-stil); KSNI-menu med enhedsliste/aktivitet.                                                                       | **Lav (Polish)**    |
| **Backup**                    | ✅ Mappeliste, `Tilføj mappe`, `Udeluk…`-editor med scope-vælger (PR #5), `Fjern` med `AdwAlertDialog` (PR #10).                                                                                                                                                                                                                                                                | **Backup Schedule** (`scaninterval`), `Scan nu`-knap (`jotta-cli scan`), pæn `AdwStatusPage` ved 0 mapper.                                             | **Høj**             |
| **Sync**                      | ✅ Info, start/stop med bekræftelse, selektiv mappe-dialog (`sync selective`, PR #5).                                                                                                                                                                                                                                                                                           | **`Åbn lokal Sync-mappe`** (`xdg-open`) og **`Synkroniser nu`** (`sync trigger`).                                                                      | **Middel**          |
| **Overførsler**               | ✅ Dedikeret fane: *Aktive uploads*, *Aktive downloads*, *Seneste fejl* (5s-polling, toast-dedup pr. unik `path+message`, PR #11).                                                                                                                                                                                                                                              | Badge-indikator i sidebaren ved aktive overførsler/ulæste fejl.                                                                                        | **Lav (Polish)**    |
| **Gendan &amp; Versioner**    | ✅ Remote browser (`ls --json`, rodsymbol "Jottacloud"), download-dialog **med destination-check + bekræftelse før kommando** ( ingen TTY-prompt), abort/retry pr. entry. ✅ **Fil-revisioner vises allerede i browseren** (Version/Revision-rækker m. størrelse). ✅ Ghost/døde enheder håndteres graceful. ✅ Papirkurv-web-knap (UriLauncher; `trash list` har ingen `--json`). | **Gendan gammel version**: revisions-data parses og vises, men kan endnu ikke vælges som gendannelseskilde.                                            | **Middel**          |
| **Arkiv &amp; Deling**        | ❌ Ikke eksponeret i GUI (kun synlig som mappe i *Gendan*).                                                                                                                                                                                                                                                                                                                     | Upload til `/Archive` + offentligt delingslink via `jotta-cli archive <fil> --share`.                                                                  | **Middel**          |
| **Indstillinger**             | ✅ *Minimer til tray* (default ON, `settings.conf` deles med tray) og *Start ved login* (XDG-autostart, PR #12).                                                                                                                                                                                                                                                                | **Båndbredde** (`maxuploadrate`/`maxdownloadrate`), **Ignorer skjulte filer** (`ignorehiddenfiles`), **Slowmo-mode**, **Webhooks**.                    | **Høj**             |
| **Info &amp; Konto**          | ✅ Egen fane: enhedsnavn, e-mail, abonnement (parser fra `status --json`), `jotta-cli`-version (whitelisted `Version`, PR #12).                                                                                                                                                                                                                                                 | Flyt ind som gruppe i *Indstillinger*; **`Log ud`**-knap (destruktiv → confirm); `jottad`-version (ikke tilgængelig via CLI — evt. direkte fra binær). | **Høj**             |
| **Login &amp; onboarding**    | ✅ `NeedsLogin`-dialog implementeret (PR #11): instruktioner + terminal-vejledning + retry. Beholdes som fallback.                                                                                                                                                                                                                                                              | Grafisk token-onboarding via PTY/stdin (afsnit 2B) — ✅ ejer-godkendt 23.10.2026; implementeres efter empirisk stdin/PTY-test i VM.                     | **Høj (til beta)**  |
| **Sikkerhed &amp; robusthed** | ✅ `JottaCommand` argv-whitelist (GUI shell'er aldrig rå strenge). ✅ GApplication single-instance. ✅ Ren nedlukning `std::process::exit(0)` + reaper-tråde (ingen zombier). ✅ Model B: genbruger host-daemon, dræber kun egne children. ✅ Poller udsteder aldrig kommandoer (kun brugerhandling).                                                                               | — (vedligehold som invariant)                                                                                                                          | Invariant           |
| **Kerne &amp; build** | ✅ jotta-core: **zero-dependency**, 41/41 unit tests (inkl. 2 Flatpak-tests: `host_view`/`host_spawn_prefix`), tolerance-parser for alle JSON-schemas (verificeret mod reel output). ✅ Ikon-tema (5 tilstande + app + symbolic, alle hicolor-størrelser incl. 512px + app-id-aliasser), desktop-entries, AppStream-metainfo (PR #4). ✅ **Flatpak-pakning (PR #13, merged 5.11.2026):** manifest GNOME 50, cargo-sources.json (262 entries), Cargo.lock committet, D-Bus service-file, `--device=dri`, OOM-safe build. | Flathub: **extra-data i stedet for host-CLI-tilgang** (afsnit 2A — Flathub afviser host-afhængige apps); screenshots i metainfo, ikon-re-render (rsvg-convert), test på GNOME/KDE/COSMIC; autostart `Exec=flatpak run …` ved `/.flatpak-info`. | **Høj (til beta)** |
| **Sprog (i18n)**              | ✅ Dansk UI overalt (ejer-krav), inkl. positive switch-formuleringer.                                                                                                                                                                                                                                                                                                           | i18n (da/en): streng-ekstraktion, `en`-locale, engelsk Flathub-beskrivelse. Dansk forbliver fallback/default.                                          | **Lav (post-beta)** |


---

## 5. Vejen videre — Definition of Done for Flathub-betain

1. **Extra-data-manifest**: pinned `jotta-cli`/`jottad` version + SHA256 fra `repo.jotta.cloud` (ændrer main's nuværende host-CLI-tilgang fra PR #13 — Flathub afviser host-afhængige apps).
2. **Jottacloud-mail** ([support@jottacloud.com](mailto:support@jottacloud.com) → bed om videresendelse til product/dev-relations): (a) accept af extra-data-download / evt. bundling-tilladelse, (b) logo/brand-regler, (c) mention i deres kanaler.
3. **Metainfo**: screenshots (min. 1 pr. hovedside), engelsk summary, korrekt license-deklaration (MPL-2.0) og `extra-data`-kilde.
4. **Ikon-re-render** med `rsvg-convert` (alle hicolor-størrelser + scalable + symbolic).
5. **Regressionstest på Fedora (KDE), GNOME og COSMIC** — tray (ksni/StatusNotifierWatcher), autostart, sandbox-socket til jottad.
6. **Grafisk login (2B)** — ✅ beslutningsændring ejer-godkendt 23.10.2026 (erstatt PR #11-beslutningen). Implementeres som egen iteration: empirisk stdin-pipe-test i VM først (PTY kun hvis nødvendigt), prompt-matching med fallback til `NeedsLogin`-dialogen, PTY/stdin-kode i jotta-gui (`jotta-core` zero-dep), token aldrig i argv/logs.
7. **Launch**: Flathub-side klar → "Show HN" + Reddit (r/jottacloud, r/linux, r/selfhosted) + Mastodon + én LinkedIn-post + dansk/norsk tech-presse.