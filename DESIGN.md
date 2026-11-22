# Scriptura 設計メモ

この文書は設計判断の記録です。利用者向けの説明は `README.md`、開発規約（契約・検証コマンド・テストの対応）は `AGENTS.md` を参照してください。

## 目的

利用者が登録した自分のリポジトリへの push 後に、`AGENTS.md` / `DESIGN.md` / `README.md`（オプトインで `CHANGELOG.md` と利用者定義のカスタム種類）を AI（Claude Code の worker）で保守する単独アプリです。他人に配る製品として、既定は安全側（オプトイン）に倒します。Codex・`~/.codex` には依存しません。

## 主要コンポーネントと責務

設定・種類・タイミング、保守の本体、保守履歴、CHANGELOG、`ops`、`cli` / `ui`、`connect` / `app_update`、`locale` の責務を表にまとめています。詳細は [docs/design/section-01.md](docs/design/section-01.md) を参照してください。

## データフロー

hook の起動から、期限判定、worker、保守後検査、commit と push までの流れです。詳細は [docs/design/section-02.md](docs/design/section-02.md) を参照してください。

## 重要な不変条件

設定の読み込み元、設定の書き込み経路、検査の主体、秘密値、状態 DB の版、Velopack 管理外での無作用を定めています。詳細は [docs/design/section-03.md](docs/design/section-03.md) を参照してください。

## 設計判断

同意と propose の廃止、旧設定の変換、hook 経路で tokio を使わないこと、Setup 版の適用タイミング、上限違反の扱い、保守履歴、管理画面の常駐などを、理由とトレードオフとともに記録しています。詳細は [docs/design/section-04.md](docs/design/section-04.md) を参照してください。

## ローカライズ

- 目的・言語の決め方: [docs/design/section-05.md](docs/design/section-05.md)
- 辞書・状態との互換: [docs/design/section-06.md](docs/design/section-06.md)
