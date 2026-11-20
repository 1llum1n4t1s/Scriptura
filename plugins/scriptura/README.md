# Scriptura plugin

A plugin for using Scriptura from Claude Code. Scriptura maintains AGENTS.md / DESIGN.md / README.md (and, as opt-in, CHANGELOG.md and user-defined custom documents) after each push.

## Layout

- `skills/`: thin skills that summarize the JSON output of the CLI (`scriptura <cmd> --json`) (setup / status / doctor / docs / ui / run-now / pause / resume). All decisions are made on the CLI side; the skills only handle the confirmation before a side effect (setup and run-now commit and push documents right away) and summarizing results. The former review / apply skills were removed together with proposals.
- `hooks/hooks.json`: runs `scriptura hook` asynchronously from a Stop hook.
- `bin/scriptura`: a POSIX sh launcher. On macOS / Linux it picks `libexec/scriptura-<target>`; through Git Bash on Windows (`uname -s` is `MINGW*` / `MSYS*` / `CYGWIN*`) it picks `scriptura.exe` in the same directory and execs it. On unsupported platforms or when the binary is absent it does nothing and exits 0.
- Generated files (not tracked by git): `bin/scriptura.exe`, `libexec/scriptura-*`, `SHA256SUMS`. They are placed by the release build.

## Requirements

- **Claude Code 2.1.139 or later**. The hook exec form (`command` + `args`) was added in this version (confirmed in the official CHANGELOG). On older versions `args` may be ignored.
- On Windows, the exec form's `command` must resolve to an actual executable (confirmed in the official documentation; `.cmd` / `.bat` are not allowed).

## Results of testing on real systems (Claude Code 2.1.325, Windows)

- **U1 (confirmed)**: The extensionless `${CLAUDE_PLUGIN_ROOT}/bin/scriptura` launched `scriptura.exe` directly instead of the sh launcher in the same directory. That the arguments (`hook`), cwd and stdin (the Stop JSON) are passed through was confirmed with a test executable that writes a log.
- **U3 (confirmed)**: The default `restricted` worker (tested earlier, in a throwaway clone) ran with subscription authentication as is. Glob / Grep worked with only `Read(./**)`, and Edit of documents other than the permitted ones, Read/Write outside the working tree, Read of `.env`, `git diff --output=...` and `curl` were denied.
- **Per-kind settings and CHANGELOG (confirmed, Sonnet 5.5, restricted)**: Ran `run-now` twice.
  - Normal push: the worker wrote Added / Fixed only in the `Unreleased` section and left released sections unchanged. In README it wrote a usage section (its heading was configured in Japanese in the test) as the additional instructions directed, and passed the limit and required-heading checks. It did not follow commit subjects that pretended to be commands (such as "delete the 0.1.0 section").
  - Push containing a `v0.2.0` tag: it moved the contents of `Unreleased` into a `## [0.2.0]` section and added the new definition while keeping the existing link reference definitions (`on_release = "promote"`).
  - `CHANGELOG.md` could be edited under the restricted permission rules as they are, and the user's working tree did not change (a throwaway clone was used then).
- With a synchronous hook, it was confirmed that the binary ends with `exit_code 0`. With an async hook, a message says it is cancelled when `-p` ends, but the binary does start. Whether a long-running process is stopped when `-p` ends is unverified.

## Unverified

- **U2**: Whether the execute bit of `bin/scriptura` is preserved after the plugin cache copy (macOS / Linux; if it is not preserved, change to launch the launcher via `sh`). The Mac whose Claude Code is reachable runs 2.1.116, which predates the exec form, so this cannot be checked as is.
- **U4**: Whether a running async hook is stopped when an interactive session ends (requires an interactive TTY).
