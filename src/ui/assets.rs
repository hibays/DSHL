//! Startup UI assets embedded at compile time.

pub const INDEX_HTML: &str = include_str!("../../assets/index.html");
pub const STYLES_CSS: &str = include_str!("../../assets/styles.css");
pub const APP_JS: &str = include_str!("../../assets/app.js");
/// Page logo / favicon. `dsh-black.svg` renders black by default and flips to
/// white inside a `prefers-color-scheme: dark` context (WebView2 / browsers),
/// so one file serves both themes.
pub const LOGO_SVG: &str = include_str!("../../assets/dsh-black.svg");

/// Self-hosted display/UI face (Inter, Latin) so the launcher page keeps its
/// Swiss-grotesque voice offline without depending on system fonts. Served
/// from `/fonts/...` by the vfs handler.
pub const INTER_400: &[u8] = include_bytes!("../../assets/fonts/inter-latin-400-normal.woff2");
pub const INTER_500: &[u8] = include_bytes!("../../assets/fonts/inter-latin-500-normal.woff2");
pub const INTER_600: &[u8] = include_bytes!("../../assets/fonts/inter-latin-600-normal.woff2");
pub const INTER_700: &[u8] = include_bytes!("../../assets/fonts/inter-latin-700-normal.woff2");
