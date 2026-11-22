# Automatic migration of legacy users

A user who ran an older Scriptura (a version that had no settings file) and still has a state DB, when running a hook without a settings file, gets a settings file automatically created exactly once with the same behavior as before (all repositories, `push` mode, permission bypass), and is notified of this.  Nothing is created for new users. If you want to narrow the targets, register again with `enable` and set `policy = "explicit"`. The state of the old `agents-maintenance` (`~/.codex/hooks/state`) is not carried over; it starts from an empty state.
