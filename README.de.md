[日本語](README.ja.md) | [English](README.md) | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md) | [한국어](README.ko.md) | [Español](README.es.md) | [Français](README.fr.md) | **Deutsch** | [Português (Brasil)](README.pt-BR.md)

# Scriptura

Scriptura pflegt `AGENTS.md` / `DESIGN.md` / `README.md` (sowie, als Opt-in, `CHANGELOG.md` und benutzerdefinierte Dokumente) im Hintergrund, nachdem Sie in Ihre eigenen Repositorys gepusht haben. Gestartet wird es über Claude-Code-Hooks (direkt nach einem über das Bash- oder PowerShell-Tool ausgeführten `git push` und beim Start einer Sitzung, um Verpasstes nachzuholen), und als Wartungs-Worker dient die Claude Code CLI (standardmäßig `claude -p --model claude-sonnet-5-5 --effort high`). Es hängt nicht von Codex ab.

- **Standardmäßig sicher.** Nichts läuft, bis Sie ein Repository mit `scriptura enable` (oder in der Verwaltungsoberfläche) registrieren, und das Aktivieren erfordert ein Remote und einen Upstream. Es gibt keinen Prüfschritt: Nach jedem Push aktualisiert der Worker die Zieldokumente, und Scriptura committet und pusht sie automatisch; eine Prüfung nach der Wartung verweigert den Commit, wenn etwas anderes als die Zieldokumente verändert wurde. Einwilligungsprotokoll und der frühere Modus `propose` (Update-Vorschläge) wurden entfernt; siehe „Migration alter Einstellungen“.
- Einstellungen werden ausschließlich aus einer einzigen Datei im Home-Verzeichnis des Benutzers (`config.toml`) und dem daneben liegenden Ordner `templates/` gelesen. Dateien innerhalb eines Repositorys zum Aktivieren oder Konfigurieren werden nie gelesen (damit schon das bloße Klonen eines fremden Repositorys niemals ein automatisches Verhalten auslösen kann).
- Einstellungen lassen sich auch in `scriptura ui` bearbeiten (der browserbasierten Verwaltungsoberfläche, siehe unten).

Dies ist der Teil zur Dokumentenpflege des früheren `agents-maintenance` (CodexHooks), herausgelöst als eigenständiges Werkzeug. Die Verträge für Wartungsentscheidungen, Zustandsverwaltung und Veröffentlichung (Commit und Push) sind dieselben.

## Installation

### Plugin (empfohlen)

Wenn Sie es als Claude-Code-Plugin installieren, werden der Hook und die Betriebs-Skills (wie `/scriptura:setup`) gemeinsam eingerichtet. Die Binärdatei `scriptura` ist im Plugin enthalten (unter Windows `bin/scriptura.exe`; unter macOS / Linux wählt der Launcher `bin/scriptura` die Binärdatei unter `libexec/` aus. Auch unter Windows übergibt derselbe Launcher an `bin/scriptura.exe`, wenn der Hook über die sh von Git Bash läuft). Wie die Distribution gebaut wird, steht unter `xtask/` (`cargo run --manifest-path xtask/Cargo.toml --locked -- package`).

### Setup-Edition (automatische Updates)

Das Ausführen von `ScripturaApp-win-Setup.exe` (Windows), `ScripturaApp-osx-Setup.pkg` (macOS) oder des AppImage `ScripturaApp-linux-x64.AppImage` / `ScripturaApp-linux-arm64.AppImage` (Linux x64 / arm64) installiert pro Benutzer ohne Administratorrechte (unter Windows liegt die Installation in `%LOCALAPPDATA%\ScripturaApp\current\scriptura.exe`). Updates werden von `https://scriptura-updates.kagayoi.com` bereitgestellt (Velopack-Feeds mit dem Namen `releases.<channel>.json`, mit den Kanälen `win` / `osx` / `linux-x64` / `linux-arm64`). Die Windows-Distribution ist mit einem Certum-Zertifikat signiert, die macOS-Distribution mit einer Developer ID signiert und notarisiert. Voraussetzungen: macOS 11 oder neuer auf Apple Silicon (Intel wird nicht unterstützt); Linux mit glibc 2.39 oder neuer.

- **Die Registrierung erfolgt automatisch**: Bei der Installation (unter Windows über den Install-Hook; unter macOS und Linux beim ersten Start ohne Argumente nach der Installation) wird der Hook in der `settings.json` von Claude Code auf die ausführbare Datei der Setup-Edition gesetzt. Es werden nur Scripturas eigene Hooks ersetzt (ein Hook, der auf die alte eigenständige Installation `%LOCALAPPDATA%\Programs\Scriptura\bin\scriptura.exe` zeigt, wird durch den der Setup-Edition ersetzt, und die alten Dateien werden nicht gelöscht). Bestehende `agents-maintenance`-Hooks werden nicht ersetzt; `status` / `doctor` / ein Start ohne Argumente weisen Sie auf `scriptura connect claude-code --apply` hin.
- **Doppelungen mit dem Plugin vermeiden**: Ist `scriptura@*` in `enabledPlugins` der `settings.json` aktiviert, registriert sich die Setup-Edition nicht in der `settings.json` (die Wartung würde sonst doppelt laufen). Entscheiden Sie sich entweder dafür, das Plugin weiter zu verwenden, oder dafür, das Plugin zu deaktivieren und zur Setup-Edition zu wechseln. Nach dem Deaktivieren erfolgt die Registrierung automatisch bei einem Start ohne Argumente oder bei der nächsten Anmeldung.
- **Automatische Updates (standardmäßig an)**: Nachdem die Wartung eines Hooks abgeschlossen ist, wird höchstens einmal pro Tag nach einer neuen Version gesucht (für Erfolge und Fehlschläge jeweils auf 24 Stunden gedrosselt) und diese nur heruntergeladen. **Angewendet wird sie ausschließlich bei der Anmeldung**: `scriptura update-apply`, das bei der Anmeldung läuft (unter Windows der HKCU-Run-Wert `<packId> Update`; unter macOS der LaunchAgent `com.kagayoi.<packId>.update`; unter Linux der XDG-Autostart-Eintrag `com.kagayoi.<packId>.update.desktop`), prüft, dass kein Scriptura-Hook und keine Verwaltungsoberfläche läuft, und kontrolliert den Signierer; danach wendet es das Update ohne Neustart an. Läuft irgendetwas, tut es nichts und überlässt das Update der nächsten Anmeldung (so wird ein Hook mitten in der Wartung nie beendet). Auf Wunsch lässt es sich auch mit „Update installieren“ in der Verwaltungsoberfläche (unten) anwenden.
- Der Zustand liegt im Zustandsordner (siehe unten): `app-update.json` (`auto_update` sowie Zeitpunkte der letzten Prüfung bzw. des letzten Fehlschlags), `app-update.log` (für die Diagnose) und `setup-registration.json` (das Registrierungsergebnis).

