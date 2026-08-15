//! A minimal WebSocket keep-alive client.
//!
//! webui closes an embedded WebView ~1.5s after its bridge disconnects
//! (`WEBUI_RELOAD_TIMEOUT`), which happens whenever the WebView navigates to
//! an external URL (dsh). The launcher holds a raw WebSocket to its own webui
//! server so `clients_count` stays above zero, keeping the window open while
//! dsh is shown. Requires `multi_client` and `use_cookies` disabled (see
//! [`crate::ui::setup`]).

use base64::Engine;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Open a WebSocket to the launcher's own webui server on `port` and hold it
/// open (reading/discarding frames) until the process exits.
pub fn spawn(port: u16) {
    std::thread::spawn(move || {
        let addr = format!("127.0.0.1:{port}");
        crate::debug::emit(&format!("keep-alive: connecting to {addr}"));
        let Ok(mut stream) = TcpStream::connect(&addr) else {
            crate::debug::emit("keep-alive: connect failed");
            return;
        };
        crate::debug::emit("keep-alive: connected, sending handshake");

        // A well-formed 16-byte key (base64) is all the server requires.
        let key_bytes = [
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
            0xff, 0x00,
        ];
        let key = base64::engine::general_purpose::STANDARD.encode(key_bytes);
        let req = format!(
            "GET /_webui_ws_connect HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        if stream.write_all(req.as_bytes()).is_err() {
            return;
        }

        // Read the upgrade response headers.
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut buf = [0u8; 4096];
        let mut response = Vec::new();
        while !response.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => {
                    crate::debug::emit(&format!(
                        "keep-alive: no/incomplete response ({} bytes): {}",
                        response.len(),
                        String::from_utf8_lossy(&response)
                            .replace("\r", "\\r")
                            .replace("\n", "\\n")
                    ));
                    return;
                }
                Ok(n) => response.extend_from_slice(&buf[..n]),
            }
            if response.len() > 16_384 {
                return;
            }
        }
        crate::debug::emit(&format!(
            "keep-alive: response head: {}",
            String::from_utf8_lossy(&response[..response.len().min(120)])
        ));
        if !response.starts_with(b"HTTP/1.1 101") {
            crate::debug::emit("keep-alive: handshake rejected");
            return;
        }

        crate::debug::emit(&format!("keep-alive websocket established (port {port})"));

        // Clear the handshake read timeout; hold the connection open
        // indefinitely and read/discard any frames.
        let _ = stream.set_read_timeout(None);
        loop {
            let mut buf = [0u8; 1024];
            match stream.read(&mut buf) {
                Ok(0) => {
                    crate::debug::emit("keep-alive: server closed the connection");
                    return;
                }
                Err(e) => {
                    crate::debug::emit(&format!("keep-alive: read error: {e}"));
                    return;
                }
                Ok(_) => {}
            }
        }
    });
}
