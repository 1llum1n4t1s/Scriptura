---
name: run-now
description: Run Scriptura document maintenance once, right now. Run Scriptura maintenance now.
disable-model-invocation: true
allowed-tools: Bash(scriptura run-now *)
---

# Scriptura run-now

This starts the maintenance worker once without waiting for a push. It uses a Sonnet worker, so it incurs cost, and the repository contents are sent to Anthropic using the user's own Claude authentication. Any document changes are committed and pushed to the current branch right away; nothing is held back for review.

1. Tell the user the above and obtain an explicit yes in chat. Do not run anything without it.
2. After that, run `scriptura run-now --json` and summarize the result. On success `outcome` is `completed` (the maintenance ran; if the documents changed, they are already committed and pushed). When the run is refused because the repository is not registered, is paused or was stopped by the settings migration, and so on, report `error`.
3. To limit the document kinds to process, use `scriptura run-now --documents <id,...> --json` (the ids can be checked with `/scriptura:docs`). Kinds that are undefined or not enabled for this repository are refused with `error` without starting the worker.

Reply in the language the user is writing in.
