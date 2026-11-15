---
name: docs
description: Show the configuration and state of the document kinds Scriptura maintains (AGENTS.md, CHANGELOG, custom kinds). Show Scriptura document kinds.
allowed-tools: Bash(scriptura docs *)
---

# Scriptura docs

Read-only. Run `scriptura docs list --json` and summarize each entry of `kinds`. Look at `id`, `enabled` (whether the kind is enabled for this repository), `paths` (`path` and `exists`), `template` (`spec` and `state`: none / builtin / ok / error), `maxChars` / `maxBytes` (limits; null means no limit), `timing` (`when` is push / release / manual, `paths`, `minIntervalHours`) and `lastDoneAt` (last completion time; null means none yet). When `registered` is false, tell the user this repository is not registered. For a kind whose `template.state` is `error`, report `template.error` as the reason maintenance is skipped.

To learn more about one kind, use `scriptura docs show <id> --json`. Its `kind` holds the guidance (`guidance`), extra instructions (`instructions`), required headings, the CHANGELOG settings (`changelog`) and the size of the target files (`paths[].chars` / `bytes`).

Do not change settings. Point the user to edit their configuration file for changes. Do not guess facts that are not in the JSON.

Reply in the language the user is writing in.
