# hook と保守の起動・期限切れ

- 登録するのは PostToolUse（git push 直後。push のあとに後段が失敗したコマンドは PostToolUseFailure）と SessionStart（`startup|resume`。取りこぼしの回収）。PostToolUse・PostToolUseFailure は Bash・PowerShell ごとの項目に、`if` を 1 つ持つ hook（`Bash(* push*)`・`PowerShell(* push*)`）として置く。正本は `connect::PUSH_RULES`、`hooks.json` との一致は `tests/plugin_layout.rs` が検査する。
- `handle_hook` は PostToolUse・PostToolUseFailure（同じ扱い）と SessionStart を処理する。旧登録の互換で Stop と UserPromptSubmit も受け付け、知らないイベント名は何もしない。Stop は新規登録せず、旧登録の検出・移行・解除のためだけに読む。
- PostToolUse は `payload_may_push` で、`push` の語を含まないコマンドを状態 DB・ロックに触れず終える。判定できない形は処理側に倒す（誤発火は無害）。
- 非同期の PostToolUse・SessionStart は poll ロックを `HOOK_LOCK_WAIT`（25 分）まで待つ。UserPromptSubmit と旧 Stop は待たずに見送る。
- **タイミング**: 種類ごとに `when`（`push` / `release` / `manual`）・`paths`・`min_interval_hours`（AND）。期限が来た種類だけを 1 回の worker で処理し、どれも期限前なら worker を起動せず処理済み位置だけを進める。絞った種類は publication 状態の `kinds` に記録し、再開はその集合で行う。種類ごとの状態は最終完了時刻（`kind_runs`）だけで、成功時（棄却は数えない）にだけ更新する。処理済み位置はリポジトリに 1 つで、最小間隔は「間隔が過ぎたあとの次の push」で評価する。`run-now` はタイミングを無視し、`--documents` で絞っても位置は進む。
- **期限切れ・中断・HEAD 変更**: 終了を確認できたタイムアウトと中断では、安全に戻せる出力だけを戻し、判定できないときや実行中に HEAD が動いたときは触らずに止める。詳細は [troubleshooting.md](troubleshooting.md)。
