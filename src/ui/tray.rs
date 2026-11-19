// 管理画面の常駐（`scriptura ui --tray`）。タスクトレイ（Windows）・メニューバー（macOS）にアイコンを置く。
//
// 方式: トレイ用の crate は足さず、OS の API を直接使う（Windows は `Shell_NotifyIconW`、macOS は AppKit の
// `NSStatusItem`）。Linux は対応しない（デスクトップごとにトレイの仕様が違うため。`--tray` は拒否する）。
//
// このモジュールが扱うのはアイコンとメニューだけ。サーバーの起動・終了・セッションは `ui` の側にあり、
// ここからは [`Control`] を通してだけ触る（開く＝使い捨て鍵を発行し直してブラウザーを開く、終了＝通知）。
// メニューの文言は表示言語の辞書（`ui.tray.*`）から取る。
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use super::AppState;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::{notify_existing, run};
#[cfg(windows)]
pub use windows::{notify_existing, owns_console_alone, run};

/// トレイから管理画面を操作する窓口。
pub struct Control {
    state: AppState,
    port: u16,
    /// サーバーが止まった印（`ui::run_resident` が、サーバーのタスクの終了で立てる）。
    stopped: Arc<AtomicBool>,
}

impl Control {
    pub fn new(state: AppState, port: u16, stopped: Arc<AtomicBool>) -> Self {
        Self {
            state,
            port,
            stopped,
        }
    }

    /// 管理画面をブラウザーで開く。使い捨て鍵を発行し直すので、前に発行して使われなかった鍵は無効になる。
    pub fn open(&self) {
        let key = self.state.issue_key();
        super::open_browser(&super::auth_url(self.port, &key));
    }

    /// 常駐を終える（サーバーへ終了を通知する。実行中の run-now は完了を待つ）。
    pub fn quit(&self) {
        self.state.shutdown.notify_one();
    }

    /// サーバーが止まったか（Ctrl+C・画面の「終了」・異常終了を、トレイのループが定期的に見る）。
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// 「更新を確認」を出すか（Setup 版だけ。それ以外では確認する更新元が無いので、項目を出さない）。
    pub fn can_check_updates(&self) -> bool {
        crate::app_update::is_managed()
    }

    /// 管理画面の概要画面の更新パネルを開く。使い捨て鍵を発行し直し、画面が自動で更新の確認を始める
    /// （`?check_updates=1#updates`）。
    pub fn check_updates(&self) {
        let key = self.state.issue_key();
        super::open_browser(&super::auth_url_updates(self.port, &key));
    }
}
