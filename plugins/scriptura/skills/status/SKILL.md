---
name: status
description: Show the current Scriptura state (enabled, paused, latest run). Show Scriptura status.
allowed-tools: Bash(scriptura status *)
---

# Scriptura status

Read-only. Run `scriptura status --json` and summarize the output JSON. Look at `config.state` (missing / invalid / ok), `consent`, `repositories` (each `root`, `mode`, `paused`, `branches`, `proposals.pending`), `worker` and `proposals`. When `config.state` is `invalid`, report `config.error`. Do not guess facts that are not in the JSON (such as the result of the latest run).

Reply in the language the user is writing in.
