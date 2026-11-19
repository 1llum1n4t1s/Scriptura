[日本語](README.ja.md) | [English](README.md) | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md) | [한국어](README.ko.md) | [Español](README.es.md) | [Français](README.fr.md) | **Deutsch** | [Português (Brasil)](README.pt-BR.md)

# Scriptura

Scriptura pflegt `AGENTS.md` / `DESIGN.md` / `README.md` (sowie, als Opt-in, `CHANGELOG.md` und benutzerdefinierte Dokumente) im Hintergrund, nachdem Sie in Ihre eigenen Repositorys gepusht haben. Gestartet wird es über einen Claude-Code-Stop-Hook, und als Wartungs-Worker dient die Claude Code CLI (standardmäßig `claude -p --model claude-sonnet-5-5 --effort high`). Es hängt nicht von Codex ab.

- **Standardmäßig sicher.** Zum Aktivieren sind Ihre ausdrückliche Einwilligung und die Registrierung des Repositorys erforderlich; der Standardmodus ist `propose`, der lediglich einen Update-Vorschlag für Dokumente (einen Patch) erzeugt. Ihr Arbeitsverzeichnis, `.git` und die Remotes werden dabei nicht verändert.
- Der Modus `push`, der Dokumente automatisch committet und pusht, steht nur zur Verfügung, wenn Sie `--mode push` ausdrücklich angeben.
- Einstellungen werden ausschließlich aus einer einzigen Datei im Home-Verzeichnis des Benutzers (`config.toml`) und dem daneben liegenden Ordner `templates/` gelesen. Dateien innerhalb eines Repositorys zum Aktivieren oder Konfigurieren werden nie gelesen (damit schon das bloße Klonen eines fremden Repositorys niemals ein automatisches Verhalten auslösen kann).
- Einstellungen lassen sich auch in `scriptura ui` bearbeiten (der browserbasierten Verwaltungsoberfläche, siehe unten).

Dies ist der Teil zur Dokumentenpflege des früheren `agents-maintenance` (CodexHooks), herausgelöst als eigenständiges Werkzeug. Die Verträge für Wartungsentscheidungen, Zustandsverwaltung und Veröffentlichung (Commit und Push) sind dieselben.

## Installation

### Plugin (empfohlen)

Wenn Sie es als Claude-Code-Plugin installieren, werden der Stop-Hook und die Betriebs-Skills (wie `/scriptura:setup`) gemeinsam eingerichtet. Die Binärdatei `scriptura` ist im Plugin enthalten (unter Windows `bin/scriptura.exe`; unter macOS / Linux wählt der Launcher `bin/scriptura` die Binärdatei unter `libexec/` aus. Auch unter Windows übergibt derselbe Launcher an `bin/scriptura.exe`, wenn der Hook über die sh von Git Bash läuft). Wie die Distribution gebaut wird, steht unter `xtask/` (`cargo run --manifest-path xtask/Cargo.toml --locked -- package`).

### Setup-Edition (automatische Updates)

Das Ausführen von `ScripturaApp-win-Setup.exe` (Windows), `ScripturaApp-osx-Setup.pkg` (macOS) oder des AppImage `ScripturaApp-linux-x64.AppImage` / `ScripturaApp-linux-arm64.AppImage` (Linux x64 / arm64) installiert pro Benutzer ohne Administratorrechte (unter Windows liegt die Installation in `%LOCALAPPDATA%\ScripturaApp\current\scriptura.exe`). Updates werden von `https://scriptura-updates.kagayoi.com` bereitgestellt (Velopack-Feeds mit dem Namen `releases.<channel>.json`, mit den Kanälen `win` / `osx` / `linux-x64` / `linux-arm64`). Die Windows-Distribution ist mit einem Certum-Zertifikat signiert, die macOS-Distribution mit einer Developer ID signiert und notarisiert. Voraussetzungen: macOS 11 oder neuer auf Apple Silicon (Intel wird nicht unterstützt); Linux mit glibc 2.39 oder neuer.

- **Die Registrierung erfolgt automatisch**: Bei der Installation (unter Windows über den Install-Hook; unter macOS und Linux beim ersten Start ohne Argumente nach der Installation) wird der Stop-Hook in der `settings.json` von Claude Code auf die ausführbare Datei der Setup-Edition gesetzt. Es werden nur Scripturas eigene Hooks ersetzt (ein Hook, der auf die alte eigenständige Installation `%LOCALAPPDATA%\Programs\Scriptura\bin\scriptura.exe` zeigt, wird durch den der Setup-Edition ersetzt, und die alten Dateien werden nicht gelöscht). Bestehende `agents-maintenance`-Hooks werden nicht ersetzt; `status` / `doctor` / ein Start ohne Argumente weisen Sie auf `scriptura connect claude-code --apply` hin.
- **Doppelungen mit dem Plugin vermeiden**: Ist `scriptura@*` in `enabledPlugins` der `settings.json` aktiviert, registriert sich die Setup-Edition nicht in der `settings.json` (die Wartung würde sonst doppelt laufen). Entscheiden Sie sich entweder dafür, das Plugin weiter zu verwenden, oder dafür, das Plugin zu deaktivieren und zur Setup-Edition zu wechseln. Nach dem Deaktivieren erfolgt die Registrierung automatisch bei einem Start ohne Argumente oder bei der nächsten Anmeldung.
- **Automatische Updates (standardmäßig an)**: Nachdem die Wartung eines Hooks abgeschlossen ist, wird höchstens einmal pro Tag nach einer neuen Version gesucht (für Erfolge und Fehlschläge jeweils auf 24 Stunden gedrosselt) und diese nur heruntergeladen. **Angewendet wird sie ausschließlich bei der Anmeldung**: `scriptura update-apply`, das bei der Anmeldung läuft (unter Windows der HKCU-Run-Wert `<packId> Update`; unter macOS der LaunchAgent `com.kagayoi.<packId>.update`; unter Linux der XDG-Autostart-Eintrag `com.kagayoi.<packId>.update.desktop`), prüft, dass kein Scriptura-Hook und keine Verwaltungsoberfläche läuft, und kontrolliert den Signierer; danach wendet es das Update ohne Neustart an. Läuft irgendetwas, tut es nichts und überlässt das Update der nächsten Anmeldung (so wird ein Hook mitten in der Wartung nie beendet). Auf Wunsch lässt es sich auch mit „Update installieren“ in der Verwaltungsoberfläche (unten) anwenden.
- Der Zustand liegt im Zustandsordner (siehe unten): `app-update.json` (`auto_update` sowie Zeitpunkte der letzten Prüfung bzw. des letzten Fehlschlags), `app-update.log` (für die Diagnose) und `setup-registration.json` (das Registrierungsergebnis).

```powershell
scriptura update                 # show the auto-update setting, the last check, and the downloaded version
scriptura update --check         # ignore throttling and check/download now (applied at the next logon, or by running update-apply manually)
scriptura update --auto off      # turn auto-update off (on turns it back on)
scriptura update-apply           # apply a downloaded update (normally runs automatically at logon)
scriptura uninstall-setup        # preview removing the Setup edition's registration (Stop hook and logon apply)
scriptura uninstall-setup --apply
```

