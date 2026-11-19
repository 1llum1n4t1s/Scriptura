// Windows のタスクトレイ。非表示のウィンドウを 1 つ作り、通知アイコンのメッセージを同じスレッドで処理する。
//
// - 左クリック: 管理画面を開く。右クリック: メニュー（開く・更新を確認・終了）。
// - Explorer の再起動（`TaskbarCreated`）でアイコンを登録し直す。
// - サーバーが止まったか（Ctrl+C・画面の「終了」・異常終了）を 200ms ごとのタイマーで見て、ループを抜ける。
// - 2 つ目の常駐は起動しない。既存の常駐のウィンドウをクラス名で探し、「開く」をメッセージで頼む。
// - アイコンは同梱の画像を持たず、小アイコンの寸法に合わせて実行時に描く（暫定の図柄）。
use std::{
    cell::Cell,
    io,
    mem::size_of,
    ptr::{null, null_mut},
    time::Instant,
};

use windows_sys::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM},
    System::{Console::GetConsoleProcessList, LibraryLoader::GetModuleHandleW},
    UI::{
        Input::KeyboardAndMouse::GetDoubleClickTime,
        Shell::{
            NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
            Shell_NotifyIconW,
        },
        WindowsAndMessaging::*,
    },
};

use super::Control;

/// 非表示ウィンドウのクラス名。2 つ目の起動が既存の常駐を探すのにも使う。
const CLASS_NAME: &str = "ScripturaTrayWindow";
/// 通知アイコンからのマウス操作。
const ICON_MESSAGE: u32 = WM_APP + 1;
/// 2 つ目の起動からの「管理画面を開く」。
const OPEN_MESSAGE: u32 = WM_APP + 2;
const STOP_TIMER: usize = 1;
const MENU_OPEN: usize = 1;
const MENU_CHECK_UPDATES: usize = 2;
const MENU_QUIT: usize = 3;

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn last_error() -> String {
    io::Error::last_os_error().to_string()
}

/// このプロセスだけがコンソールを持っているか（Explorer・スタートメニュー・更新器から起動され、専用の
/// コンソール窓が開いた状態）。端末から起動されたときは、シェルも同じコンソールにいるので false。
pub fn owns_console_alone() -> bool {
    let mut pids = [0u32; 2];
    // 有効な長さ 2 の配列を渡す。戻り値はコンソールに付いているプロセスの数（コンソールが無ければ 0）。
    unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) == 1 }
}

/// 既に常駐している Scriptura があれば true。`open` なら、その常駐に管理画面を開くよう頼む。
pub fn notify_existing(open: bool) -> bool {
    let class = wide(CLASS_NAME);
    // クラス名は null 終端。見つからなければ null。
    let window = unsafe { FindWindowW(class.as_ptr(), null()) };
    if window.is_null() {
        return false;
    }
    if open {
        // 相手は別プロセスのウィンドウ。引数を持たない通知だけを送る。
        unsafe { PostMessageW(window, OPEN_MESSAGE, 0, 0) };
    }
    true
}

// ───────────────────────── アイコン（実行時に描く） ─────────────────────────

/// 図柄の 1 点（0.0〜1.0 の座標）が、角丸の地・白い線のどちらに入るか。
fn sample(x: f32, y: f32) -> (bool, bool) {
    // 角丸の正方形（地）。
    let radius = 0.22;
    let dx = (x - 0.5).abs() - (0.5 - radius);
    let dy = (y - 0.5).abs() - (0.5 - radius);
    let inside = if dx > 0.0 && dy > 0.0 {
        dx * dx + dy * dy <= radius * radius
    } else {
        true
    };
    // 文書を表す 3 本の横線（最後の 1 本は短い）。
    let line = [(0.25, 0.76), (0.45, 0.76), (0.65, 0.56)]
        .iter()
        .any(|(top, right)| (*top..top + 0.11).contains(&y) && (0.24..*right).contains(&x));
    (inside, inside && line)
}

