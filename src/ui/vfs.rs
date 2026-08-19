//! The webui virtual-file handler that serves the embedded startup page.

use std::ffi::CStr;
use std::os::raw::{c_char, c_void};

use webui::webui;

use super::assets;

/// The page-facing translation keys the frontend reads via `tr()` and
/// `data-i18n`. Single source of truth is the locale YAML; this lists the
/// subset the JS bundle uses so `/i18n.js` stays small.
const PAGE_KEYS: [&str; 23] = [
    "page.title",
    "page.heading.steps",
    "page.heading.config",
    "page.heading.log",
    "page.btn.restart_now",
    "page.btn.cancel",
    "page.btn.open_dsh",
    "page.btn.force_kill",
    "page.btn.retry",
    "page.btn.open_config",
    "page.btn.exit",
    "page.status.pending",
    "page.status.running",
    "page.status.done",
    "page.status.error",
    "page.status.skipped",
    "page.badge.starting",
    "page.badge.started",
    "page.badge.failed",
    "page.badge.crash",
    "page.crash.message",
    "page.config_error_prefix",
    "page.empty_value",
];

/// Escape a string as a double-quoted JS string literal.
fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Build the `/i18n.js` payload: the active locale plus every page-facing
/// translation, exposed as `window.DSHL_LOCALE` / `window.DSHL_I18N`.
fn i18n_js() -> String {
    let mut js = format!(
        "window.DSHL_LOCALE = {};\n",
        js_string(crate::i18n::locale())
    );
    js.push_str("window.DSHL_I18N = {\n");
    for key in PAGE_KEYS {
        js.push_str(&format!("  {}: {},\n", js_string(key), js_string(&t!(key))));
    }
    js.push_str("};\n");
    js
}

/// Wrap a body in an HTTP response so webui serves it with the right MIME.
fn http_response_bytes(body: &[u8], mime: &str) -> Vec<u8> {
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let mut out = head.into_bytes();
    out.extend_from_slice(body);
    out
}

/// Allocate a WebUI-owned response buffer, copying `bytes` in. Mirrors
/// `webui::malloc` but works for binary payloads that aren't valid UTF-8
/// (e.g. webfonts).
///
/// # Safety
/// `length` must be either null or a valid pointer to a writable `i32`.
unsafe fn malloc_bytes(bytes: &[u8], length: *mut i32) -> *const c_void {
    // SAFETY: webui_malloc returns a fresh allocation owned by WebUI.
    let buf = unsafe { webui::bindgen::webui_malloc(bytes.len() + 1) } as *mut u8;
    // SAFETY: buf is a fresh webui_malloc allocation of bytes.len()+1 bytes.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len()) };
    // SAFETY: the NUL terminator fits inside the allocation.
    unsafe { *buf.add(bytes.len()) = 0 };
    if !length.is_null() {
        // SAFETY: length is a valid writable i32 pointer per the API.
        unsafe { *length = bytes.len() as i32 };
    }
    buf as *const c_void
}

/// Serve the embedded startup assets from memory.
pub(crate) unsafe extern "C" fn vfs(filename: *const c_char, length: *mut i32) -> *const c_void {
    // SAFETY: webui passes a valid NUL-terminated path string.
    let name = unsafe { CStr::from_ptr(filename) }.to_str().unwrap_or("");
    let path = name.split('?').next().unwrap_or("");

    let response = match path {
        "/" | "/index.html" | "index.html" => {
            http_response_bytes(assets::INDEX_HTML.as_bytes(), "text/html")
        }
        "/styles.css" | "styles.css" => {
            http_response_bytes(assets::STYLES_CSS.as_bytes(), "text/css")
        }
        "/app.js" | "app.js" => {
            http_response_bytes(assets::APP_JS.as_bytes(), "application/javascript")
        }
        "/i18n.js" | "i18n.js" => {
            http_response_bytes(i18n_js().as_bytes(), "application/javascript")
        }
        // Theme-aware mark: black by default, white in dark mode.
        "/dsh-black.svg" | "dsh-black.svg" => {
            http_response_bytes(assets::LOGO_SVG.as_bytes(), "image/svg+xml")
        }
        // Self-hosted UI face (see assets.rs).
        "/fonts/inter-latin-400-normal.woff2" | "inter-latin-400-normal.woff2" => {
            http_response_bytes(assets::INTER_400, "font/woff2")
        }
        "/fonts/inter-latin-500-normal.woff2" | "inter-latin-500-normal.woff2" => {
            http_response_bytes(assets::INTER_500, "font/woff2")
        }
        "/fonts/inter-latin-600-normal.woff2" | "inter-latin-600-normal.woff2" => {
            http_response_bytes(assets::INTER_600, "font/woff2")
        }
        "/fonts/inter-latin-700-normal.woff2" | "inter-latin-700-normal.woff2" => {
            http_response_bytes(assets::INTER_700, "font/woff2")
        }
        _ => return std::ptr::null(),
    };

    // Memory protocol (webui.c `_webui_external_file_handler`): the
    // returned buffer MUST come from `webui_malloc` — webui sends it to
    // the client (`mg_write`) and then releases it itself
    // (`_webui_free_mem`, which only frees pointers it allocated). The
    // Rust-side `Vec` is dropped right here, so nothing leaks. Do NOT
    // "optimise" this into `Box::into_raw`/`malloc`: webui would not
    // recognise the pointer (leak) or we would double-free it.
    // SAFETY: length is either null or a valid i32 pointer per the API.
    unsafe { malloc_bytes(&response, length) }
}
