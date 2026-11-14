pub mod app;
pub mod changelog;
pub mod cli;
pub mod config;
pub mod connect;
pub mod doctor;
pub mod kinds;
pub mod maintenance;
pub mod ops;
pub mod paths;
pub mod process;
pub mod proposal;
pub mod timing;
// 管理画面（`scriptura ui`）。feature "ui"（既定で有効）のときだけ含める。
#[cfg(feature = "ui")]
pub mod ui;
