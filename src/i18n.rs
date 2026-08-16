//! i18n: system-language detection and the startup locale init.
//!
//! rust-i18n embeds the translations at compile time; the only runtime
//! decision is which locale to use. We follow the OS UI language: `zh-CN`
//! when the system speaks a Chinese dialect, `en` otherwise (the fallback
//! configured in [`crate::i18n!`] keeps any missing key in Chinese).

use std::sync::OnceLock;

/// The locale selected at startup (`"zh-CN"` or `"en"`).
static LOCALE: OnceLock<&'static str> = OnceLock::new();

/// Detect the OS UI language and set the global rust-i18n locale. Must be
/// called once at startup, before any `t!` is rendered (it is: from `main`
/// before the UI is set up).
pub fn init() {
    let locale = detect();
    let _ = LOCALE.set(locale);
    rust_i18n::set_locale(locale);
}

/// The locale currently in use (`"zh-CN"` or `"en"`).
pub fn locale() -> &'static str {
    LOCALE.get_or_init(detect)
}

/// Map the OS UI language to one of the shipped locales.
fn detect() -> &'static str {
    let sys = sys_locale::get_locale().unwrap_or_default();
    let lang = sys
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match lang.as_str() {
        "zh" | "cmn" => "zh-CN",
        // Every other language falls back to English.
        _ => "en",
    }
}
