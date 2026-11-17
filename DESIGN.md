# Scriptura 設計メモ

この文書は設計判断の記録です。利用者向けの説明は `README.md`、開発規約（契約・検証コマンド・テストの対応）は `AGENTS.md` を参照してください。

## 目的

利用者が登録した自分のリポジトリへの push 後に、`AGENTS.md` / `DESIGN.md` / `README.md`（オプトインで `CHANGELOG.md` と利用者定義のカスタム種類）を AI（Claude Code の worker）で保守する単独アプリ。他人に配る製品として、既定は安全側（オプトイン・`propose`）に倒す。Codex・`~/.codex` には依存しない。

## 主要コンポーネントと責務

| 部品 | 責務 |
|---|---|
| `config` / `kinds` / `timing` | 利用者設定の読み書き・検証・判定（`resolve` は I/O を持たない純粋関数）、文書の種類の解決と許可パス、種類ごとのタイミング |
| `maintenance` | 保守の本体。push 検知、状態 DB、publications、worker の起動、commit と push |
| `proposal` | `propose` モード。使い捨てクローンで worker を動かし、更新案（パッチ）を保存・確認・適用・破棄する |
| `changelog` | CHANGELOG 固有の処理（リリース済みエントリ不変の検査、資料の収集、`promote`） |
| `ops` | 設定を変える操作と読み取り操作の本体。CLI と管理画面が共有する |
| `cli` / `ui` | 利用者の入口。`cli` は薄い引数層、`ui` は `127.0.0.1` 限定の SSR 管理画面（feature `ui`）。`ui --tray` でタスクトレイ（Windows）・メニューバー（macOS）に常駐できる |
| `connect` / `app_update` | Claude Code の Stop hook の登録・解除と、Setup（Velopack）版の導入・自動更新 |
| `locale` | 9 言語の辞書と言語判定 |

## データフロー

1. Claude Code の Stop hook が `scriptura hook` を呼ぶ。
2. 利用者の設定ファイルと同意を `config::resolve` で判定する。未登録・同意なし・壊れた設定は何もしない。
3. push の範囲を検知し、種類ごとの期限（`timing::due_kinds`）で処理する文書を絞る。期限前だけなら worker を起動せず、処理済み位置だけを進める。
4. 未信頼の資料（コミット件名など）を枠に入れたプロンプトで worker を起動する。権限は `restricted`（許可規則と事後検査）または `bypass`。
5. Rust が保守後検査（対象外変更・サイズ上限・必須見出し・CHANGELOG のリリース済み節の不変）を行う。違反は commit せず（`push`）、または更新案を `Rejected` にする（`propose`）。
6. `push` モードは commit と push、`propose` モードは更新案の保存まで行い、利用者が `review` / `apply` で適用する。

管理画面と CLI はいずれも `ops` を通して設定を書き換え、保守エンジンの内部は直接動かさない。

## 重要な不変条件

- 設定・テンプレートは利用者側のファイルだけから読む。リポジトリ内のファイルからは有効化しない（clone しただけで動き出さない）。
- 設定の書き込みは `config::update_in`（ロック・版照合・履歴の退避・アトミック保存）だけを通す。
- 検査は権限モードに関係なく Rust が行い、worker に任せない。commit・push の結果が不確実なときは再実行しない。
- 秘密値を保存・ログ・出力に書かない。worker へ渡す環境変数はホスト由来の中継設定を除く。
- 状態 DB の版（`STATE_SCHEMA_VERSION`）は 1 のまま。新しい状態は `CREATE TABLE IF NOT EXISTS` の追加で持つ（版を上げると旧版が何もしなくなる）。
- Velopack 管理外のビルド（開発・プラグイン同梱・単独 `install`）では更新・登録の処理で状態フォルダーへ触れない。

## 設計判断

