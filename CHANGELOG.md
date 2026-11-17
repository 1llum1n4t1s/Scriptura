# Changelog

## [Unreleased]

### Fixed

- Claude Code を一度も起動していない（`~/.claude` が無い）環境で、Setup 版の導入時と `connect claude-code --apply` の Stop hook の登録が「No such file or directory」で失敗していた。設定フォルダーを作ってから登録する。

## [1.0.1] - 2026-11-18

### Added

- macOS（Apple Silicon、11 以降）と Linux（x64・arm64、glibc 2.39 以降）向けの Setup を配信する。macOS は Developer ID で署名して公証した `.pkg`、Linux は AppImage で、Windows と同じ自動更新の仕組みで配信する。

### Fixed

- macOS など大文字小文字を区別しないファイルシステムで、ファイル名の大文字小文字だけが違う CHANGELOG（例: `Changelog.md`）の保守が、常に「対象文書以外の変更」として棄却されていた。

## [1.0.0] - 2026-11-18

### Removed

- **破壊的変更**: worker の費用上限（`worker.max_budget_usd`）を廃止した。管理画面の入力欄と概要の警告もなくなった。0.1.1 以前が保存した `config.toml` には、値を設定していなくても `[worker]` に `max_budget_usd = 0.0` が入っており、1.0.0 ではこのキーを未知のキーとして拒否するため、設定全体が無効になって保守が止まる。**移行: `config.toml`（Windows は `%LOCALAPPDATA%\Scriptura\config.toml`）の `max_budget_usd` の行を削除する。**

### Added

- 管理画面を Windows のタスクトレイ・macOS のメニューバーに常駐させられるようにした（`scriptura ui --tray`）。アイコンのクリックで管理画面を開き、右クリックのメニューから「管理画面を開く」「ログオン時に起動」「終了」を選べる。常駐中は 60 分操作が無いとログインが切れ、アイコンから開き直す。
- Setup 版で、ログオン時に管理画面を常駐させられるようにした（`scriptura ui --autostart on|off`、またはアイコンのメニュー。既定は OFF）。スタートメニューからの起動でも、案内のあとに管理画面を常駐させてブラウザーで開く。

### Changed

- ソースからビルドするのに必要な Rust を 1.100 以上にした。

### Security

- 依存の zip を 9.6 へ上げ、RUSTSEC-2026-0367 / 0368 / 0369 / 0429 を解消した（Setup 版の更新前の署名者検証で nupkg を読む経路）。rusqlite を 0.43 へ上げ、同梱の SQLite（libsqlite3-sys）の RUSTSEC-2026-0459 を解消した。Velopack（1.2.161 に固定）が内部で使う zip 8.6 は、Velopack を更新できるまで残る。

## [0.1.1] - 2026-11-15

### Added

- Windows 向けの Setup（Velopack）を追加した。ユーザー単位で導入でき、管理者権限は要らない。導入版は Claude Code の Stop hook へ自動で登録され、既定で自動更新する。更新はログオン時に、署名者を検証してから適用する。
- 表示言語を 9 言語（ja・en・zh-CN・zh-TW・ko・es・fr・de・pt-BR）に対応した。CLI・hook・Setup 版の案内は `SCRIPTURA_LANGUAGE` / `LC_ALL` / `LANG` / OS の表示言語、管理画面は言語の選択欄（Cookie）と `Accept-Language` で決まる。

### Changed

- README は英語の `README.md` を正本にし、日本語版を `README.ja.md` へ移して 8 言語版を追加した。worker へのプロンプトとプラグインのスキルは英語に統一した（文書の `language` 設定の挙動は変わらない）。

### Fixed

- プラグインを有効にしているとき、settings.json に Scriptura 自身の Stop hook が残っていると保守が二重に動く問題を直した。Setup 版が、残っていた Scriptura 自身の hook だけを外す（`agents-maintenance` など他の hook は残す）。