/// `size` ピクセル四方の図柄（上から下、BGRA、乗算していないアルファ）。4×4 の点で面積を取って縁をなめらかにする。
pub(crate) fn icon_bgra(size: usize) -> Vec<u8> {
    const GRID: usize = 4;
    // 地の色（藍）と線の色（白）。
    const GROUND: [f32; 3] = [229.0, 70.0, 79.0];
    const LINE: [f32; 3] = [255.0, 255.0, 255.0];
    let mut out = Vec::with_capacity(size * size * 4);
    for py in 0..size {
        for px in 0..size {
            let (mut ground, mut line) = (0u32, 0u32);
            for sy in 0..GRID {
                for sx in 0..GRID {
                    let x = (px as f32 + (sx as f32 + 0.5) / GRID as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / GRID as f32) / size as f32;
                    let (inside, on_line) = sample(x, y);
                    ground += inside as u32;
                    line += on_line as u32;
                }
            }
            if ground == 0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            let mix = line as f32 / ground as f32;
            for channel in 0..3 {
                out.push((GROUND[channel] * (1.0 - mix) + LINE[channel] * mix).round() as u8);
            }
            out.push((ground * 255 / (GRID * GRID) as u32) as u8);
        }
    }
    out
}

/// アイコンのリソース形式（.ico の中の 1 画像と同じ。BITMAPINFOHEADER + 下から上の BGRA + 1bpp のマスク）。
pub(crate) fn icon_resource(size: usize) -> Vec<u8> {
    let pixels = icon_bgra(size);
    let mask_stride = size.div_ceil(32) * 4;
    let mut out = Vec::with_capacity(40 + pixels.len() + mask_stride * size);
    out.extend_from_slice(&40u32.to_le_bytes()); // biSize
    out.extend_from_slice(&(size as i32).to_le_bytes()); // biWidth
    out.extend_from_slice(&(size as i32 * 2).to_le_bytes()); // biHeight（色とマスクの 2 枚分）
    out.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    out.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
    out.extend_from_slice(&[0u8; 24]); // 圧縮なし。残りの項目は 0
    for row in pixels.chunks_exact(size * 4).rev() {
        out.extend_from_slice(row);
    }
    // マスクは全て 0（透過はアルファで表す）。
    out.resize(out.len() + mask_stride * size, 0);
    out
}

struct Icon(HICON);

impl Drop for Icon {
    fn drop(&mut self) {
        // CreateIconFromResourceEx で所有したアイコンだけを解放する。
        unsafe { DestroyIcon(self.0) };
    }
}

fn load_icon() -> Result<Icon, String> {
    // システムメトリックの取得はポインターを参照しない。DPI に合った小アイコンの寸法で描く。
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64);
    let resource = icon_resource(size as usize);
    // 作った範囲をそのまま渡す。LR_SHARED を使わず所有権を取る。
    let icon = unsafe {
        CreateIconFromResourceEx(
            resource.as_ptr(),
            resource.len() as u32,
            1,
            0x0003_0000,
            size,
            size,
            0,
        )
    };
    if icon.is_null() {
        return Err(last_error());
    }
    Ok(Icon(icon))
}

// ───────────────────────── ウィンドウとメニュー ─────────────────────────

struct WindowClass {
    name: Vec<u16>,
    instance: HINSTANCE,
}

impl Drop for WindowClass {
    fn drop(&mut self) {
        // このクラスのウィンドウは先に破棄され、クラス名もまだ有効。
        unsafe { UnregisterClassW(self.name.as_ptr(), self.instance) };
    }
}

struct Menu(HMENU);

impl Drop for Menu {
    fn drop(&mut self) {
        unsafe { DestroyMenu(self.0) };
    }
}

struct TrayState<'a> {
    icon: Icon,
    control: &'a Control,
    taskbar_created: u32,
    last_click: Cell<Option<Instant>>,
    /// アイコンを登録できたか。登録できないあいだは、停止の確認と同じタイマーで再試行する。
    added: Cell<bool>,
}

