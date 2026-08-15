//! A dependency-free, runtime-free async executor.
//!
//! dshl deliberately avoids `tokio`/`smol`/`async-std`: we only need
//! `std::future` + `std::task`. This module provides:
//!
//! * [`block_on`] — drives a single top-level future to completion on the
//!   current thread, parking between polls.
//! * [`sleep`] — a worker-thread friendly sleep future.
//!
//! The executor's waker captures the *executor* thread handle. Futures that
//! are completed by other threads (see [`crate::process::AsyncChild`]) call
//! [`Waker::wake`] from those threads, which unparks the executor thread.

use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::time::Duration;

/// Drive `fut` to completion on the current thread.
///
/// `Pending` futures park the thread until some other thread wakes the waker
/// (e.g. a process reader thread delivering a new line).
pub fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    let waker = thread_waker();
    let mut cx = Context::from_waker(&waker);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park(),
        }
    }
}

/// A waker that unparks the thread that created it.
fn thread_waker() -> Waker {
    let handle = Arc::new(std::thread::current());
    let data = Arc::into_raw(handle) as *const ();

    unsafe fn clone_fn(data: *const ()) -> RawWaker {
        // SAFETY: `data` is a valid `Arc<Thread>` created by `thread_waker`.
        let arc = unsafe { Arc::from_raw(data as *const std::thread::Thread) };
        std::mem::forget(arc.clone());
        RawWaker::new(data, &VTABLE)
    }
    unsafe fn wake_fn(data: *const ()) {
        // SAFETY: `data` is a valid `Arc<Thread>` created by `thread_waker`.
        let arc = unsafe { Arc::from_raw(data as *const std::thread::Thread) };
        arc.unpark();
    }
    unsafe fn wake_by_ref_fn(data: *const ()) {
        // SAFETY: `data` is a valid `Arc<Thread>` created by `thread_waker`.
        let arc = unsafe { Arc::from_raw(data as *const std::thread::Thread) };
        arc.unpark();
        std::mem::forget(arc);
    }
    unsafe fn drop_fn(data: *const ()) {
        // SAFETY: `data` is a valid `Arc<Thread>` created by `thread_waker`.
        drop(unsafe { Arc::from_raw(data as *const std::thread::Thread) });
    }

    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone_fn, wake_fn, wake_by_ref_fn, drop_fn);

    // SAFETY: `data` is a valid `Arc<Thread>` and the vtable matches it.
    unsafe { Waker::from_raw(RawWaker::new(data, &VTABLE)) }
}

/// A future that resolves after `duration`.
///
/// This implementation simply sleeps the current thread. It is intended for
/// the dedicated flow worker thread, so blocking it never stalls the UI
/// (which runs on the main thread inside `webui::wait()`).
pub async fn sleep(duration: Duration) {
    std::thread::sleep(duration);
}

/// A future that resolves immediately, yielding a poll boundary.
pub async fn yield_now() {
    // A single `std::task` poll is enough to yield to the executor loop.
    Yield(true).await;
}

struct Yield(bool);

impl Future for Yield {
    type Output = ();
    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            self.0 = false;
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}
