//! The webui virtual-file handler that serves the embedded startup page.

use std::ffi::CStr;
use std::os::raw::{c_char, c_void};

use webui::webui;

use super::assets;

/// Serve the embedded startup assets from memory.
pub(crate) unsafe extern "C" fn vfs(filename: *const c_char, length: *mut i32) -> *const c_void {
    // SAFETY: webui passes a valid NUL-terminated path string.
    let name = unsafe { CStr::from_ptr(filename) }.to_str().unwrap_or("");
    let path = name.split('?').next().unwrap_or("");

    let content: Option<(&str, &str)> = match path {
        "/" | "/index.html" | "index.html" => Some((assets::INDEX_HTML, "text/html")),
        "/styles.css" | "styles.css" => Some((assets::STYLES_CSS, "text/css")),
        "/app.js" | "app.js" => Some((assets::APP_JS, "application/javascript")),
        // Theme-aware mark: black by default, white in dark mode.
        "/dsh-black.svg" | "dsh-black.svg" => Some((assets::LOGO_SVG, "image/svg+xml")),
        _ => None,
    };

    if let Some((body, mime)) = content {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        // Memory protocol (webui.c `_webui_external_file_handler`): the
        // returned buffer MUST come from `webui_malloc` — webui sends it to
        // the client (`mg_write`) and then releases it itself
        // (`_webui_free_mem`, which only frees pointers it allocated). The
        // Rust-side `String` is dropped right here, so nothing leaks. Do NOT
        // "optimise" this into `Box::into_raw`/`malloc`: webui would not
        // recognise the pointer (leak) or we would double-free it.
        // SAFETY: length is either null or a valid i32 pointer per the API.
        return unsafe { webui::malloc(&response, length) };
    }

    // Let webui serve unknown requests (webui.js itself).
    std::ptr::null()
}