impl TrayState<'_> {
    fn notification(&self, window: HWND) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: window,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: ICON_MESSAGE,
            hIcon: self.icon.0,
            ..Default::default()
        };
        // 末尾の null を残す（szTip は 128 文字）。
        for (target, value) in data
            .szTip
            .iter_mut()
            .take(127)
            .zip(crate::t!("ui.tray.tooltip").encode_utf16())
        {
            *target = value;
        }
        data
    }

    /// アイコンを登録する。ログオン直後はタスクバーの準備前で失敗することがあるため、失敗しても常駐は
    /// 続け、タイマー（200ms ごと）と `TaskbarCreated` で登録し直す。
    fn add_icon(&self, window: HWND) {
        // 通知の構造体とアイコンは、同期の呼び出しの間ずっと有効。
        let data = self.notification(window);
        let mut added = unsafe { Shell_NotifyIconW(NIM_ADD, &data) } != 0;
        if !added {
            // 既にアイコンがあると NIM_ADD は失敗する（`TaskbarCreated` の再送など）。その場合は
            // NIM_MODIFY で今の定義に合わせ、成功すれば登録済みとして扱う。両方失敗したときだけ再試行を続ける。
            added = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) } != 0;
        }
        self.added.set(added);
    }

    fn show_menu(&self, window: HWND) {
        // メニューはこのスレッドが所有し、選択・取消のどちらでも解放する。
        let handle = unsafe { CreatePopupMenu() };
        if handle.is_null() {
            return;
        }
        let menu = Menu(handle);
        let mut items = vec![(MENU_OPEN, crate::t!("ui.tray.open"), MF_STRING)];
        // 「更新を確認」は Setup 版だけ（それ以外では確認する更新元が無いので、項目を出さない）。
        if self.control.can_check_updates() {
            items.push((
                MENU_CHECK_UPDATES,
                crate::t!("ui.tray.check_updates"),
                MF_STRING,
            ));
        }
        items.push((0, "", MF_SEPARATOR));
        items.push((MENU_QUIT, crate::t!("ui.tray.quit"), MF_STRING));
        for (id, label, flags) in items {
            if unsafe { AppendMenuW(menu.0, flags, id, wide(label).as_ptr()) } == 0 {
                return;
            }
        }
        let mut position = POINT::default();
        if unsafe { GetCursorPos(&mut position) } == 0 {
            return;
        }
        // 通知アイコンのメニューは、所有ウィンドウを前面にしてから出す（外側のクリックで閉じられるように）。
        let selected = unsafe {
            SetForegroundWindow(window);
            let selected = TrackPopupMenu(
                menu.0,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                position.x,
                position.y,
                0,
                window,
                null(),
            );
            PostMessageW(window, WM_NULL, 0, 0);
            selected as usize
        };
        match selected {
            MENU_OPEN => self.control.open(),
            MENU_CHECK_UPDATES => self.control.check_updates(),
            MENU_QUIT => self.control.quit(),
            _ => {}
        }
    }
}

struct TrayWindow<'a> {
    handle: HWND,
    state: &'a TrayState<'a>,
}

impl Drop for TrayWindow<'_> {
    fn drop(&mut self) {
        // 状態・アイコン・ウィンドウクラスより先に、通知とウィンドウを破棄する。
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &self.state.notification(self.handle));
            KillTimer(self.handle, STOP_TIMER);
            DestroyWindow(self.handle);
        }
    }
}

