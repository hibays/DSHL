//! Window geometry persistence — the SINGLE store shared by the embedded
//! WebView window and the external browser window.
//!
//! One `window-state.json` (under `<cache>/dshl/`) holds `{x, y, width,
//! height}` in PHYSICAL pixels for whatever window is currently fronting the
//! launcher. There are deliberately no per-mode entries: switching between
//! WebView and browser (startup fallback or tray restore) keeps using the same
//! geometry, so the user's layout follows the launcher, not the backend.
//!
//! Recording points:
//! * WebView close handler → [`remember_webview`] (the moment the window
//!   closes — see `window::on_webview_close`).
//! * External browser → a 1 Hz sampler in `window::track_browser_geometry`
//!   while it runs, plus a final one-shot [`remember_by_pid`] during shutdown
//!   (`exit::shutdown`) because webui gives us no browser close hook.
//!
//! All values pass through [`clamp`] before being handed to webui: webui's C
//! core *silently drops* sizes outside `100..=3840 × 100..=2160` and
//! positions outside `0..=3000 / 0..=1800` (webui.c `WEBUI_MAX_*`), which used
//! to make restored windows keep default geometry with no diagnostic.

use std::path::PathBuf;

use webui::webui;

use crate::platform::window_rect;

/// Persisted launcher-window geometry, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Geometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

// webui.c hard limits — values outside are silently refused by
// webui_set_size / webui_set_position.
const WEBUI_MAX_WIDTH: u32 = 3840;
const WEBUI_MAX_HEIGHT: u32 = 2160;
const WEBUI_MAX_X: i32 = 3000;
const WEBUI_MAX_Y: i32 = 1800;
const WEBUI_MIN_WIDTH: u32 = 100;
const WEBUI_MIN_HEIGHT: u32 = 100;

fn state_path() -> PathBuf {
    crate::platform::cache_dir()
        .join("dshl")
        .join("window-state.json")
}

/// Load the saved geometry. Rejects corrupt/hand-edited degenerate values that
/// would make webui create an absurd window at startup.
pub(super) fn load() -> Option<Geometry> {
    let text = std::fs::read_to_string(state_path()).ok()?;
    let g: Geometry = serde_json::from_str(&text).ok()?;
    if g.width < 200 || g.width > 10_000 || g.height < 150 || g.height > 10_000 {
        return None;
    }
    Some(g)
}

/// Clamp a saved geometry to something sane for the current screen AND inside
/// webui's hard acceptance ranges. Values outside those ranges are not
/// "adjusted" by webui — they are dropped entirely and the window opens at its
/// default geometry, so clamping here is what makes restoration actually work.
///
/// Returns `(width, height, x, y)` in physical pixels, ready for
/// `Window::set_size` / `set_position`.
pub(super) fn clamp(g: &Geometry) -> (u32, u32, u32, u32) {
    let (sw, sh) = crate::platform::screen_size();
    let max_w = if sw > 0 {
        sw.min(WEBUI_MAX_WIDTH)
    } else {
        WEBUI_MAX_WIDTH
    };
    let max_h = if sh > 0 {
        sh.min(WEBUI_MAX_HEIGHT)
    } else {
        WEBUI_MAX_HEIGHT
    };
    let w = g.width.clamp(WEBUI_MIN_WIDTH.max(200), max_w);
    let h = g.height.clamp(WEBUI_MIN_HEIGHT.max(150), max_h);
    // Keep the top-left corner on screen, and inside webui's position caps:
    // x/y beyond MAX_X/MAX_Y would be silently discarded wholesale.
    let x_max = (max_w as i32 - w as i32).clamp(0, WEBUI_MAX_X);
    let y_max = (max_h as i32 - h as i32).clamp(0, WEBUI_MAX_Y);
    // webui takes unsigned positions; negative (secondary monitor to the left)
    // collapses to the primary screen's origin.
    let x = g.x.clamp(0, x_max) as u32;
    let y = g.y.clamp(0, y_max) as u32;
    (w, h, x, y)
}

/// Apply the saved geometry to a not-yet-shown webui window.
///
/// The stored values are physical pixels (captured by this DPI-aware process),
/// which WebView windows take directly; external browsers interpret
/// `--window-size/--window-position` in logical pixels (DIPs), so when the
/// window is destined for a browser the values are divided by the DPI scale
/// first. Call BEFORE `show()` / `show_wv()` — webui bakes `win->width/height/
/// x/y` into the WebView2 creation and the browser command line.
pub(super) fn apply(window: &webui::Window, to_browser: bool) {
    let Some(g) = load() else {
        return;
    };
    let (w, h, x, y) = clamp(&g);
    let scale = crate::platform::dpi_scale();
    if to_browser && scale > 0.0 {
        window.set_size(
            (w as f64 / scale).round() as u32,
            (h as f64 / scale).round() as u32,
        );
        window.set_position(
            (x as f64 / scale).round() as u32,
            (y as f64 / scale).round() as u32,
        );
    } else {
        window.set_size(w, h);
        window.set_position(x, y);
    }
}

