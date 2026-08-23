//! Standalone WebSocket server for PTY sessions.
//!
//! Binds `127.0.0.1:0` (random port), mints a 256-bit bearer token, and
//! accepts WebSocket upgrades at `/_pty/<id>?token=...`. The server runs
//! on its own thread + tokio current-thread runtime so it works even when
//! the caller (napi-rs on the Node event loop) has no tokio context.

use std::{
    collections::HashMap,
    env, io,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use futures_util::{sink::SinkExt, stream::StreamExt};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::broadcast,
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, handshake::derive_accept_key, protocol::Role},
};

use super::session::{SESSIONS, Session};
use super::types::*;

// ---------------------------------------------------------------------------
// Server state (global cache)
// ---------------------------------------------------------------------------

pub(super) struct WsServerState {
    pub addr: SocketAddr,
    pub token: String,
}

/// Booted server endpoint, cached across spawns. Boot errors are deliberately
/// NOT cached: a transient bind failure must not permanently poison this slot
/// (a `OnceLock` that panics inside `get_or_init` stays poisoned for the
/// process lifetime) — the next `spawn()` simply retries the boot.
static WS_SERVER: Mutex<Option<Arc<WsServerState>>> = Mutex::new(None);

pub(super) fn random_token_pub() -> String {
    // Two UUID v4s concatenated = 256 bits of entropy from the platform
    // CSPRNG. `.simple()` emits lowercase hex without `-` separators,
    // giving a clean 64-char token safe for URL query strings.
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

pub(super) fn ensure_ws_server() -> Result<Arc<WsServerState>, String> {
    let mut slot = WS_SERVER.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(s) = slot.as_ref() {
        return Ok(Arc::clone(s));
    }

    // Channel carries Result so the boot thread can report bind errors
    // using the same pipe that carries the success value.
    let (addr_tx, addr_rx) =
        std::sync::mpsc::sync_channel::<Result<(SocketAddr, String), io::Error>>(1);
    let thread = std::thread::Builder::new()
        .name("dshl-pty-ws".to_string())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = addr_tx.send(Err(e));
                    return;
                }
            };
            rt.block_on(async move {
                let bind: SocketAddr = "127.0.0.1:0".parse().expect("valid socket literal");
                let listener = match TcpListener::bind(bind).await {
                    Ok(l) => l,
                    Err(e) => {
                        let _ = addr_tx.send(Err(io::Error::other(e)));
                        return;
                    }
                };
                let addr = listener.local_addr().expect("listener has local addr");
                let token = random_token_pub();
                if addr_tx.send(Ok((addr, token.clone()))).is_err() {
                    return;
                }
                run_server(listener, token).await;
            });
        })
        .map_err(|e| format!("failed to spawn dshl-pty-ws thread: {e}"))?;

    // The accept loop runs forever on the dedicated `dshl-pty-ws` thread with
    // its own runtime, which keeps the listener's reactor alive for the rest
    // of the process — the cached state only needs the endpoint facts (no
    // runtime / task handle kept here).
    std::mem::forget(thread);

    let (addr, token) = match addr_rx.recv() {
        Ok(Ok(v)) => v,
        // Errors are returned, not cached: the next spawn() retries the boot.
        Ok(Err(e)) => return Err(format!("dshl pty ws server failed to bind: {e}")),
        Err(_) => return Err("dshl pty ws server thread exited before sending addr".into()),
    };

    let state = Arc::new(WsServerState { addr, token });
    *slot = Some(Arc::clone(&state));
    Ok(state)
}

// ---------------------------------------------------------------------------
// Public accessor (for session.rs to build ws_url)
// ---------------------------------------------------------------------------

pub fn server_endpoint() -> Option<ServerInfo> {
    let guard = WS_SERVER.lock().unwrap_or_else(|p| p.into_inner());
    let s = guard.as_ref()?;
    Some(ServerInfo {
        host: "127.0.0.1".into(),
        port: s.addr.port(),
        token: s.token.clone(),
        url_prefix: format!("ws://127.0.0.1:{}/_pty", s.addr.port()),
    })
}

// ---------------------------------------------------------------------------
// Accept loop
// ---------------------------------------------------------------------------

