# プラグインのスキルとコマンド

- プラグインのスキルは CLI を呼ぶ薄い層。判断・検証・書き込みは `scriptura <cmd> --json` 側にある。CLI の引数・出力を変えたら `plugins/scriptura/skills/*/SKILL.md` と README を合わせて直す。
- スキルの `allowed-tools` は、そのスキルが使うサブコマンドだけを `Bash(scriptura <sub> *)` の形で事前承認する（`tests/plugin_layout.rs` が検証）。
- ランチャー `plugins/scriptura/bin/scriptura` は Git Bash では `scriptura.exe` へ渡す。