macOS und Linux haben kein Deinstallationsprogramm. Führen Sie daher `scriptura uninstall-setup --apply` aus, bevor Sie die `.app` in den Papierkorb verschieben (oder das AppImage löschen). Unter Windows führt die Deinstallation über „Einstellungen > Apps“ dieselbe Entfernung automatisch durch. In beiden Fällen werden nur der Hook, der auf sich selbst zeigt (exakte Übereinstimmung des vollständigen Pfads), und die Anmelderegistrierung entfernt; `config.toml` und der Zustandsordner bleiben erhalten.

### Eigenständiges `install` / `connect` (fortgeschritten)

```powershell
Set-Location D:\Dev\Scriptura
cargo build --release
.\target\release\scriptura.exe install                       # place it at a fixed per-user path
& "$env:LOCALAPPDATA\Programs\Scriptura\bin\scriptura.exe" connect claude-code          # preview the registration
& "$env:LOCALAPPDATA\Programs\Scriptura\bin\scriptura.exe" connect claude-code --apply  # register in settings.json
```

`connect claude-code --apply` trägt den festen Installationspfad im Stop-Hook der `settings.json` von Claude Code ein (`CLAUDE_CONFIG_DIR` oder `~/.claude`). Dabei werden Hook für Hook nur die Hooks ersetzt, die das frühere `agents-maintenance` und alte Scriptura-Hooks aufrufen (andere Hooks im selben Eintrag bleiben erhalten); alle übrigen Einstellungen bleiben unberührt. Ein Befehl gilt nur dann als Scriptura-Hook, wenn er die Form `<executable> hook` hat und der Name der ausführbaren Datei `scriptura(.exe)` lautet; andere Werkzeuge, die lediglich scriptura im Pfad haben, sind nicht betroffen. Vor der ersten Änderung wird `settings.json.scriptura-backup` angelegt, und fehlerhaftes JSON wird nicht umgeschrieben.

Beim Wechsel zum Plugin entfernen Sie die eigenständige Registrierung, damit die Wartung nicht doppelt läuft (dabei werden nur Scripturas Einträge entfernt; `agents-maintenance` und andere Einstellungen bleiben unberührt).

```powershell
scriptura connect claude-code --remove           # preview
scriptura connect claude-code --remove --apply   # write
```

Der Worker erbt die Relay-Einstellungen der Desktop-Sitzung nicht. Führen Sie daher vorab `claude auth login` in der eigenständigen CLI aus. Verwendet wird die CLI aus der Einstellung `worker.claude_path`, andernfalls `claude` im PATH (`claude.exe` unter Windows) und unter Windows zusätzlich die neueste `%LOCALAPPDATA%\Claude-Data\claude_*\claude-code\<version>\<hash>\claude.exe`, die mit der Claude-Desktop-App gebündelt ist.

## Ersteinrichtung

Führen Sie in dem Repository, das gepflegt werden soll, eines der folgenden Verfahren aus.

- Wenn Sie das Plugin installiert haben: Führen Sie `/scriptura:setup` in Claude Code aus (nach der Bestätigung der Einwilligung läuft `scriptura enable`).
- Direkt über die Kommandozeile:

```powershell
scriptura enable --repo D:\Dev\MyRepo --accept-consent 1
```

`--accept-consent` ist die aktuelle Version des Einwilligungsdokuments (derzeit `1`). Fehlt der Parameter oder ist die Version falsch, gibt der Befehl eine Erläuterung zu den gesendeten Daten und den Kosten aus und lehnt ab. Vor dem Registrieren vermerkt `enable` den „aktuellen Push-Stand“ als bereits verarbeitet, sodass Pushes vor dem Aktivieren später nie verarbeitet werden.

## Modi

| Modus | Verhalten |
| --- | --- |
| `propose` (Standard) | Bei jedem Push aktualisiert der Worker Dokumente in einem Wegwerf-Klon und **speichert die Differenz als Patch**. Arbeitsverzeichnis, `.git` und Remotes werden nicht verändert. Prüfen Sie ihn mit `review` und wenden Sie ihn mit `apply` auf das Arbeitsverzeichnis an (es wird nichts gestaged). |
| `push` | Der Worker aktualisiert die Dokumente und committet und pusht automatisch. Nur, wenn `--mode push` ausdrücklich angegeben wird. Ein Remote und ein Upstream sind erforderlich, und wenn `--branches` weggelassen wird, ist nur der aktuelle Branch betroffen. |
| `off` | Tut nichts (die Registrierung bleibt bestehen). |

## Konfigurationsdatei

| Betriebssystem | Speicherort |
| --- | --- |
| Windows | `%LOCALAPPDATA%\Scriptura\config.toml` |
| macOS | `~/Library/Application Support/Scriptura/config.toml` |
| Linux | `$XDG_CONFIG_HOME/scriptura/config.toml` (Standard `~/.config/scriptura/config.toml`) |

Sie können den Ort mit `SCRIPTURA_CONFIG` (ein absoluter Dateipfad) oder `SCRIPTURA_HOME` (`<SCRIPTURA_HOME>/config.toml`) überschreiben. Die Datei wird normalerweise von `enable` / `disable` / `pause` / `resume` und der Verwaltungsoberfläche umgeschrieben, sodass Sie sie nicht von Hand bearbeiten müssen. Eine defekte Konfiguration, unbekannte Schlüssel oder ein zu neues `schema` führen zum Rückfall auf „nichts tun“, und Befehle wie `enable` schlagen fehl, ohne sie umzuschreiben.

`schema` kann 1 oder 2 sein. Beim Speichern wird „die mindestens erforderliche Version“ verwendet: Die Datei bleibt bei `schema = 1`, solange Sie keine Einstellungen pro Art (`[kinds.*]`), kein `[defaults] instructions`, keine Konfiguration, die `CHANGELOG.md` oder eine benutzerdefinierte Art als Ziel aufführt, und keine in der Verwaltungsoberfläche erteilte Einwilligung verwenden (damit auch ältere Scriptura-Versionen sie weiterhin lesen können). Sobald eines davon genutzt wird, wird daraus `schema = 2`, und ältere Versionen, die sie lesen, fallen auf „nichts tun“ zurück.

```toml
schema = 1

[consent]
version = 1
accepted_at = "2026-11-14T00:00:00+00:00"
source = "cli"

[defaults]
mode = "propose"                                # propose / push / off
documents = ["AGENTS.md", "DESIGN.md", "README.md"]
exclude = ["private/**"]                        # ADDED to the default sensitive patterns (patterns containing `)`, whitespace, or control characters are not allowed)

[worker]
model = "claude-sonnet-5-5"
effort = "high"
timeout_secs = 1200                             # 60..=1500 (shorter than the Stop hook's 1800 seconds)
permission = "restricted"                       # restricted / bypass
claude_path = ""                                # empty means search PATH (an absolute path if specified)

[enrollment]
policy = "explicit"                             # explicit / remote-owner / all
owners = []                                     # for remote-owner, e.g. "github.com/acme"

[[repository]]
root = "D:/Dev/MyRepo"
remote = "github.com/me/myrepo"                 # expected push destination (skipped if it does not match)
mode = "propose"
branches = ["main"]                             # empty means no restriction
documents = ["README.md"]
paused = false
```

## Dokumentarten

Die zu pflegenden Dokumente werden als „Arten“ (kinds) verwaltet.

