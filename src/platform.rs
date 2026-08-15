//! Platform detection, shell selection, path discovery and OS helpers.
//!
//! Everything that differs between Windows / Linux / macOS is centralised
//! here so the rest of the code stays platform-agnostic.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
    Macos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    /// `powershell -NoProfile -Command`
    PowerShell,
    /// `cmd /C`
    Cmd,
    /// `bash -lc`
    Bash,
    /// `sh -c`
    Sh,
}

/// Detect the host OS.
pub fn os() -> Os {
    if cfg!(target_os = "windows") {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::Macos
    } else {
        Os::Linux // best-effort: treat other unix-likes as linux
    }
}

/// Detect the host CPU architecture.
pub fn arch() -> Arch {
    match env::consts::ARCH {
        "x86_64" => Arch::X86_64,
        "aarch64" => Arch::Aarch64,
        _ => Arch::Other,
    }
}

pub fn os_name() -> &'static str {
    match os() {
        Os::Windows => "windows",
        Os::Linux => "linux",
        Os::Macos => "macos",
    }
}

pub fn arch_name() -> &'static str {
    match arch() {
        Arch::X86_64 => "x86_64",
        Arch::Aarch64 => "aarch64",
        Arch::Other => env::consts::ARCH,
    }
}

/// The shell used to run script snippets (install scripts, nvm, …).
pub fn shell() -> Shell {
    match os() {
        Os::Windows => Shell::PowerShell,
        Os::Macos | Os::Linux => Shell::Bash,
    }
}

/// Build a `Command` that runs a snippet through the platform shell.
///
/// The returned command already carries the shell executable; callers append
/// the snippet as the single argument via `.arg(script)`.
pub fn shell_command() -> Command {
    match shell() {
        Shell::PowerShell => {
            let mut c = Command::new("powershell");
            c.args(["-NoProfile", "-NonInteractive", "-Command"]);
            c
        }
        Shell::Cmd => {
            let mut c = Command::new("cmd");
            c.arg("/C");
            c
        }
        Shell::Bash => {
            let mut c = Command::new("bash");
            c.args(["-lc"]);
            c
        }
        Shell::Sh => {
            let mut c = Command::new("sh");
            c.arg("-c");
            c
        }
    }
}

/// Home directory.
pub fn home_dir() -> Option<PathBuf> {
    if let Ok(h) = env::var("USERPROFILE")
        && !h.is_empty()
    {
        return Some(PathBuf::from(h));
    }
    if let Ok(h) = env::var("HOME")
        && !h.is_empty()
    {
        return Some(PathBuf::from(h));
    }
    None
}

/// The launcher cache directory: `~/.cache/dshl`.
///
/// The `~/.cache/bin` convention from the fallback guide lives under
/// [`bin_dir`] and is intentionally the same on every platform so the fnm
/// manual-install instructions stay uniform.
pub fn cache_dir() -> PathBuf {
    if let Ok(c) = env::var("DSHL_CACHE")
        && !c.is_empty()
    {
        return PathBuf::from(c);
    }
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cache")
}

/// `~/.cache/bin` — used for the fnm auto-install fallback.
pub fn bin_dir() -> PathBuf {
    cache_dir().join("bin")
}

/// The directory that holds `dshl.toml` by convention (config home).
pub fn config_dir() -> PathBuf {
    match os() {
        Os::Windows => env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_dir().unwrap_or_default())
            .join("dshl"),
        Os::Macos => home_dir()
            .unwrap_or_default()
            .join("Library")
            .join("Application Support")
            .join("dshl"),
        Os::Linux => env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_dir().unwrap_or_default().join(".config"))
            .join("dshl"),
    }
}

/// Executable extension for binaries on this platform.
pub fn executable_ext() -> &'static str {
    if cfg!(windows) { ".exe" } else { "" }
}

/// Append the executable extension to `name` if the platform needs one.
pub fn with_ext(name: &str) -> String {
    format!("{name}{}", executable_ext())
}

/// Resolve a CLI tool to a runnable path/name.
///
/// On Windows, Node tools (`npm`, `npx`, `pnpm`, `pnpx`) are `.cmd` shims, and
/// `CreateProcess` only auto-finds `.exe`, so they must be resolved to their
/// `.cmd` path. Returns the full path when found, else the name (+ `.cmd`).
pub fn tool(name: &str) -> PathBuf {
    which(name).unwrap_or_else(|| {
        if cfg!(windows) {
            PathBuf::from(format!("{name}.cmd"))
        } else {
            PathBuf::from(name)
        }
    })
}

/// Locate an executable by name on `extra_dirs` first, then `PATH` plus the
/// well-known tool locations.
///
/// `extra_dirs` lets callers find tools that were installed by an earlier
/// flow step (fnm's node bin, pnpm's global bin, …) even when those
/// directories are not on the ambient `PATH`.
pub fn which_in(name: &str, extra_dirs: &[PathBuf]) -> Option<PathBuf> {
    let candidates = if cfg!(windows) {
        vec![
            with_ext(name),
            format!("{name}.cmd"),
            format!("{name}.bat"),
            name.to_string(),
        ]
    } else {
        vec![name.to_string()]
    };

    let mut dirs: Vec<PathBuf> = extra_dirs.to_vec();
    dirs.extend(search_dirs());
    for dir in dirs {
        for candidate in &candidates {
            let full = dir.join(candidate);
            if full.is_file() && is_executable(&full) {
                return Some(full);
            }
        }
    }
    None
}

/// Locate an executable by name on `PATH` plus the well-known tool locations.
pub fn which(name: &str) -> Option<PathBuf> {
    which_in(name, &[])
}

/// Directories searched by [`which`], in priority order.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(path) = env::var_os("PATH") {
        dirs.extend(env::split_paths(&path));
    }
    dirs.extend(known_tool_dirs());
    dirs
}

