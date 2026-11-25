# Admin UI (`scriptura ui`)

```powershell
scriptura ui [--port <n>] [--no-open] [--tray]
scriptura ui --autostart on|off
```

A browser screen for repositories, settings, history, and diagnostics, in 9 languages (footer selector cookie → `Accept-Language` → English). It needs cargo feature `ui` (default); without it the command refuses.

- **Listening and access**: only `127.0.0.1` (default port 7468; `--port 0` picks a free one; exits if in use). See [Safety](safety.md) for access and protection.
- **Ending**: "Quit", Ctrl+C, or 60 minutes idle. `--tray` (tray icon on Windows, menu bar on macOS; Linux refuses) keeps it resident: no idle exit or session expiry, and each open issues a new one-time key (valid 5 minutes). With a nonzero port, the sign-in also survives restarts of the app, the PC, and the browser, so pinned tabs stay signed in; "Quit" does not sign out. See [State and storage locations](state-and-storage-locations.md) for what is stored and how to sign out everywhere.
- **Resident at logon (Setup edition; on by default)**: turn off with `scriptura ui --autostart off` or the App screen's checkbox (`tray_autostart` in `app-update.json`; missing or unreadable counts as on). It starts after the logon-time `update-apply`. Not available with the plugin-bundled or standalone `install` executable.
- **App updates (Setup edition)**: Settings › App (`/settings/app#updates`) shows the version and checkboxes, "Check for updates" (ignores the 24-hour throttle, downloads nothing), and "Install update": it downloads, verifies the signer, and, if no other Scriptura process or run-now is active, asks the updater to apply it and ends the UI (otherwise retry later). The tray's "Check for updates" opens this panel and starts a check.
- **Confirmations and conflicts**: a save is rejected with a reload request if the file changed elsewhere, and risky changes need one confirmation; see [Writing settings and change history](writing-settings-and-change-history.md).
- **Screens**: header: Overview (warnings, a live view of running maintenance and Scriptura processes, recent maintenance, repository status) / Repositories (register and enrollment policy; details and actions per repository) / Maintenance history (each run; see [State and storage locations](state-and-storage-locations.md)) / Settings (`/settings/documents`). Settings tabs: Documents / Running maintenance (model and effort) / App (updates, resident at logon, version, settings file) / Settings history (diffs, restore) / Diagnostics; the forms are described in [Writing settings and change history](writing-settings-and-change-history.md). `run-now` runs in the background.