| Art | Ziel | Standard |
| --- | --- | --- |
| `agents` / `design` / `readme` | Wie bisher (`AGENTS.md` und die persönliche Überschreibung, `AGENTS.md` / `DESIGN.md` mit Geltungsbereich, die `README.md` im Stammverzeichnis) | Aktiviert (der Standard von `defaults.documents` bleiben diese 3 Arten) |
| `changelog` | Die `CHANGELOG.md` im Stammverzeichnis | **Opt-in** (nur wenn `CHANGELOG.md` in `documents` aufgeführt ist) |
| Benutzerdefiniert (`[kinds.<id>]`) | Dokumente, die unter festen Pfaden in `paths` aufgeführt sind (bis zu 20 Arten; eine ID beginnt mit einem Kleinbuchstaben und besteht aus Kleinbuchstaben, Ziffern und Bindestrichen, höchstens 32 Zeichen) | Nur wenn die ID in `documents` aufgeführt ist |

Die Zielarten werden in `defaults.documents` (überschrieben durch das `documents` des jeweiligen Repositorys) entweder über den Dokumentnamen (`"CHANGELOG.md"`) oder über die ID (`"changelog"`; bei benutzerdefinierten Arten die ID) aufgeführt. Eine Einrichtung mit nur den ursprünglichen 3 Arten verhält sich genau wie bisher, wenn Sie keine Einstellungen pro Art schreiben (auch der an den Worker übergebene Prompt ist identisch).

### Einstellungen pro Art (`[kinds.<id>]`)

Diese werden nur aus der `config.toml` des Benutzers gelesen. Jedes Feld ist optional, und eingebaute Arten setzen auf festen Standardwerten auf. Unbekannte Schlüssel werden abgelehnt.

| Schlüssel | Beschreibung |
| --- | --- |
| (die ID der Art) | Die ID einer benutzerdefinierten Art beginnt mit einem Kleinbuchstaben und besteht nur aus Kleinbuchstaben, Ziffern und `-`, höchstens 32 Zeichen. Das reservierte Wort `new` kann nicht verwendet werden (es würde mit der URL der Verwaltungsoberfläche kollidieren). |
| `paths` | Die Zielpfade der benutzerdefinierten Art (1 bis 10 Einträge, erforderlich). Für eingebaute Arten nicht zulässig. Nur ASCII-Buchstaben und -Ziffern sowie `. _ / -`, relative Pfade und die Endungen `md` / `markdown` / `mdx` / `txt` / `rst` / `adoc`. Pfade unter `.git`, dieselben Dokumente wie eingebaute Ziele (etwa `AGENTS.md`), Pfade, die auf sensible Muster oder `exclude` passen, sowie Duplikate über Arten hinweg werden abgelehnt. |
| `guidance` | Wozu das Dokument dient (für benutzerdefinierte Arten erforderlich, höchstens 1000 Zeichen). Für eingebaute Arten nicht zulässig. |
| `create_if_missing` | Ob das Ziel angelegt werden darf, wenn es nicht existiert (true bei eingebauten Arten, false bei benutzerdefinierten Arten). Ist der Wert false, erscheint das Ziel weder in den erlaubten Pfaden noch im Prompt, solange es nicht existiert. |
| `template` | `"builtin"` / `"none"` / `"templates/<name>.md"`. Siehe unten. |
| `max_chars` / `max_bytes` | Grenzen für das Dokument (Zeichen und Bytes). Sind beide angegeben, gelten beide. Für `agents` kann `max_bytes` nur auf 29.999 oder weniger gesetzt werden. |
| `instructions` | Zusätzliche Bearbeitungsanweisungen (höchstens 2000 Zeichen). Zusammen mit `[defaults] instructions` (gilt für alle Arten) in zwei Stufen gelangen sie nur in den Abschnitt „aus den Benutzereinstellungen“ am Ende des Prompts. Sie können weder die Zielpfade noch die Sicherheitsregeln noch den Umgang mit Commit und Push ändern. |
| `language` | Die Sprache des Dokuments (`ja` / `en-US` usw.). Wird sie weggelassen, folgt das Dokument der Sprache des bestehenden Dokuments. Dies ist die Sprache der erzeugten Dokumente und unabhängig von der Sprache der Oberflächen und Ausgaben (die Prompts des Workers sind auf Englisch, das Verhalten von `language` bleibt jedoch unverändert). |
| `required_headings` | Überschriften, deren Vorhandensein nach der Wartung geprüft wird (höchstens 20). |
| `timing` | Wann die Art läuft. Siehe unten. |
| `changelog` | CHANGELOG-Einstellungen (nur für die Art `changelog`). Siehe unten. |

**Vorlagen** können nur als `templates/<name>.md` im Einstellungsordner angegeben werden (Dateien innerhalb des Repositorys und beliebige absolute Pfade sind nicht zulässig; für CHANGELOG ist `builtin` die eingebettete Keep-a-Changelog-Vorlage). Zur Laufzeit wird geprüft, dass die Datei eine reguläre Datei (kein Link) ist, höchstens 16 KiB groß, UTF-8, frei von Steuerzeichen außer Zeilenumbruch und Tabulator, frei von `<<<` und außerhalb des Repositorys liegt; schlägt eine Prüfung fehl, wird der Worker nicht gestartet, die verarbeitete Position nicht vorgerückt und der Lauf übersprungen (er wird beim nächsten Mal verarbeitet, sobald Sie die Datei korrigiert haben). Eine Vorlage dient als Struktur beim Anlegen eines neuen Dokuments; bei bestehenden Dokumenten hat die vorhandene Struktur Vorrang.

### Prüfungen nach der Wartung

Grenzen (`max_chars` / `max_bytes`), `required_headings`, die Obergrenze von 30.000 Bytes für AGENTS und die Unveränderlichkeit veröffentlichter CHANGELOG-Einträge werden unabhängig vom Berechtigungsmodus des Workers (`restricted` / `bypass`) von Rust geprüft. Bei einem Verstoß committet der Modus `push` nicht (die Änderungen verbleiben im Arbeitsverzeichnis, und der nächste Hook führt dieselben Prüfungen aus), während der Modus `propose` keinen Update-Vorschlag erzeugt und ihn ablehnt (`rejected`), wobei die verarbeitete Position vorrückt. Der Worker wird für denselben Push nicht erneut ausgeführt. Eine Ablehnung in `propose` lässt sich nur mit `run-now` neu erzeugen.

### Timing (`[kinds.<id>.timing]`)

| Schlüssel | Beschreibung |
| --- | --- |
| `when` | `push` (Standard; bei jedem Push) / `release` (wenn ein Release-Tag erscheint) / `manual` (nur bei `run-now`) |
| `paths` | Ein Array von Globs. Die Art ist nur dann ein Ziel, wenn einer der in diesem Push-Bereich geänderten Pfade passt (ohne Angabe keine Bedingung) |
| `min_interval_hours` | Mindestabstand seit dem letzten Abschluss (0 bis 720 Stunden) |

CHANGELOG und benutzerdefinierte Arten, deren Ziel nicht existiert und nicht angelegt werden darf, bleiben aus der Entscheidung ausgenommen. Gibt es kein zu verarbeitendes Dokument, wird der Worker nicht gestartet und nur die verarbeitete Position vorgerückt (auch `kind_runs` wird nicht aufgezeichnet). Wird die Wartung nach einem Worker-Fehler o. Ä. fortgesetzt, setzt sie zunächst nur mit den Arten fort, die fällig waren (im Zustand gespeichert).

