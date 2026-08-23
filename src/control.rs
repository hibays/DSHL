//! Launcher control plane: the Rust side of the `@dshl/control` plugin
//! contract.
//!
//! A small JSON-RPC-style endpoint — newline-delimited JSON over a loopback
//! socket — that exposes the launcher's native capabilities (shutdown, profile
//! switch, and later updates / window / terminal) to the supervised `dsh`
//! process. The dsh-side Cordis plugin (`@dshl/control`) connects here and
//! performs the same operations the Electron desktop shell provides, but
//! against a running `dshl` instead of an embedded host.
//!
//! # Wire protocol
//!
//! One JSON object per line. The client authenticates first:
//!
//! ```text
//! {"type":"hello","token":"<per-launch token>"}
//! ```
//!
//! then exchanges request/response frames:
//!
//! ```text
//! → {"type":"request","id":1,"method":"ping","params":{}}
//! ← {"id":1,"result":{"pong":true,...}}
//! ← {"id":1,"error":"..."}                (on failure)
//! ```
//!
//! # Transport
//!
//! The loopback TCP socket is the pragmatic first transport: cross-platform,
//! no unsafe, and connectable from node/bun with the stdlib `net` module. The
//! per-launch random token is carried to dsh via the [`CONTROL_ENV`] env var
//! and checked on every connection, so a stray local process cannot drive the
//! launcher. The protocol itself is transport-agnostic — a Windows named pipe
//! would be a drop-in swap for the bind/accept below.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// Env var carrying the control endpoint to the supervised `dsh` process.
/// Value format: `dshl://<token>@127.0.0.1:<port>`.
pub const CONTROL_ENV: &str = "DSHL_CONTROL_URL";

/// Maximum accepted frame size (a pathological client must not grow memory).
const MAX_FRAME_BYTES: usize = 64 * 1024;

/// How long a connection has to send the `hello` handshake.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// The active endpoint, created once by [`start`].
struct Endpoint {
    addr: SocketAddr,
    token: String,
}

impl Endpoint {
    fn url(&self) -> String {
        format!("dshl://{}@{}", self.token, self.addr)
    }
}

static ENDPOINT: OnceLock<Endpoint> = OnceLock::new();

/// The augmented `PATH` the most recent dsh launch resolved (node/bun/dsh
/// bin dirs), so `open-terminal` can spawn a terminal that feels like the dsh
/// environment. `None` before the first launch (fall back to the ambient PATH).
static LAST_RUNTIME_PATH: Mutex<Option<std::ffi::OsString>> = Mutex::new(None);

/// Remember the augmented PATH a dsh launch used (set by `flow::prepare`).
pub fn store_runtime_path(path: &std::ffi::OsStr) {
    *LAST_RUNTIME_PATH.lock().unwrap() = Some(path.to_os_string());
}

fn runtime_path() -> Option<std::ffi::OsString> {
    LAST_RUNTIME_PATH.lock().unwrap().clone()
}

/// A per-launch bearer token from the platform CSPRNG (UUID v4 = 122 bits of
/// entropy), same source as the PTY server token. The historic
/// `time ^ pid ^ counter` scheme was guessable: an attacker who knew the rough
/// launch time could brute-force the endpoint online and drive
/// shutdown/restart/open-terminal.
fn generate_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Start the control server: bind a loopback listener, store the endpoint,
/// and spawn the accept loop on the shared runtime. Idempotent.
pub fn start() -> std::io::Result<()> {
    crate::runtime::block_on(bind())
}

/// Async variant of [`start`] for contexts already driving the runtime
/// (e.g. the control-plane tests).
pub async fn start_async() -> std::io::Result<()> {
    bind().await
}

async fn bind() -> std::io::Result<()> {
    if ENDPOINT.get().is_some() {
        return Ok(());
    }
    let token = generate_token();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = listener.local_addr()?;
    let _ = ENDPOINT.set(Endpoint { addr, token });
    // Redacted: logs land on disk, so never emit the token-bearing URL —
    // the port alone is diagnostic.
    crate::debug::emit(&format!(
        "control endpoint: 127.0.0.1:{}",
        ENDPOINT.get().map(|e| e.addr.port()).unwrap_or(0)
    ));
    std::mem::drop(crate::runtime::spawn(accept_loop(listener)));
    Ok(())
}

/// The control endpoint descriptor, or `None` when the server is not running.
pub fn endpoint_url() -> Option<String> {
    ENDPOINT.get().map(Endpoint::url)
}

/// Set the `DSHL_CONTROL_URL` env var on a command about to spawn dsh, so the
/// dsh-side plugin knows where to connect. No-op when the server is off.
pub fn inject_env(cmd: &mut std::process::Command) {
    if let Some(url) = endpoint_url() {
        cmd.env(CONTROL_ENV, url);
    }
}