/// Well-known install locations for the tools dshl manages.
pub fn known_tool_dirs() -> Vec<PathBuf> {
    let home = home_dir().unwrap_or_default();
    let mut dirs = vec![
        home.join(".bun").join("bin"),
        home.join(".fnm"),
        home.join(".local").join("bin"),
        bin_dir(),
        home.join(".nvm").join("versions").join("node"),
    ];
    if cfg!(windows) {
        dirs.push(
            env::var("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join("npm"),
        );
        dirs.push(
            env::var("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join("fnm"),
        );
        // pnpm global bin: pnpm 10 used %LOCALAPPDATA%\pnpm, pnpm 11 uses
        // %LOCALAPPDATA%\pnpm\bin.
        let local = env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_default();
        dirs.push(local.join("pnpm"));
        dirs.push(local.join("pnpm").join("bin"));
    } else {
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(home.join(".local").join("share").join("pnpm"));
    }
    dirs
}

/// pnpm's default global bin directory (pnpm 10/11 layout), used as a
/// fallback when `pnpm bin -g` cannot be queried.
pub fn default_pnpm_bin_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        env::var("LOCALAPPDATA")
            .ok()
            .map(|d| PathBuf::from(d).join("pnpm").join("bin"))
    } else {
        home_dir().map(|h| h.join(".local").join("share").join("pnpm"))
    }
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            return meta.is_file() && meta.permissions().mode() & 0o111 != 0;
        }
        false
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Open a file (selected in the file manager) or a directory in the OS shell.
pub fn open_path(path: &Path) -> std::io::Result<()> {
    match os() {
        Os::Windows => {
            if path.is_dir() {
                Command::new("explorer").arg(path).spawn().map(|_| ())
            } else {
                Command::new("explorer")
                    .arg(format!("/select,{}", path.display()))
                    .spawn()
                    .map(|_| ())
            }
        }
        Os::Macos => Command::new("open").arg(path).spawn().map(|_| ()),
        Os::Linux => Command::new("xdg-open").arg(path).spawn().map(|_| ()),
    }
}

