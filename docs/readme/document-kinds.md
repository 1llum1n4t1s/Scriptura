# Document kinds

Maintained documents are "kinds" (admin UI: Settings > Documents).

| Kind | Target | Default |
| --- | --- | --- |
| `agents` / `design` / `readme` | `AGENTS.md`, `DESIGN.md`, root `README.md` | Enabled |
| `changelog` | Root `CHANGELOG.md` | Opt-in |
| Custom (`[kinds.<id>]`) | Fixed `paths` (up to 20 kinds) | Only when listed |

Kinds are listed in `defaults.documents` (per repository: `documents`) by file name or id.

### Per-kind settings (`[kinds.<id>]`)

Read only from `config.toml`; all optional, unknown keys rejected. A custom id is lowercase letters, digits and `-`, starting with a letter, at most 32 characters (`new` reserved).

| Key | Description |
| --- | --- |
| `paths` | Custom only, required: 1 to 10 relative paths (`md` / `markdown` / `mdx` / `txt` / `rst` / `adoc`). `.git`, built-in targets, `exclude` matches, and duplicates are rejected. |
| `guidance` | Custom only, required: purpose (at most 1000 characters). |
| `create_if_missing` | Whether a missing target may be created (built-in true, custom false). |
| `template` | `"builtin"` / `"none"` / `"templates/<name>.md"` under the settings folder (at most 16 KiB; invalid skips the run). Used only when creating. |
| `max_chars` / `max_bytes` | Limits; `[defaults]` values take precedence. `agents` `max_bytes` at most 29,999. |
| `split_dir` | Where overflow detail may move (default `docs/<id>`; up to 20 files). |
| `instructions` | Extra instructions (at most 2000 characters); cannot change paths or safety rules. |
| `language` | Document language (`ja`, `en-US`, ...); default follows the existing document. |
| `required_headings` | Headings checked after maintenance (at most 20). |

### Post-maintenance checks

Rust checks limits, headings, AGENTS under 30,000 bytes, and released CHANGELOG immutability in every mode. Limit or heading violations trigger up to two reruns, then commit and push with a warning. An AGENTS over 30,000 bytes or a changed released CHANGELOG entry discards the output without committing.

### Timing (`[kinds.<id>.timing]`)

| Key | Description |
| --- | --- |
| `when` | `push` (default) / `release` (a tag matching `release_tag`, default `v*`, has no `CHANGELOG.md` section) / `manual` (only `run-now`) |
| `paths` | Globs; due only if a changed path in the push range matches |
| `min_interval_hours` | Minimum interval since last completion (0 to 720) |

Conditions are ANDed and evaluated only on a new push or `run-now` (which ignores timing).

### CHANGELOG maintenance (`[kinds.changelog.changelog]`)

- Each push updates only `Unreleased`; changing released sections is a violation.
- `on_release = "promote"` (default) moves `Unreleased` into the new tag's section; `ignore` does not.
- `backfill = "tags"` (default) builds sections from tags when the file is missing (`backfill_max_releases` 1 to 50, default 20; `backfill_max_commits` 1 to 1000, default 300); `none` does not.
- A tag-only push is detected on the next branch push.