Alle Bedingungen werden als UND ausgewertet. `release` greift, wenn ein zu `release_tag` passender Tag (`v*` für andere Arten als CHANGELOG) von der gepushten Position aus erreichbar ist und der Abschnitt für diesen Tag in `CHANGELOG.md` noch nicht existiert (andere Arten als CHANGELOG verwenden dasselbe Kriterium; schreibt Ihr Release-Verfahren den Abschnitt der Version also vor dem Taggen, tritt `release` für diese Arten nie ein). Nur die fälligen Arten werden gemeinsam in einem einzigen Worker-Lauf verarbeitet, und ist keine Art fällig, wird der Worker nicht gestartet und nur die verarbeitete Position vorgerückt. Der Zeitpunkt des letzten Abschlusses wird pro Art in der Tabelle `kind_runs` der Zustands-DB geführt (die Version der Zustands-DB ändert sich nicht).

Die Auswertung erfolgt nur, wenn ein neuer Push erkannt wird, und bei `run-now`. Der Mindestabstand bedeutet daher „beim **nächsten Push** ausführen, nachdem das Intervall verstrichen ist“; die bloße Zeit löst nichts aus. Änderungen eines zu frühen Pushs werden nicht auf den nächsten Lauf übertragen, aber der Worker betrachtet beim Aktualisieren der Dokumente die gesamte Differenz des Push-Bereichs. `run-now` ignoriert das Timing und verarbeitet alle aktivierten Arten.

### CHANGELOG-Pflege (`[kinds.changelog.changelog]`)

- Es folgt dem bestehenden Format (die eingebettete Keep-a-Changelog-Vorlage wird nur beim Anlegen der Datei verwendet). Jeder Push aktualisiert nur den Abschnitt `Unreleased`, und der Worker kann veröffentlichte Abschnitte nicht ändern. Rust vergleicht den Stand vor und nach der Wartung; hat sich auch nur ein Byte eines veröffentlichten Abschnitts geändert, ist ein Abschnitt verschwunden oder umsortiert worden oder gingen Linkreferenz-Definitionen verloren, gilt das als Verstoß gegen die obigen Prüfungen (der Modus push committet nicht; propose lehnt ab).
- Für die Historie sammelt Rust alles vom Commit, der `CHANGELOG.md` zuletzt geändert hat, bis zu diesem Push und übergibt es dem Prompt als **nicht vertrauenswürdige Daten** (Commit-Betreffzeilen, geänderte Pfade und Tags im Bereich, insgesamt höchstens 48 KiB). Das liegt daran, dass ein eingeschränkter Worker `git log` nicht ausführen kann. Scripturas eigene Wartungs-Commits werden ausgeschlossen.
- `on_release = "promote"` (Standard): Gibt es für die zu einem Tag im Bereich gehörende Version noch keinen Abschnitt, wird der Inhalt von `Unreleased` in den Abschnitt dieser Version verschoben. Existiert der Abschnitt bereits, geschieht nichts (das Ergebnis ist dasselbe, auch wenn das Release-Verfahren ihn zuerst geschrieben hat). `ignore` legt keine Versionsabschnitte an.
- `backfill = "tags"` (Standard): Existiert `CHANGELOG.md` nicht, legt es Abschnitte pro Version an, indem es erreichbare Tags, neueste zuerst, zurückverfolgt, bis zu `backfill_max_releases` (1 bis 50, Standard 20) Tags und insgesamt bis zu `backfill_max_commits` (1 bis 1000, Standard 300) Commits zwischen benachbarten Tags. `none` geht nicht zurück. Auch `format` (`existing` / `keep-a-changelog`) und `release_tag` (Standard `v*`) können angegeben werden.
- Der Dateiname wird anhand seiner Schreibweise in Git ohne Beachtung der Groß-/Kleinschreibung aufgelöst (auch `Changelog.md` und Ähnliches sind Ziele). Lässt sich die Schreibweise nicht eindeutig bestimmen oder der Inhalt vor der Bearbeitung nicht aus Git lesen, gilt das als nicht prüfbar und zählt als Verstoß. Im Modus push wird eine neue Wartung übersprungen, ohne den Worker zu starten, wenn `CHANGELOG.md` von Git ignoriert wird.
- Beim Abgleich von Versionen mit Tags gilt auch der Versionsname ohne den festen Präfix von `release_tag` (das `v` von `v*`, das `release-` von `release-*` usw.) als Treffer (`## [1.2.0]`). Verstoßgründe geben keinen Überschriftentext aus, sondern werden per Nummer angegeben, etwa „der n-te Versionsabschnitt“. Bei Linkreferenz-Definitionen veröffentlichter Abschnitte wird zudem geprüft, dass Inhalt und Reihenfolge erhalten bleiben und bestehende Labels nicht neu definiert werden.
- Einschränkungen: Ein reiner Tag-Push (`git push --tags`) kann nicht erkannt werden. Er wird beim nächsten Branch-Push oder mit `--follow-tags` erfasst. Wenn `CHANGELOG.md` im Modus push gepflegt wird, kann ein Release-Verfahren, das dieselbe Datei aktualisiert, in Konflikt geraten (`doctor` weist darauf hin).

### Beispielkonfiguration (Schema 2)

```toml
schema = 2

[defaults]
documents = ["AGENTS.md", "DESIGN.md", "README.md", "CHANGELOG.md", "architecture"]
instructions = "Write in a plain, declarative style and never speculate"      # additional instructions shared by all kinds

[kinds.agents]
max_bytes = 20000

[kinds.changelog]
max_chars = 40000
instructions = "Record only user-visible changes"
[kinds.changelog.timing]
when = "push"
[kinds.changelog.changelog]
on_release = "promote"
backfill = "tags"

[kinds.architecture]                              # a custom kind
guidance = "Describe the system structure, responsibilities, and data flow"
paths = ["docs/ARCHITECTURE.md"]
create_if_missing = false
template = "templates/architecture.md"            # under the settings folder
max_chars = 15000
required_headings = ["Overview"]
[kinds.architecture.timing]
paths = ["src/**", "Cargo.toml"]
min_interval_hours = 24

[[repository]]
root = "D:/Dev/MyRepo"
documents = ["README.md", "CHANGELOG.md"]
```

## Schreiben von Einstellungen und Änderungsverlauf

Alle Schreibvorgänge auf Einstellungen, ob über die CLI, die Verwaltungsoberfläche oder die automatische Migration von Altbenutzern, laufen über denselben Pfad (`src/ops.rs` und `config::update_in`). Schreibende werden über `config-lock.sqlite3` im Zustandsordner serialisiert, und die Verwaltungsoberfläche vergleicht die Version, die sie angezeigt hat (das SHA-256 der Datei, oder `missing`, wenn die Datei nicht existiert); wurde die Datei anderswo geändert, speichert sie nicht und fordert Sie zum Neuladen auf (die CLI vergleicht keine Versionen). Unmittelbar vor dem Speichern wird der vorherige Inhalt in `config-history/` im Zustandsordner beiseitegelegt (50 Einträge bleiben erhalten), und das Speichern ist atomar. Eine defekte Konfiguration wird nicht umgeschrieben. Eine Operation, die nichts ändert, speichert nichts und legt nichts beiseite. Über „History“ in der Verwaltungsoberfläche können Sie Differenzen einsehen und wiederherstellen; auch der Inhalt vor einer Wiederherstellung bleibt im Verlauf erhalten. Würde eine Wiederherstellung Registrierungen hinzufügen, erscheint ein Bestätigungsbildschirm, und der Startpunkt jeder Registrierung (die Position, die verhindert, dass Pushes vor dem Aktivieren verarbeitet werden) wird vor dem Speichern festgehalten. Eine Wiederherstellung, die Registrierungen im Modus push hinzufügen oder Registrierungen auf push ändern würde, wird abgelehnt und auf `enable --mode push` verwiesen (das Remote und Upstream prüft).

