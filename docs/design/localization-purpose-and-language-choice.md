# ローカライズ（目的・言語の決め方）
### 目的と範囲

共通仕様（`D:\Dev\APP-DISTRIBUTION.md` の 8 節）に従い、9 言語（ja・en・zh-CN・zh-TW・ko・es・fr・de・pt-BR）で、人が読む文言を実行時に切り替える。

- 9 言語にする: CLI の出力・ヘルプ・エラー、Claude Code の hook が画面へ出す通知（`systemMessage`）、Setup 版の案内（`welcome`・`doctor`・`update`）、管理画面（テンプレート・画面のエラー・確認・ジョブの結果・JS の単位表示）、README（`README.md` が英語の正本）。
- 英語に固定する: worker（Claude Code）へのプロンプト（`maintenance::build_maintenance_reason_with`、`kinds::settings_frame`、`changelog::prompt_parts`）、プラグインの SKILL.md。AI の応答言語は利用者の依頼に合わせる。
- 訳さない: 利用者のデータ（生成文書の本文、利用者が設定した `guidance`・`instructions`）、識別子・コマンド・パス・設定キー、JSON のキーと値、技術的なエラー詳細（OS・git の原文）。
- 生成する文書の言語設定 `language` は文書の言語で、表示言語とは別。worker のプロンプトを英語にしたため、`language` が無いときの挙動（既存文書の言語に従い、新規作成は日本語）はプロンプトに明記して従来どおりに保つ。

### 言語の決め方

- CLI・hook・Setup 版の案内: `SCRIPTURA_LANGUAGE` → `LC_ALL` → `LC_MESSAGES` → `LANG`（空・`C`・`POSIX` は飛ばす。未対応の言語はその場で英語）→ OS の表示言語（Windows は `GetUserDefaultUILanguage`、macOS は `AppleLanguages`）→ 英語。判定は 1 プロセスで 1 回（`locale::detect_with` が純粋関数）。
- 管理画面: 明示の選択（Cookie `scriptura-language-<port>`）→ `Accept-Language`（q 値つきの最良一致。未対応の言語は飛ばす）→ 英語。選択欄は `POST /language` で Cookie を書く（GET は副作用なし）。Cookie 名にポートを含め、別の起動と混ざらない。戻り先は同一オリジンの Referer のパスだけ。
- タグの照合: `zh-Hans`・`zh-CN`・`zh-SG` は zh-CN、`zh-Hant`・`zh-TW`・`zh-HK`・`zh-MO` は zh-TW、`pt` 系は pt-BR、それ以外は主言語。
