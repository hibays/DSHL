//! Win32 window helpers: geometry capture/clamp, HWND discovery, focus,
//! liveness and theme application. All FFI goes through `windows-rs`.

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn contains_slice(hay: &[u16], needle: &[u16]) -> bool {
    needle.is_empty() || hay.windows(needle.len()).any(|w| w == needle)
}

/// A captured window rectangle, in physical pixels.
#[derive(Debug, Clone, Copy)]
pub struct WindowRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// True when the window is maximized or covers the whole screen (such a
    /// state should not be persisted).
    pub maximized: bool,
}

/// Capture a window's position/size and whether it is maximized.
///
/// Only implemented on Windows (via the HWND); returns `None` elsewhere.
pub fn window_rect(hwnd: usize) -> Option<WindowRect> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::{HWND, RECT};
        use windows::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, GetWindowRect, IsZoomed, SM_CXSCREEN, SM_CYSCREEN,
        };

        // SAFETY: hwnd is a valid window handle; the struct is stack-local.
        unsafe {
            let mut rect = RECT::default();
            if GetWindowRect(HWND(hwnd as *mut _), &mut rect).is_err() {
                return None;
            }
            let w = rect.right.saturating_sub(rect.left).max(0) as u32;
            let h = rect.bottom.saturating_sub(rect.top).max(0) as u32;
            let zoomed = IsZoomed(HWND(hwnd as *mut _)).as_bool();
            let covers_screen = (rect.right.saturating_sub(rect.left))
                >= GetSystemMetrics(SM_CXSCREEN)
                && (rect.bottom.saturating_sub(rect.top)) >= GetSystemMetrics(SM_CYSCREEN);
            Some(WindowRect {
                x: rect.left,
                y: rect.top,
                width: w,
                height: h,
                maximized: zoomed || covers_screen,
            })
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = hwnd;
        None
    }
}

/// True if the Win32 window handle refers to an existing window.
///
/// On non-Windows this always returns true (there is no HWND concept; the
/// WebView close handler is the signal instead).
pub fn is_window_alive(hwnd: usize) -> bool {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::IsWindow;
        if hwnd == 0 {
            return true; // not captured yet
        }
        // SAFETY: IsWindow accepts any handle value and only reports liveness.
        unsafe { IsWindow(Some(HWND(hwnd as *mut _))).as_bool() }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = hwnd;
        true
    }
}

/// Bring an existing window to the foreground (single-instance activation).
/// Windows: SetForegroundWindow. Other platforms: no portable window focus
/// API is available through webui, so this is a no-op (tray restore still
/// re-creates the window).
pub fn focus_window(hwnd: usize) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, keybd_event,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, SW_RESTORE,
            SetForegroundWindow, ShowWindow, SwitchToThisWindow,
        };

        // Undocumented but stable since Win95: the internal ALT+TAB switch,
        // which bypasses the foreground lock entirely. Used as the last
        // resort when SetForegroundWindow is denied (e.g. the activating
        // second instance has exited and revoked its AllowSetForegroundWindow
        // grant).
        let hwnd = HWND(hwnd as *mut _);
        // SAFETY: hwnd is a live top-level window handle from webui; all calls
        // are best-effort foreground manipulation.
        unsafe {
            // Restore a minimized window first, then try the normal path.
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            if SetForegroundWindow(hwnd).as_bool() {
                crate::debug::emit(&format!("focus_window: hwnd {hwnd:?}"));
                return;
            }
            // Foreground-lock fallback 1: attaching our input queue to the
            // current foreground thread makes SetForegroundWindow succeed
            // even when we are not the foreground process.
            let fg = GetForegroundWindow();
            let fg_tid = GetWindowThreadProcessId(fg, None);
            let my_tid = GetCurrentThreadId();
            if fg_tid != 0 && fg_tid != my_tid {
                let _ = AttachThreadInput(my_tid, fg_tid, true);
                let _ = BringWindowToTop(hwnd);
                let _ = SetForegroundWindow(hwnd);
                let _ = AttachThreadInput(my_tid, fg_tid, false);
            }
            // Fallback 2: synthesize an Alt key press. Windows grants the
            // foreground to a process right after it receives input, so a
            // tiny fake Alt tap (the classic trick) defeats the foreground
            // lock that plain SetForegroundWindow hits for windows created
            // long after startup (e.g. a tray-restored window).
            keybd_event(0x12, 0, KEYEVENTF_EXTENDEDKEY, 0); // VK_MENU (Alt)
            keybd_event(0x12, 0, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP, 0);
            let _ = SetForegroundWindow(hwnd);
            // Fallback 3: SwitchToThisWindow ignores the foreground lock
            // (it is what ALT+TAB uses), so a window that was re-created by
            // a tray restore can still be brought to the front reliably.
            SwitchToThisWindow(hwnd, true);
        }
        crate::debug::emit(&format!("focus_window: hwnd {hwnd:?} (forced)"));
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = hwnd;
        crate::debug::emit("focus_window: not supported on this platform");
    }
}

