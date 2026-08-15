//! Windows hidden-console process spawn + graceful stop helpers.
//!
//! All Win32 calls go through the `windows` crate (windows-rs 0.62) — no
//! hand-written `#[link] extern "system"` FFI. Used by
//! [`crate::process::child`] (`spawn_console` / `signal_stop` / raw-handle
//! reaping).

#![cfg(target_os = "windows")]

use std::ffi::OsString;
use std::fs::File;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::process::Command;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation};
use windows::Win32::System::Console::{
    AttachConsole, CTRL_C_EVENT, FreeConsole, GenerateConsoleCtrlEvent, SetConsoleCtrlHandler,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CREATE_NEW_CONSOLE, CREATE_NEW_PROCESS_GROUP, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
    GetExitCodeProcess, INFINITE, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
    STARTF_USESHOWWINDOW, STARTF_USESTDHANDLES, STARTUPINFOW, STARTUPINFOW_FLAGS,
    WaitForSingleObject,
};
use windows::core::{PCWSTR, PWSTR};

/// A process spawned with hidden console, its pid and piped stdio.
pub struct Spawned {
    pub process: RawHandle,
    pub pid: u32,
    pub stdout: File,
    pub stderr: File,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn quote_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".to_string();
    }
    if !arg.chars().any(|c| c.is_whitespace() || c == '"') {
        return arg.to_string();
    }
    let mut s = String::from("\"");
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                s.push_str(&"\\".repeat(backslashes * 2 + 1));
                s.push('"');
                backslashes = 0;
            }
            _ => {
                s.push_str(&"\\".repeat(backslashes));
                s.push(c);
                backslashes = 0;
            }
        }
    }
    s.push_str(&"\\".repeat(backslashes * 2));
    s.push('"');
    s
}

fn build_cmdline(cmd: &Command) -> Vec<u16> {
    let mut s = String::new();
    s.push_str(&quote_arg(&cmd.get_program().to_string_lossy()));
    for arg in cmd.get_args() {
        s.push(' ');
        s.push_str(&quote_arg(&arg.to_string_lossy()));
    }
    wide(&s)
}

fn build_env_block(cmd: &Command) -> Vec<u16> {
    use std::collections::HashMap;
    let mut env: HashMap<OsString, Option<OsString>> =
        std::env::vars_os().map(|(k, v)| (k, Some(v))).collect();
    for (k, v) in cmd.get_envs() {
        env.insert(k.to_owned(), v.map(|s| s.to_owned()));
    }
    let mut block: Vec<u16> = Vec::new();
    for (k, v) in env {
        if let Some(v) = v {
            block.extend(k.encode_wide());
            block.push('=' as u16);
            block.extend(v.encode_wide());
            block.push(0);
        }
    }
    block.push(0);
    block
}

/// Spawn `cmd` with `CREATE_NEW_CONSOLE | CREATE_NEW_PROCESS_GROUP` and a
/// hidden window, wiring piped stdout/stderr. The child can later be stopped
/// gracefully with [`send_ctrl_c`].
pub fn spawn_hidden_console(cmd: &Command) -> std::io::Result<Spawned> {
    // Create pipes for stdout/stderr.
    let (out_r, out_w) = create_pipe()?;
    let (err_r, err_w) = create_pipe()?;

    let app = wide(&cmd.get_program().to_string_lossy());
    let mut cmdline = build_cmdline(cmd);
    let env_block = build_env_block(cmd);

    let si = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        lpReserved: PWSTR::null(),
        lpDesktop: PWSTR::null(),
        lpTitle: PWSTR::null(),
        dwX: 0,
        dwY: 0,
        dwXSize: 0,
        dwYSize: 0,
        dwXCountChars: 0,
        dwYCountChars: 0,
        dwFillAttribute: 0,
        dwFlags: STARTUPINFOW_FLAGS(STARTF_USESTDHANDLES.0 | STARTF_USESHOWWINDOW.0),
        wShowWindow: 0, // SW_HIDE
        cbReserved2: 0,
        lpReserved2: std::ptr::null_mut(),
        hStdInput: HANDLE::default(),
        hStdOutput: HANDLE(out_w),
        hStdError: HANDLE(err_w),
    };
    let mut pi = PROCESS_INFORMATION::default();

    // SAFETY: all pointers are valid for the duration of the call; the
    // pipe handles are closed on error paths.
    let result = unsafe {
        CreateProcessW(
            PCWSTR(app.as_ptr()),
            Some(PWSTR(cmdline.as_mut_ptr())),
            None,
            None,
            true, // inherit handles (for the stdio pipes)
            PROCESS_CREATION_FLAGS(
                CREATE_NEW_CONSOLE.0 | CREATE_NEW_PROCESS_GROUP.0 | CREATE_UNICODE_ENVIRONMENT.0,
            ),
            Some(env_block.as_ptr() as *const core::ffi::c_void),
            None,
            &si,
            &mut pi,
        )
    };

    // Close our copies of the write ends regardless.
    unsafe {
        let _ = CloseHandle(HANDLE(out_w));
        let _ = CloseHandle(HANDLE(err_w));
    }

    if result.is_err() {
        unsafe {
            let _ = CloseHandle(HANDLE(out_r));
            let _ = CloseHandle(HANDLE(err_r));
            if !pi.hProcess.0.is_null() {
                let _ = CloseHandle(pi.hProcess);
            }
            if !pi.hThread.0.is_null() {
                let _ = CloseHandle(pi.hThread);
            }
        }
        return Err(std::io::Error::last_os_error());
    }

    unsafe {
        let _ = CloseHandle(pi.hThread); // we don't need the thread handle
    }

    Ok(Spawned {
        process: pi.hProcess.0,
        pid: pi.dwProcessId,
        // SAFETY: the read ends are owned by us.
        stdout: unsafe { File::from_raw_handle(out_r) },
        stderr: unsafe { File::from_raw_handle(err_r) },
    })
}