## Verwaltungsoberfläche (`scriptura ui`)

```powershell
scriptura ui [--port <n>] [--no-open] [--tray]
scriptura ui --autostart on|off
```

Eine browserbasierte Oberfläche für Einstellungen, Update-Vorschläge und Diagnose. Sie ist in 9 Sprachen verfügbar (ja, en, zh-CN, zh-TW, ko, es, fr, de, pt-BR). Die Sprache wird in dieser Reihenfolge gewählt: die Sprachauswahl oben auf der Seite (in einem Cookie gespeichert) → der `Accept-Language` des Browsers → Englisch. Standardmäßig läuft sie nicht dauerhaft: Der Befehl, der sie gestartet hat, endet, wenn Sie auf der Seite „Quit“ wählen, Strg+C drücken oder 60 Minuten lang nichts geschieht (mit `--tray` bleibt sie dauerhaft aktiv; siehe unten). Hooks prüfen nicht, ob die Oberfläche geöffnet ist. Sie wird durch das Cargo-Feature `ui` gesteuert (standardmäßig aktiviert) und ist bei einem Build mit `--no-default-features` nicht enthalten (`scriptura ui` lehnt dann ab und beendet sich).

- **Lauschen**: nur auf `127.0.0.1` (der Standardport ist 7468; `--port 0` wählt einen freien Port). Ist der Port belegt, wird beendet.
- **Dauerbetrieb (`--tray`)**: Bleibt mit einem Symbol im Infobereich der Taskleiste unter Windows bzw. in der Menüleiste unter macOS dauerhaft aktiv (Linux wird nicht unterstützt; `--tray` lehnt ab und beendet sich). Ein Klick auf das Symbol öffnet die Verwaltungsoberfläche, und das Rechtsklickmenü bietet „Verwaltungsoberfläche öffnen“, „Nach Updates suchen“ (nur Setup-Edition; öffnet das Panel „App-Updates“ der Übersicht) und „Beenden“. Im Dauerbetrieb endet sie nicht bei Inaktivität. Stattdessen läuft die Sitzung nach 60 Minuten Inaktivität ab (offen gelassene Tabs erhalten 401), und bei jedem Öffnen der Oberfläche über das Symbol wird ein neuer Einmalschlüssel ausgestellt (ein neu ausgestellter Schlüssel läuft nach 5 Minuten ab, und ein früherer, nicht verwendeter Schlüssel wird ungültig). Unter Windows startet ein erneutes `scriptura ui --tray`, während bereits eine Instanz auf dem Standardport dauerhaft läuft, keine zweite; die laufende Instanz öffnet den Browser. Wird sie aus etwas anderem als einem Terminal gestartet (etwa dem Startmenü), startet sie sich selbst als Prozess ohne Konsolenfenster neu.
- **Dauerbetrieb bei der Anmeldung (nur Setup-Edition; standardmäßig an)**: Bleibt aktiv, sofern Sie es nicht mit `scriptura ui --autostart off` oder dem Kontrollkästchen im Panel „App-Updates“ ausschalten (`tray_autostart` in `app-update.json` im Statusordner; ein früher gespeichertes Aus bleibt erhalten, ein fehlender oder unlesbarer Wert gilt als an). Es wird kein zusätzlicher Anmeldeeintrag angelegt; die dauerhaft laufende Oberfläche wird gestartet, nachdem das `update-apply` bei der Anmeldung abgeschlossen ist (würde sie zuerst starten, gälte der Installationsordner als von einem anderen Prozess belegt, und Updates würden nie angewendet). Bei einer Anmeldung, bei der ein Update angewendet wird, startet der Updater nach dem Austausch die dauerhaft laufende Oberfläche der neuen Version. Der Startmenüeintrag der Setup-Edition (ein Start ohne Argumente) zeigt seinen Hinweis und hält danach die Verwaltungsoberfläche dauerhaft aktiv und öffnet sie im Browser. Da Scriptura im Dauerbetrieb weiter aus dem Installationsordner läuft, wird ein heruntergeladenes Update bei der nächsten Anmeldung angewendet oder sofort mit „Update installieren“ im Panel (unten). `--autostart` kann mit der im Plugin mitgelieferten oder der eigenständigen `install`-Programmdatei nicht verwendet werden (der Dauerbetrieb mit `ui --tray` funktioniert weiterhin).
- **Panel „App-Updates“ (nur Setup-Edition)**: Das Panel „App-Updates“ (`#updates`) der Übersicht zeigt die aktuelle Version und bietet Kontrollkästchen für automatische Updates und den Dauerbetrieb bei der Anmeldung, eine Schaltfläche „Nach Updates suchen“ sowie eine Schaltfläche „Update installieren“ (sichtbar, wenn eine neue Version vorhanden ist). Die Suche ignoriert das 24-Stunden-Intervall und lädt nichts herunter. Die Installation lädt herunter, prüft den Unterzeichner und bittet den Updater, es anzuwenden, wenn aus dem Installationsordner kein anderer Scriptura-Prozess (Hook, Worker oder weitere Verwaltungsoberfläche) läuft und kein run-now aktiv ist; danach wird die Verwaltungsoberfläche beendet (andernfalls geschieht nichts, und es wird zu einem späteren Versuch geraten; es wird kein Prozess erzwungen beendet). Im Dauerbetrieb startet der Updater die neue Version; die Sitzung ist neu, daher weist der geöffnete Tab darauf hin, sie über das Symbol im Infobereich (Menüleiste) erneut zu öffnen. Eine aus einem Terminal gestartete Oberfläche wird nur beendet. „Nach Updates suchen“ im Menü des Symbols öffnet dieses Panel und beginnt automatisch zu suchen. Ohne JavaScript funktionieren dieselben Schaltflächen, und das Ergebnis erscheint auf einer eigenen Seite. In anderen Editionen weist das Panel nur darauf hin, dass es in der Setup-Edition verfügbar ist.
- **Schutz**: Die bei jedem Start erzeugte Einmalschlüssel-URL (`/auth?key=…`) wird einmalig gegen ein Sitzungs-Cookie (`HttpOnly; SameSite=Strict`) eingetauscht (ohne das Cookie lautet die Antwort 401). Host darf nur `127.0.0.1:<port>` oder `localhost:<port>` sein; ist ein Origin vorhanden, muss er same-origin sein; und ein POST erfordert Same-Origin (`Origin` oder `Sec-Fetch-Site`) plus ein CSRF-Token. Antwort-Header wie CSP, `nosniff` und `no-store` werden hinzugefügt, und geheime Werte werden nie angezeigt. Da Browser Cookies nicht nach Port isolieren, wird das Cookie auch an Dienste gesendet, die auf anderen 127.0.0.1-Ports desselben PCs laufen (Prozesse desselben Benutzers können die Einstellungsdatei bereits ändern, sodass nur ein lokaler Dienst mit anderen Berechtigungen ein Problem darstellt).
- **Konflikte**: Beim Speichern wird die angezeigte Version verglichen, und wurde die Datei anderswo geändert, werden Sie zum Neuladen aufgefordert.
- **Bestätigungsbildschirm**: Der Wechsel in den Modus push, der Wechsel der Berechtigung auf `bypass`, die Änderung der Enrollment-Richtlinie auf `all` / `remote-owner`, das Hinzufügen eines Zielpfads einer benutzerdefinierten Art, das Erfassen der Einwilligung, das Entfernen einer Registrierung, das Anwenden eines Update-Vorschlags und `run-now` im Modus push (sowie das Wiederherstellen des Verlaufs auf eine Version, die eines davon enthält) schreiben bei der ersten Übermittlung nicht; sie zeigen eine Liste der Änderungen und verlangen eine einmalige Bestätigung (10 Minuten gültig).
- **Seiten**: Die Kopfzeile hat fünf Einträge: Overview / Repositories (registrieren, aktivieren, pausieren, fortsetzen, entfernen, Modus und Zieldokumente, `run-now`) / Document kinds (Arten hinzufügen, bearbeiten und löschen; Vorlagen in `templates/` bearbeiten) / Update proposals (Differenzen ansehen, anwenden, verwerfen) / Einstellungen. Die Einstellungen erscheinen als Tabs: Allgemein / Worker / Einwilligung / Verlauf (Änderungsverlauf, Differenzen, Wiederherstellung) / Diagnose. Auf der Worker-Seite werden Modell und Effort aus Listen gewählt, nicht eingetippt: Die Modellliste ist eine integrierte Liste; ist die Umgebungsvariable `ANTHROPIC_API_KEY` bereits gesetzt, holt die Schaltfläche „Modellliste aktualisieren“ die neuesten Modelle aus der Anthropic-API in den Arbeitsspeicher (Scriptura speichert oder zeigt den Schlüssel nie, das bloße Öffnen der Seite verbindet nicht, und ein gespeicherter Wert, der nicht in der Liste steht, bleibt als „nicht in der Liste“ erhalten). Zeiten werden in der lokalen Zeitzone des PCs angezeigt, auf dem die Oberfläche läuft (`YYYY-MM-DD HH:MM`); das gespeicherte Format und das JSON der CLI bleiben unverändert. `run-now` dauert, daher läuft es im Hintergrund, und Sie können seinen Status auf der Seite prüfen.