// Main accept loop. Runs forever on the `dshl-pty-ws` thread.
async fn run_server(listener: TcpListener, token: String) {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[dshl-pty] accept failed: {e}");
                continue;
            }
        };
        if env::var_os("DSHL_PTY_TRACE").is_some() {
            eprintln!("[dshl-pty] new tcp from {peer}");
        }
        let token = token.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_tcp(stream, token).await
                && env::var_os("DSHL_PTY_TRACE").is_some()
            {
                eprintln!("[dshl-pty] tcp handler ended: {e}");
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Per-TCP handler: parse HTTP upgrade, validate path + token, wire WS.
// ---------------------------------------------------------------------------

async fn handle_tcp(
    mut stream: tokio::net::TcpStream,
    expected_token: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut head = Vec::<u8>::with_capacity(1024);
    let mut buf = [0u8; 1024];
    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            return Err("peer closed before HTTP upgrade".into());
        }
        head.extend_from_slice(&buf[..n]);
        if head.len() > 16 * 1024 {
            return Err("HTTP upgrade head too long".into());
        }
        if head.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if head.len() >= 2 && head.windows(2).any(|w| w == b"\n\n") {
            break;
        }
    }

    let head_str = String::from_utf8_lossy(&head);
    let mut lines = head_str.split("\r\n");
    let start = lines.next().ok_or("missing start line")?;
    let mut parts = start.split_whitespace();
    let method = parts.next().ok_or("missing method")?;
    let target = parts.next().ok_or("missing target")?;
    let _version = parts.next().unwrap_or("HTTP/1.1");
    if method != "GET" {
        write_http_response(&mut stream, 405, "Method Not Allowed", None, "").await?;
        return Ok(());
    }

    let mut headers: HashMap<String, String> = HashMap::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    let upgrade_ok = headers
        .get("upgrade")
        .map(|v| v.to_ascii_lowercase().contains("websocket"))
        .unwrap_or(false);
    let version = headers.get("sec-websocket-version").map(String::as_str);
    let key = headers.get("sec-websocket-key").cloned();
    if !upgrade_ok || version != Some("13") || key.is_none() {
        write_http_response(
            &mut stream,
            400,
            "Bad Request",
            None,
            "WebSocket upgrade required",
        )
        .await?;
        return Ok(());
    }

    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (target, None),
    };
    let expected_prefix = "/_pty/";
    let id = match path.strip_prefix(expected_prefix) {
        Some(id) if !id.is_empty() && !id.contains('/') => id.to_string(),
        _ => {
            write_http_response(&mut stream, 404, "Not Found", None, "").await?;
            return Ok(());
        }
    };
    let valid_token = query
        .and_then(|q| url_query_get_pub(q, "token"))
        .map(|t| t == expected_token)
        .unwrap_or(false);
    if !valid_token {
        write_http_response(
            &mut stream,
            403,
            "Forbidden",
            None,
            "missing or invalid token",
        )
        .await?;
        return Ok(());
    }

    let session = {
        let guard = SESSIONS.lock().unwrap();
        guard.get(&id).cloned()
    };
    let Some(session) = session else {
        write_http_response(&mut stream, 404, "Not Found", None, "unknown pty session").await?;
        return Ok(());
    };

    let accept = derive_accept_key(key.unwrap().as_bytes());
    let resp = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {accept}\r\n\
         \r\n"
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.flush().await?;

    let ws = WebSocketStream::from_raw_socket(stream, Role::Server, None).await;

    run_ws(ws, session).await;
    Ok(())
}

pub(super) fn url_query_get_pub<'a>(query: &'a str, needle: &str) -> Option<&'a str> {
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some(kv) => kv,
            None => (pair, ""),
        };
        if k == needle {
            return Some(v);
        }
    }
    None
}