/// トレイのアイコンを出し、サーバーが止まるまでメッセージを処理する。呼び出したスレッドを占有する。
/// 失敗は OS のエラーの文で返す。
pub fn run(control: &Control) -> Result<(), String> {
    if control.is_stopped() {
        return Ok(());
    }
    // null 指定は現在の実行ファイルを表し、ハンドルの解放は不要。
    let instance = unsafe { GetModuleHandleW(null()) };
    if instance.is_null() {
        return Err(last_error());
    }
    let name = wide(CLASS_NAME);
    let definition = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: name.as_ptr(),
        ..Default::default()
    };
    // クラス名とコールバックは登録中ずっと有効。
    if unsafe { RegisterClassW(&definition) } == 0 {
        return Err(last_error());
    }
    let class = WindowClass { name, instance };
    let taskbar_message = wide("TaskbarCreated");
    // Explorer が再起動のときに全ウィンドウへ送る登録メッセージ。
    let taskbar_created = unsafe { RegisterWindowMessageW(taskbar_message.as_ptr()) };
    if taskbar_created == 0 {
        return Err(last_error());
    }
    let state = TrayState {
        icon: load_icon()?,
        control,
        taskbar_created,
        last_click: Cell::new(None),
        added: Cell::new(false),
    };
    // state はこの関数の終わりまで動かない。非表示のトップレベルウィンドウにして、TaskbarCreated の
    // ブロードキャストと、2 つ目の起動からの FindWindowW を受けられるようにする。
    let handle = unsafe {
        CreateWindowExW(
            0,
            class.name.as_ptr(),
            class.name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            (&state as *const TrayState).cast(),
        )
    };
    if handle.is_null() {
        return Err(last_error());
    }
    let window = TrayWindow {
        handle,
        state: &state,
    };
    state.add_icon(window.handle);
    // コールバックなしのタイマーは、同じスレッドのメッセージループで処理される。
    if unsafe { SetTimer(window.handle, STOP_TIMER, 200, None) } == 0 {
        return Err(last_error());
    }
    let mut message = MSG::default();
    loop {
        // MSG は有効な書き込み先。このスレッドのウィンドウのメッセージを配送する。
        let result = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
        if result == -1 {
            return Err(last_error());
        }
        if result == 0 {
            return Ok(());
        }
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // WM_NCCREATE の CREATESTRUCTW は OS が渡す。保存する参照先は run が DestroyWindow の完了まで所有し、
    // このスレッド以外から参照しない。
    unsafe {
        if message == WM_NCCREATE {
            let creation = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(window, GWLP_USERDATA, creation.lpCreateParams as isize);
        }
        let pointer = GetWindowLongPtrW(window, GWLP_USERDATA) as *const TrayState;
        if !pointer.is_null() {
            let state = &*pointer;
            if message == state.taskbar_created {
                state.add_icon(window);
                return 0;
            }
            match message {
                ICON_MESSAGE if lparam as u32 == WM_LBUTTONUP => {
                    let now = Instant::now();
                    let interval = u128::from(GetDoubleClickTime());
                    // 左ボタンを離した通知だけで開き、ダブルクリックの後半もまとめる。
                    if state
                        .last_click
                        .get()
                        .is_none_or(|last| now.duration_since(last).as_millis() > interval)
                    {
                        state.last_click.set(Some(now));
                        state.control.open();
                    }
                    return 0;
                }
                ICON_MESSAGE if matches!(lparam as u32, WM_RBUTTONUP | WM_CONTEXTMENU) => {
                    state.show_menu(window);
                    return 0;
                }
                OPEN_MESSAGE => {
                    state.control.open();
                    return 0;
                }
                WM_TIMER if wparam == STOP_TIMER => {
                    if state.control.is_stopped() {
                        EndMenu();
                        PostQuitMessage(0);
                    } else if !state.added.get() {
                        state.add_icon(window);
                    }
                    return 0;
                }
                WM_CLOSE => {
                    state.control.quit();
                    return 0;
                }
                WM_NCDESTROY => {
                    SetWindowLongPtrW(window, GWLP_USERDATA, 0);
                }
                _ => {}
            }
        }
        DefWindowProcW(window, message, wparam, lparam)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_has_opaque_ground_white_lines_and_transparent_corners() {
        for size in [16usize, 20, 24, 32, 48] {
            let pixels = icon_bgra(size);
            assert_eq!(pixels.len(), size * size * 4);
            let at = |x: usize, y: usize| &pixels[(y * size + x) * 4..(y * size + x) * 4 + 4];
            // 角は透明、縁の中央は不透明な地の色。
            assert_eq!(at(0, 0)[3], 0, "size {size}");
            assert_eq!(at(size / 2, 1), [229, 70, 79, 255], "size {size}");
            // 1 本目の線の中ほどは白。
            assert_eq!(
                at(size / 2, size * 30 / 100),
                [255, 255, 255, 255],
                "size {size}"
            );
        }
    }

    #[test]
    fn icon_resource_has_header_pixels_and_mask() {
        let resource = icon_resource(20);
        // ヘッダー 40 + 色 20×20×4 + マスク（1 行は 32 ビット境界 = 4 バイト）。
        assert_eq!(resource.len(), 40 + 20 * 20 * 4 + 4 * 20);
        assert_eq!(&resource[..4], &40u32.to_le_bytes());
        assert_eq!(&resource[8..12], &40i32.to_le_bytes());
    }
}