```powershell
scriptura update                 # show the auto-update setting, the last check, and the downloaded version
scriptura update --check         # ignore throttling and check/download now (applied at the next logon, or by running update-apply manually)
scriptura update --auto off      # turn auto-update off (on turns it back on)
scriptura update-apply           # apply a downloaded update (normally runs automatically at logon)
scriptura uninstall-setup        # preview removing the Setup edition's registration (hook and logon apply)
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

`connect claude-code --apply` trägt den festen Installationspfad im Hook der `settings.json` von Claude Code ein (`CLAUDE_CONFIG_DIR` oder `~/.claude`). Dabei werden Hook für Hook nur die Hooks ersetzt, die das frühere `agents-maintenance` und alte Scriptura-Hooks aufrufen (andere Hooks im selben Eintrag bleiben erhalten); alle übrigen Einstellungen bleiben unberührt. Ein Befehl gilt nur dann als Scriptura-Hook, wenn er die Form `<executable> hook` hat und der Name der ausführbaren Datei `scriptura(.exe)` lautet; andere Werkzeuge, die lediglich scriptura im Pfad haben, sind nicht betroffen. Vor der ersten Änderung wird `settings.json.scriptura-backup` angelegt, und fehlerhaftes JSON wird nicht umgeschrieben.

Die frühere Registrierung nutzte einen Stop-Hook, der bei jedem Turn ausgelöst wird. `--apply` und die Registrierung der Setup-Edition entfernen Scripturas alten Stop-Hook und registrieren stattdessen drei Hooks: PostToolUse und PostToolUseFailure (Letzterer wird ausgelöst, wenn ein Befehl wie `git push && gh pr create` nach dem Push fehlschlägt) für Befehle mit `push` in den Tools Bash und PowerShell sowie SessionStart bei `startup` / `resume`; `--remove` entfernt alte wie neue. Die Hooks nach einem Befehl werden über eine Berechtigungsregel pro Tool eingegrenzt, `Bash(* push*)` (und `PowerShell(* push*)`), sodass `git push`, `rtk git push`, `git -C <path> push`, `FOO=1 git push`, `bash -lc 'git push'` und `make push` alle zutreffen; ein überzähliger Treffer wie `echo push x` findet schlicht keinen neuen Push und tut nichts. Diese Hooks laufen asynchron und warten, statt zu überspringen, bis zu 25 Minuten auf das Ende eines anderen Wartungslaufs, sodass ein Push während einer Wartung danach trotzdem verarbeitet wird. Ein Push außerhalb von Claude Code oder in ein anderes Repository als das Arbeitsverzeichnis der Sitzung (`git -C <anderer Pfad> push`) wird beim nächsten Sitzungsstart in diesem Repository nachgeholt. Der alte Stop-Hook wird weiterhin akzeptiert (ohne Warten), und wird Scriptura darüber aufgerufen, migriert es die alte Registrierung in Ihrer benutzerweiten settings.json selbstständig und nur einmal auf die neuen Hooks und teilt das per Meldung mit (eine Sicherung bleibt als `settings.json.scriptura-migration-backup[-N]` erhalten; die `.claude/settings*.json` des Projekts werden nie angefasst, und eine mit `--remove` entfernte Registrierung wird nicht wiederhergestellt). Ist das scriptura-Plugin aktiviert, wird nur der alte Hook entfernt, ohne neue hinzuzufügen. Die Setup-Edition migriert über ihre eigene Registrierung.

Beim Wechsel zum Plugin entfernen Sie die eigenständige Registrierung, damit die Wartung nicht doppelt läuft (dabei werden nur Scripturas Einträge entfernt; `agents-maintenance` und andere Einstellungen bleiben unberührt).

```powershell
scriptura connect claude-code --remove           # preview
scriptura connect claude-code --remove --apply   # write
```

Der Worker erbt die Relay-Einstellungen der Desktop-Sitzung nicht. Führen Sie daher vorab `claude auth login` in der eigenständigen CLI aus. Verwendet wird die CLI aus der Einstellung `worker.claude_path`, andernfalls `claude` im PATH (`claude.exe` unter Windows) und unter Windows zusätzlich die neueste `%LOCALAPPDATA%\Claude-Data\claude_*\claude-code\<version>\<hash>\claude.exe`, die mit der Claude-Desktop-App gebündelt ist.

## Ersteinrichtung

Führen Sie in dem Repository, das gepflegt werden soll, eines der folgenden Verfahren aus.

- Wenn Sie das Plugin installiert haben: Führen Sie in Claude Code `/scriptura:setup` aus (es lässt Sie im Chat bestätigen, dass Dokumente automatisch committet und gepusht werden, und führt dann `scriptura enable` aus).
- Direkt über die Kommandozeile:

```powershell
scriptura enable --repo D:\Dev\MyRepo
```

`enable` erfordert ein Remote, einen Branch (kein losgelöstes HEAD) und einen Upstream; wird `--branches` weggelassen, wird nur der aktuelle Branch berücksichtigt. Danach werden Dokumente nach jedem Push automatisch committet und gepusht (siehe „Kosten und Datenübertragung“). Vor der Registrierung hält `enable` den „aktuellen Push-Stand“ als bereits verarbeitet fest, sodass Pushes vor dem Aktivieren danach nie verarbeitet werden. Die früheren Optionen `--mode` und `--accept-consent` wurden entfernt und werden mit einem Fehler abgelehnt.

## Konfigurationsdatei

| Betriebssystem | Speicherort |
| --- | --- |
| Windows | `%LOCALAPPDATA%\Scriptura\config.toml` |
| macOS | `~/Library/Application Support/Scriptura/config.toml` |
| Linux | `$XDG_CONFIG_HOME/scriptura/config.toml` (Standard `~/.config/scriptura/config.toml`) |

Sie können den Ort mit `SCRIPTURA_CONFIG` (ein absoluter Dateipfad) oder `SCRIPTURA_HOME` (`<SCRIPTURA_HOME>/config.toml`) überschreiben. Die Datei wird normalerweise von `enable` / `disable` / `pause` / `resume` und der Verwaltungsoberfläche umgeschrieben, sodass Sie sie nicht von Hand bearbeiten müssen. Eine defekte Konfiguration, unbekannte Schlüssel oder ein zu neues `schema` führen zum Rückfall auf „nichts tun“, und Befehle wie `enable` schlagen fehl, ohne sie umzuschreiben.

Beim Speichern wird immer `schema = 3` geschrieben. Eine Datei mit `schema` 1, 2 oder ohne Angabe (von einer älteren Scriptura-Version geschrieben) wird als altes Format gelesen und beim Lesen umgewandelt (siehe „Migration alter Einstellungen“); ein Schema, das neuer als 3 ist, fällt auf „nichts tun“ zurück. Ältere Scriptura-Versionen, die eine schema-3-Datei lesen, tun ebenfalls nichts; wenn Sie Plugin und Setup-Edition gemeinsam nutzen, aktualisieren Sie daher beide.

```toml
schema = 3

