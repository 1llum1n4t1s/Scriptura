# 分割先（`split_dir`）の契約

- `kinds::split_candidates` が許可パスへ入れる既存ファイルは、分割先直下の追跡済みで安全な名前の `*.md` だけ（`section-NN.md` も編集でき、件数に数える）。新規ファイル名は worker が内容に合わせて付ける（理由は DESIGN.md）。
- 名前（`kinds::split_file_name_ok`）: 直下のみ、拡張子は小文字の `.md`、拡張子前は ASCII 小文字・数字・`-` の 3〜40 文字で先頭は英字、`-` は末尾・連続不可。`agents` / `design` / `readme` / `changelog` と Windows の予約名、`-` で区切った部分がすべて数字か仮置き語（`section`・`part`・`details`・`misc` など）の名前は不可。重複は大文字小文字を区別せず判定する。
- 新規作成の許可（`kinds::split_grant`）: 分割先がリポジトリ内のリンクでない安全なパスで、無視対象でなく、直下の `.md` がすべて編集を許した追跡済みの未変更ファイルで（`git status` と `git ls-files` で確かめる）、件数が上限未満のときだけ。欠ければ既存の編集のみ。件数は既存と新規を合わせて 1 種類 20 件（`MAX_SPLIT_FILES`）まで。
- 公開の状態の `split_dirs` に分割先・既存エントリ・許可の有無を記録し、`restricted` は許可があり残り件数がある分割先ごとに `Edit(./<dir>/*.md)` を足す。記録のない古い状態では新規作成を許さない。同じ HEAD の再試行では起動前に既存エントリと許可を取り直し、条件を欠いた分割先は新規作成不可にする（失敗した worker の受理済みファイルが未追跡で残る場合を含む）。
- 検査（`kinds::new_split_entries`）は、worker が正常終了・終了を確認できた期限切れ・失敗で終わるたびに、全体比較より前に行う。終了を確認できない期限切れと中断では行わない。受理した新規ファイルは許可パス・`before` / `after`（null）・その起動の checkpoint へ足してすぐ保存する（復旧は記録と違うファイルを消さない）。Git の無視対象になる（または確かめられない）新規ファイルは拒否する。
- 受理しない新規エントリは違反: 許可した分割先ではフォルダー・リンク・`.md` 以外・名前違反・件数超過、許可していない分割先では `.md`。1 つでもあれば権限モードに関係なく `worker_failed` にし、commit せず、ファイルは消さない。指紋は拒否パスの `path_fingerprint`（`split:` 印）で取る（無視対象や空フォルダーは Git の状態に出ないため）。解除は内容の削除・変更で、`git add` では解除されない。同じ HEAD では違反のパスを消すか変えるまで再実行しない。新しい push では記録を作り直し、残った直下の `.md` は新規作成の停止要因になる。
- 終了を確認できた期限切れと拒否が重なったときは、先に checkpoint の内容へ戻し、期限切れの通知に違反の文（`maint.split_name_invalid_no_retry`）を続け、追い修正の通知は `maint.fixup_timeout_reverted` にして、commit せず `worker_failed` と `worker_timed_out` にする。同じ HEAD では自動で再実行しない（新しい push か run-now）。
- 案内: 分割先直下の各 `.md` は親文書から相対パスで案内し、`kinds::check_split_links` が欠けを `MissingSplitLink`（ソフト違反）にする。規則・一致の判定・宛先は [split-link-check.md](split-link-check.md)。
- 早めの分割（`kinds::size_stage`、段階は `SizeStage`）: 文字数（`max_chars`）かバイト数（`max_bytes`）のどちらか一方でも上限の 80% を超えた（`n * 5 > 上限 * 4`、u128 で比べる。ちょうど 80% は含まない）、上限に達していない文書は、`settings_frame` の `Early split` 行で分割を検討するよう worker に伝える（要点は残し、話題ごとの詳細を `split_dir` へ移し、親文書から案内する）。違反ではなく、検査は上限超過のままである。ちょうど上限（`AtLimit`）と上限超過の文書には、既存の `Splitting` 行（「達したら分割」）だけを出す。上限（`max_chars` / `max_bytes`）の無い種類と CHANGELOG（どちらも `split_dir` を持たない）と、分割先へ何も置けない状態では出さない。AGENTS の 30,000 バイト未満への丸めは `max_bytes` を設定した場合の話で、上限を設定していない AGENTS は `split_dir` を持たない（`resolve_split_dir` のまま）。
- 起動前の一覧に無いファイルは worker 作と区別できないため、保守の実行中は分割先へファイルを置かない。
