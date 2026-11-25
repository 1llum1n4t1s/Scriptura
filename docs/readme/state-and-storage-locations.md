# State and storage locations

| Location | Windows | macOS | Linux |
| --- | --- | --- | --- |
| Install location (the fixed executable `bin\scriptura(.exe)`) | `%LOCALAPPDATA%\Programs\Scriptura` | `~/Library/Application Support/Scriptura/app` | `$XDG_DATA_HOME/scriptura` (default `~/.local/share/scriptura`) |
| State (DB, locks, publications,  the maintenance history (in the DB), the settings change history `config-history/`, the record of running maintenance `activity/` (removed when it ends), `ui-handoff.json` for the automatic sign-in after an update install (only a hash of the key; removed at the next start), `ui-sessions-<port>.json` to keep the resident sign-in (only hashes of sessions; kept after quitting)) | `%LOCALAPPDATA%\Scriptura\state` | `~/Library/Application Support/Scriptura/state` | `$XDG_STATE_HOME/scriptura` (default `~/.local/state/scriptura`) |

Setting `SCRIPTURA_HOME` to an absolute path puts state in `<SCRIPTURA_HOME>/state`, the install location in `<SCRIPTURA_HOME>/app`, and settings in `<SCRIPTURA_HOME>/config.toml`. `SCRIPTURA_DB` can also move just the state DB elsewhere. Templates go in `templates/` in the same folder as the settings file. Nothing is written to the Codex or Claude Code home (except when `connect claude-code --apply` / `--remove`, the Setup edition's automatic registration, and `uninstall-setup --apply` rewrite `settings.json`). The Setup edition's install location (Windows `%LOCALAPPDATA%\ScripturaApp`) is managed by Velopack and replaced on updates, so no state or settings are kept there.

**Maintenance history**: each run that started the worker, and each attempt that failed before it could, is recorded in the state DB (`maintenance_runs`) and shown in the admin UI: trigger, kinds, result (committed / no changes / rejected / held / failed / outcome unknown), documents with line counts, commit, and reason, but no contents or diffs. Up to 200 runs per repository and 2,000 overall are kept.

**Resident sign-in**: with `ui --tray` on a nonzero port, the cookie `scriptura_resident_<port>` (`HttpOnly; SameSite=Strict; Path=/`, 400 days, renewed each time a screen is opened) is checked against SHA-256 hashes of up to 16 sessions in `ui-sessions-<port>.json`. To sign out everywhere, quit and delete that file; see [Safety](safety.md) for how long a sign-in stays valid.
