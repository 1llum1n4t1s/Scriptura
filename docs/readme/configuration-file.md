# Configuration file

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
