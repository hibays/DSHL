//! Windows tray: a hidden message-only window + `Shell_NotifyIconW`.
//!
//! All Win32 calls go through the `windows` crate (windows-rs 0.62) — no
//! hand-written `#[link] extern "system"` FFI. The message loop runs on its
//! own thread; the icon and menu are driven by the tray window's WndProc,
//! which only flips atomics. The UI event loop polls those atomics (see
//! [`crate::tray`]).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, keybd_event,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFY_ICON_DATA_FLAGS,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu,
    DispatchMessageW, GetCursorPos, GetMessageW, HCURSOR, HICON, IMAGE_FLAGS, IMAGE_ICON,
    LR_LOADFROMFILE, LoadIconW, LoadImageW, MF_STRING, MSG, PostMessageW, PostQuitMessage,
    RegisterClassExW, SetForegroundWindow, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_COMMAND, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP,
    WNDCLASS_STYLES, WNDCLASSEXW,
};
use windows::core::PCWSTR;

/// Custom message: asked (from another thread) to end the message loop.
const WM_TRAY_QUIT: u32 = WM_APP + 2;
const WM_TRAY_CALLBACK: u32 = WM_APP + 1;
/// Max gap (ms) between two clicks that counts as a double click.
const DOUBLE_CLICK_MS: u64 = 400;
const MENU_RESTORE: usize = 1;
const MENU_QUIT: usize = 2;
const MENU_OPEN_URL: usize = 3;
const CW_USEDEFAULT: i32 = -1;
const HWND_MESSAGE: HWND = HWND(-3isize as *mut _);
/// Tray icon resource id (embedded by build.rs, same as the window icon).
const ICON_RESOURCE_ID: usize = 101;

/// HWND of the hidden tray message window (0 until created).
static TRAY_HWND: AtomicUsize = AtomicUsize::new(0);
/// Tray icon currently added to the notification area.
static ICON_ACTIVE: AtomicBool = AtomicBool::new(false);
/// User chose "quit" from the tray menu.
static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);
/// User chose "restore window" from the tray (click or menu).
static RESTORE_REQUESTED: AtomicBool = AtomicBool::new(false);
/// User chose "open dsh url" from the tray menu.
static OPEN_URL_REQUESTED: AtomicBool = AtomicBool::new(false);
/// Currently applied tray icon handle (so a replacement can be freed).
static CURRENT_ICON: AtomicUsize = AtomicUsize::new(0);
/// Timestamp (ms) of the last left click, for double-click detection.
static LAST_CLICK: AtomicU64 = AtomicU64::new(0);
static CLASS_REGISTERED: OnceLock<()> = OnceLock::new();