/// Accept connections until the listener dies.
async fn accept_loop(listener: TcpListener) {
    while let Ok((stream, _)) = listener.accept().await {
        std::mem::drop(crate::runtime::spawn(handle_connection(stream)));
    }
}

/// One connection: authenticate, then serve request/response frames.
async fn handle_connection(stream: TcpStream) {
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);

    // The very first frame must be the hello handshake.
    let mut line = String::new();
    match tokio::time::timeout(HELLO_TIMEOUT, reader.read_line(&mut line)).await {
        Err(_) => return,     // no handshake in time
        Ok(Err(_)) => return, // read error
        Ok(Ok(0)) => return,  // closed before handshake
        Ok(Ok(n)) if n > MAX_FRAME_BYTES => return,
        Ok(Ok(_)) => {}
    }
    let Ok(frame) = serde_json::from_str::<Frame>(line.trim()) else {
        let _ = respond(&mut write, &Response::error(0, "expected hello")).await;
        return;
    };
    let Frame::Hello { token } = frame else {
        let _ = respond(&mut write, &Response::error(0, "expected hello")).await;
        return;
    };
    let Some(endpoint) = ENDPOINT.get() else {
        return;
    };
    if token != endpoint.token {
        let _ = respond(&mut write, &Response::error(0, "bad token")).await;
        return;
    }

    // Serve requests.
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.len() > MAX_FRAME_BYTES {
            let _ = respond(&mut write, &Response::error(0, "frame too large")).await;
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Frame>(trimmed) {
            Ok(Frame::Request { id, method, params }) => match dispatch(&method, params).await {
                Ok(result) => Response::ok(id, result),
                Err(error) => Response::error(id, &error),
            },
            Ok(_) => Response::error(0, "expected request"),
            Err(_) => Response::error(0, "invalid frame"),
        };
        if respond(&mut write, &response).await.is_err() {
            break;
        }
    }
}

async fn respond(
    write: &mut tokio::net::tcp::OwnedWriteHalf,
    response: &Response,
) -> std::io::Result<()> {
    let mut buf = serde_json::to_vec(response).unwrap_or_else(|_| b"{}".to_vec());
    buf.push(b'\n');
    write.write_all(&buf).await
}

/// Inbound frames (one per line).
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum Frame {
    /// `{"type":"hello","token":"…"}` — authentication, sent first.
    Hello { token: String },
    /// `{"type":"request","id":1,"method":"…","params":{…}}`.
    Request {
        id: u64,
        method: String,
        #[serde(default)]
        params: Value,
    },
}

/// Outbound frame: exactly one of `result` / `error` is present.
#[derive(Serialize)]
struct Response {
    id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl Response {
    fn ok(id: u64, result: Value) -> Response {
        Response {
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: u64, error: impl Into<String>) -> Response {
        Response {
            id,
            result: None,
            error: Some(error.into()),
        }
    }
}

/// Route one method to an existing launcher capability.
async fn dispatch(method: &str, params: Value) -> Result<Value, String> {
    match method {
        "ping" => Ok(json!({
            "pong": true,
            "version": env!("CARGO_PKG_VERSION"),
        })),
        "shutdown" => {
            crate::ui::request_shutdown();
            Ok(json!({ "ok": true }))
        }
        "switch-profile" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "switch-profile requires a string `name`".to_string())?;
            set_pending_profile(name);
            crate::ui::request_shutdown();
            Ok(json!({ "ok": true, "pending": name }))
        }
        "open-terminal" => {
            if let Some(action) = test_terminal_action() {
                action();
            } else {
                let cwd = std::env::current_dir()
                    .map_err(|e| format!("open-terminal: no working directory ({e})"))?;
                crate::platform::open_terminal(runtime_path().as_deref(), &cwd)
                    .map_err(|e| format!("open-terminal: {e}"))?;
            }
            Ok(json!({ "ok": true }))
        }
        "restart" => {
            if let Some(action) = test_restart_action() {
                action();
            } else {
                crate::ui::request_restart();
            }
            Ok(json!({ "ok": true }))
        }
        _ => Err(format!("unknown method: {method}")),
    }
}

// --- pending profile (switch-profile) ------------------------------

/// Path of the persisted pending profile; read once by the next launch and
/// then cleared, so a `switch-profile` survives a full launcher restart.
fn pending_profile_path() -> PathBuf {
    crate::platform::cache_dir()
        .join("dshl")
        .join("pending-profile")
}

/// Persist a profile selection that the NEXT launch will boot with. Survives
/// process exit (switch-profile also requests shutdown).
fn set_pending_profile(name: &str) {
    let path = pending_profile_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::write(&path, name).is_ok() {
        crate::debug::emit(&format!("control: pending profile -> {name}"));
    }
}

