# ローカライズ（辞書・状態との互換）
### 辞書

- 言語ごとの JSON（`src/locales/<タグ>.json`）。キーは `cli.*`・`doctor.*`・`ui.*` のような安定した識別子、プレースホルダーは `{name}`。辞書に無いキーは英語、英語にも無ければキーそのもの。
- 管理画面の言語は task-local（`locale::scope_request`）で要求ごとに持ち、`spawn_blocking` へ渡す処理は `locale::scoped` で言語を明示する（ops・エンジン由来の文言も画面と同じ言語になる）。run-now のジョブは、起動した画面の言語で結果の文言を作る。
- 整合テスト（`tests/localization.rs`）: 全言語のキー集合とプレースホルダーの一致、コード・テンプレートが参照するキーが辞書にあること、辞書に使われないキーが無いこと、タグ照合・判定順・Cookie と Accept-Language。

### 状態との互換

文言を分岐の条件に使わない。worker のタイムアウトは `WorkerFailure.timed_out`、commit・push の結果未確定は publication 状態の `outcome_unknown_error`（旧版が残した日本語の文言との互換のため、読む側は旧文言も受け付ける）。状態 DB の版（`STATE_SCHEMA_VERSION`）は変えない。