[defaults]
documents = ["AGENTS.md", "DESIGN.md", "README.md"]
exclude = ["private/**"]                        # ADDED to the default sensitive patterns (patterns containing `)`, whitespace, or control characters are not allowed)

[worker]
model = "claude-sonnet-5-5"
effort = "high"
timeout_secs = 1200                             # 60..=1500 (shorter than the hook's 1800 seconds)
permission = "restricted"                       # restricted / bypass
claude_path = ""                                # empty means search PATH (an absolute path if specified)

[enrollment]
policy = "explicit"                             # explicit / remote-owner / all
owners = []                                     # for remote-owner, e.g. "github.com/acme"

[[repository]]
root = "D:/Dev/MyRepo"
remote = "github.com/me/myrepo"                 # expected push destination (skipped if it does not match)
branches = ["main"]                             # empty means no restriction
documents = ["README.md"]
paused = false
```

## Dokumentarten

Die zu pflegenden Dokumente werden als „Arten“ (kinds) verwaltet.

In der Verwaltungsoberfläche werden dieselben Einstellungen unter Einstellungen > Dokumente bearbeitet; dort werden auch die standardmäßigen Zieldokumente (`defaults.documents`) festgelegt.

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
| `split_dir` | Ordner, in den Details über den Grenzen verschoben werden dürfen (Standard `docs/<id>`; nur für Arten mit `max_chars` / `max_bytes`; nicht für `changelog`). Bis zu 20 Dateien, ein Thema pro Datei. |
| `instructions` | Zusätzliche Bearbeitungsanweisungen (höchstens 2000 Zeichen). Zusammen mit `[defaults] instructions` (gilt für alle Arten) in zwei Stufen gelangen sie nur in den Abschnitt „aus den Benutzereinstellungen“ am Ende des Prompts. Sie können weder die Zielpfade noch die Sicherheitsregeln noch den Umgang mit Commit und Push ändern. |
| `language` | Die Sprache des Dokuments (`ja` / `en-US` usw.). Wird sie weggelassen, folgt das Dokument der Sprache des bestehenden Dokuments. Dies ist die Sprache der erzeugten Dokumente und unabhängig von der Sprache der Oberflächen und Ausgaben (die Prompts des Workers sind auf Englisch, das Verhalten von `language` bleibt jedoch unverändert). |
| `required_headings` | Überschriften, deren Vorhandensein nach der Wartung geprüft wird (höchstens 20). |
| `timing` | Wann die Art läuft. Siehe unten. |
| `changelog` | CHANGELOG-Einstellungen (nur für die Art `changelog`). Siehe unten. |

**Vorlagen** können nur als `templates/<name>.md` im Einstellungsordner angegeben werden (Dateien innerhalb des Repositorys und beliebige absolute Pfade sind nicht zulässig; für CHANGELOG ist `builtin` die eingebettete Keep-a-Changelog-Vorlage). Zur Laufzeit wird geprüft, dass die Datei eine reguläre Datei (kein Link) ist, höchstens 16 KiB groß, UTF-8, frei von Steuerzeichen außer Zeilenumbruch und Tabulator, frei von `<<<` und außerhalb des Repositorys liegt; schlägt eine Prüfung fehl, wird der Worker nicht gestartet, die verarbeitete Position nicht vorgerückt und der Lauf übersprungen (er wird beim nächsten Mal verarbeitet, sobald Sie die Datei korrigiert haben). Eine Vorlage dient als Struktur beim Anlegen eines neuen Dokuments; bei bestehenden Dokumenten hat die vorhandene Struktur Vorrang.

### Prüfungen nach der Wartung

Limits (`max_chars` / `max_bytes`), `required_headings`, dass AGENTS unter 30.000 Bytes bleibt, und die Unveränderlichkeit veröffentlichter CHANGELOG-Einträge werden von Rust unabhängig vom Berechtigungsmodus des Workers (`restricted` / `bypass`) geprüft. Verstoßen Limits oder erforderliche Überschriften weiterhin, wird der Worker im selben Hook bis zu zweimal erneut gestartet, um nur diese Verstöße zu beheben; bleiben sie bestehen, wird mit einer Warnung committet und gepusht (die Warnung erscheint in der Hook-Meldung, als `warnings` in `run-now --json` und im Verlauf). Bleibt AGENTS bei 30.000 Bytes oder mehr oder wurden veröffentlichte CHANGELOG-Einträge geändert, wird die Ausgabe des Workers verworfen und die Pflege ohne Commit abgeschlossen.

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

- Es folgt dem bestehenden Format (die eingebettete Keep-a-Changelog-Vorlage wird nur beim Anlegen der Datei verwendet). Jeder Push aktualisiert nur den Abschnitt `Unreleased`, und der Worker kann veröffentlichte Abschnitte nicht ändern. Rust vergleicht den Stand vor und nach der Wartung; hat sich auch nur ein Byte eines veröffentlichten Abschnitts geändert, ist ein Abschnitt verschwunden oder umsortiert worden oder gingen Linkreferenz-Definitionen verloren, gilt das als Verstoß gegen die obigen Prüfungen (es wird nichts committet).
- Für die Historie sammelt Rust alles vom Commit, der `CHANGELOG.md` zuletzt geändert hat, bis zu diesem Push und übergibt es dem Prompt als **nicht vertrauenswürdige Daten** (Commit-Betreffzeilen, geänderte Pfade und Tags im Bereich, insgesamt höchstens 48 KiB). Das liegt daran, dass ein eingeschränkter Worker `git log` nicht ausführen kann. Scripturas eigene Wartungs-Commits werden ausgeschlossen.
- `on_release = "promote"` (Standard): Gibt es für die zu einem Tag im Bereich gehörende Version noch keinen Abschnitt, wird der Inhalt von `Unreleased` in den Abschnitt dieser Version verschoben. Existiert der Abschnitt bereits, geschieht nichts (das Ergebnis ist dasselbe, auch wenn das Release-Verfahren ihn zuerst geschrieben hat). `ignore` legt keine Versionsabschnitte an.
- `backfill = "tags"` (Standard): Existiert `CHANGELOG.md` nicht, legt es Abschnitte pro Version an, indem es erreichbare Tags, neueste zuerst, zurückverfolgt, bis zu `backfill_max_releases` (1 bis 50, Standard 20) Tags und insgesamt bis zu `backfill_max_commits` (1 bis 1000, Standard 300) Commits zwischen benachbarten Tags. `none` geht nicht zurück. Auch `format` (`existing` / `keep-a-changelog`) und `release_tag` (Standard `v*`) können angegeben werden.
- Der Dateiname wird anhand seiner Schreibweise in Git ohne Beachtung der Groß-/Kleinschreibung aufgelöst (auch `Changelog.md` und Ähnliches sind Ziele). Lässt sich die Schreibweise nicht eindeutig bestimmen oder der Inhalt vor der Bearbeitung nicht aus Git lesen, gilt das als nicht prüfbar und zählt als Verstoß. Es wird eine neue Wartung übersprungen, ohne den Worker zu starten, wenn `CHANGELOG.md` von Git ignoriert wird.
- Beim Abgleich von Versionen mit Tags gilt auch der Versionsname ohne den festen Präfix von `release_tag` (das `v` von `v*`, das `release-` von `release-*` usw.) als Treffer (`## [1.2.0]`). Verstoßgründe geben keinen Überschriftentext aus, sondern werden per Nummer angegeben, etwa „der n-te Versionsabschnitt“. Bei Linkreferenz-Definitionen veröffentlichter Abschnitte wird zudem geprüft, dass Inhalt und Reihenfolge erhalten bleiben und bestehende Labels nicht neu definiert werden.
- Einschränkungen: Ein reiner Tag-Push (`git push --tags`) kann nicht erkannt werden. Er wird beim nächsten Branch-Push oder mit `--follow-tags` erfasst. Wenn `CHANGELOG.md` gepflegt wird, kann ein Release-Verfahren, das dieselbe Datei aktualisiert, in Konflikt geraten (`doctor` weist darauf hin).

