//! OS theme detection (dark mode) and window theming.
//!
//! Windows: `RegGetValueW` (Personalize keys), `DwmSetWindowAttribute` and
//! `LoadImageW`/`SendMessageW` — all through the `windows` crate. macOS: the
//! `AppleInterfaceStyle` default is read with a `defaults` spawn (no FFI).
//! Other platforms: light theme, no-op theming.

use std::path::Path;

/// True when the OS is in dark mode.
///
/// Windows: `AppsUseLightTheme` / `SystemUsesLightTheme` DWORD = 0. macOS:
/// the global `AppleInterfaceStyle` default equals `Dark`. Others: false.
pub fn is_dark_mode() -> bool {
    #[cfg(target_os = "windows")]
    {
        win_dark_mode()
    }
    #[cfg(target_os = "macos")]
    {
        macos_dark_mode()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        false
    }
}

#[cfg(target_os = "windows")]
fn win_dark_mode() -> bool {
    use windows::Win32::Foundation::WIN32_ERROR;
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_ROUTINE_FLAGS, RegGetValueW};
    use windows::core::PCWSTR;

    /// `RRF_RT_REG_DWORD` — accept only DWORD values.
    const RRF_RT_REG_DWORD: REG_ROUTINE_FLAGS = REG_ROUTINE_FLAGS(0x0000_0018);
    /// `ERROR_SUCCESS`.
    const ERROR_SUCCESS: WIN32_ERROR = WIN32_ERROR(0);

    let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // Default to light when neither value exists.
    let mut value: u32 = 1;
    // SAFETY: constant hive pseudo-handle + stack-local buffer; the read
    // is best-effort and returns an error code instead of panicking.
    unsafe {
        for name in ["AppsUseLightTheme", "SystemUsesLightTheme"] {
            let wname: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            let mut size = std::mem::size_of::<u32>() as u32;
            let rc = RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey.as_ptr()),
                PCWSTR(wname.as_ptr()),
                RRF_RT_REG_DWORD,
                None,
                Some((&mut value as *mut u32).cast()),
                Some(&mut size),
            );
            if rc == ERROR_SUCCESS {
                return value == 0;
            }
        }
    }
    false
}

#[cfg(target_os = "macos")]
fn macos_dark_mode() -> bool {
    // `defaults read -g AppleInterfaceStyle` prints `Dark` when the system
    // appearance is dark, and fails (or prints nothing) in light mode.
    let out = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output();
    matches!(out, Ok(o) if String::from_utf8_lossy(&o.stdout).trim() == "Dark")
}

/// Make the Win32 window `hwnd` use the OS dark titlebar when `dark` is true
/// (DWMWA_USE_IMMERSIVE_DARK_MODE). Best-effort: attributes 20 (Win10 1903+)
/// and 19 (1809) are both attempted.
pub fn set_dark_titlebar(hwnd: usize, dark: bool) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Dwm::{
            DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWINDOWATTRIBUTE, DwmSetWindowAttribute,
        };
        let v: i32 = if dark { 1 } else { 0 };
        // SAFETY: hwnd is a valid window handle; the attribute call is
        // best-effort and ignores the HRESULT (older builds reject attr 20).
        unsafe {
            for attr in [DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWINDOWATTRIBUTE(19)] {
                let _ =
                    DwmSetWindowAttribute(HWND(hwnd as *mut _), attr, (&v as *const i32).cast(), 4);
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (hwnd, dark);
    }
}

/// Replace the small/big icons of the Win32 window `hwnd` with the one loaded
/// from the `.ico` file at `ico_path`. Used to swap in the white (night) icon
/// when the system is in dark mode.
pub fn set_window_icon(hwnd: usize, ico_path: &Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;

        use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            HICON, IMAGE_FLAGS, IMAGE_ICON, LR_DEFAULTSIZE, LR_LOADFROMFILE, LoadImageW,
            SendMessageW, WM_SETICON,
        };
        use windows::core::PCWSTR;

        const ICON_SMALL: usize = 0; // titlebar icon
        const ICON_BIG: usize = 1; // taskbar / Alt-Tab icon

        let wide: Vec<u16> = ico_path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: LoadImageW reads the .ico file; SendMessageW forwards the
        // returned icon handle to the window (kept alive for the window's
        // lifetime, so it is intentionally not destroyed).
        unsafe {
            let Ok(icon) = LoadImageW(
                None,
                PCWSTR(wide.as_ptr()),
                IMAGE_ICON,
                0,
                0,
                IMAGE_FLAGS(LR_LOADFROMFILE.0 | LR_DEFAULTSIZE.0),
            ) else {
                return false;
            };
            let HANDLE(ptr) = icon;
            let icon = HICON(ptr);
            SendMessageW(
                HWND(hwnd as *mut _),
                WM_SETICON,
                Some(WPARAM(ICON_SMALL)),
                Some(LPARAM(icon.0 as isize)),
            );
            SendMessageW(
                HWND(hwnd as *mut _),
                WM_SETICON,
                Some(WPARAM(ICON_BIG)),
                Some(LPARAM(icon.0 as isize)),
            );
        }
        true
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (hwnd, ico_path);
        false
    }
}

/// Apply the OS-theme look to a window: dark titlebar when the system is in
/// dark mode, and the matching window icon — the white (night) `.ico` in dark
/// mode, the black (day) `.ico` in light mode. The icons are raw bytes of
/// multi-size .ico files (empty on non-Windows); the active one is written to
/// the cache directory and applied via `WM_SETICON`, so the titlebar and
/// taskbar follow the OS theme in *both* directions.
pub fn apply_window_theme(hwnd: usize, black_icon: &[u8], white_icon: &[u8]) {
    #[cfg(target_os = "windows")]
    {
        if hwnd == 0 {
            return;
        }
        let dark = is_dark_mode();
        crate::debug::emit(&format!(
            "apply_window_theme: hwnd {hwnd:#x} dark={dark} icons {}B/{}B",
            black_icon.len(),
            white_icon.len()
        ));
        set_dark_titlebar(hwnd, dark);
        let (bytes, name) = if dark {
            (white_icon, "dsh-white.ico")
        } else {
            (black_icon, "dsh.ico")
        };
        if !bytes.is_empty() {
            let dir = crate::platform::cache_dir().join("dshl");
            if std::fs::create_dir_all(&dir).is_ok() {
                let path = dir.join(name);
                if std::fs::write(&path, bytes).is_ok() {
                    set_window_icon(hwnd, &path);
                } else {
                    crate::debug::emit(&format!("apply_window_theme: failed to write {name}"));
                }
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (hwnd, black_icon, white_icon);
    }
}