/// Capture the current rect of a live HWND and persist it. Skips maximized /
/// fullscreen states (never restored) and degenerate sizes. Returns whether a
/// value was written.
pub(super) fn remember_by_hwnd(hwnd: usize) -> bool {
    if hwnd == 0 {
        return false;
    }
    let Some(rect) = window_rect(hwnd) else {
        crate::debug::emit("geometry: window_rect returned None");
        return false;
    };
    crate::debug::emit(&format!(
        "geometry: captured {}x{} @ ({},{}) maximized={}",
        rect.width, rect.height, rect.x, rect.y, rect.maximized
    ));
    if rect.maximized {
        return false;
    }
    persist(rect.x, rect.y, rect.width, rect.height)
}

/// WebView close path: resolve the window id to its HWND and persist.
pub(super) fn remember_webview(window_id: usize) -> bool {
    let hwnd = webui::Window::from_id(window_id).get_hwnd() as usize;
    crate::debug::emit(&format!("geometry: webview hwnd {hwnd:#x}"));
    remember_by_hwnd(hwnd)
}

/// Write the persisted geometry (skipping degenerate values).
pub(super) fn persist(x: i32, y: i32, width: u32, height: u32) -> bool {
    // Same acceptance window `load()` enforces: never let an out-of-range
    // sample (broken HWND rect, DPI glitch) poison the store.
    if !(200..=10_000).contains(&width) || !(150..=10_000).contains(&height) {
        return false;
    }
    let json = match serde_json::to_string_pretty(&Geometry {
        x,
        y,
        width,
        height,
    }) {
        Ok(j) => j,
        Err(_) => return false,
    };
    let path = state_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&path, json) {
        Ok(()) => {
            crate::debug::emit(&format!("geometry: wrote {}", path.display()));
            true
        }
        Err(e) => {
            crate::debug::emit(&format!("geometry: write failed: {e}"));
            false
        }
    }
}

/// Browser close path (one-shot): find the browser's visible top-level window
/// by pid and persist its rect. Used at shutdown because webui has no
/// browser-side close hook; the running sampler already persists continuously.
pub(super) fn remember_by_pid(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    match crate::platform::find_hwnd_by_pid(pid) {
        Some(hwnd) => remember_by_hwnd(hwnd),
        None => {
            crate::debug::emit(&format!("geometry: no browser window found for pid {pid}"));
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialise env mutation across the test binary: DSHL_CACHE is
    /// process-global and other tests may read cache_dir concurrently.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_sandbox_cache(f: impl FnOnce()) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!("dshl-geom-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let prev = std::env::var_os("DSHL_CACHE");
        // Edition 2024 marks env mutation unsafe (process-global state); we
        // serialise through ENV_LOCK, so no concurrent observer is possible.
        unsafe { std::env::set_var("DSHL_CACHE", &dir) };
        f();
        match prev {
            Some(v) => unsafe { std::env::set_var("DSHL_CACHE", v) },
            None => unsafe { std::env::remove_var("DSHL_CACHE") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn persist_load_roundtrip_is_lossless() {
        with_sandbox_cache(|| {
            assert!(load().is_none(), "sandbox starts empty");
            assert!(persist(120, 80, 1280, 960));
            assert_eq!(
                load(),
                Some(Geometry {
                    x: 120,
                    y: 80,
                    width: 1280,
                    height: 960
                })
            );
        });
    }

    #[test]
    fn degenerate_sizes_are_never_persisted_or_loaded() {
        with_sandbox_cache(|| {
            assert!(!persist(0, 0, 100, 50), "sub-minimum must be skipped");
            assert!(!persist(0, 0, 20_000, 20_000));
            // Hand-edited garbage on disk is rejected on load too.
            let p = state_path();
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            std::fs::write(&p, r#"{"x":0,"y":0,"width":50,"height":50}"#).unwrap();
            assert_eq!(load(), None);
        });
    }

    #[test]
    fn clamp_keeps_geometry_inside_webui_hard_limits() {
        // Whatever the host screen size is, nothing may exceed the ranges
        // webui silently enforces — that was the "restore does nothing" bug.
        let cases = [
            Geometry {
                x: 5000,
                y: 4000,
                width: 8000,
                height: 8000,
            },
            Geometry {
                x: -900,
                y: -900,
                width: 1280,
                height: 960,
            },
            Geometry {
                x: 2990,
                y: 1795,
                width: 3840,
                height: 2160,
            },
        ];
        for g in cases {
            let (w, h, x, y) = clamp(&g);
            assert!((WEBUI_MIN_WIDTH..=WEBUI_MAX_WIDTH).contains(&w), "w={w}");
            assert!((WEBUI_MIN_HEIGHT..=WEBUI_MAX_HEIGHT).contains(&h), "h={h}");
            assert!(x <= WEBUI_MAX_X as u32, "x={x} exceeds webui cap");
            assert!(y <= WEBUI_MAX_Y as u32, "y={y} exceeds webui cap");
        }
    }
}