async fn write_http_response(
    stream: &mut tokio::net::TcpStream,
    code: u16,
    reason: &str,
    extra_headers: Option<&str>,
    body: &str,
) -> io::Result<()> {
    let mut head = format!("HTTP/1.1 {code} {reason}\r\n");
    head.push_str("Connection: close\r\n");
    head.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    if let Some(extra) = extra_headers {
        head.push_str(extra);
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    if !body.is_empty() {
        stream.write_all(body.as_bytes()).await?;
    }
    stream.flush().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// WS framing loop — stdin/stdout control loop.
// ---------------------------------------------------------------------------

async fn run_ws<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    mut ws: WebSocketStream<S>,
    session: Arc<Session>,
) {
    let init = serde_json::json!({
        "t": "init",
        "id": session.id,
        "pid": session.pid,
    });
    let _ = ws.send(Message::Text(init.to_string().into())).await;

    let mut rx = session.outbound.subscribe();
    loop {
        tokio::select! {
            broadcast_msg = rx.recv() => {
                match broadcast_msg {
                    Ok(bytes) => {
                        let msg = Message::Binary(bytes.into());
                        if ws.send(msg).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // Backpressure tradeoff: drop the skipped chunks and
                        // keep the session alive rather than disconnecting.
                        // Leave a trace-level breadcrumb so missing output
                        // under heavy load is diagnosable.
                        if env::var_os("DSHL_PTY_TRACE").is_some() {
                            eprintln!("[dshl-pty] ws receiver lagged, {n} chunk(s) dropped");
                        }
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            ws_msg = ws.next() => {
                let Some(Ok(m)) = ws_msg else { break };
                // Read ALL discriminators BEFORE consuming the message into
                // raw bytes (into_data() moves `m`). tungstenite 0.30 wraps
                // Text/Binary bodies in opaque newtypes, so we avoid them.
                let is_text = m.is_text();
                let is_binary = m.is_binary();
                let is_ping = m.is_ping();
                let bytes: Vec<u8> = m.into_data().to_vec();
                // Channel split: TEXT frames are ALWAYS user input and go
                // straight to the shell — pasting text that merely looks like
                // a control message (`{"op":"kill", …}`) must never be able
                // to resize/kill the session. Control messages ride BINARY
                // frames only (see [`parse_control_frame`]); non-control
                // binary bytes still fall through to the shell.
                if is_text {
                    let _ = session
                        .stdin_tx
                        .lock()
                        .unwrap()
                        .write_all(&bytes);
                } else if is_binary {
                    match parse_control_frame(false, &bytes) {
                        Some(ctrl) => {
                            let _ = session.control_tx.send(ctrl);
                        }
                        None => {
                            let _ = session
                                .stdin_tx
                                .lock()
                                .unwrap()
                                .write_all(&bytes);
                        }
                    }
                } else if is_ping {
                    // Echo the ping body back as a pong (per RFC 6455).
                    let _ = ws.send(Message::Pong(bytes.into())).await;
                } else {
                    // Pong / Close / Frame — treat as EOF on this stream.
                    break;
                }
            }
        }
    }
    let _ = ws.close(None).await;
}

fn try_parse_control(t: &str) -> Option<ControlMsg> {
    if t.is_empty() || !t.starts_with('{') {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(t).ok()?;
    let op = v.get("op")?.as_str()?;
    match op {
        "resize" => {
            let cols = v.get("cols")?.as_u64()? as u16;
            let rows = v.get("rows")?.as_u64()? as u16;
            if cols < 1 || rows < 1 {
                return None;
            }
            Some(ControlMsg::Resize { cols, rows })
        }
        "kill" => Some(ControlMsg::Kill),
        _ => None,
    }
}

/// Frame-channel classifier: only BINARY frames may carry control JSON;
/// text frames are unconditionally shell input. Non-JSON / invalid UTF-8 /
/// unknown ops on the binary channel fall through to shell input too.
fn parse_control_frame(is_text: bool, bytes: &[u8]) -> Option<ControlMsg> {
    if is_text {
        return None;
    }
    try_parse_control(std::str::from_utf8(bytes).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_frames_are_never_control() {
        // Pasted JSON that looks exactly like a control op stays shell input.
        assert_eq!(
            parse_control_frame(true, br#"{"op":"kill"}"#),
            None,
            "text channel must never intercept user input"
        );
        assert_eq!(
            parse_control_frame(true, br#"{"op":"resize","cols":80,"rows":24}"#),
            None
        );
    }

    #[test]
    fn binary_json_frames_are_control() {
        assert_eq!(
            parse_control_frame(false, br#"{"op":"resize","cols":80,"rows":24}"#),
            Some(ControlMsg::Resize { cols: 80, rows: 24 })
        );
        assert_eq!(
            parse_control_frame(false, br#"{"op":"kill"}"#),
            Some(ControlMsg::Kill)
        );
    }

    #[test]
    fn binary_non_json_is_shell_input() {
        assert_eq!(parse_control_frame(false, b"\x1b[A"), None);
        assert_eq!(parse_control_frame(false, b"not json"), None);
        assert_eq!(
            parse_control_frame(false, &[0xff, 0xfe]),
            None,
            "invalid UTF-8 falls through to shell input"
        );
    }
}
