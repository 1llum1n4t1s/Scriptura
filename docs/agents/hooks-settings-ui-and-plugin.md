# hook・Claude Code 設定・管理画面・プラグイン

- **hook のイベントと保守の起動**（PostToolUse・PostToolUseFailure・SessionStart、旧 Stop の受け付け）は [workflow.md](workflow.md)。
- **Claude Code の `settings.json`** の書き換え（登録・解除・旧 Stop 登録の移行・バックアップ）は [configuration.md](configuration.md)。
- **CLI の出力と管理画面の保護**（秘密値・Host・Origin・Cookie・CSRF・二段階確認）は [security.md](security.md)。
- **管理画面の画面とルート**は [api-reference.md](api-reference.md)、構成（`src/ui/`）は [modules-and-responsibilities.md](modules-and-responsibilities.md)。
- **プラグインのスキル**（CLI を呼ぶ薄い層・`allowed-tools`・ランチャー）は [commands.md](commands.md)。CLI の引数・出力を変えたら、スキルと README を合わせて直す。
