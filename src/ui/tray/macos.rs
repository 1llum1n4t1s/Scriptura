// macOS のメニューバー（NSStatusItem）。AppKit はメインスレッドでしか動かせないので、`ui::run_resident` は
// 呼び出したスレッド（メインスレッド）でこの `run` を回し、サーバーはランタイムのワーカーで動かす。
//
// - クリック: 管理画面を開く。右クリック（または Control + クリック）: メニュー（開く・更新を確認・終了）。
// - Dock にアイコンを出さない（Accessory）。
// - サーバーが止まったか（Ctrl+C・画面の「終了」・異常終了）を約 100ms ごとに見て、ループを抜ける。
// - アイコンは同梱の画像を持たず、システムの記号（SF Symbols の `doc.text`）を使う（暫定の図柄）。
use std::cell::RefCell;

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSEventMask, NSEventModifierFlags,
    NSEventTrackingRunLoopMode, NSEventType, NSImage, NSMenu, NSMenuItem, NSSquareStatusItemLength,
    NSStatusBar, NSStatusBarButton,
};
use objc2_foundation::{
    MainThreadMarker, NSDate, NSDefaultRunLoopMode, NSObject, NSRunLoop, NSString, NSTimer,
    ns_string,
};

use super::Control;

/// メニューとクリックの受け手が持つ状態。`control` は `run` の呼び出しの間だけ有効な参照を、
/// 受け手より長く生きる形（`run` の中で受け手を先に解放する）で渡す。
struct TargetState {
    control: *const Control,
    tracking_menu: RefCell<Option<Retained<NSMenu>>>,
}

define_class!(
    // 安全性: NSObject に追加のサブクラス要件はなく、AppKit と同じメインスレッドだけで扱う。
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = TargetState]
    struct TrayTarget;

    impl TrayTarget {
        // 安全性: target/action の sender を受け取る Objective-C のシグネチャに一致する。
        #[unsafe(method(openManager:))]
        fn open_manager(&self, _sender: &AnyObject) {
            self.control().open();
        }

        #[unsafe(method(checkUpdates:))]
        fn check_updates(&self, _sender: &AnyObject) {
            self.control().check_updates();
        }

        #[unsafe(method(quitResident:))]
        fn quit_resident(&self, _sender: &AnyObject) {
            self.control().quit();
        }

        #[unsafe(method(checkStopped:))]
        fn check_stopped(&self, _sender: &AnyObject) {
            if self.control().is_stopped()
                && let Some(menu) = self.ivars().tracking_menu.borrow().as_ref()
            {
                menu.cancelTracking();
            }
        }

        #[unsafe(method(handleClick:))]
        fn handle_click(&self, sender: &NSStatusBarButton) {
            let app = NSApplication::sharedApplication(self.mtm());
            if let Some(event) = app.currentEvent()
                && (event.r#type() == NSEventType::RightMouseUp
                    || event.modifierFlags().contains(NSEventModifierFlags::Control))
            {
                let menu = self.menu();
                self.ivars().tracking_menu.replace(Some(menu.clone()));
                // メニューの表示中も停止を確かめ、Ctrl+C やサーバーの異常終了でメニューを閉じる。
                // selector を実装した target と timer は、このメインスレッドで処理が終わるまで保持する。
                let timer = unsafe {
                    let timer = NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                        0.1,
                        self,
                        sel!(checkStopped:),
                        None,
                        true,
                    );
                    NSRunLoop::currentRunLoop()
                        .addTimer_forMode(&timer, NSEventTrackingRunLoopMode);
                    timer
                };
                NSMenu::popUpContextMenu_withEvent_forView(&menu, &event, sender);
                timer.invalidate();
                self.ivars().tracking_menu.take();
            } else {
                self.control().open();
            }
        }
    }
);

impl TrayTarget {
    fn control(&self) -> &Control {
        // 安全性: `run` は、この受け手を解放してから戻る。その間 `control` の参照先は生きている。
        unsafe { &*self.ivars().control }
    }