## Ausgabesprache

Die Ausgaben von CLI, Hook und Installer-Hinweisen sind in 9 Sprachen verfügbar (ja, en, zh-CN, zh-TW, ko, es, fr, de, pt-BR). Die Sprache wird in dieser Reihenfolge gewählt: `SCRIPTURA_LANGUAGE` → `LC_ALL` → `LC_MESSAGES` → `LANG` → die Anzeigesprache des Betriebssystems → Englisch. Die Verwaltungsoberfläche verwendet ihre eigene Auswahlreihenfolge (siehe oben). JSON-Schlüssel und für Maschinen bestimmte Werte (wie `status`, `outcome` und die `id` von `doctor`) werden nicht übersetzt.

## Befehle

Alle Befehle außer `ui` akzeptieren `--json` (gibt ein einzelnes maschinenlesbares JSON-Objekt aus). Erfolg ist Exit-Code 0, ein Benutzerfehler oder eine Ablehnung ist 1. Die Ausgabe enthält keine geheimen Werte.

| Befehl | Beschreibung |
| --- | --- |
| `enable --repo <path> --accept-consent <version> [--mode propose\|push\|off] [--branches a,b] [--documents AGENTS.md,README.md]` | Registriert und aktiviert ein Repository. `--documents` akzeptiert die 4 eingebauten Arten (einschließlich `CHANGELOG.md`; ohne Beachtung der Groß-/Kleinschreibung) und die IDs benutzerdefinierter Arten, die in den Einstellungen definiert sind. |
| `disable --repo <path>` | Entfernt die Registrierung. |
| `pause [--repo <path>]` / `resume [--repo <path>]` | Pausieren / fortsetzen (das Repository des aktuellen Verzeichnisses, wenn `--repo` weggelassen wird). |
| `status` | Zeigt den Installationszustand, die Einstellungen, die Einwilligung, die Registrierungen, die Worker-Einstellungen, die Anzahl der Update-Vorschläge und die Informationen der Setup-Edition (`setup`: ob verwaltet, packId, Version, ausführbare Datei, automatisches Update, ob die Verwaltungsoberfläche bei der Anmeldung dauerhaft aktiv bleibt (`trayAutostart`), Zeitpunkte der letzten Prüfung / des letzten Fehlschlags, die heruntergeladene Version, das Registrierungsergebnis und eine eventuell verbliebene alte eigenständige Installation). `installedVersion` stammt aus dem Eintrag der eigenständigen `install` (`install.json`, oder die `current.json` einer früheren Version, falls diese fehlt). `installForm` gibt die Form des laufenden Programms an (`setup` = Setup-Edition, `standalone` = eigenständige `install`, `other` = im Plugin enthalten oder Entwicklungs-Build), während `installed` nur angibt, ob am Ort der eigenständigen `install` eine ausführbare Datei liegt. |
| `doctor [--verify]` | Diagnostiziert Umgebung und Einstellungen (siehe unten). |
| `run-now [--repo <path>] [--documents <id,...>]` | Führt die Wartung einmal sofort aus, ohne auf einen Push zu warten (nur für registrierte Repositorys mit Einwilligung; das Timing wird ignoriert und alle aktivierten Arten werden verarbeitet). `--documents` schränkt die zu verarbeitenden Arten ein (eine nicht definierte Art oder eine, die für dieses Repository deaktiviert ist, wird abgelehnt, ohne den Worker zu starten). **Hinweis**: Auch ein eingeschränktes `run-now` rückt die verarbeitete Position vor, und da es pro Commit einen Update-Vorschlag gibt, werden Arten außerhalb der Einschränkung für diesen Push nicht gepflegt. Ein propose, das gegen die Prüfungen nach der Wartung verstößt, liefert ein `outcome` von `rejected` (mit einem `reason`; das ist kein Fehlschlag, und dieser Push gilt als verarbeitet). |
| `docs list [--repo <path>]` / `docs show <id> [--repo <path>]` | Listet die Dokumentarten (eingebaute und benutzerdefinierte) auf und zeigt die Details einer Art (aktiviert oder deaktiviert, Zielpfade und ob sie existieren, Grenzen, Timing, Vorlagenstatus, Zeitpunkt des letzten Abschlusses; `show` gibt außerdem die konfigurierte guidance und zusätzliche Anweisungen aus. Nur lesend, und es gibt keine Dokumentinhalte aus). |
| `ui [--port <n>] [--no-open] [--tray]` / `ui --autostart on\|off` | Startet die Verwaltungsoberfläche (siehe oben; `--tray` hält sie dauerhaft aktiv) / schaltet die Einstellung für den Dauerbetrieb bei der Anmeldung der Setup-Edition um. `--json` ist nicht verwendbar. |
| `review [sha\|latest] [--repo <path>]` | Prüft diffstat, Patch-Pfad und Zieldokumente eines Update-Vorschlags. |
| `apply [sha\|latest] [--repo <path>]` | Wendet einen Update-Vorschlag auf das Arbeitsverzeichnis an. Wird abgelehnt, wenn die Zieldokumente nicht committete Änderungen haben. |
| `discard [sha\|latest] [--repo <path>]` | Verwirft einen Update-Vorschlag. |
| `connect claude-code [--apply] [--executable <absolute path>]` / `--remove [--apply]` | Registriert den Stop-Hook in der settings.json (ersetzt alte Wartungs-Hooks Hook für Hook) / entfernt nur Scripturas Hook. Eine Vorschau, bis Sie `--apply` angeben. |
| `update [--check] [--auto on\|off]` | Zeigt den Zustand der automatischen Updates der Setup-Edition / prüft und lädt jetzt herunter (wendet nicht an) / schaltet die automatischen Updates um. |
| `update-apply` | Wendet ein heruntergeladenes Update nur an, wenn kein anderer Scriptura-Prozess läuft, nachdem der Signierer geprüft wurde (wird bei der Anmeldung automatisch gestartet. Das Ergebnis ist eine JSON-Zeile, deren `status` `unmanaged` / `disabled` / `nothingPending` / `busy` / `untrusted` (Exit-Code 2) / `applied` lautet). |
| `uninstall-setup [--apply]` | Entfernt die Registrierung der Setup-Edition (den Stop-Hook, der auf sich selbst zeigt, und das Anwenden bei der Anmeldung). Eine Vorschau, bis Sie `--apply` angeben. |
| `install` / `hook` | Legt es am Installationsort ab (der traditionelle fortgeschrittene Weg; in `install.json` im Zustandsordner festgehalten) / läuft als Stop-Hook (wird normalerweise nicht direkt aufgerufen). |

