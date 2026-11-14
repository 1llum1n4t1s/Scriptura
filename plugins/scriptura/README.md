# Scriptura プラグイン

push 後に AGENTS.md / DESIGN.md / README.md を保守する Scriptura を Claude Code から使うためのプラグインです。

## 構成

- `skills/` : CLI（`scriptura <cmd> --json`）の JSON 出力を要約する薄いスキル（setup / status / doctor / review / apply / run-now / pause / resume）。判断はすべて CLI 側にあり、スキルは同意の確認と結果の要約だけを担う。
- `hooks/hooks.json` : Stop フックで `scriptura hook` を非同期に実行します。
- `bin/scriptura` : POSIX sh ランチャー。macOS / Linux では `libexec/scriptura-<target>` を、Windows の Git Bash 経由（`uname -s` が `MINGW*` / `MSYS*` / `CYGWIN*`）では同じディレクトリの `scriptura.exe` を選んで exec し、対応外・本体不在では exit 0 で何もしません。
- 生成物（git 管理外）: `bin/scriptura.exe`、`libexec/scriptura-*`、`SHA256SUMS`。リリースビルドで配置します。

## 未検証の仮定（実機確認が必要）

- U1: Windows の exec form（`command` + `args`）で、拡張子なしの `${CLAUDE_PLUGIN_ROOT}/bin/scriptura` が `scriptura.exe` に解決されるか。
- U2: プラグインのキャッシュ複製後も `bin/scriptura` の実行ビットが保たれるか。保たれない場合はランチャーを `sh` 経由で起動する形へ変更する。
- フックの `async` と `timeout` の挙動（Stop で非同期に最長 1800 秒まで動くか）も実機確認前の前提です。
