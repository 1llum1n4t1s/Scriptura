# Installation

### Plugin (recommended)

Installing it as a Claude Code plugin brings in the hook and skills (such as `/scriptura:setup`). Build: `xtask/`.

### Setup edition (auto-update)

`ScripturaApp-win-Setup.exe`, `ScripturaApp-osx-Setup.pkg`, or `ScripturaApp-linux-x64.AppImage` / `-arm64.AppImage` installs per user without administrator rights. Updates come from `https://scriptura-updates.kagayoi.com` (Velopack). Requires macOS 11+ on Apple Silicon or Linux with glibc 2.39+.

- **Registration is automatic** (at install on Windows; first launch without arguments on macOS / Linux): Claude Code's `settings.json` hook points at the Setup edition. Only Scriptura's own hooks are replaced.
- **Plugin conflict**: if `scriptura@*` is in `enabledPlugins`, the Setup edition does not register (it would run twice). After disabling the plugin, it registers on a launch without arguments or at the next logon.
- **Auto-update (on by default)**: after maintenance it checks at most once a day and only downloads. **It is applied only at logon** by `scriptura update-apply`, which checks the signer and that no hook or admin UI is running, otherwise waits for the next logon. The admin UI can also apply it.

```powershell
scriptura update [--check]       # show setting / check now
scriptura update --auto off      # or on
scriptura update-apply           # apply a downloaded update
scriptura uninstall-setup [--apply]   # remove registration
```

macOS and Linux have no uninstaller: run `scriptura uninstall-setup --apply` before removing the `.app` / AppImage (Windows "Settings > Apps" does it automatically). `config.toml` and state stay.

### Standalone `install` / `connect` (advanced)

Run `cargo build --release`, then `scriptura install` and `scriptura connect claude-code --apply` (omit `--apply` to preview). It registers the install path in the `settings.json` hook (`CLAUDE_CONFIG_DIR` or `~/.claude`), replacing only old Scriptura hooks. It backs up to `settings.json.scriptura-backup` first; broken JSON is not rewritten.

The old per-turn Stop hook is replaced by PostToolUse and PostToolUseFailure for Bash / PowerShell commands matching `Bash(* push*)` (and `PowerShell(* push*)`), plus SessionStart on `startup` / `resume`. They are asynchronous and wait up to 25 minutes for another maintenance run. A push outside Claude Code is picked up when a session next starts in that repository. The old Stop hook migrates the user-level `settings.json` once; with the plugin enabled, it is only removed.

When moving to the plugin, run `scriptura connect claude-code --remove --apply`.

The worker does not inherit the desktop session's relay settings, so run `claude auth login` in the standalone CLI first. The CLI is `worker.claude_path`, otherwise `claude` on PATH (on Windows also the Claude desktop app's).
