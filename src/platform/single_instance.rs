//! Launcher-level single-instance support (`[ui] single-instance`).
//!
//! A mutex lock file (`<cache>/dshl/instance.lock`) is held with
//! `File::try_lock` (LockFileEx on Windows, flock on Unix — the kernel
//! releases it automatically when the process dies, so stale locks from a
//! crash are impossible). A second dshl fails to acquire it and instead
//! signals the running instance through an activation file: the running
//! instance then restores its window if it is hidden in the tray, or
//! focuses it if it is visible.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn cache_dir() -> PathBuf {
    crate::platform::cache_dir().join("dshl")
}

fn lock_path() -> PathBuf {
    cache_dir().join("instance.lock")
}

fn activate_path() -> PathBuf {
    cache_dir().join("activate")
}

/// Try to become the single running instance. Returns `Some(file)` when
/// this process owns the lock (the file must be kept alive), `None` when
/// another dshl already holds it.
pub fn acquire() -> Option<File> {
    let path = lock_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .ok()?;
    match file.try_lock() {
        Ok(()) => {
            crate::debug::emit("single-instance: lock acquired (first instance)");
            Some(file)
        }
        Err(_) => {
            crate::debug::emit("single-instance: another dshl is running");
            None
        }
    }
}

/// Called by the *second* instance: ask the running one to come to the
/// foreground (restore from tray or focus its window), then exit.
pub fn notify_activate() {
    // On Windows, the second instance is the one with foreground rights
    // (launched by the user), so it grants the running instance the ability
    // to steal the foreground before signalling it.
    #[cfg(target_os = "windows")]
    unsafe {
        // SAFETY: user32 call with the ASFW_ANY constant; best-effort.
        use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
        let _ = AllowSetForegroundWindow(0xFFFF_FFFF); // ASFW_ANY
    }
    let path = activate_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let _ = writeln!(f, "{ts}");
    }
}

/// Called periodically by the *first* instance. Returns `true` once when
/// a second instance asked for activation (file grew since last check).
pub fn poll_activate() -> bool {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST_LEN: OnceLock<AtomicU64> = OnceLock::new();
    let path = activate_path();
    // Initialise the baseline with the CURRENT file length so a leftover
    // activate file from a previous run does not trigger a spurious
    // activation on the first poll (which would focus/restore the window
    // for no reason at every startup).
    let last = LAST_LEN.get_or_init(|| {
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        AtomicU64::new(len)
    });
    let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let prev = last.swap(len, Ordering::SeqCst);
    len > prev
}