### Beispielkonfiguration (Schema 3)

```toml
schema = 3

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

Alle Schreibvorgänge auf Einstellungen, ob über die CLI, die Verwaltungsoberfläche oder die automatische Migration von Altbenutzern, laufen über denselben Pfad (`src/ops.rs` und `config::update_in`). Schreibende werden über `config-lock.sqlite3` im Zustandsordner serialisiert, und die Verwaltungsoberfläche vergleicht die Version, die sie angezeigt hat (das SHA-256 der Datei, oder `missing`, wenn die Datei nicht existiert); wurde die Datei anderswo geändert, speichert sie nicht und fordert Sie zum Neuladen auf (die CLI vergleicht keine Versionen). Unmittelbar vor dem Speichern wird der vorherige Inhalt in `config-history/` im Zustandsordner beiseitegelegt (50 Einträge bleiben erhalten), und das Speichern ist atomar. Eine defekte Konfiguration wird nicht umgeschrieben. Eine Operation, die nichts ändert, speichert nichts und legt nichts beiseite. Über „Einstellungsänderungen“ in der Verwaltungsoberfläche können Sie Differenzen einsehen und wiederherstellen; auch der Inhalt vor einer Wiederherstellung bleibt im Verlauf erhalten. Würde eine Wiederherstellung Registrierungen hinzufügen (auch pausierte), wird der Startpunkt jeder Registrierung (die Position, die verhindert, dass Pushes vor dem Aktivieren verarbeitet werden) vor dem Speichern festgehalten. Eine Wiederherstellung, die aktive Registrierungen (weder pausiert noch durch die Migration gestoppt) hinzufügen oder Registrierungen aktiv machen würde, wird abgelehnt und auf `enable` verwiesen (das Remote und Upstream prüft); eine durch die Migration gestoppte Registrierung lässt sich auch durch eine Wiederherstellung nicht fortsetzen.

## Verwaltungsoberfläche (`scriptura ui`)

```powershell
scriptura ui [--port <n>] [--no-open] [--tray]
scriptura ui --autostart on|off
```

Eine browserbasierte Oberfläche für Repositories, Verlauf, Einstellungen (einschließlich Dokumente) und Diagnose. Sie ist in 9 Sprachen verfügbar (ja, en, zh-CN, zh-TW, ko, es, fr, de, pt-BR). Die Sprache wird in dieser Reihenfolge gewählt: die Sprachauswahl oben auf der Seite (in einem Cookie gespeichert) → der `Accept-Language` des Browsers → Englisch. Standardmäßig läuft sie nicht dauerhaft: Der Befehl, der sie gestartet hat, endet, wenn Sie auf der Seite „Quit“ wählen, Strg+C drücken oder 60 Minuten lang nichts geschieht (mit `--tray` bleibt sie dauerhaft aktiv; siehe unten). Hooks prüfen nicht, ob die Oberfläche geöffnet ist. Sie wird durch das Cargo-Feature `ui` gesteuert (standardmäßig aktiviert) und ist bei einem Build mit `--no-default-features` nicht enthalten (`scriptura ui` lehnt dann ab und beendet sich).

- **Lauschen**: nur auf `127.0.0.1` (der Standardport ist 7468; `--port 0` wählt einen freien Port). Ist der Port belegt, wird beendet.
- **Dauerbetrieb (`--tray`)**: Bleibt mit einem Symbol im Infobereich der Taskleiste unter Windows bzw. in der Menüleiste unter macOS dauerhaft aktiv (Linux wird nicht unterstützt; `--tray` lehnt ab und beendet sich). Ein Klick auf das Symbol öffnet die Verwaltungsoberfläche, und das Rechtsklickmenü bietet „Verwaltungsoberfläche öffnen“, „Nach Updates suchen“ (nur Setup-Edition; öffnet das Panel „App-Updates“ der Übersicht) und „Beenden“. Im Dauerbetrieb endet sie nicht bei Inaktivität. Stattdessen läuft die Sitzung nach 60 Minuten Inaktivität ab (offen gelassene Tabs erhalten 401), und bei jedem Öffnen der Oberfläche über das Symbol wird ein neuer Einmalschlüssel ausgestellt (ein neu ausgestellter Schlüssel läuft nach 5 Minuten ab, und ein früherer, nicht verwendeter Schlüssel wird ungültig). Unter Windows startet ein erneutes `scriptura ui --tray`, während bereits eine Instanz auf dem Standardport dauerhaft läuft, keine zweite; die laufende Instanz öffnet den Browser. Wird sie aus etwas anderem als einem Terminal gestartet (etwa dem Startmenü), startet sie sich selbst als Prozess ohne Konsolenfenster neu.
- **Dauerbetrieb bei der Anmeldung (nur Setup-Edition; standardmäßig an)**: Bleibt aktiv, sofern Sie es nicht mit `scriptura ui --autostart off` oder dem Kontrollkästchen im Panel „App-Updates“ ausschalten (`tray_autostart` in `app-update.json` im Statusordner; ein früher gespeichertes Aus bleibt erhalten, ein fehlender oder unlesbarer Wert gilt als an). Es wird kein zusätzlicher Anmeldeeintrag angelegt; die dauerhaft laufende Oberfläche wird gestartet, nachdem das `update-apply` bei der Anmeldung abgeschlossen ist (würde sie zuerst starten, gälte der Installationsordner als von einem anderen Prozess belegt, und Updates würden nie angewendet). Bei einer Anmeldung, bei der ein Update angewendet wird, startet der Updater nach dem Austausch die dauerhaft laufende Oberfläche der neuen Version. Der Startmenüeintrag der Setup-Edition (ein Start ohne Argumente) zeigt seinen Hinweis und hält danach die Verwaltungsoberfläche dauerhaft aktiv und öffnet sie im Browser. Da Scriptura im Dauerbetrieb weiter aus dem Installationsordner läuft, wird ein heruntergeladenes Update bei der nächsten Anmeldung angewendet oder sofort mit „Update installieren“ im Panel (unten). `--autostart` kann mit der im Plugin mitgelieferten oder der eigenständigen `install`-Programmdatei nicht verwendet werden (der Dauerbetrieb mit `ui --tray` funktioniert weiterhin).
- **Panel „App-Updates“ (nur Setup-Edition)**: Das Panel „App-Updates“ (`#updates`) der Übersicht zeigt die aktuelle Version und bietet Kontrollkästchen für automatische Updates und den Dauerbetrieb bei der Anmeldung, eine Schaltfläche „Nach Updates suchen“ sowie eine Schaltfläche „Update installieren“ (sichtbar, wenn eine neue Version vorhanden ist). Die Suche ignoriert das 24-Stunden-Intervall und lädt nichts herunter. Die Installation lädt herunter, prüft den Unterzeichner und bittet den Updater, es anzuwenden, wenn aus dem Installationsordner kein anderer Scriptura-Prozess (Hook, Worker oder weitere Verwaltungsoberfläche) läuft und kein run-now aktiv ist; danach wird die Verwaltungsoberfläche beendet (andernfalls geschieht nichts, und es wird zu einem späteren Versuch geraten; es wird kein Prozess erzwungen beendet). Im Dauerbetrieb startet der Updater die neue Version; die Sitzung ist neu, daher weist der geöffnete Tab darauf hin, sie über das Symbol im Infobereich (Menüleiste) erneut zu öffnen. Eine aus einem Terminal gestartete Oberfläche wird nur beendet. „Nach Updates suchen“ im Menü des Symbols öffnet dieses Panel und beginnt automatisch zu suchen. Ohne JavaScript funktionieren dieselben Schaltflächen, und das Ergebnis erscheint auf einer eigenen Seite. In anderen Editionen weist das Panel nur darauf hin, dass es in der Setup-Edition verfügbar ist.
- **Schutz**: Die bei jedem Start erzeugte Einmalschlüssel-URL (`/auth?key=…`) wird einmalig gegen ein Sitzungs-Cookie (`HttpOnly; SameSite=Strict`) eingetauscht (ohne das Cookie lautet die Antwort 401). Host darf nur `127.0.0.1:<port>` oder `localhost:<port>` sein; ist ein Origin vorhanden, muss er same-origin sein; und ein POST erfordert Same-Origin (`Origin` oder `Sec-Fetch-Site`) plus ein CSRF-Token. Antwort-Header wie CSP, `nosniff` und `no-store` werden hinzugefügt, und geheime Werte werden nie angezeigt. Da Browser Cookies nicht nach Port isolieren, wird das Cookie auch an Dienste gesendet, die auf anderen 127.0.0.1-Ports desselben PCs laufen (Prozesse desselben Benutzers können die Einstellungsdatei bereits ändern, sodass nur ein lokaler Dienst mit anderen Berechtigungen ein Problem darstellt).
- **Konflikte**: Beim Speichern wird die angezeigte Version verglichen, und wurde die Datei anderswo geändert, werden Sie zum Neuladen aufgefordert.
- **Bestätigungsbildschirm**: Das Aktivieren eines Repositorys (Dokumente werden automatisch committet und gepusht), der Wechsel der Berechtigung auf `bypass`, der Wechsel der Registrierungsrichtlinie auf `all` / `remote-owner` (alle Repositorys, in die Sie pushen, oder alle der genannten Eigentümer würden gewartet), das Hinzufügen eines Zielpfads einer benutzerdefinierten Art, das Entfernen einer Registrierung und `run-now` (sowie das Wiederherstellen des Verlaufs auf eine Version, die eines davon enthält) schreiben beim ersten Absenden nichts; sie zeigen eine Liste der Änderungen und verlangen eine einmalige Bestätigung (10 Minuten gültig).
- **Seiten**: Die Kopfzeile hat vier Einträge: Overview / Repositories (registrieren, aktivieren, pausieren, fortsetzen, entfernen, Zieldokumente, `run-now`; eine durch die Einstellungsmigration gestoppte Registrierung zeigt statt der Schaltfläche „Fortsetzen“ einen Hinweis und wird mit „Aktivieren“ erneut aktiviert) / Verlauf (Wartungsverlauf: Ergebnisse, Commits, Gründe) / Einstellungen. Die Einstellungen erscheinen als Tabs: Allgemein (Ausschlüsse, gemeinsame zusätzliche Anweisungen, Registrierungsrichtlinie) / Dokumente (die standardmäßigen Zieldokumente sowie Hinzufügen, Bearbeiten und Löschen von Dokumentarten und Bearbeiten der Vorlagen in `templates/`; `/settings/documents`) / Worker / Einstellungsänderungen (Änderungsverlauf, Differenzen, Wiederherstellung) / Diagnose. Die früheren Seiten Document kinds, Update proposals und Einwilligung wurden entfernt (ihre alten URLs liefern 404).Auf der Worker-Seite werden Modell und Effort aus Listen gewählt, nicht eingetippt: Die Modellliste ist eine integrierte Liste; ist die Umgebungsvariable `ANTHROPIC_API_KEY` bereits gesetzt, holt die Schaltfläche „Modellliste aktualisieren“ die neuesten Modelle aus der Anthropic-API in den Arbeitsspeicher (Scriptura speichert oder zeigt den Schlüssel nie, das bloße Öffnen der Seite verbindet nicht, und ein gespeicherter Wert, der nicht in der Liste steht, bleibt als „nicht in der Liste“ erhalten). Zeiten werden in der lokalen Zeitzone des PCs angezeigt, auf dem die Oberfläche läuft (`YYYY-MM-DD HH:MM`); das gespeicherte Format und das JSON der CLI bleiben unverändert. `run-now` dauert, daher läuft es im Hintergrund, und Sie können seinen Status auf der Seite prüfen.
- **Wartungsverlauf**: Jeder Wartungslauf, der den Worker gestartet hat, und jeder Versuch, der davor fehlgeschlagen ist (per Hook oder `run-now`), wird in der Zustands-DB (`maintenance_runs`) festgehalten und unter „Verlauf“ angezeigt (nach Repository filterbar, 50 pro Seite): Auslöser, Arten, Ergebnis (committet / keine Änderungen / abgelehnt / angehalten / fehlgeschlagen / Ergebnis unbekannt), die Dokumente mit hinzugefügten und entfernten Zeilen, Bereich und resultierender Commit sowie der Grund. Dokumentinhalte und Diffs werden nicht gespeichert. Pro Repository bleiben bis zu 200 Läufe (neueste zuerst), insgesamt 2.000; wiederholte gleiche Fehlschläge erscheinen als eine Zeile („×Anzahl“). Läufe, die den Worker weder gestartet haben noch fehlgeschlagen sind (z. B. weil keine Art fällig war), werden nicht aufgezeichnet.