/// Force-kill a process and, on Windows, its descendants.
///
/// On Windows `taskkill /F /T` walks the parent-child tree; on Unix we send
/// `SIGKILL`. Graceful stopping is done separately via
/// [`crate::process::AsyncChild::signal_stop`].
pub fn kill_tree(pid: u32) {
    if cfg!(windows) {
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    } else {
        let _ = Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// Return the path of the current executable.
pub fn current_exe_dir() -> Option<PathBuf> {
    env::current_exe().ok()?.parent().map(|p| p.to_path_buf())
}

/// Make the process DPI-aware (per-monitor v2) so the embedded WebView and
/// window are not bitmap-scaled/blurred on high-DPI displays. Must be called
/// before any window is created.
pub fn make_dpi_aware() {
    #[cfg(windows)]
    {
        use std::os::raw::c_void;
        #[link(name = "user32")]
        unsafe extern "system" {
            fn SetProcessDpiAwarenessContext(value: *mut c_void) -> i32;
        }
        // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 == -4
        // SAFETY: a constant argument is fine; the call is best-effort.
        unsafe {
            SetProcessDpiAwarenessContext((-4isize) as *mut c_void);
        }
    }
    #[cfg(not(windows))]
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
    #[cfg(windows)]
    {
        use std::os::raw::c_void;
        #[link(name = "user32")]
        unsafe extern "system" {
            fn GetDpiForSystem() -> u32;
            fn GetDpiForWindow(hwnd: *mut c_void) -> u32;
        }
        // SAFETY: GetDpiForWindow/GetDpiForSystem take handles or nothing
        // and return the DPI (96 = 100%); GetDpiForWindow accepts any handle
        // value and returns the system DPI for invalid ones.
        let dpi = unsafe {
            if hwnd != 0 {
                GetDpiForWindow(hwnd as *mut c_void)
            } else {
                GetDpiForSystem()
            }
        };
        if dpi > 0 { dpi as f64 / 96.0 } else { 1.0 }
    }
    #[cfg(target_os = "linux")]
    {
        // Physical geometry (pixels + millimetres) gives the real DPI.
        if let Some((w, h, w_mm, h_mm)) = x11_geometry() {
            if w > 0 && h > 0 && w_mm > 0 && h_mm > 0 {
                let dpi_x = w as f64 * 25.4 / w_mm as f64;
                let dpi_y = h as f64 * 25.4 / h_mm as f64;
                let dpi = (dpi_x + dpi_y) / 2.0;
                if dpi > 0.0 {
                    return dpi / 96.0;
                }
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
        let open: Option<OpenDisplay> = std::mem::transmute(libc::dlsym(lib, c"XOpenDisplay".as_ptr()));
        let width: Option<ScreenFn> = std::mem::transmute(libc::dlsym(lib, c"XDisplayWidth".as_ptr()));
        let height: Option<ScreenFn> = std::mem::transmute(libc::dlsym(lib, c"XDisplayHeight".as_ptr()));
        let width_mm: Option<ScreenFn> = std::mem::transmute(libc::dlsym(lib, c"XDisplayWidthMM".as_ptr()));
        let height_mm: Option<ScreenFn> = std::mem::transmute(libc::dlsym(lib, c"XDisplayHeightMM".as_ptr()));
        let default_screen: Option<DefaultScreen> =
            std::mem::transmute(libc::dlsym(lib, c"XDefaultScreen".as_ptr()));
        let (Some(open), Some(width), Some(height), Some(width_mm), Some(height_mm),
             Some(default_screen)) = (open, width, height, width_mm, height_mm, default_screen)
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

/// The system DPI scale factor — `dpi_scale_for_window(0)`.
pub fn dpi_scale() -> f64 {
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn GetDpiForSystem() -> u32;
        }
        // SAFETY: GetDpiForSystem takes no arguments and returns the system
        // DPI (96 = 100%).
        unsafe { GetDpiForSystem() as f64 / 96.0 }
    }
    #[cfg(not(windows))]
    {
        1.0
    }
}

/// True when the OS is in dark mode (theme `AppsUseLightTheme` /
/// `SystemUsesLightTheme` DWORD = 0). Only meaningful on Windows.
pub fn is_dark_mode() -> bool {
    #[cfg(windows)]
    {
        use std::os::raw::c_void;

        #[link(name = "advapi32")]
        unsafe extern "system" {
            fn RegGetValueW(
                key: *mut c_void,
                subkey: *const u16,
                value: *const u16,
                flags: u32,
                ty: *mut u32,
                data: *mut c_void,
                size: *mut u32,
            ) -> i32;
        }

        const HKEY_CURRENT_USER: *mut c_void = 0x8000_0001usize as *mut c_void;
        const RRF_RT_REG_DWORD: u32 = 0x0000_0018;
        const ERROR_SUCCESS: i32 = 0;

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
                    subkey.as_ptr(),
                    wname.as_ptr(),
                    RRF_RT_REG_DWORD,
                    std::ptr::null_mut(),
                    (&mut value as *mut u32).cast(),
                    &mut size,
                );
                if rc == ERROR_SUCCESS {
                    return value == 0;
                }
            }
        }
        false
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Make the Win32 window `hwnd` use the OS dark titlebar when `dark` is true
/// (DWMWA_USE_IMMERSIVE_DARK_MODE). Best-effort: attributes 20 (Win10 1903+)
/// and 19 (1809) are both attempted.
pub fn set_dark_titlebar(hwnd: usize, dark: bool) {
    #[cfg(windows)]
    {
        use std::os::raw::c_void;
        #[link(name = "dwmapi")]
        unsafe extern "system" {
            fn DwmSetWindowAttribute(
                hwnd: *mut c_void,
                attribute: u32,
                value: *const c_void,
                size: u32,
            ) -> i32;
        }
        let v: i32 = if dark { 1 } else { 0 };
        // SAFETY: hwnd is a valid window handle; the attribute call is
        // best-effort and ignores the HRESULT (older builds reject attr 20).
        unsafe {
            for attr in [20u32, 19u32] {
                DwmSetWindowAttribute(hwnd as *mut c_void, attr, (&v as *const i32).cast(), 4);
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (hwnd, dark);
    }
}

/// Replace the small/big icons of the Win32 window `hwnd` with the one loaded
/// from the `.ico` file at `ico_path`. Used to swap in the white (night) icon
/// when the system is in dark mode.
pub fn set_window_icon(hwnd: usize, ico_path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::raw::c_void;
        use std::os::windows::ffi::OsStrExt;

        #[link(name = "user32")]
        unsafe extern "system" {
            fn LoadImageW(
                instance: *mut c_void,
                name: *const u16,
                ty: u32,
                cx: i32,
                cy: i32,
                flags: u32,
            ) -> *mut c_void;
            fn SendMessageW(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> isize;
        }

        const IMAGE_ICON: u32 = 1;
        const LR_LOADFROMFILE: u32 = 0x10;
        const LR_DEFAULTSIZE: u32 = 0x40;
        const WM_SETICON: u32 = 0x0080;
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
            let icon = LoadImageW(
                std::ptr::null_mut(),
                wide.as_ptr(),
                IMAGE_ICON,
                0,
                0,
                LR_LOADFROMFILE | LR_DEFAULTSIZE,
            );
            if icon.is_null() {
                return false;
            }
            SendMessageW(hwnd as *mut c_void, WM_SETICON, ICON_SMALL, icon as isize);
            SendMessageW(hwnd as *mut c_void, WM_SETICON, ICON_BIG, icon as isize);
        }
        true
    }
    #[cfg(not(windows))]
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
    #[cfg(windows)]
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
            let dir = cache_dir().join("dshl");
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
    #[cfg(not(windows))]
    {
        let _ = (hwnd, black_icon, white_icon);
    }
}

/// The current geometry of a window, plus whether it is maximized/fullscreen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    #[cfg(windows)]
    {
        use std::os::raw::c_void;

        #[repr(C)]
        #[derive(Default, Clone, Copy)]
        struct Rect {
            left: i32,
            top: i32,
            right: i32,
            bottom: i32,
        }

        #[link(name = "user32")]
        unsafe extern "system" {
            fn GetWindowRect(hwnd: *mut c_void, rect: *mut Rect) -> i32;
            fn IsZoomed(hwnd: *mut c_void) -> i32;
            fn GetSystemMetrics(index: i32) -> i32;
        }
        const SM_CXSCREEN: i32 = 0;
        const SM_CYSCREEN: i32 = 1;

        // SAFETY: hwnd is a valid window handle; the structs are stack-local.
        unsafe {
            let mut rect = Rect::default();
            if GetWindowRect(hwnd as *mut c_void, &mut rect) == 0 {
                return None;
            }
            let w = rect.right.saturating_sub(rect.left).max(0) as u32;
            let h = rect.bottom.saturating_sub(rect.top).max(0) as u32;
            let zoomed = IsZoomed(hwnd as *mut c_void) != 0;
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
    #[cfg(not(windows))]
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
    #[cfg(windows)]
    {
        use std::os::raw::c_void;
        #[link(name = "user32")]
        unsafe extern "system" {
            fn IsWindow(hwnd: *mut c_void) -> i32;
        }
        if hwnd == 0 {
            return true; // not captured yet
        }
        // SAFETY: IsWindow accepts any handle value and only reports liveness.
        unsafe { IsWindow(hwnd as *mut c_void) != 0 }
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_alive_detects_death() {
        #[cfg(windows)]
        let mut child = Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("spawn ping");
        #[cfg(unix)]
        let mut child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");

        let pid = child.id();
        assert!(process_alive(pid), "live process should be alive");

        child.kill().expect("kill child");
        let _ = child.wait();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!process_alive(pid), "killed process should be dead");
    }
}

#[cfg(windows)]
mod win_proc {
    use std::os::raw::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetExitCodeProcess(process: *mut c_void, code: *mut u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;

    pub fn alive(pid: u32) -> bool {
        // SAFETY: FFI calls are guarded; the handle is closed on all paths.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut code: u32 = 0;
            let ok = GetExitCodeProcess(handle, &mut code);
            CloseHandle(handle);
            ok != 0 && code == STILL_ACTIVE
        }
    }
}

/// True if the process identified by `pid` is still running.
pub fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(windows)]
    {
        win_proc::alive(pid)
    }
    #[cfg(unix)]
    {
        // SAFETY: kill(pid, 0) only probes for existence, it sends no signal.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
}

/// Find a process whose command line contains `needle`, returning its pid.
///
/// Used to track the external browser window webui launched (its command line
/// carries `--app=http://localhost:<port>`), so we can detect when the user
/// closes it. Prefer this over webui's own `get_child_process_id`, which
/// relies on the now-removed `wmic`.
pub fn find_process_by_cmdline(needle: &str) -> Option<u32> {
    #[cfg(windows)]
    {
        // Restrict to browser binaries so the query never matches its own
        // PowerShell/cmd wrapper (whose command line also contains `needle`).
        let script = format!(
            "(Get-CimInstance Win32_Process | Where-Object {{ \
             $_.Name -match 'msedge|chrome|firefox|chromium|brave|vivaldi|opera|yandex|epic' -and \
             $_.CommandLine -and $_.CommandLine -match [regex]::Escape('{needle}') }} | \
             Select-Object -First 1).ProcessId"
        );
        let mut cmd = Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
        crate::process::run(&mut cmd)
            .ok()
            .and_then(|res| res.stdout.trim().parse::<u32>().ok())
    }
    #[cfg(unix)]
    {
        // pgrep excludes itself and we spawn it directly (no shell wrapper),
        // so the only `-f` match is the browser process.
        let mut cmd = Command::new("pgrep");
        cmd.args(["-f", needle]);
        crate::process::run(&mut cmd).ok().and_then(|res| {
            res.stdout
                .lines()
                .next()
                .and_then(|l| l.trim().parse::<u32>().ok())
        })
    }
}

/// System-tray support (Windows only).
///
/// When `close-to-tray` is enabled, closing the launcher window hides it
/// instead of exiting so dsh keeps running in the background. A tray icon
/// (resource 101, the same icon embedded for the window) restores the
/// window on click and offers a small menu: restore or quit. Quitting from
/// the tray flags the launcher to shut down (the event loop picks it up and
/// goes through the normal Ctrl+C clean-shutdown path, which stops dsh
/// gracefully via SIGINT/SIGTERM — the same cross-platform mechanism as
/// everywhere else).
///
/// Non-Windows platforms have no tray implementation (no appindicator/GTK
/// dependency); `close-to-tray` is ignored there and closing keeps the
/// original exit behaviour.
#[cfg(windows)]
pub mod tray {
    use std::os::raw::c_void;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NotifyIconDataW {
        cb_size: u32,
        h_wnd: *mut c_void,
        u_id: u32,
        u_flags: u32,
        u_callback_message: u32,
        h_icon: *mut c_void,
        sz_tip: [u16; 128],
        dw_state: u32,
        dw_state_mask: u32,
        sz_info: [u16; 256],
        u_version_or_timeout: u32,
        sz_info_title: [u16; 64],
        dw_info_flags: u32,
        guid_item: [u8; 16],
        h_balloon_icon: *mut c_void,
    }

    #[link(name = "user32")]
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn RegisterClassExW(class: *const WndClassExW) -> u16;
        fn CreateWindowExW(
            ex_style: u32,
            class: *const u16,
            name: *const u16,
            style: u32,
            x: i32,
            y: i32,
            w: i32,
            h: i32,
            parent: *mut c_void,
            menu: *mut c_void,
            instance: *mut c_void,
            param: *mut c_void,
        ) -> *mut c_void;
        fn DefWindowProcW(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> isize;
        fn GetMessageW(msg: *mut Msg, hwnd: *mut c_void, min: u32, max: u32) -> i32;
        fn TranslateMessage(msg: *const Msg) -> i32;
        fn DispatchMessageW(msg: *const Msg) -> isize;
        fn PostQuitMessage(code: i32);
        fn Shell_NotifyIconW(dw_message: u32, data: *mut NotifyIconDataW) -> i32;
        fn LoadIconW(instance: *mut c_void, name: *const u16) -> *mut c_void;
        fn GetCursorPos(point: *mut Point) -> i32;
        fn LoadImageW(
            instance: *mut c_void,
            name: *const u16,
            ty: u32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> *mut c_void;
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
        fn CreatePopupMenu() -> *mut c_void;
        fn AppendMenuW(menu: *mut c_void, flags: u32, id: usize, text: *const u16) -> i32;
        fn TrackPopupMenu(
            menu: *mut c_void,
            flags: u32,
            x: i32,
            y: i32,
            reserved: i32,
            hwnd: *mut c_void,
            rect: *mut c_void,
        ) -> i32;
        fn DestroyMenu(menu: *mut c_void) -> i32;
        fn DestroyIcon(icon: *mut c_void) -> i32;
    }

    #[repr(C)]
    struct WndClassExW {
        cb_size: u32,
        style: u32,
        wnd_proc: usize,
        cb_cls_extra: i32,
        cb_wnd_extra: i32,
        instance: *mut c_void,
        icon: *mut c_void,
        cursor: *mut c_void,
        background: *mut c_void,
        menu_name: *const u16,
        class_name: *const u16,
        icon_small: *mut c_void,
    }

    #[repr(C)]
    struct Msg {
        hwnd: *mut c_void,
        message: u32,
        wparam: usize,
        lparam: isize,
        time: u32,
        pt_x: i32,
        pt_y: i32,
    }

    const WM_APP: u32 = 0x8000;
    const WM_TRAY_CALLBACK: u32 = WM_APP + 1;
    const NIM_ADD: u32 = 0;
    const NIM_MODIFY: u32 = 1;
    const NIM_DELETE: u32 = 2;
    const IMAGE_ICON: u32 = 1;
    const LR_LOADFROMFILE: u32 = 0x10;
    const NIF_MESSAGE: u32 = 0x1;
    const NIF_ICON: u32 = 0x2;
    const NIF_TIP: u32 = 0x4;
    const WM_LBUTTONUP: usize = 0x0202;
    const WM_RBUTTONUP: usize = 0x0205;
    const WM_COMMAND: u32 = 0x0111;
    /// Custom message: asked (from another thread) to end the message loop.
    const WM_TRAY_QUIT: u32 = WM_APP + 2;
    /// Max gap (ms) between two clicks that counts as a double click.
    const DOUBLE_CLICK_MS: u64 = 400;
    const MF_STRING: u32 = 0;
    const TPM_RIGHTBUTTON: u32 = 0x2;
    const MENU_RESTORE: usize = 1;
    const MENU_QUIT: usize = 2;
    const CW_USEDEFAULT: i32 = -1;
    const HWND_MESSAGE: *mut c_void = -3isize as *mut c_void;

    /// HWND of the hidden tray message window (0 until created).
    static TRAY_HWND: AtomicUsize = AtomicUsize::new(0);
    /// Tray icon currently added to the notification area.
    static ICON_ACTIVE: AtomicBool = AtomicBool::new(false);
    /// User chose "quit" from the tray menu.
    static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);
    /// User chose "restore window" from the tray (click or menu).
    static RESTORE_REQUESTED: AtomicBool = AtomicBool::new(false);
    /// Currently applied tray icon handle (so a replacement can be freed).
    static CURRENT_ICON: AtomicUsize = AtomicUsize::new(0);
    /// Timestamp (ms) of the last left click, for double-click detection.
    static LAST_CLICK: AtomicU64 = AtomicU64::new(0);
    static CLASS_REGISTERED: OnceLock<()> = OnceLock::new();

    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }

    fn cursor_pos() -> (i32, i32) {
        // SAFETY: GetCursorPos with a stack-local POINT; best-effort.
        let mut pt = Point { x: 0, y: 0 };
        let ok = unsafe { GetCursorPos(&mut pt) };
        if ok == 0 { (0, 0) } else { (pt.x, pt.y) }
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: *mut c_void,
        msg: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize {
        // SAFETY: all calls below are best-effort Win32 window/tray calls on
        // valid handles owned by this module; failures are ignored.
        unsafe {
            if msg == WM_TRAY_QUIT {
                // Runs on the tray thread: PostQuitMessage ends that thread's loop.
                PostQuitMessage(0);
                return 0;
            }
            // Menu items chosen in the right-click popup arrive here (the
            // TrackPopupMenu notification mode sends WM_COMMAND to the owner
            // window). LOWORD(wparam) is the item id.
            if msg == WM_COMMAND && wparam & 0xffff == MENU_RESTORE {
                RESTORE_REQUESTED.store(true, Ordering::SeqCst);
                return 0;
            }
            if msg == WM_COMMAND && wparam & 0xffff == MENU_QUIT {
                QUIT_REQUESTED.store(true, Ordering::SeqCst);
                return 0;
            }
            if msg == WM_TRAY_CALLBACK {
                // For Shell_NotifyIcon callback messages, wParam is the icon
                // ID and lParam is the mouse message (WM_LBUTTONUP /
                // WM_RBUTTONUP / …). Matching the wrong one makes every click
                // a no-op.
                match lparam as usize {
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
                        if !menu.is_null() {
                            let restore: Vec<u16> = "恢复窗口"
                                .encode_utf16()
                                .chain(std::iter::once(0))
                                .collect();
                            let quit: Vec<u16> =
                                "退出".encode_utf16().chain(std::iter::once(0)).collect();
                            AppendMenuW(menu, MF_STRING, MENU_RESTORE, restore.as_ptr());
                            AppendMenuW(menu, MF_STRING, MENU_QUIT, quit.as_ptr());
                            let (x, y) = cursor_pos();
                            TrackPopupMenu(
                                menu,
                                TPM_RIGHTBUTTON,
                                x,
                                y,
                                0,
                                hwnd,
                                std::ptr::null_mut(),
                            );
                            DestroyMenu(menu);
                        }
                    }
                    _ => {}
                }
                return 0;
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
            // SAFETY: FFI window-class registration with a static WndProc.
            unsafe {
                let wc = WndClassExW {
                    cb_size: std::mem::size_of::<WndClassExW>() as u32,
                    style: 0,
                    wnd_proc: wnd_proc as *const () as usize,
                    cb_cls_extra: 0,
                    cb_wnd_extra: 0,
                    instance: GetModuleHandleW(std::ptr::null()),
                    icon: std::ptr::null_mut(),
                    cursor: std::ptr::null_mut(),
                    background: std::ptr::null_mut(),
                    menu_name: std::ptr::null(),
                    class_name: class_name.as_ptr(),
                    icon_small: std::ptr::null_mut(),
                };
                RegisterClassExW(&wc);
            }
        });
        std::thread::spawn(message_loop);
    }

    fn message_loop() {
        let class_name: Vec<u16> = "dshl_tray_wnd"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: FFI window creation; message-only parent (HWND_MESSAGE).
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class_name.as_ptr(),
                class_name.as_ptr(),
                0,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                HWND_MESSAGE,
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null_mut(),
            )
        };
        if hwnd.is_null() {
            crate::debug::emit("tray: message window creation failed");
            return;
        }
        TRAY_HWND.store(hwnd as usize, Ordering::SeqCst);
        add_icon(hwnd);
        // Match the current OS theme right away (the window-theme watcher
        // may already have stopped when the window closed).
        set_icon(crate::platform::is_dark_mode());
        crate::debug::emit("tray: active (close-to-tray)");
        // SAFETY: standard message loop on the tray window.
        unsafe {
            let mut msg = std::mem::zeroed::<Msg>();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        remove_icon();
        crate::debug::emit("tray: message loop ended");
    }

    fn add_icon(hwnd: *mut c_void) {
        if ICON_ACTIVE.swap(true, Ordering::SeqCst) {
            return;
        }
        // SAFETY: zeroed + fully initialised before use (only fields we set
        // are read by Shell_NotifyIconW for NIM_ADD).
        let mut data = unsafe { std::mem::zeroed::<NotifyIconDataW>() };
        data.cb_size = std::mem::size_of::<NotifyIconDataW>() as u32;
        data.h_wnd = hwnd;
        data.u_id = 1;
        data.u_flags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.u_callback_message = WM_TRAY_CALLBACK;
        // Icon resource 101 (the window icon embedded by build.rs).
        // SAFETY: constant MAKEINTRESOURCE-ish pointer + null module = exe.
        data.h_icon =
            unsafe { LoadIconW(GetModuleHandleW(std::ptr::null()), 101usize as *const u16) };
        let tip: Vec<u16> = "DSHL · DeepSeek Harness"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        for (i, c) in tip.iter().take(127).enumerate() {
            data.sz_tip[i] = *c;
        }
        // SAFETY: FFI call with a correctly sized, zeroed structure.
        let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &mut data) };
        if ok == 0 {
            ICON_ACTIVE.store(false, Ordering::SeqCst);
            crate::debug::emit("tray: Shell_NotifyIcon(NIM_ADD) failed");
        }
    }

    fn remove_icon() {
        if !ICON_ACTIVE.swap(false, Ordering::SeqCst) {
            return;
        }
        let hwnd = TRAY_HWND.load(Ordering::SeqCst) as *mut c_void;
        // SAFETY: zeroed; only cb_size/hWnd/uID are read for NIM_DELETE.
        let mut data = unsafe { std::mem::zeroed::<NotifyIconDataW>() };
        data.cb_size = std::mem::size_of::<NotifyIconDataW>() as u32;
        data.h_wnd = hwnd;
        data.u_id = 1;
        // SAFETY: FFI call with a correctly sized, zeroed structure.
        unsafe { Shell_NotifyIconW(NIM_DELETE, &mut data) };
    }

    /// The close handler lets the WebView window close for real (destroying
    /// the WebView2 processes and freeing memory); the tray icon is already
    /// active, so there is nothing left to hide.
    pub fn hide_to_tray() {
        crate::debug::emit("tray: window closed, keeping dsh running (close-to-tray)");
    }

    /// Swap the tray icon for the theme-appropriate variant (white "night"
    /// icon in dark mode, black in light mode). The `.ico` bytes are written
    /// to the cache dir and loaded with LoadImageW, then applied via
    /// `NIM_MODIFY` — same pattern as the window icon in
    /// [`crate::platform::apply_window_theme`].
    pub fn set_icon(dark: bool) {
        let hwnd = TRAY_HWND.load(Ordering::SeqCst) as *mut c_void;
        if hwnd.is_null() || !ICON_ACTIVE.load(Ordering::SeqCst) {
            return;
        }
        let (bytes, name) = if dark {
            (include_bytes!("../packing/windows/dsh-white.ico"), "dsh-white.ico")
        } else {
            (include_bytes!("../packing/windows/dsh.ico"), "dsh.ico")
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
                std::ptr::null_mut(),
                wide.as_ptr(),
                IMAGE_ICON,
                px,
                px,
                LR_LOADFROMFILE,
            )
        };
        if icon.is_null() {
            return;
        }
        // Release the previously applied icon (the tray no longer uses it).
        let prev = CURRENT_ICON.swap(icon as usize, Ordering::SeqCst) as *mut c_void;
        if !prev.is_null() {
            // SAFETY: DestroyIcon on a handle LoadImageW returned.
            unsafe { DestroyIcon(prev) };
        }
        let mut data = unsafe { std::mem::zeroed::<NotifyIconDataW>() };
        data.cb_size = std::mem::size_of::<NotifyIconDataW>() as u32;
        data.h_wnd = hwnd;
        data.u_id = 1;
        data.u_flags = NIF_ICON;
        data.h_icon = icon;
        // SAFETY: FFI call with a correctly sized structure.
        let ok = unsafe { Shell_NotifyIconW(NIM_MODIFY, &mut data) };
        if ok != 0 {
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

    /// Remove the icon and stop the message loop (on shutdown).
    pub fn shutdown() {
        remove_icon();
        // Ask the tray thread to end its message loop (WndProc posts WM_QUIT).
        let hwnd = TRAY_HWND.load(Ordering::SeqCst) as *mut c_void;
        if !hwnd.is_null() {
            // SAFETY: PostMessageW with a valid window handle is safe.
            unsafe { PostMessageW(hwnd, WM_TRAY_QUIT, 0, 0) };
        }
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn PostMessageW(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> i32;
    }
}

/// Linux tray support via StatusNotifier (libayatana-appindicator3 + GTK3),
/// loaded at runtime with `dlopen` so the binary never hard-depends on the
/// desktop libs — on systems without the library, `close-to-tray` degrades
/// to the original close-to-exit behaviour with a log line.
///
/// Unlike Windows (where the close handler intercepts and hides the window),
/// WebKitGTK windows cannot be intercepted, so closing the window leaves the
/// launcher running with dsh alive; the tray icon restores the window
/// (re-created and re-navigated to the dsh URL) or quits.
#[cfg(target_os = "linux")]
pub mod tray {
    use std::os::raw::{c_char, c_int, c_void};
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, Ordering};

    static STARTED: AtomicBool = AtomicBool::new(false);
    static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);
    static RESTORE_REQUESTED: AtomicBool = AtomicBool::new(false);
    static LIBS: OnceLock<(Option<*mut c_void>, Option<*mut c_void>)> = OnceLock::new();

    type DlsymFn = unsafe extern "C" fn();

    unsafe fn sym(handle: *mut c_void, name: &str) -> Option<DlsymFn> {
        let cname = std::ffi::CString::new(name).ok()?;
        let p = libc::dlsym(handle, cname.as_ptr());
        if p.is_null() {
            None
        } else {
            Some(std::mem::transmute::<*mut c_void, DlsymFn>(p))
        }
    }

    /// The menu callbacks run on the GTK main-loop thread and only flip
    /// atomics; the launcher thread acts on them (same pattern as Windows).
    unsafe extern "C" fn on_restore(_w: *mut c_void, _d: *mut c_void) {
        RESTORE_REQUESTED.store(true, Ordering::SeqCst);
    }
    unsafe extern "C" fn on_quit(_w: *mut c_void, _d: *mut c_void) {
        QUIT_REQUESTED.store(true, Ordering::SeqCst);
    }

    /// Spawn the AppIndicator thread (idempotent).
    pub fn start() {
        if STARTED.swap(true, Ordering::SeqCst) {
            return;
        }
        std::thread::spawn(|| {
            unsafe {
                // SAFETY: dlopen/dlsym with fixed sonames; failures are logged
                // and degrade gracefully (no tray).
                let appind = libc::dlopen(
                    c"libayatana-appindicator3.so.1".as_ptr(),
                    libc::RTLD_NOW | libc::RTLD_LOCAL,
                );
                let gtk =
                    libc::dlopen(c"libgtk-3.so.0".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
                LIBS.get_or_init(|| (appind, gtk));
                let (Some(appind), Some(gtk)) = (appind, gtk) else {
                    crate::debug::emit(
                        "tray: libayatana-appindicator3 / libgtk-3 not available; close-to-tray disabled",
                    );
                    STARTED.store(false, Ordering::SeqCst);
                    return;
                };

                // GTK3 function pointers. Any missing symbol degrades the
                // tray (log + no-op) instead of panicking on a null fn ptr.
                let (Some(gtk_init), Some(gtk_main), Some(gtk_menu_new),
                     Some(gtk_menu_item_new_with_label), Some(gtk_menu_shell_append),
                     Some(gtk_widget_show_all), Some(g_signal_connect),
                     Some(indicator_new), Some(indicator_set_status),
                     Some(indicator_set_menu)) = (
                    sym(gtk, "gtk_init"),
                    sym(gtk, "gtk_main"),
                    sym(gtk, "gtk_menu_new"),
                    sym(gtk, "gtk_menu_item_new_with_label"),
                    sym(gtk, "gtk_menu_shell_append"),
                    sym(gtk, "gtk_widget_show_all"),
                    sym(gtk, "g_signal_connect"),
                    sym(appind, "app_indicator_new"),
                    sym(appind, "app_indicator_set_status"),
                    sym(appind, "app_indicator_set_menu"),
                ) else {
                    crate::debug::emit(
                        "tray: required symbol missing; close-to-tray disabled",
                    );
                    STARTED.store(false, Ordering::SeqCst);
                    return;
                };
                let gtk_init: unsafe extern "C" fn(*mut c_int, *mut *mut *mut c_char) =
                    std::mem::transmute(gtk_init);
                let gtk_main: unsafe extern "C" fn() = std::mem::transmute(gtk_main);
                let gtk_menu_new: unsafe extern "C" fn() -> *mut c_void =
                    std::mem::transmute(gtk_menu_new);
                let gtk_menu_item_new_with_label: unsafe extern "C" fn(
                    *const c_char,
                )
                    -> *mut c_void = std::mem::transmute(gtk_menu_item_new_with_label);
                let gtk_menu_shell_append: unsafe extern "C" fn(*mut c_void, *mut c_void) =
                    std::mem::transmute(gtk_menu_shell_append);
                let gtk_widget_show_all: unsafe extern "C" fn(*mut c_void) =
                    std::mem::transmute(gtk_widget_show_all);
                let g_signal_connect: unsafe extern "C" fn(
                    *mut c_void,
                    *const c_char,
                    unsafe extern "C" fn(*mut c_void, *mut c_void),
                    *mut c_void,
                ) -> u64 = std::mem::transmute(g_signal_connect);
                let indicator_new: unsafe extern "C" fn(
                    *const c_char,
                    *const c_char,
                    c_int,
                ) -> *mut c_void = std::mem::transmute(indicator_new);
                let indicator_set_status: unsafe extern "C" fn(*mut c_void, c_int) =
                    std::mem::transmute(indicator_set_status);
                let indicator_set_menu: unsafe extern "C" fn(*mut c_void, *mut c_void) =
                    std::mem::transmute(indicator_set_menu);

                gtk_init(std::ptr::null_mut(), std::ptr::null_mut());
                let id = c"dshl".as_ptr();
                let icon = c"dsh".as_ptr();
                let indicator = indicator_new(
                    id, icon, 0, /* APP_INDICATOR_CATEGORY_APPLICATION_STATUS */
                );
                if indicator.is_null() {
                    crate::debug::emit("tray: app_indicator_new failed");
                    STARTED.store(false, Ordering::SeqCst);
                    return;
                }

                let menu = gtk_menu_new();
                let restore = c"恢复窗口".as_ptr();
                let quit = c"退出".as_ptr();
                let restore_item = gtk_menu_item_new_with_label(restore);
                let quit_item = gtk_menu_item_new_with_label(quit);
                g_signal_connect(
                    restore_item,
                    c"activate".as_ptr(),
                    on_restore,
                    std::ptr::null_mut(),
                );
                g_signal_connect(
                    quit_item,
                    c"activate".as_ptr(),
                    on_quit,
                    std::ptr::null_mut(),
                );
                gtk_menu_shell_append(menu, restore_item);
                gtk_menu_shell_append(menu, quit_item);
                gtk_widget_show_all(menu);
                indicator_set_menu(indicator, menu);
                indicator_set_status(indicator, 1 /* APP_INDICATOR_STATUS_ACTIVE */);
                crate::debug::emit("tray: active (close-to-tray)");
                gtk_main();
            }
        });
    }

    /// Called when the user closes the window; on Linux the window is already
    /// gone, so this only makes sure the tray exists (dsh keeps running).
    pub fn hide_to_tray() {
        crate::debug::emit("tray: window closed, keeping dsh running in tray");
    }

    pub fn quit_requested() -> bool {
        QUIT_REQUESTED.load(Ordering::SeqCst)
    }

    /// True when the user chose "restore window" from the tray menu.
    pub fn restore_requested() -> bool {
        RESTORE_REQUESTED.swap(false, Ordering::SeqCst)
    }

    /// Stop the tray thread. The GTK main loop is left running; the process
    /// exits right after, so no cleanup is strictly required (dlclose would
    /// race the GTK thread).
    pub fn shutdown() {}
}

/// Tray support stub for macOS: `close-to-tray` is ignored (no tray code
/// path yet), closing keeps the original exit behaviour.
#[cfg(target_os = "macos")]
pub mod tray {
    pub fn start() {}
    pub fn hide_to_tray() {}
    pub fn quit_requested() -> bool {
        false
    }
    pub fn restore_requested() -> bool {
        false
    }
    pub fn shutdown() {}
}
/// Launcher-level single-instance support (`[ui] single-instance`).
///
/// A mutex lock file (`<cache>/dshl/instance.lock`) is held with
/// `File::try_lock` (LockFileEx on Windows, flock on Unix — the kernel
/// releases it automatically when the process dies, so stale locks from a
/// crash are impossible). A second dshl fails to acquire it and instead
/// signals the running instance through an activation file: the running
/// instance then restores its window if it is hidden in the tray, or
/// focuses it if it is visible.
pub mod single_instance {
    use std::fs::{File, OpenOptions};
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn cache_dir() -> PathBuf {
        crate::platform::cache_dir().join("dshl")
    }

    fn lock_path() -> PathBuf {
        cache_dir().join("instance.lock")
    }

    fn activate_path() -> PathBuf {
        cache_dir().join("activate")
    }

    /// Try to become the single running instance. Returns `Some(file)` when
    /// this process owns the lock (the file must be kept alive), `None` when
    /// another dshl already holds it.
    pub fn acquire() -> Option<File> {
        let path = lock_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .ok()?;
        match file.try_lock() {
            Ok(()) => {
                crate::debug::emit("single-instance: lock acquired (first instance)");
                Some(file)
            }
            Err(_) => {
                crate::debug::emit("single-instance: another dshl is running");
                None
            }
        }
    }

    /// Called by the *second* instance: ask the running one to come to the
    /// foreground (restore from tray or focus its window), then exit.
    pub fn notify_activate() {
        // On Windows, the second instance is the one with foreground rights
        // (launched by the user), so it grants the running instance the
        // ability to steal the foreground before signalling it.
        #[cfg(windows)]
        unsafe {
            // SAFETY: user32 call with the ASFW_ANY constant; best-effort.
            let _ = allow_set_foreground_window(0xFFFF_FFFF); // ASFW_ANY
        }
        let path = activate_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            let _ = writeln!(f, "{ts}");
        }
    }

    /// Called periodically by the *first* instance. Returns `true` once when
    /// a second instance asked for activation (file grew since last check).
    pub fn poll_activate() -> bool {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::OnceLock;
        static LAST_LEN: OnceLock<AtomicU64> = OnceLock::new();
        let path = activate_path();
        // Initialise the baseline with the CURRENT file length so a leftover
        // activate file from a previous run does not trigger a spurious
        // activation on the first poll (which would focus/restore the window
        // for no reason at every startup).
        let last = LAST_LEN.get_or_init(|| {
            let len = std::fs::metadata(&path)
                .map(|m| m.len())
                .unwrap_or(0);
            AtomicU64::new(len)
        });
        let len = std::fs::metadata(&path)
            .map(|m| m.len())
            .unwrap_or(0);
        let prev = last.swap(len, Ordering::SeqCst);
        len > prev
    }

    #[cfg(windows)]
    #[link(name = "user32")]
    unsafe extern "system" {
        fn AllowSetForegroundWindow(pid: u32) -> i32;
    }
    #[cfg(windows)]
    unsafe fn allow_set_foreground_window(pid: u32) -> i32 {
        // SAFETY: FFI call, best-effort foreground grant.
        unsafe { AllowSetForegroundWindow(pid) }
    }
}

