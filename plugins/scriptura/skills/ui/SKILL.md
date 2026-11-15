---
name: ui
description: Open the Scriptura management UI (settings, document kinds, proposals, history, diagnostics) in the browser. Open the Scriptura settings UI in the browser.
allowed-tools: Bash(scriptura ui *)
disable-model-invocation: true
---

# Scriptura ui

Use this only when the user explicitly invokes it. The management UI can change settings, so it is never started automatically.

Run `scriptura ui --port 7468` **in the background** (this command does not finish until the UI is closed; use the Bash tool's `run_in_background` and do not wait for it to complete). Once started, the first line of standard output shows a URL of the form `http://127.0.0.1:<port>/auth?key=...` and the default browser opens.

- If the browser did not open, tell the user to open the URL from the output. The URL contains a one-time key, so tell them not to copy or share it outside the conversation (do not repeat the URL in this conversation).
- If the command exited because the port is in use, tell the user another management UI may already be running, and suggest running again with `--port 0` (any free port).
- If the command reports that the management UI is not included (the CLI's own message, whatever the language), tell the user the build does not include the `ui` feature.
- The UI ends with the "quit" button, Ctrl+C, or 60 minutes of inactivity.

The user changes settings themselves inside the UI. Do not change settings from the skill or operate the UI on the user's behalf.

Reply in the language the user is writing in.