fn cursor_pos() -> (i32, i32) {
    // SAFETY: GetCursorPos with a stack-local POINT; best-effort.
    let mut pt = windows::Win32::Foundation::POINT::default();
    let ok = unsafe { GetCursorPos(&mut pt) };
    if ok.is_err() { (0, 0) } else { (pt.x, pt.y) }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: all calls below are best-effort Win32 window/tray calls on
    // valid handles owned by this module; failures are ignored.
    unsafe {
        if msg == WM_TRAY_QUIT {
            // Runs on the tray thread: PostQuitMessage ends that thread's loop.
            PostQuitMessage(0);
            return LRESULT(0);
        }
        // Menu items chosen in the right-click popup arrive here (the
        // TrackPopupMenu notification mode sends WM_COMMAND to the owner
        // window). LOWORD(wparam) is the item id.
        if msg == WM_COMMAND && wparam.0 & 0xffff == MENU_RESTORE {
            RESTORE_REQUESTED.store(true, Ordering::SeqCst);
            return LRESULT(0);
        }
        if msg == WM_COMMAND && wparam.0 & 0xffff == MENU_QUIT {
            QUIT_REQUESTED.store(true, Ordering::SeqCst);
            return LRESULT(0);
        }
        if msg == WM_COMMAND && wparam.0 & 0xffff == MENU_OPEN_URL {
            OPEN_URL_REQUESTED.store(true, Ordering::SeqCst);
            return LRESULT(0);
        }
        if msg == WM_TRAY_CALLBACK {
            // For Shell_NotifyIcon callback messages, wParam is the icon
            // ID and lParam is the mouse message (WM_LBUTTONUP /
            // WM_RBUTTONUP / …). Matching the wrong one makes every click
            // a no-op.
            match lparam.0 as u32 {
                WM_LBUTTONUP => {
                    // Single click: remember the time; a second click
                    // within DOUBLE_CLICK_MS is a double click and asks
                    // the launcher to re-create the window (the WebView
                    // was destroyed on close to save memory). Single
                    // clicks do nothing.
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    let prev = LAST_CLICK.swap(now, Ordering::SeqCst);
                    if prev != 0 && now.saturating_sub(prev) < DOUBLE_CLICK_MS {
                        RESTORE_REQUESTED.store(true, Ordering::SeqCst);
                    }
                }
                WM_RBUTTONUP => {
                    // Right click: popup menu (restore / quit). Standard
                    // notification mode (NO TPM_RETURNCMD): the menu is a
                    // regular modal popup that dismisses itself when the
                    // user clicks anywhere outside it; the chosen item is
                    // delivered as a WM_COMMAND to this window. With
                    // TPM_RETURNCMD the popup only returns after an
                    // explicit selection (or Esc), which feels stuck.
                    let menu = CreatePopupMenu();
                    if let Ok(menu) = menu {
                        let restore: Vec<u16> = "恢复窗口"
                            .encode_utf16()
                            .chain(std::iter::once(0))
                            .collect();
                        let open_dsh: Vec<u16> = "打开 dsh"
                            .encode_utf16()
                            .chain(std::iter::once(0))
                            .collect();
                        let quit: Vec<u16> =
                            "退出".encode_utf16().chain(std::iter::once(0)).collect();
                        let _ =
                            AppendMenuW(menu, MF_STRING, MENU_RESTORE, PCWSTR(restore.as_ptr()));
                        let _ =
                            AppendMenuW(menu, MF_STRING, MENU_OPEN_URL, PCWSTR(open_dsh.as_ptr()));
                        let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, PCWSTR(quit.as_ptr()));
                        let (x, y) = cursor_pos();
                        // TrackPopupMenu only dismisses on an outside click
                        // when its owner is the foreground window (MSDN:
                        // "the menu will not disappear when the user clicks
                        // outside of the menu"). The tray click belongs to
                        // Explorer, so the foreground lock would otherwise
                        // keep the popup open until an item (or Esc) is
                        // consumed. The classic fix is SetForegroundWindow
                        // before tracking — and when the lock still denies
                        // it (possible for our message-only window), a
                        // synthetic Alt tap grants foreground rights (the
                        // same trick as `platform::window::focus_window`).
                        if !SetForegroundWindow(hwnd).as_bool() {
                            keybd_event(0x12, 0, KEYEVENTF_EXTENDEDKEY, 0); // VK_MENU (Alt)
                            keybd_event(0x12, 0, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP, 0);
                            let _ = SetForegroundWindow(hwnd);
                        }
                        let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, x, y, None, hwnd, None);
                        // Re-sync the foreground state after the popup
                        // closes (standard tray dance; harmless for our
                        // message-only window).
                        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
                        let _ = DestroyMenu(menu);
                    }
                }
                _ => {}
            }
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

/// Create the hidden message window and its message loop (background).
/// Idempotent: only the first call spawns the loop thread; later calls
/// are no-ops (the tray icon stays the one created first).
pub fn start() {
    if TRAY_HWND.load(Ordering::SeqCst) != 0 {
        return;
    }
    CLASS_REGISTERED.get_or_init(|| {
        let class_name: Vec<u16> = "dshl_tray_wnd"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: window-class registration with a static WndProc.
        unsafe {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: WNDCLASS_STYLES(0),
                lpfnWndProc: Some(wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: exe_module(),
                hIcon: HICON::default(),
                hCursor: HCURSOR::default(),
                hbrBackground: HBRUSH::default(),
                lpszMenuName: PCWSTR::null(),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                hIconSm: HICON::default(),
            };
            RegisterClassExW(&wc);
        }
    });
    std::thread::spawn(message_loop);
}

/// The current executable's module handle (null on failure).
fn exe_module() -> windows::Win32::Foundation::HINSTANCE {
    // SAFETY: GetModuleHandleW(null) returns the exe's own module handle.
    unsafe { GetModuleHandleW(PCWSTR::null()) }
        .map(|m| windows::Win32::Foundation::HINSTANCE(m.0))
        .unwrap_or_default()
}

fn message_loop() {
    let class_name: Vec<u16> = "dshl_tray_wnd"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: window creation; message-only parent (HWND_MESSAGE).
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(class_name.as_ptr()),
            WINDOW_STYLE(0),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            Some(HWND_MESSAGE),
            None,
            Some(exe_module()),
            None,
        )
    };
    let Ok(hwnd) = hwnd else {
        crate::debug::emit("tray: message window creation failed");
        return;
    };
    TRAY_HWND.store(hwnd.0 as usize, Ordering::SeqCst);
    add_icon(hwnd);
    // Match the current OS theme right away (the window-theme watcher
    // may already have stopped when the window closed).
    set_icon(crate::platform::is_dark_mode());
    crate::debug::emit("tray: active (close-to-tray)");
    // SAFETY: standard message loop on the tray window.
    unsafe {
        let mut msg = std::mem::zeroed::<MSG>();
        // GetMessageW returns 0 on WM_QUIT, -1 on error, >0 otherwise.
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    remove_icon();
    crate::debug::emit("tray: message loop ended");
}