    fn new(mtm: MainThreadMarker, control: &Control) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetState {
            control,
            tracking_menu: RefCell::new(None),
        });
        // 安全性: NSObject の init を、初期化済みの ivars を持つ割り当てに対して一度だけ呼ぶ。
        unsafe { msg_send![super(this), init] }
    }

    fn menu(&self) -> Retained<NSMenu> {
        let menu = NSMenu::initWithTitle(NSMenu::alloc(self.mtm()), ns_string!("Scriptura"));
        menu.setAutoenablesItems(false);
        // 「更新を確認」は Setup 版だけ（それ以外では確認する更新元が無いので、項目を出さない）。
        let mut items = vec![(crate::t!("ui.tray.open"), sel!(openManager:))];
        if self.control().can_check_updates() {
            items.push((crate::t!("ui.tray.check_updates"), sel!(checkUpdates:)));
        }
        items.push((crate::t!("ui.tray.quit"), sel!(quitResident:)));
        for (title, action) in items {
            if action == sel!(quitResident:) {
                menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
            }
            // 各 selector はこの target が実装し、同期のメニュー処理が終わるまで target は保持される。
            let item = unsafe {
                let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(self.mtm()),
                    &NSString::from_str(title),
                    Some(action),
                    ns_string!(""),
                );
                item.setTarget(Some(self));
                item
            };
            menu.addItem(&item);
        }
        menu
    }
}

/// 既に常駐している Scriptura を探して「開く」を頼む仕組みは、macOS には無い（常に false）。
/// 2 つ目の起動は、ポートが使用中という通常のエラーで終わる。
pub fn notify_existing(_open: bool) -> bool {
    false
}

/// メニューバーに項目を出し、サーバーが止まるまでイベントを処理する。メインスレッドで呼ぶ。
/// 失敗は短い英語の理由で返す（呼び出し元が利用者向けの文に入れる）。
pub fn run(control: &Control) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("the status item needs the main thread")?;
    autoreleasepool(|_| {
        let app = NSApplication::sharedApplication(mtm);
        if !app.setActivationPolicy(NSApplicationActivationPolicy::Accessory) {
            return Err("the activation policy could not be set".to_string());
        }
        app.finishLaunching();

        let target = TrayTarget::new(mtm, control);
        let status_bar = NSStatusBar::systemStatusBar();
        let status_item = status_bar.statusItemWithLength(NSSquareStatusItemLength);
        let Some(button) = status_item.button(mtm) else {
            status_bar.removeStatusItem(&status_item);
            return Err("the status item has no button".to_string());
        };
        // システムの記号はテンプレート画像なので、メニューバーの明暗に合わせて描かれる。
        match NSImage::imageWithSystemSymbolName_accessibilityDescription(
            ns_string!("doc.text"),
            Some(ns_string!("Scriptura")),
        ) {
            Some(image) => button.setImage(Some(&image)),
            // 記号が無い古い macOS では、文字で代える。
            None => button.setTitle(ns_string!("S")),
        }
        button.setToolTip(Some(&NSString::from_str(crate::t!("ui.tray.tooltip"))));
        button.sendActionOn(NSEventMask::LeftMouseUp | NSEventMask::RightMouseUp);
        // 安全性: target は handleClick: を実装し、下で解除するまで保持する。
        unsafe {
            button.setTarget(Some(&target));
            button.setAction(Some(sel!(handleClick:)));
        }

        while !control.is_stopped() {
            // 各待機の一時オブジェクトを解放し、停止を最大約 100ms ごとに確かめる。
            autoreleasepool(|_| {
                let deadline = NSDate::dateWithTimeIntervalSinceNow(0.1);
                // 安全性: Foundation が提供する不変の既定の run-loop mode を参照する。
                let mode = unsafe { NSDefaultRunLoopMode };
                if let Some(event) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
                    NSEventMask::Any,
                    Some(&deadline),
                    mode,
                    true,
                ) {
                    app.sendEvent(&event);
                }
            });
        }

        // 安全性: 有効な action を解除してから、保持している target を解放する。
        unsafe {
            button.setAction(None);
            button.setTarget(None);
        }
        status_bar.removeStatusItem(&status_item);
        drop(target);
        Ok(())
    })
}