## Ausgabesprache

Die Ausgaben von CLI, Hook und Installer-Hinweisen sind in 9 Sprachen verfügbar (ja, en, zh-CN, zh-TW, ko, es, fr, de, pt-BR). Die Sprache wird in dieser Reihenfolge gewählt: `SCRIPTURA_LANGUAGE` → `LC_ALL` → `LC_MESSAGES` → `LANG` → die Anzeigesprache des Betriebssystems → Englisch. Die Verwaltungsoberfläche verwendet ihre eigene Auswahlreihenfolge (siehe oben). JSON-Schlüssel und für Maschinen bestimmte Werte (wie `status`, `outcome` und die `id` von `doctor`) werden nicht übersetzt.

## Befehle

Alle Befehle außer `ui` akzeptieren `--json` (gibt ein einzelnes maschinenlesbares JSON-Objekt aus). Erfolg ist Exit-Code 0, ein Benutzerfehler oder eine Ablehnung ist 1. Die Ausgabe enthält keine geheimen Werte.

| Befehl | Beschreibung |
| --- | --- |
| `enable --repo <path> [--branches a,b] [--documents AGENTS.md,README.md]` | Registriert und aktiviert ein Repository (Dokumente werden danach automatisch committet und gepusht); aktiviert auch eine durch die Einstellungsmigration gestoppte Registrierung erneut. Ein Remote, ein Branch und ein Upstream sind erforderlich; ohne `--branches` wird der aktuelle Branch berücksichtigt. `--documents` akzeptiert die 4 eingebauten Arten (einschließlich `CHANGELOG.md`; ohne Beachtung der Groß-/Kleinschreibung) und die IDs benutzerdefinierter Arten, die in den Einstellungen definiert sind. Die entfernten Optionen `--mode` / `--accept-consent` und die entfernten Befehle `review` / `apply` / `discard` werden mit einem Fehler abgelehnt (ein in `<state>/proposals/` verbliebener Patch lässt sich weiterhin mit `git apply` anwenden). |
| `disable --repo <path>` | Entfernt die Registrierung. |
| `pause [--repo <path>]` / `resume [--repo <path>]` | Pausieren / fortsetzen (das Repository des aktuellen Verzeichnisses, wenn `--repo` weggelassen wird). |
| `status` | Zeigt den Installationszustand, die Einstellungen, die Registrierungen (jeweils mit `paused` und `migratedFrom`), die Worker-Einstellungen, `legacy` (ob die Einstellungsdatei noch im alten Format vorliegt) und die Informationen der Setup-Edition (`setup`: ob verwaltet, packId, Version, ausführbare Datei, automatisches Update, ob die Verwaltungsoberfläche bei der Anmeldung dauerhaft aktiv bleibt (`trayAutostart`), Zeitpunkte der letzten Prüfung / des letzten Fehlschlags, die heruntergeladene Version, das Registrierungsergebnis und eine eventuell verbliebene alte eigenständige Installation). `installedVersion` stammt aus dem Eintrag der eigenständigen `install` (`install.json`, oder die `current.json` einer früheren Version, falls diese fehlt). `installForm` gibt die Form des laufenden Programms an (`setup` = Setup-Edition, `standalone` = eigenständige `install`, `other` = im Plugin enthalten oder Entwicklungs-Build), während `installed` nur angibt, ob am Ort der eigenständigen `install` eine ausführbare Datei liegt. |
| `doctor [--verify]` | Diagnostiziert Umgebung und Einstellungen (siehe unten). |
| `run-now [--repo <path>] [--documents <id,...>]` | Führt die Wartung einmal sofort aus, ohne auf einen Push zu warten (nur für registrierte, nicht pausierte Repositorys; das Timing wird ignoriert, und alle aktivierten Arten werden verarbeitet). Dokumente werden wie bei einem normalen Lauf committet und gepusht. `--documents` schränkt die zu verarbeitenden Arten ein (eine nicht definierte oder für dieses Repository deaktivierte Art wird abgelehnt, ohne den Worker zu starten). **Hinweis**: Ein eingeschränktes `run-now` rückt ebenfalls die verarbeitete Position vor, daher werden Arten außerhalb der Einschränkung für diesen Push nicht gewartet. Bei Erfolg ist `outcome` `completed`. |
| `docs list [--repo <path>]` / `docs show <id> [--repo <path>]` | Listet die Dokumentarten (eingebaute und benutzerdefinierte) auf und zeigt die Details einer Art (aktiviert oder deaktiviert, Zielpfade und ob sie existieren, Grenzen, Timing, Vorlagenstatus, Zeitpunkt des letzten Abschlusses; `show` gibt außerdem die konfigurierte guidance und zusätzliche Anweisungen aus. Nur lesend, und es gibt keine Dokumentinhalte aus). |
| `ui [--port <n>] [--no-open] [--tray]` / `ui --autostart on\|off` | Startet die Verwaltungsoberfläche (siehe oben; `--tray` hält sie dauerhaft aktiv) / schaltet die Einstellung für den Dauerbetrieb bei der Anmeldung der Setup-Edition um. `--json` ist nicht verwendbar. |
| `connect claude-code [--apply] [--executable <absolute path>]` / `--remove [--apply]` | Registriert den Hook in der settings.json (ersetzt alte Wartungs-Hooks Hook für Hook) / entfernt nur Scripturas Hook. Eine Vorschau, bis Sie `--apply` angeben. |
| `update [--check] [--auto on\|off]` | Zeigt den Zustand der automatischen Updates der Setup-Edition / prüft und lädt jetzt herunter (wendet nicht an) / schaltet die automatischen Updates um. |
| `update-apply` | Wendet ein heruntergeladenes Update nur an, wenn kein anderer Scriptura-Prozess läuft, nachdem der Signierer geprüft wurde (wird bei der Anmeldung automatisch gestartet. Das Ergebnis ist eine JSON-Zeile, deren `status` `unmanaged` / `disabled` / `nothingPending` / `busy` / `untrusted` (Exit-Code 2) / `applied` lautet). |
| `uninstall-setup [--apply]` | Entfernt die Registrierung der Setup-Edition (den Hook, der auf sich selbst zeigt, und das Anwenden bei der Anmeldung). Eine Vorschau, bis Sie `--apply` angeben. |
| `install` / `hook` | Legt es am Installationsort ab (der traditionelle fortgeschrittene Weg; in `install.json` im Zustandsordner festgehalten) / läuft als Hook (wird normalerweise nicht direkt aufgerufen). |

