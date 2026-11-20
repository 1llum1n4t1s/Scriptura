pub mod app;
// Setup（Velopack）導入版の更新・登録（ログオン時の適用・settings.json への登録・自動更新）。
pub mod app_update;
pub mod changelog;
pub mod cli;
pub mod config;
pub mod connect;
pub mod doctor;
pub mod kinds;
// 人が読む文言の言語判定と辞書（9 言語）。
pub mod locale;
pub mod maintenance;
pub mod ops;
pub mod paths;
pub mod process;
// 保守履歴（状態 DB の `maintenance_runs`）。
pub mod runlog;
pub mod timing;
// 管理画面（`scriptura ui`）。feature "ui"（既定で有効）のときだけ含める。
#[cfg(feature = "ui")]
pub mod ui;