fn add_icon(hwnd: HWND) {
    if ICON_ACTIVE.swap(true, Ordering::SeqCst) {
        return;
    }
    // SAFETY: zeroed + fully initialised before use (only fields we set
    // are read by Shell_NotifyIconW for NIM_ADD).
    let mut data = unsafe { std::mem::zeroed::<NOTIFYICONDATAW>() };
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = 1;
    data.uFlags = NOTIFY_ICON_DATA_FLAGS(NIF_MESSAGE.0 | NIF_ICON.0 | NIF_TIP.0);
    data.uCallbackMessage = WM_TRAY_CALLBACK;
    // Icon resource 101 (the window icon embedded by build.rs).
    // SAFETY: constant MAKEINTRESOURCE-ish pointer + exe module handle.
    data.hIcon = unsafe {
        LoadIconW(Some(exe_module()), PCWSTR(ICON_RESOURCE_ID as *const u16)).unwrap_or_default()
    };
    let tip: Vec<u16> = "DSHL · DeepSeek Harness"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let tip = &tip[..tip.len().min(127)];
    data.szTip[..tip.len()].copy_from_slice(tip);
    // SAFETY: FFI call with a correctly sized, zeroed structure.
    let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &data) };
    if !ok.as_bool() {
        ICON_ACTIVE.store(false, Ordering::SeqCst);
        crate::debug::emit("tray: Shell_NotifyIcon(NIM_ADD) failed");
    }
}