## Sicherheit

- **Opt-in**: Solange kein Repository registriert ist (`enable` oder die Verwaltungsoberfläche), läuft nichts, und es wird nicht einmal ein Zustandsordner angelegt. Registrierungen werden nur in der eigenen Einstellungsdatei des Benutzers gespeichert.
- **Automatischer Commit und Push**: Der Worker läuft in Ihrem echten Arbeitsverzeichnis, und Scriptura committet und pusht die Zieldokumente ohne Rückfrage. Deshalb erfordert `enable` ein Remote, einen Branch und einen Upstream, und es wird nichts committet, wenn die folgenden Prüfungen nach dem Lauf ein Problem finden. Aktivieren Sie es nur für Repositorys, bei denen das akzeptabel ist; den früheren Modus propose (Update-Vorschläge) gibt es nicht mehr.
- **Sensible Muster**: `.env*` / `*.pem` / `*.key` / `*.p12` / `*.pfx` / `id_rsa*` / `id_ed25519*` / `**/secrets/**` / `.git/config` werden bei `restricted` als Read- / Edit-Verweigerungsregeln des Workers angewendet (`--disallowedTools`). Sie können mit `exclude` ergänzen, aber nicht ersetzen, um welche zu entfernen. Bei `bypass` (altkompatibel; überspringt alle Berechtigungsabfragen) werden diese Regeln nicht angewendet.
- **Prüfungen nach dem Lauf**: Hat der Worker etwas anderes als die Zieldokumente (und `.gitignore`) verändert, wird sein Ergebnis nicht übernommen. Da der Worker im echten Arbeitsverzeichnis des Benutzers läuft, werden nur unter `restricted` die verfolgten und nicht verfolgten Dateien des Arbeitsverzeichnisses vor und nach dem Worker verglichen (Änderungen an ignorierten Dateien wie `target/` sind ausgenommen, weil sie sich nicht von der gleichzeitigen Arbeit des Benutzers unterscheiden lassen), während `bypass` wie bisher nur die Hash-Prüfung der zu committenden Dokumente macht. Unsichere Commit- und Push-Zustände werden nicht erneut ausgeführt, sondern über die Zustands-DB abgeglichen. Limits, erforderliche Überschriften und die Unveränderlichkeit veröffentlichter CHANGELOG-Einträge werden unabhängig vom Berechtigungsmodus geprüft (siehe oben).
- **Umgebungshygiene**: Relay-Einstellungen aus der übergeordneten Sitzung (etwa der Desktop-App) werden aus den an den Worker übergebenen Umgebungsvariablen entfernt.

