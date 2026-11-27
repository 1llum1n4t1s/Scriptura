[日本語](README.ja.md) | **English** | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md) | [한국어](README.ko.md) | [Español](README.es.md) | [Français](README.fr.md) | [Deutsch](README.de.md) | [Português (Brasil)](README.pt-BR.md)

# Scriptura

Scriptura maintains `AGENTS.md` / `DESIGN.md` / `README.md` (and, as opt-ins, `CHANGELOG.md` and custom documents) in the background after you push to your own repositories. Claude Code hooks start it, and its worker is the Claude Code CLI.

- **Safe by default.** Nothing runs until you register a repository with `scriptura enable` (or in the admin UI). After each push, the worker updates the target documents, and Scriptura commits and pushes them only if the post-run checks pass.
- Settings are read only from `config.toml` in your user settings folder, never from files inside a repository.

## Contents

- [Installation](docs/readme/installation.md): Plugin, Setup edition (auto-update), and standalone install.
- [First-time setup](docs/readme/first-time-setup.md): Registering a repository with `scriptura enable`.
- [Configuration file](docs/readme/configuration-file.md): Location and format of `config.toml`.
- [Document kinds](docs/readme/document-kinds.md): Built-in and custom kinds, limits, timing, and CHANGELOG.
- [Writing settings and change history](docs/readme/writing-settings-and-change-history.md): How settings are saved and restored.
- [Admin UI (`scriptura ui`)](docs/readme/admin-ui-scriptura-ui.md): The local browser UI and tray mode.
- [Language of output](docs/readme/language-of-output.md): The nine supported languages and how one is chosen.
- [Commands](docs/readme/commands.md): The CLI commands.
- [Safety](docs/readme/safety.md): Opt-in, commit and push, and post-run checks.
- [Cost and data transmission](docs/readme/cost-and-data-transmission.md): Your Claude authentication and usage charges.
- [Diagnostics (doctor)](docs/readme/diagnostics-doctor.md): The `doctor` checks.
- [State and storage locations](docs/readme/state-and-storage-locations.md): Where files are kept on each OS.
- [Automatic migration of legacy users](docs/readme/automatic-migration-of-legacy-users.md): Settings created for older installs.
- [Migrating old settings (consent and propose removed)](docs/readme/migrating-old-settings-consent-and-propo.md): Conversion of schema 1 and 2 settings.
- [Not supported / not verified](docs/readme/not-supported-not-verified.md): Platforms and features not verified on hardware.
- [Verification](docs/readme/verification.md): Test and build commands.