/// Read (and clear) the persisted pending profile, if any.
pub fn take_pending_profile() -> Option<String> {
    let path = pending_profile_path();
    let name = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let name = name.trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

/// In-process pending-profile override used by tests (never the on-disk one).
static TEST_PENDING_PROFILE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
fn test_pending() -> &'static Mutex<Option<String>> {
    TEST_PENDING_PROFILE.get_or_init(|| Mutex::new(None))
}

/// Test-only overrides that replace the real `open-terminal` / `restart`
/// side effects (spawning a window / relaunching dsh), so dispatch tests stay
/// hermetic. Each override is consumed once, mirroring a single method call.
type TestAction = Box<dyn Fn() + Send + Sync>;
static TEST_TERMINAL_ACTION: OnceLock<Mutex<Option<TestAction>>> = OnceLock::new();
static TEST_RESTART_ACTION: OnceLock<Mutex<Option<TestAction>>> = OnceLock::new();

fn test_terminal_action() -> Option<TestAction> {
    TEST_TERMINAL_ACTION
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap()
        .take()
}

fn test_restart_action() -> Option<TestAction> {
    TEST_RESTART_ACTION
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap()
        .take()
}

#[cfg(test)]
fn set_test_action(
    storage: &'static OnceLock<Mutex<Option<TestAction>>>,
    f: impl Fn() + Send + Sync + 'static,
) {
    *storage.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(Box::new(f));
}

/// Substitute the profile name in a dsh flag vector: replace the value that
/// follows `--profile`, or append `--profile <name>` when absent. Pure.
fn override_profile_flags(flags: Vec<String>, profile: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(flags.len() + 2);
    let mut replaced = false;
    let mut iter = flags.into_iter();
    while let Some(flag) = iter.next() {
        if flag == "--profile" && !replaced {
            if iter.next().is_some() {
                out.push(flag);
                out.push(profile.to_string());
                replaced = true;
            }
        } else {
            out.push(flag);
        }
    }
    if !replaced {
        out.push("--profile".to_string());
        out.push(profile.to_string());
    }
    out
}