## Kosten und Datenübertragung

Der Wartungs-Worker (standardmäßig Sonnet) läuft mit **Ihrer eigenen Claude-Authentifizierung**. Die Repository-Inhalte, die der Worker liest, werden an Anthropic gesendet, und bei jedem Lauf fallen Claude-Nutzungsgebühren (Verbrauch) an. Der Worker läuft höchstens einmal pro Push, wobei `worker.timeout_secs` die Obergrenze bildet.

## Diagnose (doctor)

`scriptura doctor [--verify] [--json]` liefert für jeden Punkt `{id, status, detail}` (status ist `ok` / `warn` / `fail` / `info`). Gibt es ein `fail`, ist der Exit-Code 1.

Geprüft werden die unterstützte Plattform, `git`, `claude` (Auflösung der ausführbaren Datei), `claude auth status` (in derselben Umgebung wie der Worker ausgeführt, wobei nur Exit-Code und `authMethod` betrachtet werden), `gh` (nur wenn es ein aktiviertes GitHub-Repository gibt), Einstellungen (einschließlich `configLegacy` für Einstellungen im alten Format, `migratedStopped` für durch die Migration gestoppte Registrierungen und `legacyProposals` für Patches, die die entfernten Update-Vorschläge hinterlassen haben), ob der Zustandsordner beschreibbar ist, doppelte Hook-Registrierungen, `allowManagedHooksOnly` / `disableAllHooks` sowie verbliebene Registrierungen des alten `agents-maintenance`. Für die Setup-Edition werden außerdem die doppelte Registrierung zwischen Plugin und Setup-Edition (`setupPlugin`), das Vorhandensein des Hooks der Setup-Edition (`setupHook`), eine verbliebene alte eigenständige Installation (`legacyInstall`) und der Zustand der automatischen Updates (`autoUpdate`) gemeldet (bei einer Nicht-Setup-Edition nur ein einzelner `info`-Punkt, `setup`). Der Punkt `kinds` prüft, ob die Vorlagen der Dokumentarten gelesen werden können (Ort und Größe), und meldet `info`, wenn eine Grenze kleiner ist als die aktuelle Größe eines bestehenden Dokuments (die nächste Wartung könnte abgelehnt werden) und wenn es ein Repository gibt, das `CHANGELOG.md` pflegt (ein Konflikt mit dem Release-Verfahren). `--verify` prüft nur das SHA-256 der Binärdateien, die in der mitgelieferten `SHA256SUMS` des Plugins aufgeführt sind (`bin/scriptura.exe` und `libexec/scriptura-*`) (Launcher, Hooks und Skills sind nicht erfasst; ausführbare Dateien außerhalb des Plugins ebenfalls nicht).