fn remove_icon() {
    if !ICON_ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    let hwnd = TRAY_HWND.load(Ordering::SeqCst) as *mut _;
    // SAFETY: zeroed; only cbSize/hWnd/uID are read for NIM_DELETE.
    let mut data = unsafe { std::mem::zeroed::<NOTIFYICONDATAW>() };
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = HWND(hwnd);
    data.uID = 1;
    // SAFETY: FFI call with a correctly sized, zeroed structure.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

/// The close handler lets the WebView window close for real (destroying
/// the WebView2 processes and freeing memory); the tray icon is already
/// active, so there is nothing left to hide.
pub fn hide_to_tray() {
    crate::debug::emit("tray: window closed, keeping dsh running (close-to-tray)");
}

/// Swap the tray icon for the theme-appropriate variant (white "night"
/// icon in dark mode, black in light mode). The `.ico` bytes are written
/// to the cache dir and loaded with `LoadImageW`, then applied via
/// `NIM_MODIFY` — same pattern as the window icon in
/// [`crate::platform::apply_window_theme`].
pub fn set_icon(dark: bool) {
    let hwnd = TRAY_HWND.load(Ordering::SeqCst) as *mut std::ffi::c_void;
    if hwnd.is_null() || !ICON_ACTIVE.load(Ordering::SeqCst) {
        return;
    }
    let (bytes, name) = if dark {
        (
            include_bytes!("../../packing/windows/dsh-white.ico"),
            "dsh-white.ico",
        )
    } else {
        (include_bytes!("../../packing/windows/dsh.ico"), "dsh.ico")
    };
    let dir = crate::platform::cache_dir().join("dshl");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(name);
    if std::fs::write(&path, bytes).is_err() {
        return;
    }
    let wide: Vec<u16> = path
        .as_os_str()
        .to_str()
        .unwrap_or("")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // The notification area draws the icon at 16 logical pixels; on a
    // >100% DPI display that is 20/24 physical pixels. Loading with
    // cx=cy=0 picks the ICO's first (16x16) entry, which gets upscaled
    // and looks blurry — request the physical size so LoadImageW picks
    // the 32x32 entry (crisp downscale instead of blurry upscale).
    let scale = crate::platform::dpi_scale();
    let px = (16.0 * scale).round() as i32;
    // SAFETY: LoadImageW reads the .ico file we just wrote; the returned
    // handle is owned by the tray icon until replaced below.
    let icon = unsafe {
        LoadImageW(
            None,
            PCWSTR(wide.as_ptr()),
            IMAGE_ICON,
            px,
            px,
            IMAGE_FLAGS(LR_LOADFROMFILE.0),
        )
    };
    let Ok(icon) = icon else {
        return;
    };
    let hicon = HICON(icon.0);
    // Release the previously applied icon (the tray no longer uses it).
    let prev = CURRENT_ICON.swap(hicon.0 as usize, Ordering::SeqCst) as *mut std::ffi::c_void;
    if !prev.is_null() {
        // SAFETY: DestroyIcon on a handle LoadImageW returned.
        unsafe {
            let _ = DestroyIcon(HICON(prev));
        }
    }
    let mut data = unsafe { std::mem::zeroed::<NOTIFYICONDATAW>() };
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = HWND(hwnd);
    data.uID = 1;
    data.uFlags = NIF_ICON;
    data.hIcon = hicon;
    // SAFETY: FFI call with a correctly sized structure.
    let ok = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
    if ok.as_bool() {
        crate::debug::emit(&format!("tray: icon updated (dark={dark}, {px}px)"));
    }
}

/// True when the user chose "quit" from the tray menu.
pub fn quit_requested() -> bool {
    QUIT_REQUESTED.load(Ordering::SeqCst)
}

/// True when the user chose "restore window" from the tray (click or
/// menu). The launcher re-creates the WebView window (it was destroyed
/// on close to save memory).
pub fn restore_requested() -> bool {
    RESTORE_REQUESTED.swap(false, Ordering::SeqCst)
}

/// True when the user chose "打开 dsh" from the tray menu — the launcher
/// opens the dsh URL in the system default browser.
pub fn open_url_requested() -> bool {
    OPEN_URL_REQUESTED.swap(false, Ordering::SeqCst)
}

/// Remove the icon and stop the message loop (on shutdown).
pub fn shutdown() {
    remove_icon();
    // Ask the tray thread to end its message loop (WndProc posts WM_QUIT).
    let hwnd = TRAY_HWND.load(Ordering::SeqCst) as *mut std::ffi::c_void;
    if !hwnd.is_null() {
        // SAFETY: PostMessageW with a valid window handle is safe.
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd)), WM_TRAY_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}