## Sicherheit

- **Opt-in**: Ohne Einwilligung (versioniert) und Registrierung des Repositorys läuft nichts, und es wird nicht einmal ein Zustandsordner angelegt. Registrierungen werden nur in der eigenen Einstellungsdatei des Benutzers gehalten.
- **Verändert das Arbeitsverzeichnis nicht (propose)**: Der gepushte Commit wird in einen Wegwerf-Klon unter dem Zustandsordner entpackt, und der Worker läuft nur dort. Da der Klon aus dem gepushten Commit aufgebaut wird, existieren die nicht verfolgten und ignorierten Dateien des Benutzers (wie `.env`) im Klon nicht. Das ist jedoch nur die Tatsache, dass „die Dateien nicht im Klon sind“; es ist keine Garantie dafür, dass die Berechtigungsregeln von `restricted` (`Read(./**)` und die Verweigerung sensibler Muster) das Lesen außerhalb des Klons verbieten (ob die Regeln Pfade außerhalb des Klons verweigern, wurde nicht verifiziert). Das `origin` des Klons wird entfernt, sodass selbst ein Push des Workers das Repository des Benutzers nicht erreicht.
- **Sensible Muster**: `.env*` / `*.pem` / `*.key` / `*.p12` / `*.pfx` / `id_rsa*` / `id_ed25519*` / `**/secrets/**` / `.git/config` werden bei `restricted` als Read- / Edit-Verweigerungsregeln des Workers angewendet (`--disallowedTools`). Sie können mit `exclude` ergänzen, aber nicht ersetzen, um welche zu entfernen. Bei `bypass` (altkompatibel; überspringt alle Berechtigungsabfragen) werden diese Regeln nicht angewendet.
- **Prüfungen nach dem Lauf**: Hat der Worker etwas anderes als die Zieldokumente (und `.gitignore`) geändert, wird sein Ergebnis nicht übernommen. Propose vergleicht den gesamten Wegwerf-Klon (einschließlich jedes Wachstums ignorierter Dateien). Der Modus push läuft im echten Arbeitsverzeichnis des Benutzers, daher vergleicht er nur unter `restricted` die verfolgten und nicht verfolgten Dateien des Arbeitsverzeichnisses vor und nach dem Worker (Änderungen an ignorierten Dateien wie `target/` sind ausgenommen, da sie sich nicht von der gleichzeitigen Arbeit des Benutzers unterscheiden lassen), während `bypass` wie bisher nur die Hash-Prüfung der zu committenden Dokumente durchführt. Im Modus push werden unsichere Commit- und Push-Zustände nicht erneut ausgeführt, sondern über die Zustands-DB abgeglichen. Grenzen, erforderliche Überschriften und die Unveränderlichkeit veröffentlichter CHANGELOG-Einträge werden unabhängig vom Berechtigungsmodus geprüft (siehe oben).
- **Umgebungshygiene**: Relay-Einstellungen aus der übergeordneten Sitzung (etwa der Desktop-App) werden aus den an den Worker übergebenen Umgebungsvariablen entfernt.

## Kosten und Datenübertragung

Der Wartungs-Worker (standardmäßig Sonnet) läuft mit **Ihrer eigenen Claude-Authentifizierung**. Die Repository-Inhalte, die der Worker liest, werden an Anthropic gesendet, und bei jedem Lauf fallen Claude-Nutzungsgebühren (Verbrauch) an. Der Worker läuft höchstens einmal pro Push, wobei `worker.timeout_secs` die Obergrenze bildet.

## Diagnose (doctor)

`scriptura doctor [--verify] [--json]` liefert für jeden Punkt `{id, status, detail}` (status ist `ok` / `warn` / `fail` / `info`). Gibt es ein `fail`, ist der Exit-Code 1.

Geprüft werden die unterstützte Plattform, `git`, `claude` (Auflösung der ausführbaren Datei), `claude auth status` (in derselben Umgebung wie der Worker ausgeführt, wobei nur Exit-Code und `authMethod` betrachtet werden), `gh` (nur wenn es ein GitHub-Repository im Modus push gibt), Einstellungen und Einwilligung, ob der Zustandsordner beschreibbar ist, doppelte Hook-Registrierungen, `allowManagedHooksOnly` / `disableAllHooks` sowie verbliebene Registrierungen des alten `agents-maintenance`. Für die Setup-Edition werden außerdem die doppelte Registrierung zwischen Plugin und Setup-Edition (`setupPlugin`), das Vorhandensein des Hooks der Setup-Edition (`setupHook`), eine verbliebene alte eigenständige Installation (`legacyInstall`) und der Zustand der automatischen Updates (`autoUpdate`) gemeldet (bei einer Nicht-Setup-Edition nur ein einzelner `info`-Punkt, `setup`). Der Punkt `kinds` prüft, ob die Vorlagen der Dokumentarten gelesen werden können (Ort und Größe), und meldet `info`, wenn eine Grenze kleiner ist als die aktuelle Größe eines bestehenden Dokuments (die nächste Wartung könnte abgelehnt werden) und wenn es ein Repository gibt, das `CHANGELOG.md` im Modus push pflegt (ein Konflikt mit dem Release-Verfahren). `--verify` prüft nur das SHA-256 der Binärdateien, die in der mitgelieferten `SHA256SUMS` des Plugins aufgeführt sind (`bin/scriptura.exe` und `libexec/scriptura-*`) (Launcher, Hooks und Skills sind nicht erfasst; ausführbare Dateien außerhalb des Plugins ebenfalls nicht).

## Zustand und Speicherorte

