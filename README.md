[日本語](README.ja.md) | **English** | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md) | [한국어](README.ko.md) | [Español](README.es.md) | [Français](README.fr.md) | [Deutsch](README.de.md) | [Português (Brasil)](README.pt-BR.md)

# Scriptura

Scriptura maintains `AGENTS.md` / `DESIGN.md` / `README.md` (and, as opt-ins, `CHANGELOG.md` and user-defined custom documents) in the background after you push to your own repositories. It is launched from Claude Code hooks (right after a `git push` run through the Bash or PowerShell tool, and when a session starts, to catch up on anything missed), and its maintenance worker is the Claude Code CLI (by default `claude -p --model claude-sonnet-5-5 --effort high`). It does not depend on Codex.

- **Safe by default.** Nothing runs until you register a repository with `scriptura enable` (or in the admin UI), and enabling requires a remote and an upstream. There is no review step: after each push the worker updates the target documents and Scriptura automatically commits and pushes them, and a post-maintenance check refuses to commit anything outside the target documents. Consent records and the former `propose` mode (update proposals) were removed; see "Migrating old settings".
- Settings are read only from a single file under the user's home directory (`config.toml`) and the `templates/` folder next to it. Enabling or configuring through files inside a repository is never read (so that merely cloning someone else's repository can never start automatic behavior).
- Settings can also be edited in `scriptura ui` (the browser-based admin UI, described below).

This is the document-maintenance part of the former `agents-maintenance` (CodexHooks), split out as a standalone tool. The contracts for maintenance decisions, state management, and publishing (commit and push) are the same.

## Installation

### Plugin (recommended)

Installing it as a Claude Code plugin brings in the hook and the operation skills (such as `/scriptura:setup`) together. The `scriptura` binary is bundled with the plugin (on Windows it is `bin/scriptura.exe`; on macOS / Linux the `bin/scriptura` launcher selects the binary under `libexec/`. Even on Windows, when the hook goes through Git Bash's sh, the same launcher hands off to `bin/scriptura.exe`). See `xtask/` (`cargo run --manifest-path xtask/Cargo.toml --locked -- package`) for how the distribution is built.

### Setup edition (auto-update)

Running `ScripturaApp-win-Setup.exe` (Windows), `ScripturaApp-osx-Setup.pkg` (macOS), or the AppImage `ScripturaApp-linux-x64.AppImage` / `ScripturaApp-linux-arm64.AppImage` (Linux x64 / arm64) installs it per user without administrator rights (on Windows the install location is `%LOCALAPPDATA%\ScripturaApp\current\scriptura.exe`). Updates are served from `https://scriptura-updates.kagayoi.com` (Velopack feeds named `releases.<channel>.json`, with channels `win` / `osx` / `linux-x64` / `linux-arm64`). The Windows distribution is signed with a Certum certificate, and the macOS distribution is signed with a Developer ID and notarized. Requirements: macOS 11 or later on Apple Silicon (Intel is not supported); Linux with glibc 2.39 or later.

- **Registration is automatic**: at install time (the install hook on Windows; the first launch without arguments after installation on macOS and Linux), the hook in Claude Code's `settings.json` is pointed at the Setup edition's executable. Only Scriptura's own hooks are replaced (a hook pointing at the old standalone install `%LOCALAPPDATA%\Programs\Scriptura\bin\scriptura.exe` is replaced with the Setup edition's, and the old files are not deleted). Existing `agents-maintenance` hooks are not replaced; `status` / `doctor` / a launch without arguments point you to `scriptura connect claude-code --apply`.
- **Avoiding duplication with the plugin**: if `scriptura@*` is enabled in `enabledPlugins` of `settings.json`, the Setup edition does not register in `settings.json` (maintenance would otherwise run twice). Choose either to keep using the plugin, or to disable the plugin and switch to the Setup edition. After you disable it, registration happens automatically on a launch without arguments or at the next logon.
- **Auto-update (on by default)**: after a hook's maintenance finishes, it checks for a new version at most once a day (throttled to 24 hours for both successes and failures) and only downloads it. **It is applied only at logon**: `scriptura update-apply`, which runs at logon (on Windows the HKCU Run value `<packId> Update`; on macOS the LaunchAgent `com.kagayoi.<packId>.update`; on Linux the XDG autostart entry `com.kagayoi.<packId>.update.desktop`), verifies that no Scriptura hook or admin UI is running and checks the signer, then applies the update without a restart. If anything is running, it does nothing and leaves the update for the next logon (so a hook in the middle of maintenance is never killed). The admin UI's "Install update" button (below) can also apply it on request.
- State lives in the state folder (described below): `app-update.json` (`auto_update` and the last check / failure times), `app-update.log` (for diagnostics), and `setup-registration.json` (the registration result).

```powershell
scriptura update                 # show the auto-update setting, the last check, and the downloaded version
scriptura update --check         # ignore throttling and check/download now (applied at the next logon, or by running update-apply manually)
scriptura update --auto off      # turn auto-update off (on turns it back on)
scriptura update-apply           # apply a downloaded update (normally runs automatically at logon)
scriptura uninstall-setup        # preview removing the Setup edition's registration (hook and logon apply)
scriptura uninstall-setup --apply
```

