//! Windows job-object helpers: assign spawned children to a kill-on-close job
//! so the OS reaps them automatically when the launcher process exits (even on
//! `TerminateProcess`). All Win32 calls go through windows-rs 0.62.
//!
//! Used by [`crate::process::child`] right after spawning.

#![cfg(target_os = "windows")]

use std::os::windows::io::RawHandle;
use std::sync::OnceLock;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows::core::PCWSTR;

/// The shared kill-on-close job, stored as `usize` (raw handles are neither
/// `Send` nor `Sync`, so they cannot live in a `static`).
static JOB: OnceLock<usize> = OnceLock::new();

fn job_handle() -> RawHandle {
    let raw = *JOB.get_or_init(|| {
        // SAFETY: all calls are guarded; null input is allowed.
        let handle = unsafe { CreateJobObjectW(None, PCWSTR::null()) };
        let Ok(handle) = handle else {
            crate::debug::emit(&format!(
                "win_job: CreateJobObjectW failed: {}",
                std::io::Error::last_os_error()
            ));
            return 0usize;
        };
        // `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` is used instead of the
        // basic variant: some environments (sandboxes/CI runners) reject
        // `JobObjectBasicLimitInformation` with ERROR_INVALID_PARAMETER
        // even for a correct struct, while the extended class works
        // everywhere.
        let info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
            BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                ..Default::default()
            },
            ..Default::default()
        };
        let ok = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok.is_err() {
            crate::debug::emit(&format!(
                "win_job: SetInformationJobObject failed: {}",
                std::io::Error::last_os_error()
            ));
            return 0usize;
        }
        handle.0 as usize
    });
    raw as RawHandle
}

/// Assign a spawned child to the shared kill-on-close job (best-effort).
pub fn assign(child: &std::process::Child) {
    use std::os::windows::io::AsRawHandle;
    let job = job_handle();
    if job.is_null() {
        crate::debug::emit("win_job: no kill-on-close job (creation failed)");
        return;
    }
    // SAFETY: `child` is a valid spawned process handle.
    let ok = unsafe { AssignProcessToJobObject(HANDLE(job), HANDLE(child.as_raw_handle())) };
    if ok.is_err() {
        // Typical cause: the launcher itself already runs inside a job
        // that does not allow nesting, so the child cannot be added to
        // ours. The child then survives an abrupt launcher kill.
        crate::debug::emit(&format!(
            "win_job: AssignProcessToJobObject failed: {}",
            std::io::Error::last_os_error()
        ));
    }
}

/// Assign a raw process handle to the shared kill-on-close job.
pub fn assign_raw(process: RawHandle) {
    let job = job_handle();
    if job.is_null() {
        crate::debug::emit("win_job: no kill-on-close job (creation failed)");
        return;
    }
    // SAFETY: `process` is a valid spawned process handle.
    let ok = unsafe { AssignProcessToJobObject(HANDLE(job), HANDLE(process)) };
    if ok.is_err() {
        crate::debug::emit(&format!(
            "win_job: AssignProcessToJobObject failed: {}",
            std::io::Error::last_os_error()
        ));
    }
}
