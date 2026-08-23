//! `RunHandle` — opaque, clone-able handle to a running kernel.
//!
//! The handle is created by `run_with_options` (Track B addon entry) and
//! exposes `is_running()` / `wait()` so Node worker callbacks, HTTP
//! handlers and the JS main thread can all observe / join the kernel
//! thread. All methods are thread-safe — the underlying `ui::*` and
//! `tray::*` contract they ultimately drive is already built for
//! cross-thread signalling.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Opaque, clone-able handle to a running launcher kernel. All methods are
/// thread-safe so Node worker callbacks / HTTP handlers / the JS main thread
/// can call them from any thread (mirroring the internal ui::* and tray::*
/// contract which is already built for cross-thread signalling).
#[derive(Clone)]
pub struct RunHandle {
    pub(crate) inner: Arc<RunHandleInner>,
}

pub(crate) struct RunHandleInner {
    pub(crate) started: Arc<AtomicBool>,
    pub(crate) thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// Owned drop-guard for the ctrlc handler. `ctrlc` v3 exposes no
    /// unregister API, so this is currently `None` after install — kept as
    /// a boxed slot so a future upgrade to a handler that supports
    /// unregister drops cleanly here.
    #[allow(dead_code)]
    pub(crate) ctrlc_guard: Option<Box<dyn std::any::Any + Send + Sync>>,
}

impl RunHandle {
    pub fn is_running(&self) -> bool {
        self.inner.started.load(Ordering::SeqCst)
    }

    pub fn wait(&self) {
        if let Some(jh) = self
            .inner
            .thread
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            let _ = jh.join();
        }
    }
}

/// Convenience constructor used by `run_with_options`.
pub(crate) fn new(
    started: Arc<AtomicBool>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    ctrlc_guard: Option<Box<dyn std::any::Any + Send + Sync>>,
) -> RunHandle {
    RunHandle {
        inner: Arc::new(RunHandleInner {
            started,
            thread,
            ctrlc_guard,
        }),
    }
}