/// Find the visible top-level window handle (HWND) owned by `pid`.
///
/// Windows-only; returns `None` on other platforms. Used to sample the
/// external browser window's geometry so its position/size can be restored on
/// the next launch. Only *visible* windows are considered: browser processes
/// own several hidden helper windows, and tracking one of those would persist
/// a degenerate 0x0 geometry.
pub fn find_hwnd_by_pid(pid: u32) -> Option<usize> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            FindWindowExW, GetWindowThreadProcessId, IsWindowVisible,
        };
        use windows::core::PCWSTR;

        // SAFETY: FindWindowExW enumerates top-level windows; the pid and
        // visibility are read per window. Same approach webui.c uses to
        // locate its browser window, plus a visibility filter.
        unsafe {
            let mut hwnd: Option<HWND> = None;
            loop {
                let Ok(next) = FindWindowExW(None, hwnd, PCWSTR::null(), PCWSTR::null()) else {
                    return None;
                };
                let mut win_pid: u32 = 0;
                GetWindowThreadProcessId(next, Some(&mut win_pid));
                if win_pid == pid && IsWindowVisible(next).as_bool() {
                    return Some(next.0 as usize);
                }
                hwnd = Some(next);
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = pid;
        None
    }
}

/// The OS pid that owns `hwnd` right now (0 when the window is gone).
pub fn window_pid(hwnd: usize) -> u32 {
    #[cfg(target_os = "windows")]
    unsafe {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(HWND(hwnd as *mut _), Some(&mut pid));
        pid
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = hwnd;
        0
    }
}

/// Find a VISIBLE top-level window whose title contains `needle`
/// (case-sensitive), excluding windows owned by `exclude_pid`.
///
/// This is the browser-tracking anchor: our launcher page has a distinctive
/// title, and a HWND — unlike a process — cannot be shared with other
/// webui.me apps, so identity anchored on the window is collision-free.
///
/// Other platforms have no HWND evidence and return `None`.
///
/// NOTE: currently unused — prepared for a future title-based tracking
/// path that complements the cmdline-port probe in `window/tracking.rs`.
pub fn find_visible_window_by_title(needle: &str, exclude_pid: u32) -> Option<(usize, u32)> {
    #[cfg(target_os = "windows")]
    unsafe {
        use windows::Win32::Foundation::{HWND, LPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
        };
        use windows::core::BOOL;

        struct Ctx<'a> {
            needle: Vec<u16>,
            exclude_pid: u32,
            found: Option<(usize, u32)>,
            _marker: std::marker::PhantomData<&'a ()>,
        }
        let mut ctx = Ctx {
            needle: utf16(needle),
            exclude_pid,
            found: None,
            _marker: std::marker::PhantomData,
        };

        unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
            // SAFETY: EnumWindows supplies a valid HWND per invocation; the
            // lparam points at our Ctx, which outlives the enumeration.
            unsafe {
                let ctx = &mut *(lparam.0 as *mut Ctx<'static>);
                if !IsWindowVisible(hwnd).as_bool() {
                    return BOOL(1);
                }
                let mut buf = [0u16; 512];
                let len = GetWindowTextW(hwnd, &mut buf);
                let title = &buf[..len as usize];
                if !contains_slice(title, &ctx.needle) {
                    return BOOL(1);
                }
                let mut pid: u32 = 0;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                if pid != 0 && pid != ctx.exclude_pid {
                    ctx.found = Some((hwnd.0 as usize, pid));
                    return BOOL(0); // stop enumeration
                }
                BOOL(1)
            }
        }

        let _ = EnumWindows(Some(cb), LPARAM(&mut ctx as *mut _ as isize));
        ctx.found
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (needle, exclude_pid);
        None
    }
}