| Ort | Windows | macOS | Linux |
| --- | --- | --- | --- |
| Installationsort (die feste ausführbare Datei `bin\scriptura(.exe)`) | `%LOCALAPPDATA%\Programs\Scriptura` | `~/Library/Application Support/Scriptura/app` | `$XDG_DATA_HOME/scriptura` (Standard `~/.local/share/scriptura`) |
| Zustand (DB, Sperren, Veröffentlichungen, Update-Vorschläge, der Änderungsverlauf der Einstellungen `config-history/`) | `%LOCALAPPDATA%\Scriptura\state` | `~/Library/Application Support/Scriptura/state` | `$XDG_STATE_HOME/scriptura` (Standard `~/.local/state/scriptura`) |

Wird `SCRIPTURA_HOME` auf einen absoluten Pfad gesetzt, liegt der Zustand in `<SCRIPTURA_HOME>/state`, der Installationsort in `<SCRIPTURA_HOME>/app` und die Einstellungen in `<SCRIPTURA_HOME>/config.toml`. Mit `SCRIPTURA_DB` lässt sich auch nur die Zustands-DB an einen anderen Ort verlegen. Vorlagen kommen in `templates/` im selben Ordner wie die Einstellungsdatei. Es wird nichts in das Codex- oder Claude-Code-Home geschrieben (außer wenn `connect claude-code --apply` / `--remove`, die automatische Registrierung der Setup-Edition und `uninstall-setup --apply` die `settings.json` umschreiben). Der Installationsort der Setup-Edition (Windows `%LOCALAPPDATA%\ScripturaApp`) wird von Velopack verwaltet und bei Updates ersetzt; dort werden daher weder Zustand noch Einstellungen aufbewahrt.

## Automatische Migration von Altbenutzern

Ein Benutzer, der ein älteres Scriptura (eine Version ohne Einstellungsdatei) ausgeführt hat und noch eine Zustands-DB besitzt, erhält beim Ausführen eines Hooks ohne Einstellungsdatei automatisch genau einmal eine Einstellungsdatei mit demselben Verhalten wie zuvor (alle Repositorys, Modus `push`, Berechtigung bypass) und wird darüber benachrichtigt. Der Ursprung der Einwilligung wird als `legacy-migration` festgehalten. Für neue Benutzer wird nichts angelegt. Wenn Sie die Ziele eingrenzen möchten, registrieren Sie erneut mit `enable` und setzen Sie `policy = "explicit"`. Der Zustand des alten `agents-maintenance` (`~/.codex/hooks/state`) wird nicht übernommen; es beginnt mit einem leeren Zustand.

## Nicht unterstützt / nicht verifiziert

- Auf echter Hardware ist nichts verifiziert (Entwicklung und Verifikation erfolgen unter Windows). Die CI auf von GitHub bereitgestellten Runnern ist so eingerichtet, dass sie Linux (x64 / arm64) mit einem isolierten E2E der Setup-Edition prüft und macOS baut, testet, signiert und notarisiert; das Anwenden von Updates unter macOS ist nicht verifiziert. macOS Intel ist nicht Gegenstand. Der Code der Setup-Edition für macOS und Linux (LaunchAgent, XDG-Autostart, Prozesserkennung sowie Signatur- und Ortsprüfungen) ist eine Portierung der Implementierung von Remnifold, und es wurde nicht einmal bestätigt, dass er unter Windows kompiliert (er wird nur in der CI geprüft).
- Das Umschalten der automatischen Updates in der Verwaltungsoberfläche (`ui`) wird nicht unterstützt (verwenden Sie `scriptura update --auto on|off`).
- Unter Windows erbt der Run-Wert der Anmeldung `SCRIPTURA_HOME` nicht (der macOS-LaunchAgent und der Linux-Autostart-Eintrag tun dies).
- Auf Nicht-Windows-Plattformen wird das Fortsetzen einer Wartung, die während der Ausführung des Workers unterbrochen wurde, übersprungen (es gibt keine Möglichkeit zu bestätigen, dass der Worker beendet ist).
- Die Veröffentlichung über Branch + PR ist für später geplant. Benutzerdefinierte Arten sind auf feste Pfade beschränkt (Globs und das Durchsuchen von Unterverzeichnissen folgen später), und ebenfalls für später geplant sind Überschreibungen von Modell und Effort pro Art, Art-Einstellungen pro Repository (`[repository.kinds]`), das erzwungene Anwenden von Vorlagen, das Bearbeiten von Einstellungen über die CLI (`set`).
- Der Dauerbetrieb der Verwaltungsoberfläche (`ui --tray`) ist nur unter Windows und macOS verfügbar (Linux wird nicht unterstützt). Das Symbol ist ein vorläufiges Design (unter Windows zur Laufzeit gezeichnet, unter macOS ein Systemsymbol). Unter macOS wurden der Start und das Beenden mit Strg+C auf echter Hardware bestätigt, doch das Aussehen des Menüleistensymbols, Klicks und das Menü wurden nicht mit eigenen Augen geprüft. Der Dauerbetrieb bei der Anmeldung und der Start der dauerhaft laufenden Oberfläche nach dem Anwenden eines Updates (Setup-Edition) wurden in einer echten Installation nicht verifiziert.

## Verifikation

```powershell
Set-Location D:\Dev\Scriptura
cargo test --locked --features test-support -- --test-threads=1
cargo build --release
cargo build --release --no-default-features   # a build without the admin UI (the ui feature)
git diff --check
```

Die Verifikation der Setup-Edition (Velopack) wird wie folgt isoliert, damit sie nie die echte Installation, `~/.claude/settings.json` oder den HKCU-Run-Wert berührt.

- Verwenden Sie eine Test-packId (zum Beispiel `ScripturaVelopackTest`; verwenden Sie nicht `ScripturaApp`) und ein `--installto` unter einem externen Arbeitsordner.
- Richten Sie `SCRIPTURA_HOME`, `CLAUDE_CONFIG_DIR` und `TEMP`/`TMP` prozessweise auf den Arbeitsordner (unter macOS und Linux außerdem `HOME` und `XDG_CONFIG_HOME`).
- Bauen Sie die ausführbare Datei mit `cargo build --release --features update-feed-test`. Nur Builds mit diesem Feature lesen `SCRIPTURA_UPDATE_FEED_DIR` (macht die Update-Quelle zu einem lokalen Ordner) und `SCRIPTURA_UPDATE_ALLOW_UNSIGNED=1` (erlaubt das Anwenden unsignierter Pakete). Nehmen Sie es niemals in Produktions-Builds auf.
- Entfernen Sie am Ende den Test-Run-Wert (`<test packId> Update`), die Deinstallationsinformationen und den Installationsort.
- Die Registry-Unit-Tests verwenden einen eigens dafür vorgesehenen, auf eine Datei beschränkten Hive (`RegLoadAppKeyW`), nicht den echten Run-Schlüssel.

Die Tests verwenden lokale, isolierte Git-Repositorys und native Stubs und rufen weder die echte Claude CLI noch ein externes Repository auf. Der Ordner `validation` (oder der durch `SCRIPTURA_TEST_BASE` angegebene Ort) wird nicht automatisch gelöscht; prüfen Sie daher den ausgegebenen absoluten Pfad und räumen Sie ihn mit Prunlet auf. `hook-test-stub` ist ausschließlich dem Feature `test-support` vorbehalten und wird nicht ausgeliefert.