- **`propose` を既定にする**: 作業ツリー・`.git`・remote を変えずに更新案だけを作れるため、他人に配る製品の既定として安全。`push` は `enable --mode push` を明示したときだけ。
- **hook の経路で tokio を使わない**: hook・CLI は同期の Rust だけで動かす。tokio は管理画面 `ui::run` の中だけで初期化し、feature `ui` を外したビルドでは axum・askama・tokio をリンクしない。
- **Setup 版は多プロセス型で、適用をログオン時だけにする**: Velopack の Windows の適用が導入先の他プロセスを強制終了するため、日中に適用しない。確認・取得は hook の後に同期で 1 回、適用前に署名者を検証する。
- **管理画面の常駐は OS の API を直接使い、既定を OFF にする**: トレイ用の crate を足さず（Windows は `Shell_NotifyIconW`、macOS は `NSStatusItem`）、依存と対応のずれを減らす。Linux はデスクトップごとにトレイの仕様が違うため対応しない。常駐は無操作で終了しない代わりに 60 分でセッションを失効させ、「開く」のたびに使い捨て鍵を発行し直すので、常駐が長くても開きっぱなしのタブは使えなくなる。ログオン時の常駐は Setup 版だけの任意機能で、ログオンエントリは増やさず `update-apply` の後に起動する（先に常駐すると更新の適用が `busy` で止まるため）。
- **worker の費用上限（`max_budget_usd`）は持たない**: worker の使用量はタイムアウトとモデル・effort の設定で調整する（既定は Sonnet の high、1200 秒）。廃止したキーは他の未知キーと同じく拒否し、設定全体を無効にして保守を止める。
- **プラグインのスキルは CLI を呼ぶ薄い層にする**: 判断・検証・書き込みを `scriptura <cmd> --json` に集め、スキルは同意の確認と要約だけにする。

## ローカライズ

### 目的と範囲

共通仕様（`D:\Dev\APP-DISTRIBUTION.md` の 8 節）に従い、9 言語（ja・en・zh-CN・zh-TW・ko・es・fr・de・pt-BR）で、人が読む文言を実行時に切り替える。

- 9 言語にする: CLI の出力・ヘルプ・エラー、Claude Code の Stop hook が画面へ出す通知（`systemMessage`）、Setup 版の案内（`welcome`・`doctor`・`update`）、管理画面（テンプレート・画面のエラー・確認・ジョブの結果・JS の単位表示）、README（`README.md` が英語の正本）。
- 英語に固定する: worker（Claude Code）へのプロンプト（`maintenance::build_maintenance_reason_with`、`kinds::settings_frame`、`changelog::prompt_parts`）、プラグインの SKILL.md。AI の応答言語は利用者の依頼に合わせる。
- 訳さない: 利用者のデータ（生成文書の本文、利用者が設定した `guidance`・`instructions`）、識別子・コマンド・パス・設定キー、JSON のキーと値、技術的なエラー詳細（OS・git の原文）。
- 生成する文書の言語設定 `language` は文書の言語で、表示言語とは別。worker のプロンプトを英語にしたため、`language` が無いときの挙動（既存文書の言語に従い、新規作成は日本語）はプロンプトに明記して従来どおりに保つ。

### 言語の決め方

- CLI・hook・Setup 版の案内: `SCRIPTURA_LANGUAGE` → `LC_ALL` → `LC_MESSAGES` → `LANG`（空・`C`・`POSIX` は飛ばす。未対応の言語はその場で英語）→ OS の表示言語（Windows は `GetUserDefaultUILanguage`、macOS は `AppleLanguages`）→ 英語。判定は 1 プロセスで 1 回（`locale::detect_with` が純粋関数）。
- 管理画面: 明示の選択（Cookie `scriptura-language-<port>`）→ `Accept-Language`（q 値つきの最良一致。未対応の言語は飛ばす）→ 英語。選択欄は `POST /language` で Cookie を書く（GET は副作用なし）。Cookie 名にポートを含め、別の起動と混ざらない。戻り先は同一オリジンの Referer のパスだけ。
- タグの照合: `zh-Hans`・`zh-CN`・`zh-SG` は zh-CN、`zh-Hant`・`zh-TW`・`zh-HK`・`zh-MO` は zh-TW、`pt` 系は pt-BR、それ以外は主言語。

### 辞書

- 言語ごとの JSON（`src/locales/<タグ>.json`）。キーは `cli.*`・`doctor.*`・`ui.*` のような安定した識別子、プレースホルダーは `{name}`。辞書に無いキーは英語、英語にも無ければキーそのもの。
- 管理画面の言語は task-local（`locale::scope_request`）で要求ごとに持ち、`spawn_blocking` へ渡す処理は `locale::scoped` で言語を明示する（ops・エンジン由来の文言も画面と同じ言語になる）。run-now のジョブは、起動した画面の言語で結果の文言を作る。
- 整合テスト（`tests/localization.rs`）: 全言語のキー集合とプレースホルダーの一致、コード・テンプレートが参照するキーが辞書にあること、辞書に使われないキーが無いこと、タグ照合・判定順・Cookie と Accept-Language。

### 状態との互換

文言を分岐の条件に使わない。worker のタイムアウトは `WorkerFailure.timed_out`、commit・push の結果未確定は publication 状態の `outcome_unknown_error`（旧版が残した日本語の文言との互換のため、読む側は旧文言も受け付ける）。状態 DB の版（`STATE_SCHEMA_VERSION`）は変えない。