/// Apply the pending profile (on-disk or in-process) to the dsh flags, once.
pub(crate) fn apply_pending_profile(flags: Vec<String>) -> Vec<String> {
    let name = test_pending()
        .lock()
        .unwrap()
        .take()
        .or_else(take_pending_profile);
    match name {
        Some(profile) => override_profile_flags(flags, &profile),
        None => flags,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    async fn send_frame(write: &mut tokio::net::tcp::OwnedWriteHalf, frame: Value) {
        let mut buf = serde_json::to_vec(&frame).unwrap();
        buf.push(b'\n');
        write.write_all(&buf).await.expect("write frame");
    }

    async fn recv_frame(reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>) -> Value {
        let mut line = String::new();
        reader.read_line(&mut line).await.expect("read line");
        serde_json::from_str(line.trim()).expect("parse frame")
    }

    #[test]
    fn ping_dispatch_answers() {
        let result = crate::runtime::block_on(dispatch("ping", json!({})));
        let value = result.expect("ping should succeed");
        assert_eq!(value["pong"], true);
        assert!(value["version"].as_str().is_some());
    }

    #[test]
    fn unknown_method_is_an_error() {
        let result = crate::runtime::block_on(dispatch("nope", json!({})));
        assert!(result.is_err());
    }

    #[test]
    fn open_terminal_dispatch_answers() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static CALLED: AtomicBool = AtomicBool::new(false);
        set_test_action(&TEST_TERMINAL_ACTION, || {
            CALLED.store(true, Ordering::SeqCst)
        });
        let result = crate::runtime::block_on(dispatch("open-terminal", json!({})));
        assert!(result.is_ok(), "open-terminal should succeed");
        assert!(
            CALLED.load(Ordering::SeqCst),
            "open-terminal must invoke the terminal action"
        );
    }

    #[test]
    fn restart_dispatch_relaunches() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static CALLED: AtomicBool = AtomicBool::new(false);
        set_test_action(&TEST_RESTART_ACTION, || {
            CALLED.store(true, Ordering::SeqCst)
        });
        let result = crate::runtime::block_on(dispatch("restart", json!({})));
        assert!(result.is_ok(), "restart should succeed");
        assert!(
            CALLED.load(Ordering::SeqCst),
            "restart must invoke the restart action"
        );
    }

    #[test]
    fn override_replaces_existing_profile() {
        let flags = vec![
            "--profile".into(),
            "web".into(),
            "--port".into(),
            "0".into(),
        ];
        let out = override_profile_flags(flags, "desktop");
        assert_eq!(
            out,
            vec![
                "--profile".to_string(),
                "desktop".to_string(),
                "--port".to_string(),
                "0".to_string()
            ]
        );
    }

    #[test]
    fn override_appends_when_absent() {
        let flags = vec!["--host".into(), "127.0.0.1".into()];
        let out = override_profile_flags(flags, "desktop");
        assert_eq!(
            out,
            vec![
                "--host".to_string(),
                "127.0.0.1".to_string(),
                "--profile".to_string(),
                "desktop".to_string()
            ]
        );
    }

    #[test]
    fn override_keeps_first_profile_value() {
        let flags = vec![
            "--profile".into(),
            "web".into(),
            "--profile".into(),
            "other".into(),
        ];
        let out = override_profile_flags(flags, "desktop");
        assert_eq!(
            out,
            vec![
                "--profile".to_string(),
                "desktop".to_string(),
                "--profile".to_string(),
                "other".to_string()
            ]
        );
    }

    #[test]
    fn inject_env_sets_control_url() {
        crate::runtime::block_on(async {
            crate::control::start_async()
                .await
                .expect("control server should start");
            let mut cmd = std::process::Command::new("echo");
            crate::control::inject_env(&mut cmd);
            let mut seen = false;
            for (key, _) in cmd.get_envs() {
                if key == CONTROL_ENV {
                    seen = true;
                }
            }
            assert!(
                seen,
                "DSHL_CONTROL_URL must be injected into the dsh command"
            );
        });
    }

    #[test]
    fn protocol_roundtrip_over_the_wire() {
        crate::runtime::block_on(async {
            crate::control::start_async()
                .await
                .expect("control server should start");
            let url = crate::control::endpoint_url().expect("endpoint url");
            // Format: dshl://<token>@127.0.0.1:<port>
            let rest = url.strip_prefix("dshl://").expect("dshl:// prefix");
            let (token, hostport) = rest.split_once('@').expect("token@hostport");

            let stream = tokio::net::TcpStream::connect(hostport)
                .await
                .expect("connect");
            let (read, mut write) = stream.into_split();
            let mut reader = BufReader::new(read);

            // hello handshake (no reply on success)
            send_frame(&mut write, json!({ "type": "hello", "token": token })).await;

            // ping
            send_frame(
                &mut write,
                json!({ "type": "request", "id": 1, "method": "ping", "params": {} }),
            )
            .await;
            let r = recv_frame(&mut reader).await;
            assert_eq!(r["id"], 1);
            assert_eq!(r["result"]["pong"], true);
            assert!(r["result"]["version"].as_str().is_some());

            // switch-profile persists a pending profile and requests shutdown
            send_frame(
                &mut write,
                json!({ "type": "request", "id": 2, "method": "switch-profile", "params": { "name": "desktop" } }),
            )
            .await;
            let r = recv_frame(&mut reader).await;
            assert_eq!(r["id"], 2);
            assert_eq!(r["result"]["pending"], "desktop");
            assert!(
                pending_profile_path().exists(),
                "pending profile must persist"
            );

            // shutdown sets the launcher's shutdown flags
            send_frame(
                &mut write,
                json!({ "type": "request", "id": 3, "method": "shutdown", "params": {} }),
            )
            .await;
            let r = recv_frame(&mut reader).await;
            assert_eq!(r["id"], 3);
            assert_eq!(r["result"]["ok"], true);
            assert!(
                crate::ui::shutdown_requested(),
                "shutdown must be requested"
            );

            // Unknown methods surface as errors on the wire.
            send_frame(
                &mut write,
                json!({ "type": "request", "id": 4, "method": "nope", "params": {} }),
            )
            .await;
            let r = recv_frame(&mut reader).await;
            assert_eq!(r["id"], 4);
            assert!(r["error"].as_str().is_some());

            // Clean up the persisted file the test wrote (never leave one on
            // the developer's machine).
            let _ = std::fs::remove_file(pending_profile_path());
        });
    }

    #[test]
    fn rejects_wrong_hello_token() {
        crate::runtime::block_on(async {
            crate::control::start_async()
                .await
                .expect("control server should start");
            let url = crate::control::endpoint_url().expect("endpoint url");
            let rest = url.strip_prefix("dshl://").expect("dshl:// prefix");
            let (_, hostport) = rest.split_once('@').expect("token@hostport");

            let stream = tokio::net::TcpStream::connect(hostport)
                .await
                .expect("connect");
            let (read, mut write) = stream.into_split();
            let mut reader = BufReader::new(read);

            send_frame(
                &mut write,
                json!({ "type": "hello", "token": "wrong-token" }),
            )
            .await;
            let r = recv_frame(&mut reader).await;
            assert_eq!(r["error"], "bad token");
        });
    }
}
