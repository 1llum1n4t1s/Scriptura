---
name: setup
description: Enable Scriptura for this repository (with a confirmation). Enable Scriptura document maintenance for the current repository.
disable-model-invocation: true
allowed-tools: Bash(scriptura enable *)
---

# Scriptura setup

This enables post-push document maintenance for this repository. Because it has side effects, always proceed in this order.

1. Explain the following to the user.
   - Documents are committed and pushed automatically: after every push, the worker updates the documents and Scriptura commits and pushes them to the current branch right away. Nothing is held back for review. The repository needs a remote and an upstream, and enabling is refused without them.
   - Data sent: the repository contents the worker reads are sent to Anthropic using the user's own Claude authentication.
   - Cost: for each push, a Sonnet worker runs at most once, up to the time limit.
2. Obtain the user's explicit yes in chat. If it is missing or ambiguous, run nothing and stop.
3. Only after that, run the following for the current directory.

```
scriptura enable --repo <cwd> --json
```

Do not pass `--mode` or `--accept-consent`; they were removed and the CLI refuses them. If the repository was stopped by the settings migration (`migratedFrom` in `scriptura status --json`), this same command enables it again.

By default three kinds of documents are maintained: AGENTS.md / DESIGN.md / README.md. Add CHANGELOG.md or a custom kind (an id defined as a document kind in the configuration file or on the management screen's Settings > Documents tab) only when the user asks, by listing them explicitly, for example `--documents AGENTS.md,DESIGN.md,README.md,CHANGELOG.md` (the CLI rejects ids that are not defined).

4. Briefly summarize the resulting JSON (`ok`, `branches`, `documents`, `configPath`, and `error` on failure). On failure, report the content of `error` and, if needed, point the user to `/scriptura:doctor`.

Reply in the language the user is writing in.
