# ローカライズ（9 言語）

- 共通仕様は `D:\Dev\APP-DISTRIBUTION.md` の 8 節。対応言語は ja・en・zh-CN・zh-TW・ko・es・fr・de・pt-BR。実装は `src/locale.rs`（言語判定・辞書・`t!` マクロ）と `src/locales/<タグ>.json`、管理画面の判定は `src/ui/locale.rs`（Cookie `scriptura-language-<port>` → `Accept-Language` → 英語。`POST /language` は他の POST と同じく `guard` を通る）。
- 人が読む文言（CLI の出力・ヘルプ・エラー・hook の `systemMessage`・Setup 版の案内・管理画面）は辞書のキー（`t!("cli.xxx")`、テンプレートは `crate::locale::tr("...")`）で参照し、直書きしない。**人が読む文言を足したら、9 言語の辞書（`src/locales/*.json`）と整合テスト（`tests/localization.rs`）を更新する**。キーは文字列リテラルで書く（テストがソースを走査する）。プレースホルダー `{name}` は全言語で同じ集合。
- AI が読む文言（worker へのプロンプト・`kinds::settings_frame`・`changelog::prompt_parts`・プラグインの SKILL.md）は**英語に固定**し、辞書に入れない。文書の言語設定（`language`）は画面の言語とは別（無いとき既存文書の言語に従い、新規作成は日本語）。
- 分岐に文言を使わない（未確定で止めた印は `outcome_unknown_error`、タイムアウトは `WorkerFailure.timed_out`）。旧版の日本語文言との互換だけ `contains` を残している。
- テストの言語: `.cargo/config.toml` が `SCRIPTURA_LANGUAGE=ja` を渡す。管理画面のテストは `Accept-Language: ja` を付ける。
- **README を変えたら全言語版（`README.md` が英語の正本、ほかは `README.<タグ>.md` の 8 つ）を同時に直す**。先頭の 9 言語の切替行は全ファイルで同じ（自分の言語だけ太字）。AGENTS.md・DESIGN.md・CHANGELOG.md は訳さない。
