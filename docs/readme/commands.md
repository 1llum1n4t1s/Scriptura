# Commands

All commands except `ui` accept `--json` (one JSON object). Exit code 0 is success; 1 is a user error or refusal. Output contains no secret values.

| Command | Description |
| --- | --- |
| `enable --repo <path> [--branches a,b] [--documents AGENTS.md,README.md]` | Registers and enables a repository (documents are then committed and pushed automatically); also re-enables a registration stopped by the settings migration. A remote, a branch and an upstream are required; the current branch is used if `--branches` is omitted. `--documents` accepts the 4 built-in kinds (case-insensitive) and custom kind ids. The removed `--mode` / `--accept-consent` options and `review` / `apply` / `discard` commands are refused. |
| `disable --repo <path>` | Removes the registration. |
| `pause [--repo <path>]` / `resume [--repo <path>]` | Pause / resume (the cwd's repository if `--repo` is omitted). |
| `status` | Shows installation state, settings, registrations (with `paused`, `migratedFrom`), worker settings, `legacy` (old-format settings file), and `setup` (Setup edition details, including `trayAutostart`). `installedVersion` comes from `install.json` (or an earlier `current.json`). `installForm` is `setup`, `standalone`, or `other`; `installed` tells whether the standalone `install` location holds an executable. |
| `doctor [--verify]` | Diagnoses the environment and settings. |
| `run-now [--repo <path>] [--documents <id,...>]` | Runs maintenance once now (registered, not paused; timing ignored) and commits and pushes as usual. `--documents` narrows the kinds (an undefined or disabled kind is refused). **Note**: a narrowed run also advances the processed position, so other kinds are skipped for that push. |
| `docs list [--repo <path>]` / `docs show <id> [--repo <path>]` | Lists document kinds / shows one (enabled, targets, limits, timing, template status, last completion). Read-only; no contents. |
| `ui [--port <n>] [--no-open] [--tray]` / `ui --autostart on\|off` | Launches the admin UI (see [Admin UI](admin-ui-scriptura-ui.md)) / switches resident-at-logon (Setup edition). No `--json`. |
| `connect claude-code [--apply] [--executable <absolute path>]` / `--remove [--apply]` | Registers the settings.json hook / removes only Scriptura's hook. Preview without `--apply`. |
| `update [--check] [--auto on\|off]` | Shows the Setup edition's auto-update state / checks and downloads now (no apply) / switches auto-update. |
| `update-apply` | Applies a downloaded update after verifying the signer, only if no other Scriptura process runs (started at logon). Prints one JSON line with `status`: `unmanaged` / `disabled` / `nothingPending` / `busy` / `untrusted` (exit 2) / `applied`. |
| `uninstall-setup [--apply]` | Removes the Setup edition's registration (hook and logon apply). Preview without `--apply`. |
| `install` / `hook` | Places it at the install location (advanced; recorded in `install.json`) / runs as the hook (not called directly). |
