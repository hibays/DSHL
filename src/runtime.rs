//! A thin async entry point over a single global tokio runtime.
//!
//! The launcher drives the whole startup pipeline from a dedicated worker
//! thread through [`block_on`]. Inside that future everything runs on the
//! shared multi-thread runtime: process I/O, timers and the keep-alive socket
//! are true async (tokio) operations, so the executor is never blocked by the
//! subprocesses it manages.

use std::future::Future;
use std::sync::OnceLock;

/// The shared runtime, created lazily on first use and kept for the process
/// lifetime. Multi-thread so spawned tasks (process readers, reapers, probes,
/// keep-alive) make progress concurrently with the driven top-level future.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("dshl-async")
            .build()
            .expect("failed to create the tokio runtime")
    })
}

/// Drive `fut` to completion on the current thread.
///
/// Called from the dedicated flow worker thread (and the shutdown helpers).
/// The future runs on the shared runtime, which also drives every spawned
/// task (process readers, reapers, keep-alive).
pub fn block_on<F: Future>(fut: F) -> F::Output {
    runtime().block_on(fut)
}

/// Spawn a background task on the shared runtime from any thread.
pub fn spawn<F>(fut: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    runtime().spawn(fut)
}