macOS and Linux have no uninstaller, so run `scriptura uninstall-setup --apply` before moving the `.app` to the Trash (or deleting the AppImage). On Windows, uninstalling from "Settings > Apps" performs the same removal automatically. In both cases only the hook that points at itself (exact full-path match) and the logon registration are removed; `config.toml` and the state folder are kept.

### Standalone `install` / `connect` (advanced)

```powershell
Set-Location D:\Dev\Scriptura
cargo build --release
.\target\release\scriptura.exe install                       # place it at a fixed per-user path
& "$env:LOCALAPPDATA\Programs\Scriptura\bin\scriptura.exe" connect claude-code          # preview the registration
& "$env:LOCALAPPDATA\Programs\Scriptura\bin\scriptura.exe" connect claude-code --apply  # register in settings.json
```

`connect claude-code --apply` registers the fixed install path in the hook of Claude Code's `settings.json` (`CLAUDE_CONFIG_DIR` or `~/.claude`). It replaces, hook by hook, only the hooks that call the former `agents-maintenance` and old Scriptura hooks (other hooks sharing the same entry are kept), and leaves all other settings alone. A command counts as a Scriptura hook only when it has the form `<executable> hook` and the executable name is `scriptura(.exe)`; other tools that merely have scriptura in their path are not affected. Before the first change it creates `settings.json.scriptura-backup`, and it does not rewrite broken JSON.

The former registration used a Stop hook, which fires on every turn. `--apply` and the Setup edition's registration remove Scriptura's old Stop hook and register three hooks instead: PostToolUse and PostToolUseFailure (the latter fires when a command such as `git push && gh pr create` fails after the push) for commands containing `push` in the Bash and PowerShell tools, and SessionStart on `startup` / `resume`. `--remove` removes both the old and the new ones. The hooks after a command are filtered by one permission rule per tool, `Bash(* push*)` (and `PowerShell(* push*)`), so `git push`, `rtk git push`, `git -C <path> push`, `FOO=1 git push`, `bash -lc 'git push'` and `make push` all match; an occasional extra match such as `echo push x` simply finds no new push and does nothing. These hooks are asynchronous and wait up to 25 minutes for another maintenance run to finish instead of skipping, so a push made during a maintenance run is still processed afterwards. A push made outside Claude Code, or into a repository other than the session's working directory (`git -C <other path> push`), is picked up the next time a session starts in that repository. The old Stop hook is still accepted (without waiting), and when Scriptura is called by it, it migrates the old registration in your user-level settings.json to the new hooks by itself, once, and tells you with a message (a backup is kept as `settings.json.scriptura-migration-backup[-N]`; project `.claude/settings*.json` files are never touched, and a `--remove`d registration is not restored). With the scriptura plugin enabled it only removes the old hook instead of adding new ones. The Setup edition migrates through its own registration.

