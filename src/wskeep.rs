//! A minimal WebSocket keep-alive client.
//!
//! webui keeps a window's server alive only while at least one client stays
//! connected. In WebView mode, navigating the window to dsh disconnects the
//! embedded WebView's bridge from webui's server — and without a client webui
//! stops the server ~1.5s later (`WEBUI_RELOAD_TIMEOUT`) and closes the
//! WebView. The launcher holds a raw WebSocket to its own webui server so
//! `clients_count` stays above zero, keeping the window open while dsh is
//! shown. Requires `multi_client` and `use_cookies` disabled (see
//! [`crate::ui::setup`]).
//!
//! Browser mode needs no keep-alive: webui's server-timeout path tries to
//! terminate the *external* browser process, but that lookup (`wmic` /
//! PowerShell command-line match) is best-effort and does not fire on modern
//! Windows/Edge, so the browser stays open on its own and the launcher tracks
//! its process directly instead.

use base64::Engine;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Handle to a keep-alive WebSocket thread.
///
/// The thread holds the `Arc`, so dropping the handle does not stop it (it
/// lives for the process lifetime, as intended). Call [`KeepAlive::stop`] to
/// close the connection — e.g. when the window closes to the tray, so the old
/// window's webui server can shut down instead of staying alive on the
/// keep-alive client.
pub struct KeepAlive {
    stop: Arc<AtomicBool>,
}

impl KeepAlive {
    /// Ask the keep-alive thread to close its WebSocket connection. The
    /// thread notices on its next poll (≤200ms) and drops the socket.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Open a WebSocket to the launcher's own webui server on `port` and hold it
/// open (reading/discarding frames) until the process exits or the returned
/// handle is stopped.
pub fn spawn(port: u16) -> KeepAlive {
    let keepalive = KeepAlive {
        stop: Arc::new(AtomicBool::new(false)),
    };
    let stop = keepalive.stop.clone();
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

        // Poll with a short read timeout so a stop request is noticed quickly
        // and the socket is dropped, letting the old window's webui server
        // shut down (it keeps running while any client is connected).
        let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
        loop {
            if stop.load(Ordering::SeqCst) {
                crate::debug::emit(&format!("keep-alive: stop requested (port {port})"));
                return;
            }
            match stream.read(&mut buf) {
                Ok(0) => {
                    crate::debug::emit("keep-alive: server closed the connection");
                    return;
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => {
                    crate::debug::emit(&format!("keep-alive: read error: {e}"));
                    return;
                }
                Ok(_) => {}
            }
        }
    });
    keepalive
}
