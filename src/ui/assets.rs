//! Startup UI assets embedded at compile time.

pub const INDEX_HTML: &str = include_str!("../../assets/index.html");
pub const STYLES_CSS: &str = include_str!("../../assets/styles.css");
pub const APP_JS: &str = include_str!("../../assets/app.js");
/// Page logo / favicon. `dsh-black.svg` renders black by default and flips to
/// white inside a `prefers-color-scheme: dark` context (WebView2 / browsers),
/// so one file serves both themes.
pub const LOGO_SVG: &str = include_str!("../../assets/dsh-black.svg");