fn create_pipe() -> std::io::Result<(RawHandle, RawHandle)> {
    let mut read = HANDLE::default();
    let mut write = HANDLE::default();
    // SAFETY: null attrs/size use the defaults.
    unsafe {
        CreatePipe(&mut read, &mut write, None, 0).map_err(|_| std::io::Error::last_os_error())?;
    }
    // The child must inherit the write end; the parent keeps the read end.
    unsafe {
        let _ = SetHandleInformation(write, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT);
    }
    Ok((read.0, write.0))
}

/// Wait for the process to exit and return its exit code.
pub fn wait_handle(handle: RawHandle) -> Option<i32> {
    // SAFETY: handle is a valid process handle.
    unsafe {
        let _ = WaitForSingleObject(HANDLE(handle), INFINITE);
        let mut code: u32 = 0;
        if GetExitCodeProcess(HANDLE(handle), &mut code).is_ok() {
            Some(code as i32)
        } else {
            None
        }
    }
}

/// Send Ctrl+C to the child's process group (graceful stop).
///
/// `AttachConsole` requires the caller to have no console of its own, so
/// we detach first (a no-op for the GUI launcher, and what makes the
/// signal reach the child even when dshl runs from a terminal).
///
/// The child was spawned with `CREATE_NEW_CONSOLE | CREATE_NEW_PROCESS_GROUP`,
/// so its console only contains the child and its descendants. IMPORTANT:
/// `CTRL_C_EVENT` cannot target a process group — with a nonzero group id
/// the call succeeds but no process receives the signal. Broadcasting with
/// group 0 reaches every process sharing the (attached) console instead.
///
/// That broadcast also reaches THIS launcher (it just attached to that
/// console). Without a handler the default Ctrl+C action terminates the
/// process with `STATUS_CONTROL_C_EXIT` (0xc000013a) — exactly what the
/// user sees as "closed the window but dshl died on Ctrl+C". So we ignore
/// Ctrl+C for ourselves while the broadcast is in flight.
pub fn send_ctrl_c(pid: u32) {
    // SAFETY: best-effort console signalling; failures are ignored.
    unsafe {
        // Ignore Ctrl+C for this process (handler=NULL + add=TRUE),
        // then restore the default handler afterwards.
        let _ = SetConsoleCtrlHandler(None, true);
        let _ = FreeConsole();
        if AttachConsole(pid).is_ok() {
            // The ignore flag is PER-CONSOLE: it applied to our previous
            // console above, but we just attached to the CHILD's console.
            // Without re-ignoring here, the broadcast below delivers a
            // CTRL_C to US as well, which the default handler turns into
            // STATUS_CONTROL_C_EXIT (0xc000013a) — the launcher dies
            // "on Ctrl+C" instead of exiting cleanly.
            let _ = SetConsoleCtrlHandler(None, true);
            let _ = GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0);
            // Ctrl+C delivery is asynchronous: keep ignoring briefly so
            // the event cannot land after the ignore flag is removed.
            std::thread::sleep(std::time::Duration::from_millis(100));
            let _ = SetConsoleCtrlHandler(None, false);
            let _ = FreeConsole();
        }
        let _ = SetConsoleCtrlHandler(None, false);
    }
}
