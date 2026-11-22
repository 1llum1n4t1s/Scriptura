# Migrating old settings (consent and propose removed)

- The settings file is now always saved as `schema = 3`. A file with `schema` 1, 2 or none is read as the old format and converted when read; the next hook (or any settings change) saves it as schema 3 once, setting the previous content aside in `config-history/`, and notifies you once of how many registrations were stopped.
- A registration that had consent and effectively ran in `push` mode keeps running unchanged. Every other registration (`propose`, `off`, or no consent) is paused with a note of where it came from (`migrated_from`) and does nothing until you enable it again with `scriptura enable --repo <path>` or in the admin UI (`resume` and restoring from the history do not resume it, because `enable` checks the remote and upstream). `status` shows `migratedFrom`, and `doctor` reports these registrations.
- `enrollment.policy` of `all` / `remote-owner` goes back to `explicit` (the owners are kept), unless the file had consent and `defaults.mode = "push"`.
- The `consent` and `mode` keys are removed. A schema 3 file that still has them is invalid, and nothing runs.
- Patches left in `<state>/proposals/` are neither read nor deleted. Delete them by hand if you do not need them; an unapplied patch can be applied with `git apply`. Scriptura no longer cleans up the throwaway clones (`work-*`) either.
- An older Scriptura reading a schema 3 file does nothing, so when you use the plugin and the Setup edition together, update both.
