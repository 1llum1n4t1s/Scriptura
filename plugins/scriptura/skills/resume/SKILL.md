---
name: resume
description: Resume Scriptura after it was paused. Resume Scriptura.
disable-model-invocation: true
allowed-tools: Bash(scriptura resume *)
---

# Scriptura resume

Run `scriptura resume --json` and tell the user it has resumed. If the CLI refuses because the registration was stopped by the settings migration, tell the user to run `/scriptura:setup` (that is, `scriptura enable --repo <path>`) instead.

Reply in the language the user is writing in.