When moving to the plugin, remove the standalone registration so that maintenance does not run twice (this removes only Scriptura's entries and does not touch `agents-maintenance` or other settings).

```powershell
scriptura connect claude-code --remove           # preview
scriptura connect claude-code --remove --apply   # write
```

The worker does not inherit the desktop session's relay settings, so run `claude auth login` in the standalone CLI beforehand. The CLI used is the `worker.claude_path` setting, otherwise `claude` on PATH (`claude.exe` on Windows), and on Windows additionally the newest `%LOCALAPPDATA%\Claude-Data\claude_*\claude-code\<version>\<hash>\claude.exe` bundled with the Claude desktop app.

## First-time setup

In the repository you want maintained, do one of the following.

- If you have installed the plugin: run `/scriptura:setup` in Claude Code (it asks you to confirm that documents will be committed and pushed automatically, then runs `scriptura enable`).
- Directly from the command line:

```powershell
scriptura enable --repo D:\Dev\MyRepo
```

`enable` requires a remote, a branch (not a detached HEAD) and an upstream; when `--branches` is omitted only the current branch is targeted. From then on, documents are committed and pushed automatically after each push (see "Cost and data transmission"). Before registering, `enable` records the "current push state" as already processed, so pushes made before enabling are never processed afterwards. The former `--mode` and `--accept-consent` options were removed and are refused with an error.

## Configuration file

| OS | Location |
| --- | --- |
| Windows | `%LOCALAPPDATA%\Scriptura\config.toml` |
| macOS | `~/Library/Application Support/Scriptura/config.toml` |
| Linux | `$XDG_CONFIG_HOME/scriptura/config.toml` (default `~/.config/scriptura/config.toml`) |

You can override it with `SCRIPTURA_CONFIG` (an absolute file path) or `SCRIPTURA_HOME` (`<SCRIPTURA_HOME>/config.toml`). It is normally rewritten by `enable` / `disable` / `pause` / `resume` and the admin UI, so you do not need to edit it by hand. A broken config, unknown keys, or a `schema` that is too new fall back to "do nothing", and commands such as `enable` fail without rewriting it.

Saving always writes `schema = 3`. A file with `schema` 1, 2 or none (written by an older Scriptura) is read as the old format and converted on the fly (see "Migrating old settings"); a file with a schema newer than 3 falls back to "do nothing". Older Scriptura versions that read a schema 3 file also do nothing, so when you use both the plugin and the Setup edition, update both.

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

## Document kinds

The documents to maintain are managed as "kinds".

In the admin UI the same settings are edited under Settings > Documents, which also sets the default target documents (`defaults.documents`).

| Kind | Target | Default |
| --- | --- | --- |
| `agents` / `design` / `readme` | As before (`AGENTS.md` and the personal override, scoped `AGENTS.md` / `DESIGN.md`, the root `README.md`) | Enabled (the default of `defaults.documents` stays these 3 kinds) |
| `changelog` | The root `CHANGELOG.md` | **Opt-in** (only when `CHANGELOG.md` is listed in `documents`) |
| Custom (`[kinds.<id>]`) | Documents listed at fixed paths in `paths` (up to 20 kinds; an id starts with a lowercase letter and consists of lowercase letters, digits, and hyphens, at most 32 characters) | Only when the id is listed in `documents` |

Target kinds are listed in `defaults.documents` (overridden by each repository's `documents`) either by document name (`"CHANGELOG.md"`) or by id (`"changelog"`; for custom kinds, the id). A setup with only the original 3 kinds behaves exactly as before if you write no per-kind settings (the prompt passed to the worker is identical too).

### Per-kind settings (`[kinds.<id>]`)

These are read only from the user's `config.toml`. Every field is optional, and built-in kinds layer on top of fixed defaults. Unknown keys are rejected.

| Key | Description |
| --- | --- |
| (the kind's id) | A custom kind's id starts with a lowercase letter and consists only of lowercase letters, digits, and `-`, at most 32 characters. The reserved word `new` cannot be used (it would collide with the admin UI's URL). |
| `paths` | The custom kind's target paths (1 to 10 entries, required). Cannot be written for built-in kinds. Only ASCII alphanumerics and `. _ / -`, relative paths, and the extensions `md` / `markdown` / `mdx` / `txt` / `rst` / `adoc`. Paths under `.git`, the same documents as built-in targets (such as `AGENTS.md`), paths matching sensitive patterns or `exclude`, and duplicates across kinds are rejected. |
| `guidance` | What the document is for (required for custom kinds, at most 1000 characters). Cannot be written for built-in kinds. |
| `create_if_missing` | Whether the target may be created when it does not exist (true for built-in kinds, false for custom kinds). When false, the target appears in neither the allowed paths nor the prompt while it does not exist. |
| `template` | `"builtin"` / `"none"` / `"templates/<name>.md"`. See below. |
| `max_chars` / `max_bytes` | Limits on the document (characters and bytes). If both are given, both apply. For `agents`, `max_bytes` can only be set to 29,999 or less. `max_chars` / `max_bytes` under `[defaults]` (the General screen) apply the same limits to every document except CHANGELOG and take precedence over these per-kind values; the screens no longer edit the per-kind values. |
| `split_dir` | Directory where detail beyond the limits may be moved (default `docs/<id>`; only for kinds with `max_chars` / `max_bytes`; not for `changelog`). Up to 20 files, one topic per file. |
| `instructions` | Additional editing instructions (at most 2000 characters). Together with `[defaults] instructions` (shared by all kinds) in two tiers, they go only into the "from user settings" section at the end of the prompt. They cannot change the target paths, safety rules, or how commit and push are handled. |
| `language` | The language of the document (`ja` / `en-US`, etc.). When omitted, the existing document's language is followed. This is the language of the generated documents and is separate from the language of the screens and output (the worker's prompts are in English, but the `language` behavior is unchanged). Setting `language` under `[defaults]` (the dropdown on the General screen) applies one language to every document and takes precedence over this per-kind value; the screens no longer edit the per-kind value. |
| `required_headings` | Headings whose presence is checked after maintenance (at most 20). |
| `timing` | When to run. See below. |
| `changelog` | CHANGELOG settings (only for the `changelog` kind). See below. |

**Templates** can only be specified as `templates/<name>.md` under the settings folder (files inside the repository and arbitrary absolute paths are not allowed; for CHANGELOG, `builtin` is the embedded Keep a Changelog template). At run time the file is checked to be a regular file (not a link), at most 16 KiB, UTF-8, free of control characters other than newline and tab, free of `<<<`, and outside the repository; if any check fails, the worker is not started, the processed position is not advanced, and the run is skipped (it is processed next time once you fix the file). A template is used as the structure when creating a new document; for existing documents, the existing structure takes precedence.

### Post-maintenance checks

Limits (`max_chars` / `max_bytes`), `required_headings`, AGENTS being under 30,000 bytes, and the immutability of released CHANGELOG entries are checked by Rust regardless of the worker's permission mode (`restricted` / `bypass`). If limits or required headings are still violated, the worker is rerun up to twice within the same hook to fix only those violations; if they remain, the result is committed and pushed with a warning (shown in the hook message, as `warnings` in `run-now --json`, and in the history). If AGENTS stays over 30,000 bytes or released CHANGELOG entries were changed, the worker's output is discarded and the maintenance completes without committing.

### Timing (`[kinds.<id>.timing]`)

| Key | Description |
| --- | --- |
| `when` | `push` (default; on every push) / `release` (when a release tag appears) / `manual` (only on `run-now`) |
| `paths` | An array of globs. The kind is a target only when one of the changed paths in this push range matches (no condition when omitted) |
| `min_interval_hours` | Minimum interval since the last completion (0 to 720 hours) |

CHANGELOG and custom kinds whose target does not exist and may not be created are left out of the decision, and when there is no document to process, the worker is not started and only the processed position is advanced (`kind_runs` is not recorded either). When maintenance is resumed after a worker failure or similar, it resumes with only the kinds that were due first (recorded in the state).

All conditions are evaluated as AND. `release` applies when a tag matching `release_tag` (`v*` for kinds other than CHANGELOG) is reachable from the pushed position and the section for that tag does not yet exist in `CHANGELOG.md` (kinds other than CHANGELOG use the same criterion, so if your release procedure writes the version's section before tagging, `release` for those kinds never fires). Only the kinds that are due are processed together in a single worker run, and if no kind is due, the worker is not started and only the processed position is advanced. The last completion time is kept per kind in the state DB's `kind_runs` table (the state DB version does not change).

Evaluation happens only when a new push is detected and on `run-now`. The minimum interval therefore means "run on the **next push** after the interval has passed"; it does not fire merely because time has passed. Changes from a push that was too early are not carried over to the next run, but the worker looks at the whole diff of the push range when it updates the documents. `run-now` ignores timing and processes all enabled kinds.

### CHANGELOG maintenance (`[kinds.changelog.changelog]`)

- It follows the existing format (the Keep a Changelog embedded template is used only when creating the file). Each push updates only the `Unreleased` section, and the worker cannot change released sections. Rust compares before and after maintenance, and if even one byte of a released section changed, a section disappeared or was reordered, or link reference definitions were lost, it is treated as a violation of the checks above (nothing is committed).
- For history, Rust gathers everything from the commit that last changed `CHANGELOG.md` up to this push and passes it to the prompt as **untrusted data** (commit subjects, changed paths, and tags in the range, up to 48 KiB in total). This is because a restricted worker cannot run `git log`. Scriptura's own maintenance commits are excluded.
- `on_release = "promote"` (default): if there is no section yet for the version corresponding to a tag in the range, the contents of `Unreleased` are moved into that version's section. If the section already exists, nothing happens (the result is the same even if the release procedure wrote it first). `ignore` does not create version sections.
- `backfill = "tags"` (default): when `CHANGELOG.md` does not exist, it creates per-version sections by going back through reachable tags, newest first, up to `backfill_max_releases` (1 to 50, default 20) tags and up to `backfill_max_commits` (1 to 1000, default 300) commits in total between adjacent tags. `none` does not go back. `format` (`existing` / `keep-a-changelog`) and `release_tag` (default `v*`) can also be specified.
- The file name is resolved from its spelling in Git case-insensitively (`Changelog.md` and the like are also targets). When the spelling cannot be settled on a single one, or the pre-edit content cannot be read from Git, it is treated as unverifiable and counts as a violation. If `CHANGELOG.md` is ignored by Git, new maintenance is skipped without starting the worker.
- Matching versions with tags also treats the version name without the fixed prefix of `release_tag` (the `v` of `v*`, the `release-` of `release-*`, and so on) as a match (`## [1.2.0]`). Violation reasons do not print heading text and are given by number, such as "the n-th version section". For link reference definitions of released sections, it also checks that their content and order are preserved and that existing labels are not redefined.
- Limitations: a tag-only push (`git push --tags`) cannot be detected. It is picked up by the next branch push or with `--follow-tags`. When maintaining `CHANGELOG.md`, a release procedure that updates the same file may conflict (`doctor` points this out).

### Example configuration (schema 3)

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

## Writing settings and change history

All settings writes, whether from the CLI, the admin UI, or the automatic migration of legacy users, go through the same path (`src/ops.rs` and `config::update_in`). Writers are serialized with `config-lock.sqlite3` in the state folder, and the admin UI compares the version it displayed (the file's SHA-256, or `missing` if the file does not exist) and, if the file was changed elsewhere, does not save and tells you to reload (the CLI does not compare versions). Immediately before saving, the previous content is set aside in `config-history/` in the state folder (50 entries are kept), and the save is atomic. A broken config is not rewritten. An operation that changes nothing neither saves nor sets anything aside. From "Settings history" in the admin UI you can inspect diffs and restore; the content from before a restore is also kept in the history. When a restore would add registrations (paused ones included), the starting point of each registration (the position that prevents processing pushes from before enabling) is recorded before saving. A restore that would add active registrations (neither paused nor stopped by the migration) or turn registrations active is refused and directed to `enable` (which checks the remote and upstream); a registration stopped by the migration cannot be resumed by a restore either.

## Admin UI (`scriptura ui`)

```powershell
scriptura ui [--port <n>] [--no-open] [--tray]
scriptura ui --autostart on|off
```

A browser-based screen for handling repositories, settings (including documents), history, and diagnostics. It is available in 9 languages (ja, en, zh-CN, zh-TW, ko, es, fr, de, pt-BR). The language is chosen in this order: the language selector at the top of the screen (saved in a cookie) → the browser's `Accept-Language` → English. By default it is not resident: the command that launched it ends when you choose "Quit" on the screen, press Ctrl+C, or after 60 minutes of inactivity (add `--tray` to keep it resident; see below). Hooks do not look at whether the screen is open. It is controlled by the cargo feature `ui` (enabled by default) and is not included when built with `--no-default-features` (`scriptura ui` then refuses and exits).

- **Listening**: only on `127.0.0.1` (the default port is 7468; `--port 0` picks a free port). If the port is in use, it exits.
- **Resident mode (`--tray`)**: stays resident with an icon in the system tray on Windows and in the menu bar on macOS (Linux is not supported; `--tray` refuses and exits). Clicking the icon opens the admin UI, and the right-click menu offers "Open the management screen", "Check for updates" (Setup edition only; opens the overview screen's "App updates" panel), and "Exit". While resident it does not exit on inactivity. The session does not expire either (tabs left open stay signed in), and a new one-time key is issued every time you open the screen from the icon (a reissued key expires after 5 minutes, and an earlier unused key becomes invalid). On Windows, running `scriptura ui --tray` again while one is already resident on the default port does not start a second one; the resident one opens the browser. When launched from something other than a terminal (such as the Start menu), it relaunches itself as a process without a console window.
- **Resident at logon (Setup edition only; on by default)**: it stays on unless you turn it off with `scriptura ui --autostart off` or the checkbox in the "App updates" panel (`tray_autostart` in `app-update.json` in the state folder; an explicit off that was saved earlier is kept, and a missing or unreadable value counts as on). No extra logon entry is added; the resident UI is started after the logon-time `update-apply` finishes (if it started first, the install folder would count as having another process running and updates would never be applied). At a logon where an update is applied, the updater starts the new version's resident UI after the swap. The Setup edition's Start menu entry (a launch with no arguments) shows its guidance and then keeps the admin UI resident and opens it in the browser. Because Scriptura keeps running from the install folder while resident, a downloaded update is applied at the next logon, or right away with the panel's "Install update" (below). `--autostart` cannot be used with the plugin-bundled or standalone `install` executable (staying resident with `ui --tray` still works).
- **"App updates" panel (Setup edition only)**: the overview screen's "App updates" panel (`#updates`) shows the current version and has checkboxes for auto-update and the resident-at-logon setting, a "Check for updates" button, and an "Install update" button (shown when a new version exists). Checking ignores the 24-hour throttle and downloads nothing. Installing downloads the update, verifies the signer, and, if no other Scriptura process (a hook, a worker, or another admin UI) is running from the install folder and no run-now is in progress, asks the updater to apply it and then ends the admin UI (otherwise it does nothing and asks you to try again later; nothing is force-killed). When the UI is resident, the updater starts the new version; the session is new, so the open tab says to reopen it from the tray (menu bar) icon. An admin UI started from a terminal is only closed. The tray menu's "Check for updates" opens this panel and starts checking automatically. Without JavaScript, the same buttons work and the result is shown on a separate screen. In other editions, the panel only says it is available in the Setup edition.
- **Protection**: the disposable key URL generated at each launch (`/auth?key=…`) is exchanged once for a session cookie (`HttpOnly; SameSite=Strict`) (without the cookie, the response is 401). Host must be only `127.0.0.1:<port>` or `localhost:<port>`; if an Origin is present it must be same-origin; and a POST requires same-origin (`Origin` or `Sec-Fetch-Site`) plus a CSRF token. Response headers such as CSP, `nosniff`, and `no-store` are added, and secret values are never displayed. Because browsers do not isolate cookies by port, the cookie is also sent to services running on other 127.0.0.1 ports on the same PC (processes of the same user can already touch the settings file, so the only concern is a local service with different privileges).
- **Conflicts**: a save compares the version it displayed and, if the file was changed elsewhere, tells you to reload.
- **Confirmation screen**: enabling a repository (documents will be committed and pushed automatically), switching the permission to `bypass`, changing the enrollment policy to `all` / `remote-owner` (every repository you push to, or every one owned by the listed owners, would be maintained), adding a custom kind's target path, removing a registration, and `run-now` (and restoring history to a version that includes any of these) do not write on the first submission; they show a list of the changes and ask for a one-time confirmation (valid for 10 minutes).
- **Screens**: the header has four items: Overview / Repositories (register, enable, pause, resume, remove, target documents, `run-now`; a registration stopped by the settings migration shows a notice instead of a Resume button and is enabled again with "Enable") / History (the maintenance history: results, commits, reasons) / Settings. Settings is shown as tabs: General (exclusions, shared additional instructions, enrollment policy) / Documents (the default target documents, and adding, editing, and deleting document kinds and editing templates in `templates/`; `/settings/documents`) / Worker / Settings history (change history, diffs, restore) / Diagnostics. The former Document kinds, Update proposals, and Consent screens were removed (their old URLs return 404).On the Worker screen the model and effort are chosen from lists, not typed: the model list is a built-in one, and if the `ANTHROPIC_API_KEY` environment variable is already set, the "Refresh model list" button fetches the newest models from the Anthropic API into memory (Scriptura never stores or shows the key, nothing is fetched just by opening the screen, and a saved value that is not in the list is kept as "not in the list"). Times are shown in the local time zone of the PC running the UI (`YYYY-MM-DD HH:MM`); the stored format and the CLI's JSON are unchanged. `run-now` takes time, so it runs in the background and you can check its status on the screen.
- **Maintenance history**: each maintenance run that started the worker, and each attempt that failed before it could (from a hook or `run-now`), is recorded in the state DB (`maintenance_runs`) and shown under "History" (filter by repository, 50 per page): the trigger, kinds, result (committed / no changes / rejected / held / failed / outcome unknown), the documents with added and removed line counts, the range and resulting commit, and the reason. Document contents and diffs are not stored. Up to 200 runs per repository (newest first) and 2,000 overall are kept, and repeated identical failures are shown as one row ("×count"). Runs that neither started the worker nor failed (for example, when no kind was due) are not recorded.

## Language of output

The CLI, hook, and installer guidance output is available in 9 languages (ja, en, zh-CN, zh-TW, ko, es, fr, de, pt-BR). The language is chosen in this order: `SCRIPTURA_LANGUAGE` → `LC_ALL` → `LC_MESSAGES` → `LANG` → the OS display language → English. The admin UI uses its own selection order (described above). JSON keys and values meant for machines (such as `status`, `outcome`, and `doctor`'s `id`) are not translated.

## Commands

All commands except `ui` accept `--json` (prints a single machine-readable JSON object). Success is exit code 0, and a user error or refusal is 1. The output contains no secret values.

| Command | Description |
| --- | --- |
| `enable --repo <path> [--branches a,b] [--documents AGENTS.md,README.md]` | Registers and enables a repository (documents are then committed and pushed automatically); it also enables again a registration stopped by the settings migration. A remote, a branch and an upstream are required, and the current branch is targeted when `--branches` is omitted. `--documents` accepts the 4 built-in kinds (including `CHANGELOG.md`; case-insensitive) and the ids of custom kinds defined in the settings. The removed `--mode` / `--accept-consent` options and the removed `review` / `apply` / `discard` commands are refused with an error (a patch left in `<state>/proposals/` can still be applied with `git apply`). |
| `disable --repo <path>` | Removes the registration. |
| `pause [--repo <path>]` / `resume [--repo <path>]` | Pause / resume (the cwd's repository when `--repo` is omitted). |
| `status` | Shows the installation state, settings, registrations (each with `paused` and `migratedFrom`), worker settings, `legacy` (whether the settings file is still in the old format), and the Setup edition's information (`setup`: whether it is managed, packId, version, executable, auto-update, whether the admin UI stays resident at logon (`trayAutostart`), the last check / failure times, the downloaded version, the registration result, and any leftover old standalone install). `installedVersion` comes from the standalone `install` record (`install.json`, or an earlier version's `current.json` if absent). `installForm` is the form of the running binary (`setup` = Setup edition, `standalone` = standalone `install`, `other` = plugin-bundled or development build), while `installed` only tells whether the standalone `install` location holds an executable. |
| `doctor [--verify]` | Diagnoses the environment and settings (described below). |
| `run-now [--repo <path>] [--documents <id,...>]` | Runs maintenance once right now without waiting for a push (only for registered repositories that are not paused; timing is ignored and all enabled kinds are processed). Documents are committed and pushed as in a normal run. `--documents` narrows the kinds to process (an undefined kind, or one that is disabled for this repository, is refused without starting the worker). **Note**: a narrowed `run-now` also advances the processed position, so kinds outside the narrowing are not maintained for that push. On success `outcome` is `completed`. |
| `docs list [--repo <path>]` / `docs show <id> [--repo <path>]` | Lists the document kinds (built-in and custom) and shows the details of one (enabled or disabled, target paths and whether they exist, limits, timing, template status, last completion time; `show` also prints the configured guidance and additional instructions. Read-only, and it does not print document contents). |
| `ui [--port <n>] [--no-open] [--tray]` / `ui --autostart on\|off` | Launches the admin UI (described above; `--tray` keeps it resident) / switches the Setup edition's resident-at-logon setting. `--json` cannot be used. |
| `connect claude-code [--apply] [--executable <absolute path>]` / `--remove [--apply]` | Registers the settings.json hook (replacing old maintenance hooks hook by hook) / removes only Scriptura's hook. A preview until you pass `--apply`. |
| `update [--check] [--auto on\|off]` | Shows the Setup edition's auto-update state / checks and downloads now (does not apply) / switches auto-update. |
| `update-apply` | Applies a downloaded update only when no other Scriptura process is running, after verifying the signer (started automatically at logon. The result is one JSON line whose `status` is `unmanaged` / `disabled` / `nothingPending` / `busy` / `untrusted` (exit code 2) / `applied`). |
| `uninstall-setup [--apply]` | Removes the Setup edition's registration (the hook pointing at itself and the logon apply). A preview until you pass `--apply`. |
| `install` / `hook` | Places it at the install location (the traditional advanced path; recorded in `install.json` in the state folder) / runs as the hook (normally not called directly). |

## Safety

- **Opt-in**: until a repository is registered (`enable` or the admin UI), nothing runs and no state folder is even created. Registrations are kept only in the user's own settings file.
- **Automatic commit and push**: the worker runs in your real working tree, and Scriptura commits and pushes the target documents without asking. `enable` therefore requires a remote, a branch and an upstream, and nothing is committed if the post-run checks below find a problem. Enable it only for repositories where that is acceptable; the former propose mode (update proposals) no longer exists.
- **Sensitive patterns**: `.env*` / `*.pem` / `*.key` / `*.p12` / `*.pfx` / `id_rsa*` / `id_ed25519*` / `**/secrets/**` / `.git/config` are applied as the worker's Read / Edit deny rules (`--disallowedTools`) when `restricted`. You can add to them with `exclude`, but you cannot replace them to remove any. With `bypass` (legacy-compatible; skips all permission prompts), these rules are not applied.
- **Post-run checks**: if the worker changed anything other than the target documents (and `.gitignore`), its result is not adopted. Because the worker runs in the user's real working tree, only under `restricted` does it compare the working tree's tracked and untracked files before and after the worker (changes to ignored files such as `target/` are excluded because they cannot be told apart from the user's concurrent work), while `bypass` does only the hash check of the documents to be committed. Uncertain commit and push states are not rerun and are reconciled through the state DB. Limits, required headings, and the immutability of released CHANGELOG entries are checked regardless of permission mode (see above).
- **Environment hygiene**: relay settings originating from the parent session (such as the desktop app) are removed from the environment variables passed to the worker.

## Cost and data transmission

The maintenance worker (Sonnet by default) runs with **your own Claude authentication**. The repository contents the worker reads are sent to Anthropic, and Claude usage charges (consumption) are incurred on every run. The worker runs at most once per push, with `worker.timeout_secs` as its upper bound.

## Diagnostics (doctor)

`scriptura doctor [--verify] [--json]` returns `{id, status, detail}` for each item (status is `ok` / `warn` / `fail` / `info`). If there is any `fail`, the exit code is 1.

It checks the supported platform, `git`, `claude` (executable resolution), `claude auth status` (run in the same environment as the worker, looking only at the exit code and `authMethod`), `gh` (only when there is an enabled GitHub repository), settings (including `configLegacy` for old-format settings, `migratedStopped` for registrations stopped by the migration, and `legacyProposals` for patches left by the removed update proposals), whether the state folder is writable, duplicate hook registrations, `allowManagedHooksOnly` / `disableAllHooks`, and leftover registrations of the old `agents-maintenance`. For the Setup edition, it also reports duplicate registration between the plugin and the Setup edition (`setupPlugin`), whether the Setup edition's hook exists (`setupHook`), a leftover old standalone install (`legacyInstall`), and the auto-update state (`autoUpdate`) (for a non-Setup edition, only a single `info` item, `setup`). The `kinds` item checks whether the document kinds' templates can be read (location and size), and reports `info` when a limit is smaller than an existing document's current size (the next maintenance may be rejected) and when there is a repository that maintains `CHANGELOG.md` (a conflict with the release procedure). `--verify` checks only the SHA-256 of the binaries listed in the plugin's bundled `SHA256SUMS` (`bin/scriptura.exe` and `libexec/scriptura-*`) (launchers, hooks, and skills are not covered; executables outside the plugin are not covered either).

## State and storage locations

| Location | Windows | macOS | Linux |
| --- | --- | --- | --- |
| Install location (the fixed executable `bin\scriptura(.exe)`) | `%LOCALAPPDATA%\Programs\Scriptura` | `~/Library/Application Support/Scriptura/app` | `$XDG_DATA_HOME/scriptura` (default `~/.local/share/scriptura`) |
| State (DB, locks, publications,  the maintenance history (in the DB), the settings change history `config-history/`) | `%LOCALAPPDATA%\Scriptura\state` | `~/Library/Application Support/Scriptura/state` | `$XDG_STATE_HOME/scriptura` (default `~/.local/state/scriptura`) |

Setting `SCRIPTURA_HOME` to an absolute path puts state in `<SCRIPTURA_HOME>/state`, the install location in `<SCRIPTURA_HOME>/app`, and settings in `<SCRIPTURA_HOME>/config.toml`. `SCRIPTURA_DB` can also move just the state DB elsewhere. Templates go in `templates/` in the same folder as the settings file. Nothing is written to the Codex or Claude Code home (except when `connect claude-code --apply` / `--remove`, the Setup edition's automatic registration, and `uninstall-setup --apply` rewrite `settings.json`). The Setup edition's install location (Windows `%LOCALAPPDATA%\ScripturaApp`) is managed by Velopack and replaced on updates, so no state or settings are kept there.

## Automatic migration of legacy users

A user who ran an older Scriptura (a version that had no settings file) and still has a state DB, when running a hook without a settings file, gets a settings file automatically created exactly once with the same behavior as before (all repositories, `push` mode, permission bypass), and is notified of this.  Nothing is created for new users. If you want to narrow the targets, register again with `enable` and set `policy = "explicit"`. The state of the old `agents-maintenance` (`~/.codex/hooks/state`) is not carried over; it starts from an empty state.

## Migrating old settings (consent and propose removed)

- The settings file is now always saved as `schema = 3`. A file with `schema` 1, 2 or none is read as the old format and converted when read; the next hook (or any settings change) saves it as schema 3 once, setting the previous content aside in `config-history/`, and notifies you once of how many registrations were stopped.
- A registration that had consent and effectively ran in `push` mode keeps running unchanged. Every other registration (`propose`, `off`, or no consent) is paused with a note of where it came from (`migrated_from`) and does nothing until you enable it again with `scriptura enable --repo <path>` or in the admin UI (`resume` and restoring from the history do not resume it, because `enable` checks the remote and upstream). `status` shows `migratedFrom`, and `doctor` reports these registrations.
- `enrollment.policy` of `all` / `remote-owner` goes back to `explicit` (the owners are kept), unless the file had consent and `defaults.mode = "push"`.
- The `consent` and `mode` keys are removed. A schema 3 file that still has them is invalid, and nothing runs.
- Patches left in `<state>/proposals/` are neither read nor deleted. Delete them by hand if you do not need them; an unapplied patch can be applied with `git apply`. Scriptura no longer cleans up the throwaway clones (`work-*`) either.
- An older Scriptura reading a schema 3 file does nothing, so when you use the plugin and the Setup edition together, update both.

## Not supported / not verified

- Nothing is verified on real hardware (development and verification are done on Windows). CI on GitHub-hosted runners is set up to check Linux (x64 / arm64) with an isolated E2E of the Setup edition, and to build, test, sign, and notarize macOS; applying updates on macOS is not verified. macOS Intel is out of scope. The Setup edition's code for macOS and Linux (LaunchAgent, XDG autostart, process detection, and signature and location checks) is a port of Remnifold's implementation, and it has not even been confirmed to compile on Windows (it is checked only in CI).
- Switching auto-update in the admin UI (`ui`) is not supported (use `scriptura update --auto on|off`).
- On Windows, the logon Run value does not inherit `SCRIPTURA_HOME` (the macOS LaunchAgent and the Linux autostart entry do).
- On non-Windows platforms, resuming maintenance that was interrupted while the worker was running is skipped (there is no way to confirm the worker has ended).
- Publishing via branch + PR is planned for later. Custom kinds are limited to fixed paths (globs and subdirectory scanning come later), and per-kind overrides of model and effort, per-repository kind settings (`[repository.kinds]`), forced template application, and editing settings from the CLI (`set`) are also planned for later.
- The admin UI's resident mode (`ui --tray`) is available on Windows and macOS only (Linux is not supported). The icon is a provisional design (drawn at run time on Windows, a system symbol on macOS). On macOS, starting it and exiting with Ctrl+C were confirmed on real hardware, but the menu bar icon's appearance, clicks, and menu have not been checked by eye. Staying resident at logon, and starting the resident UI after an update is applied (Setup edition), have not been verified in a real installation.

## Verification

```powershell
Set-Location D:\Dev\Scriptura
cargo nextest run --locked --features test-support   # cargo install cargo-nextest --locked
cargo nextest run --locked --features test-support --profile quiescence
cargo build --release
cargo build --release --no-default-features   # a build without the admin UI (the ui feature)
git diff --check
```

Verification of the Setup edition (Velopack) is isolated as follows so that it never touches the real installation, `~/.claude/settings.json`, or the HKCU Run value.

- Use a test packId (for example `ScripturaVelopackTest`; do not use `ScripturaApp`) and an `--installto` under an external working folder.
- Point `SCRIPTURA_HOME`, `CLAUDE_CONFIG_DIR`, and `TEMP`/`TMP` at the working folder on a per-process basis (on macOS and Linux, also `HOME` and `XDG_CONFIG_HOME`).
- Build the executable with `cargo build --release --features update-feed-test`. Only builds with this feature read `SCRIPTURA_UPDATE_FEED_DIR` (makes the update source a local folder) and `SCRIPTURA_UPDATE_ALLOW_UNSIGNED=1` (allows applying unsigned packages). Never include it in production builds.
- When finished, remove the test Run value (`<test packId> Update`), the uninstall information, and the install location.
- The registry unit tests use a dedicated hive confined to a file (`RegLoadAppKeyW`), not the real Run key.

The tests use local isolated Git repositories and native stubs, and do not call the real Claude CLI or any external repository. The `validation` folder (or the location given by `SCRIPTURA_TEST_BASE`) is not deleted automatically, so check the absolute path that is printed and clean it up with Prunlet. `hook-test-stub` is exclusive to the `test-support` feature and is not deployed.
