# Scriptura プラグイン

push 後に AGENTS.md / DESIGN.md / README.md（と、オプトインで CHANGELOG.md や利用者定義のカスタム文書）を保守する Scriptura を Claude Code から使うためのプラグインです。

## 構成

- `skills/` : CLI（`scriptura <cmd> --json`）の JSON 出力を要約する薄いスキル（setup / status / doctor / docs / review / apply / run-now / pause / resume）。判断はすべて CLI 側にあり、スキルは同意の確認と結果の要約だけを担う。
- `hooks/hooks.json` : Stop フックで `scriptura hook` を非同期に実行します。
- `bin/scriptura` : POSIX sh ランチャー。macOS / Linux では `libexec/scriptura-<target>` を、Windows の Git Bash 経由（`uname -s` が `MINGW*` / `MSYS*` / `CYGWIN*`）では同じディレクトリの `scriptura.exe` を選んで exec し、対応外・本体不在では exit 0 で何もしません。
- 生成物（git 管理外）: `bin/scriptura.exe`、`libexec/scriptura-*`、`SHA256SUMS`。リリースビルドで配置します。

## 動作要件

- **Claude Code 2.1.139 以降**。hook の exec form（`command` + `args`）がこの版で追加されました（公式 CHANGELOG で確認）。それより古い版では `args` が無視される可能性があります。
- Windows では、exec form の `command` が実体の実行ファイルに解決される必要があります（公式ドキュメントで確認。`.cmd` / `.bat` は不可）。

## 実機確認の結果（Claude Code 2.1.325、Windows）

- **U1（確認済み）**: 拡張子なしの `${CLAUDE_PLUGIN_ROOT}/bin/scriptura` は、同じディレクトリの sh ランチャーではなく `scriptura.exe` を直接起動しました。引数（`hook`）・cwd・stdin（Stop の JSON）が渡ることを、ログを出す試験用の実行ファイルで確認しています。
- **U3（確認済み）**: 既定の `propose` + `restricted` の worker は、サブスクリプション認証のまま動作しました。`Read(./**)` だけで Glob / Grep が通り、許可文書以外の Edit・作業ツリー外の Read/Write・`.env` の Read・`git diff --output=...`・`curl` は拒否されました。
- 同期 hook では本体が `exit_code 0` で終わることを確認済みです。async hook は `-p` の終了時に取り消されると表示されますが、本体は起動します。長い処理が `-p` 終了で止まるかは未確認です。

## 未検証

- **U2**: プラグインのキャッシュ複製後も `bin/scriptura` の実行ビットが保たれるか（macOS / Linux。保たれない場合はランチャーを `sh` 経由で起動する形へ変更する）。接続できる Mac の Claude Code は 2.1.116 で exec form より古いため、そのままでは確認できません。
- **U4**: 対話セッションの終了時に、実行中の async hook が止められるか（対話 TTY が必要）。
