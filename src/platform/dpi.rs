//! DPI awareness and scale factors.
//!
//! Windows: `GetDpiForWindow` / `GetDpiForSystem` via windows-rs. Linux: the
//! X11 display geometry is probed with runtime `dlopen`-ed libX11 so the
//! binary never hard-depends on X11.

/// Make the process DPI-aware (per-monitor v2) so the embedded WebView and
/// window are not bitmap-scaled/blurred on high-DPI displays. Must be called
/// before any window is created.
pub fn make_dpi_aware() {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
        };
        // SAFETY: constant argument; the call is best-effort.
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        // no-op on other platforms (they handle DPI natively)
    }
}

/// The DPI scale factor of the display that owns `hwnd` (e.g. 1.75 for
/// 175%), dynamically probed on every platform. `hwnd == 0` falls back to
/// the system-wide scale. Used to convert between the physical pixels a
/// DPI-aware process reads from `GetWindowRect` and the logical pixels
/// (DIPs) browsers expect in `--window-position/--window-size`, and to pick
/// crisp tray icon sizes.
///
/// Windows: `GetDpiForWindow` (per-monitor v2; falls back to
/// `GetDpiForSystem`). Linux: computed from the X11 display geometry
/// (physical size in mm vs pixels, via runtime-dlopen'd libX11); without an
/// X display it falls back to 1.0. Other platforms: 1.0.
pub fn dpi_scale_for_window(hwnd: usize) -> f64 {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
        // SAFETY: GetDpiForWindow/GetDpiForSystem take handles or nothing
        // and return the DPI (96 = 100%); GetDpiForWindow accepts any handle
        // value and returns the system DPI for invalid ones.
        let dpi = unsafe {
            if hwnd != 0 {
                GetDpiForWindow(HWND(hwnd as *mut _))
            } else {
                GetDpiForSystem()
            }
        };
        if dpi > 0 { dpi as f64 / 96.0 } else { 1.0 }
    }
    #[cfg(target_os = "linux")]
    {
        let _ = hwnd;
        // Physical geometry (pixels + millimetres) gives the real DPI.
        if let Some((w, h, w_mm, h_mm)) = x11_geometry()
            && w > 0
            && h > 0
            && w_mm > 0
            && h_mm > 0
        {
            let dpi_x = w as f64 * 25.4 / w_mm as f64;
            let dpi_y = h as f64 * 25.4 / h_mm as f64;
            let dpi = (dpi_x + dpi_y) / 2.0;
            if dpi > 0.0 {
                return dpi / 96.0;
            }
        }
        1.0
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = hwnd;
        1.0
    }
}

/// The system DPI scale factor — `dpi_scale_for_window(0)`.
pub fn dpi_scale() -> f64 {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::HiDpi::GetDpiForSystem;
        // SAFETY: GetDpiForSystem takes no arguments and returns the system
        // DPI (96 = 100%).
        unsafe { GetDpiForSystem() as f64 / 96.0 }
    }
    #[cfg(not(target_os = "windows"))]
    {
        1.0
    }
}

/// Current screen size in physical pixels, or `(0, 0)` when unknown.
///
/// Windows: `GetSystemMetrics` (primary monitor). Linux: the X11 display
/// size via `dlopen`-ed libX11 (runtime-only dependency, works under XWayland
/// too; falls back to `(0, 0)` when unavailable). Other platforms return
/// `(0, 0)` — callers then fall back to the webui hard limits.
pub fn screen_size() -> (u32, u32) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
        // SAFETY: GetSystemMetrics never fails for these indexes; a zero or
        // negative return means the metrics are unavailable.
        let w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        if w > 0 && h > 0 {
            (w as u32, h as u32)
        } else {
            (0, 0)
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Some((w, h, _, _)) = x11_geometry() {
            (w, h)
        } else {
            (0, 0)
        }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        (0, 0)
    }
}

/// X11 display geometry: `(width_px, height_px, width_mm, height_mm)`, or
/// `None` when no X display / libX11 is available.
#[cfg(target_os = "linux")]
fn x11_geometry() -> Option<(u32, u32, u32, u32)> {
    // SAFETY: dlopen/dlsym with fixed sonames; failures are ignored and
    // degrade to the caller's fallback.
    unsafe {
        let lib = libc::dlopen(c"libX11.so.6".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if lib.is_null() {
            return None;
        }
        type OpenDisplay = unsafe extern "C" fn(*const libc::c_char) -> *mut libc::c_void;
        type ScreenFn = unsafe extern "C" fn(*mut libc::c_void, libc::c_int) -> libc::c_int;
        type DefaultScreen = unsafe extern "C" fn(*mut libc::c_void) -> libc::c_int;
        // Any missing symbol (unexpected for libX11, but defensive) makes us
        // bail out instead of calling a null function pointer.
        let open: Option<OpenDisplay> =
            std::mem::transmute(libc::dlsym(lib, c"XOpenDisplay".as_ptr()));
        let width: Option<ScreenFn> =
            std::mem::transmute(libc::dlsym(lib, c"XDisplayWidth".as_ptr()));
        let height: Option<ScreenFn> =
            std::mem::transmute(libc::dlsym(lib, c"XDisplayHeight".as_ptr()));
        let width_mm: Option<ScreenFn> =
            std::mem::transmute(libc::dlsym(lib, c"XDisplayWidthMM".as_ptr()));
        let height_mm: Option<ScreenFn> =
            std::mem::transmute(libc::dlsym(lib, c"XDisplayHeightMM".as_ptr()));
        let default_screen: Option<DefaultScreen> =
            std::mem::transmute(libc::dlsym(lib, c"XDefaultScreen".as_ptr()));
        let (
            Some(open),
            Some(width),
            Some(height),
            Some(width_mm),
            Some(height_mm),
            Some(default_screen),
        ) = (open, width, height, width_mm, height_mm, default_screen)
        else {
            let _ = libc::dlclose(lib);
            return None;
        };
        let dpy = open(std::ptr::null());
        if dpy.is_null() {
            let _ = libc::dlclose(lib);
            return None;
        }
        let screen = default_screen(dpy);
        let w = width(dpy, screen);
        let h = height(dpy, screen);
        let w_mm = width_mm(dpy, screen);
        let h_mm = height_mm(dpy, screen);
        let _ = libc::dlclose(lib);
        if w > 0 && h > 0 {
            Some((w as u32, h as u32, w_mm as u32, h_mm as u32))
        } else {
            None
        }
    }
}
