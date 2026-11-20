---
name: status
description: Show the current Scriptura state (enabled, paused, latest run). Show Scriptura status.
allowed-tools: Bash(scriptura status *)
---

# Scriptura status

Read-only. Run `scriptura status --json` and summarize the output JSON. Look at `config.state` (missing / invalid / ok), `legacy` (non-null when the settings file is still in the old format and was converted on read), `repositories` (each `root`, `paused`, `migratedFrom`, `branches`) and `worker`. A repository with `migratedFrom` set (`propose` / `off` / `unconsented`) was stopped by the settings migration and runs only after `scriptura enable --repo <path>` (the `resume` command does not restart it). When `config.state` is `invalid`, report `config.error`. Do not guess facts that are not in the JSON (such as the result of the latest run).

Reply in the language the user is writing in.
