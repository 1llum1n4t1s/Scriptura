# Scriptura

自分のリポジトリへ push したあとに、`AGENTS.md` / `DESIGN.md` / `README.md` / `CHANGELOG.md` をバックグラウンドで保守する単独アプリです。Claude Code の Stop hook から起動し、保守の worker には Claude Code CLI（`claude -p --model claude-sonnet-5-5 --effort high`）を使います。Codex には依存しません。

以前の `agents-maintenance`（CodexHooks）から文書保守の部分だけを独立させたもので、保守の判断・状態管理・公開（commit と push）の契約は同じです。

## 使い方

```powershell
Set-Location D:\Dev\Scriptura
cargo build --release
.\target\release\scriptura.exe install                       # 利用者ごとの導入先へ固定パスで置く
& "$env:LOCALAPPDATA\Programs\Scriptura\bin\scriptura.exe" connect claude-code          # 登録内容のプレビュー
& "$env:LOCALAPPDATA\Programs\Scriptura\bin\scriptura.exe" connect claude-code --apply  # settings.json へ登録
& "$env:LOCALAPPDATA\Programs\Scriptura\bin\scriptura.exe" status
```

| 場所 | Windows | macOS | Linux |
| --- | --- | --- | --- |
| 導入先（固定の実行ファイル `bin\scriptura(.exe)`） | `%LOCALAPPDATA%\Programs\Scriptura` | `~/Library/Application Support/Scriptura/app` | `$XDG_DATA_HOME/scriptura`（既定 `~/.local/share/scriptura`） |
| 状態（DB・ロック・publications） | `%LOCALAPPDATA%\Scriptura\state` | `~/Library/Application Support/Scriptura/state` | `$XDG_STATE_HOME/scriptura`（既定 `~/.local/state/scriptura`） |

`SCRIPTURA_HOME` を絶対パスで設定すると、状態は `<SCRIPTURA_HOME>/state`、導入先は `<SCRIPTURA_HOME>/app` になります。`SCRIPTURA_DB` で状態 DB だけを別の場所にもできます。

`connect claude-code --apply` は Claude Code の `settings.json`（`CLAUDE_CONFIG_DIR` または `~/.claude`）の Stop hook に、導入先の固定パスを登録します。従来の `agents-maintenance` を呼ぶ項目と Scriptura の古い項目だけを置き換え、ほかの設定は保ちます。最初の変更前に `settings.json.scriptura-backup` を作り、壊れた JSON は書き換えません。

worker はデスクトップセッションの中継設定を引き継がないため、事前に単体 CLI で `claude auth login` を済ませてください。CLI は PATH 上の `claude.exe`、なければ Claude デスクトップ同梱の `%LOCALAPPDATA%\Claude-Data\claude_*\claude-code\<版>\<ハッシュ>\claude.exe` の最新を使います。

## 状態の引き継ぎ

旧 `agents-maintenance` の状態（`~/.codex/hooks/state`）は引き継がず、空の状態から始めます。最初の push から登録し直し、過去の push は再処理しません。

## 検証

```powershell
Set-Location D:\Dev\Scriptura
cargo test --locked --features test-support -- --test-threads=1
```

テストはローカルの隔離 Git リポジトリとネイティブスタブを使い、実際の Claude CLI や外部リポジトリを呼びません。`validation` フォルダーは自動削除しないので、出力された絶対パスを確認して Prunlet で清掃してください。`hook-test-stub` は `test-support` feature 専用で、配置しません。