/// Current screen size in physical pixels, or `(0, 0)` when unknown.
///
/// Windows: `GetSystemMetrics` (primary monitor). Linux: the X11 display
/// size via `dlopen`-ed libX11 (runtime-only dependency, works under XWayland
/// too; falls back to `(0, 0)` when unavailable). Other platforms return
/// `(0, 0)` — callers then fall back to the webui hard limits.
pub fn screen_size() -> (u32, u32) {
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn GetSystemMetrics(index: i32) -> i32;
        }
        const SM_CXSCREEN: i32 = 0;
        const SM_CYSCREEN: i32 = 1;
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
/// Bring an existing window to the foreground (single-instance activation).
/// Windows: SetForegroundWindow. Other platforms: no portable window focus
/// API is available through webui, so this is a no-op (tray restore still
/// re-creates the window).
pub fn focus_window(hwnd: usize) {
    #[cfg(windows)]
    {
        use std::os::raw::c_void;
        #[link(name = "user32")]
        unsafe extern "system" {
            fn SetForegroundWindow(hwnd: *mut c_void) -> i32;
            fn BringWindowToTop(hwnd: *mut c_void) -> i32;
            fn IsIconic(hwnd: *mut c_void) -> i32;
            fn ShowWindow(hwnd: *mut c_void, cmd: i32) -> i32;
            fn GetForegroundWindow() -> *mut c_void;
            fn GetWindowThreadProcessId(hwnd: *mut c_void, pid: *mut u32) -> u32;
            fn GetCurrentThreadId() -> u32;
            fn AttachThreadInput(id_attach: u32, id_attach_to: u32, attach: i32) -> i32;
            // Undocumented but stable since Win95: the internal ALT+TAB
            // switch, which bypasses the foreground lock entirely. Used as
            // the last resort when SetForegroundWindow is denied (e.g. the
            // activating second instance has exited and revoked its
            // AllowSetForegroundWindow grant).
            fn SwitchToThisWindow(hwnd: *mut c_void, restore: i32);
            fn keybd_event(vk: u8, scan: u8, flags: u32, info: usize);
        }
        const SW_RESTORE: i32 = 9;
        const TRUE: i32 = 1;
        const FALSE: i32 = 0;
        const VK_MENU: u8 = 0x12; // Alt
        const KEYEVENTF_EXTENDEDKEY: u32 = 0x1;
        const KEYEVENTF_KEYUP: u32 = 0x2;
        // SAFETY: hwnd is a live top-level window handle from webui; all calls
        // are best-effort foreground manipulation.
        unsafe {
            // Restore a minimized window first, then try the normal path.
            if IsIconic(hwnd as *mut c_void) != 0 {
                ShowWindow(hwnd as *mut c_void, SW_RESTORE);
            }
            if SetForegroundWindow(hwnd as *mut c_void) != 0 {
                crate::debug::emit(&format!("focus_window: hwnd {hwnd:#x}"));
                return;
            }
            // Foreground-lock fallback 1: attaching our input queue to the
            // current foreground thread makes SetForegroundWindow succeed
            // even when we are not the foreground process.
            let fg = GetForegroundWindow();
            let fg_tid = GetWindowThreadProcessId(fg, std::ptr::null_mut());
            let my_tid = GetCurrentThreadId();
            if fg_tid != 0 && fg_tid != my_tid {
                AttachThreadInput(my_tid, fg_tid, TRUE);
                BringWindowToTop(hwnd as *mut c_void);
                SetForegroundWindow(hwnd as *mut c_void);
                AttachThreadInput(my_tid, fg_tid, FALSE);
            }
            // Fallback 2: synthesize an Alt key press. Windows grants the
            // foreground to a process right after it receives input, so a
            // tiny fake Alt tap (the classic trick) defeats the foreground
            // lock that plain SetForegroundWindow hits for windows created
            // long after startup (e.g. a tray-restored window).
            keybd_event(VK_MENU, 0, KEYEVENTF_EXTENDEDKEY, 0);
            keybd_event(VK_MENU, 0, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP, 0);
            SetForegroundWindow(hwnd as *mut c_void);
            // Fallback 3: SwitchToThisWindow ignores the foreground lock
            // (it is what ALT+TAB uses), so a window that was re-created by
            // a tray restore can still be brought to the front reliably.
            SwitchToThisWindow(hwnd as *mut c_void, TRUE);
        }
        crate::debug::emit(&format!("focus_window: hwnd {hwnd:#x} (forced)"));
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        crate::debug::emit("focus_window: not supported on this platform");
    }
}
/// Detect a running dsh instance anywhere on the system (launched manually
/// or supervised by another dshl), returning its pid. Used by the optional
/// `single-instance` mode to refuse starting a second dsh: two processes
/// appending to the same session log corrupt it permanently ("seq gap").
///
/// Matches the bun-compiled `dsh` binary by process name and the node
/// entry (`@deepseek-ai/dsh/lib/bin.js`) by command line, so it covers both
/// `dsh --profile web …` and a manual `dsh web` invocation.
pub fn dsh_instance_running() -> Option<u32> {
    #[cfg(windows)]
    {
        let script = concat!(
            "(Get-CimInstance Win32_Process | Where-Object { ",
            "$_.Name -eq 'dsh.exe' -or ",
            "($_.Name -eq 'node.exe' -and $_.CommandLine -match '@deepseek-ai[\\/]dsh[\\/]lib[\\/]bin\\.js') ",
            "} | Select-Object -First 1).ProcessId"
        );
        let mut cmd = Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", script]);
        crate::process::run(&mut cmd)
            .ok()
            .and_then(|res| res.stdout.trim().parse::<u32>().ok())
    }
    #[cfg(unix)]
    {
        // pgrep -x matches the compiled `dsh` binary by exact process name;
        // pgrep -f covers the node entry (`…/dsh/lib/bin.js`).
        let mut cmd = Command::new("pgrep");
        cmd.args(["-x", "dsh"]);
        let direct = crate::process::run(&mut cmd).ok().and_then(|res| {
            res.stdout
                .lines()
                .next()
                .and_then(|l| l.trim().parse::<u32>().ok())
        });
        if direct.is_some() {
            return direct;
        }
        let mut cmd = Command::new("pgrep");
        cmd.args(["-f", "dsh/lib/bin.js"]);
        crate::process::run(&mut cmd).ok().and_then(|res| {
            res.stdout
                .lines()
                .next()
                .and_then(|l| l.trim().parse::<u32>().ok())
        })
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
    #[cfg(windows)]
    {
        use std::os::raw::c_void;

        #[link(name = "user32")]
        unsafe extern "system" {
            fn FindWindowExW(
                parent: *mut c_void,
                after: *mut c_void,
                class: *const u16,
                title: *const u16,
            ) -> *mut c_void;
            fn GetWindowThreadProcessId(hwnd: *mut c_void, pid: *mut u32) -> u32;
            fn IsWindowVisible(hwnd: *mut c_void) -> i32;
        }

        // SAFETY: FindWindowExW enumerates top-level windows; the pid and
        // visibility are read per window. Same approach webui.c uses to
        // locate its browser window, plus a visibility filter.
        unsafe {
            let mut hwnd: *mut c_void = std::ptr::null_mut();
            loop {
                hwnd = FindWindowExW(
                    std::ptr::null_mut(),
                    hwnd,
                    std::ptr::null(),
                    std::ptr::null(),
                );
                if hwnd.is_null() {
                    return None;
                }
                let mut win_pid: u32 = 0;
                GetWindowThreadProcessId(hwnd, &mut win_pid);
                if win_pid == pid && IsWindowVisible(hwnd) != 0 {
                    return Some(hwnd as usize);
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        None
    }
}
