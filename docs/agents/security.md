# セキュリティ（秘密値と管理画面の保護）

- CLI の出力（JSON・人が読む形式・doctor）に秘密値を入れない。パス・設定の要約・件数・定型の語だけにする。例外は `docs show` が返す利用者設定 `guidance` / `instructions` の本文。
- `claude auth status` は `maintenance::scrub_host_env` をかけ、終了コードと `authMethod` だけを見る。
- 管理画面は `127.0.0.1` だけに bind する。保護（Host → Origin / `Sec-Fetch-Site` → セッション Cookie → CSRF）は `guard` の 1 か所に集める。使い捨て鍵は一度だけ有効。セッション Cookie 名はポートを含める（`session_cookie_name`）。
- 設定を変える POST は `handlers::apply_change` を通し、版の照合と `confirm::dangers(before, after)` による二段階確認を強制する（1 回目は書き込まず、nonce は 10 分で失効）。設定の書き込みと `run-now` は `ops` の関数だけを呼ぶ。
- GET は状態フォルダー・DB・ロックを作らない。秘密値・環境変数を表示しない（エンジン由来の自由文は `views::redact` を通す）。
- 管理画面は 60 分無操作・Ctrl+C・画面の「終了」で終わる。`ui --tray`（Windows・macOS だけ）は常駐で、終了もセッションの失効もせず、アイコンの「開く」のたびに使い捨て鍵を発行し直す。
- 画面を足したら `tests/ui.rs` に保護・エスケープ・秘密値非出力の検査を足す。
