//! Cross-platform embedded PTY service + standalone WebSocket server.
//!
//! This module powers the "browser is just a terminal renderer" model: the
//! Rust side owns the PTY master/slave pair (via `portable-pty` — ConPTY on
//! Windows, `openpty`/`forkpty` on macOS/Linux), injects `PATH`, `DSH_HOME`,
//! per-call env overrides and the cwd, then exposes each session over a small
//! standalone WebSocket listener that the xterm.js UI connects to directly.
//!
//! The WebSocket server is intentionally self-hosted: we bind `127.0.0.1:0`
//! and let the OS hand us a random port, so we never need to ask the Cordis
//! / dsh `webServer` to expose WS upgrades. A single 64-char hex token is
//! minted when the server starts and must be present on the query string,
//! which keeps cross-tab / cross-origin attackers from opening sessions.
//!
//! Public surface (callers – mostly the dshl-native napi crate):
//!
//! * [`spawn`] – create a new PTY + shell, return the session id, pid and the
//!   pre-signed `ws://127.0.0.1:<port>/_pty/<id>?token=...` URL.
//! * [`list`] – snapshot of running sessions.
//! * [`resize`] – send a new (cols, rows) to the PTY master (triggers
//!   `SIGWINCH` on Unix or a ConPTY resize buffer reflow on Windows).
//! * [`write`] – write bytes directly to the PTY master (used by non-WS
//!   control-plane callers).
//! * [`kill`] – terminate the child (SIGTERM / TerminateProcess) + drop the
//!   session from the registry.
//! * [`server_endpoint`] – if the WS server is running, returns its
//!   `{ host, port, token }` so JS can build ws URLs for sessions that are
//!   *about* to be spawned (e.g. for a session picker UI).

mod server;
mod session;
mod types;

pub use server::server_endpoint;
pub use session::{kill, list, resize, spawn, write};
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_token_is_64_hex_chars() {
        let token = server::random_token_pub();
        assert_eq!(
            token.len(),
            64,
            "expected 64 hex chars, got {}",
            token.len()
        );
        assert!(
            token.chars().all(|c| c.is_ascii_hexdigit()),
            "token contains non-hex chars: {token}"
        );
        assert!(
            !token.contains('-'),
            "token should not contain hyphens: {token}"
        );
    }

    #[test]
    fn random_token_is_unique() {
        let a = server::random_token_pub();
        let b = server::random_token_pub();
        assert_ne!(a, b, "two consecutive tokens should differ");
    }

    #[test]
    fn url_query_get_finds_token() {
        let q = "token=abc123&foo=bar";
        assert_eq!(server::url_query_get_pub(q, "token"), Some("abc123"));
        assert_eq!(server::url_query_get_pub(q, "foo"), Some("bar"));
        assert_eq!(server::url_query_get_pub(q, "missing"), None);
    }

    #[test]
    fn url_query_get_empty_value() {
        assert_eq!(server::url_query_get_pub("token=", "token"), Some(""));
        assert_eq!(server::url_query_get_pub("token", "token"), Some(""));
    }
}