## Zustand und Speicherorte

| Ort | Windows | macOS | Linux |
| --- | --- | --- | --- |
| Installationsort (die feste ausführbare Datei `bin\scriptura(.exe)`) | `%LOCALAPPDATA%\Programs\Scriptura` | `~/Library/Application Support/Scriptura/app` | `$XDG_DATA_HOME/scriptura` (Standard `~/.local/share/scriptura`) |
| Zustand (DB, Sperren, Veröffentlichungen,  der Wartungsverlauf (in der DB), der Änderungsverlauf der Einstellungen `config-history/`) | `%LOCALAPPDATA%\Scriptura\state` | `~/Library/Application Support/Scriptura/state` | `$XDG_STATE_HOME/scriptura` (Standard `~/.local/state/scriptura`) |

Wird `SCRIPTURA_HOME` auf einen absoluten Pfad gesetzt, liegt der Zustand in `<SCRIPTURA_HOME>/state`, der Installationsort in `<SCRIPTURA_HOME>/app` und die Einstellungen in `<SCRIPTURA_HOME>/config.toml`. Mit `SCRIPTURA_DB` lässt sich auch nur die Zustands-DB an einen anderen Ort verlegen. Vorlagen kommen in `templates/` im selben Ordner wie die Einstellungsdatei. Es wird nichts in das Codex- oder Claude-Code-Home geschrieben (außer wenn `connect claude-code --apply` / `--remove`, die automatische Registrierung der Setup-Edition und `uninstall-setup --apply` die `settings.json` umschreiben). Der Installationsort der Setup-Edition (Windows `%LOCALAPPDATA%\ScripturaApp`) wird von Velopack verwaltet und bei Updates ersetzt; dort werden daher weder Zustand noch Einstellungen aufbewahrt.

## Automatische Migration von Altbenutzern

Ein Benutzer, der ein älteres Scriptura (eine Version ohne Einstellungsdatei) ausgeführt hat und noch eine Zustands-DB besitzt, erhält beim Ausführen eines Hooks ohne Einstellungsdatei automatisch genau einmal eine Einstellungsdatei mit demselben Verhalten wie zuvor (alle Repositorys, Modus `push`, Berechtigung bypass) und wird darüber benachrichtigt.  Für neue Benutzer wird nichts angelegt. Wenn Sie die Ziele eingrenzen möchten, registrieren Sie erneut mit `enable` und setzen Sie `policy = "explicit"`. Der Zustand des alten `agents-maintenance` (`~/.codex/hooks/state`) wird nicht übernommen; es beginnt mit einem leeren Zustand.

## Migration alter Einstellungen (Einwilligung und propose entfernt)

- Die Einstellungsdatei wird jetzt immer als `schema = 3` gespeichert. Eine Datei mit `schema` 1, 2 oder ohne Angabe wird als altes Format gelesen und beim Lesen umgewandelt; der nächste Hook (oder jede Änderung der Einstellungen) speichert sie einmalig als schema 3, legt den bisherigen Inhalt in `config-history/` beiseite und teilt Ihnen einmalig mit, wie viele Registrierungen gestoppt wurden.
- Eine Registrierung, die eine Einwilligung hatte und tatsächlich im Modus `push` lief, läuft unverändert weiter. Jede andere Registrierung (`propose`, `off` oder ohne Einwilligung) wird mit Angabe ihrer Herkunft (`migrated_from`) pausiert und tut nichts, bis Sie sie mit `scriptura enable --repo <path>` oder in der Verwaltungsoberfläche erneut aktivieren (`resume` und das Wiederherstellen aus dem Verlauf setzen sie nicht fort, weil `enable` Remote und Upstream prüft). `status` zeigt `migratedFrom`, und `doctor` meldet diese Registrierungen.
- Eine `enrollment.policy` von `all` / `remote-owner` wird auf `explicit` zurückgesetzt (die owners bleiben erhalten), außer die Datei hatte eine Einwilligung und `defaults.mode = "push"`.
- Die Schlüssel `consent` und `mode` werden entfernt. Eine schema-3-Datei, die sie noch enthält, ist ungültig, und nichts läuft.
- In `<state>/proposals/` verbliebene Patches werden weder gelesen noch gelöscht. Löschen Sie sie von Hand, wenn Sie sie nicht brauchen; ein nicht angewendeter Patch lässt sich mit `git apply` anwenden. Scriptura räumt auch die Wegwerf-Klone (`work-*`) nicht mehr automatisch auf.
- Eine ältere Scriptura-Version, die eine schema-3-Datei liest, tut nichts; wenn Sie Plugin und Setup-Edition gemeinsam nutzen, aktualisieren Sie daher beide.

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
cargo nextest run --locked --features test-support   # cargo install cargo-nextest --locked
cargo nextest run --locked --features test-support --profile quiescence
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
