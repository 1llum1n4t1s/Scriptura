# 実行中の保守の記録（`src/activity.rs`）

- **記録**: 保守 1 件ごとに、状態 DB の隣（`SCRIPTURA_DB` に従う。既定は状態フォルダー）の `activity/` へ JSON 1 ファイルを置き、`Guard` の Drop で消す。項目は version・pid・process_started・origin・repo_root・phase・started_at・phase_at・deadline_at・fixup・worker_pid だけ。
- **更新**: 記録は thread-local で、`begin`（`poll_repository_inner` の保守・再開の直前）・`phase` / `worker`（`publish_locked_with`）・`worker_spawned`（`process::capture`）で更新する。書き込みは best-effort で、失敗しても保守の結果・出力・履歴を変えない。
- **読み取り**（副作用なし）: `list()` は PID の生存と開始時刻の一致を OS で確かめた記録だけを返す（壊れた・未知の版・大きすぎるものは無視。上限 100 件）。`processes()` は自分と同じ実行ファイルのプロセスを返す。OS ごとの開始時刻は `app_update/processes.rs` の `probe`。
- **掃除**: `begin` の中で、生存していないと確認できた記録だけを消す。
