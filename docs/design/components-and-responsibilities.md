# 主要コンポーネントと責務

| 部品 | 責務 |
|---|---|
| `config` / `kinds` / `timing` | 利用者設定の読み書き・検証・判定（`resolve` は I/O を持たない純粋関数）、文書の種類の解決と許可パス、種類ごとのタイミング |
| `maintenance` | 保守の本体。push 検知、状態 DB、publications、worker の起動、commit と push |
| `runlog` | 保守履歴。worker を起動した保守と、保守を試みて失敗した回を状態 DB の `maintenance_runs` へ 1 行ずつ記録し、管理画面の「保守履歴」が読む（feature `ui` に依存しない） |
| `changelog` | CHANGELOG 固有の処理（リリース済みエントリ不変の検査、資料の収集、`promote`） |
| `ops` | 設定を変える操作と読み取り操作の本体。CLI と管理画面が共有する |
| `cli` / `ui` | 利用者の入口。`cli` は薄い引数層、`ui` は `127.0.0.1` 限定の SSR 管理画面（feature `ui`）。`ui --tray` でタスクトレイ（Windows）・メニューバー（macOS）に常駐できる |
| `connect` / `app_update` | Claude Code の hook（git push 直後の PostToolUse とセッション開始時の SessionStart）の登録・解除と、Setup（Velopack）版の導入・自動更新 |
| `locale` | 9 言語の辞書と言語判定 |
