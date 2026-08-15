//! Process discovery, liveness and tree-kill helpers.
//!
//! Windows process queries use the `windows` crate (windows-rs) instead of
//! hand-written FFI.

use std::process::Command;

/// Force-kill a process and, on Windows, its descendants.
///
/// On Windows `taskkill /F /T` walks the parent-child tree; on Unix we send
/// `SIGKILL`. Graceful stopping is done separately via
/// [`crate::process::AsyncChild::signal_stop`].
pub fn kill_tree(pid: u32) {
    if cfg!(target_os = "windows") {
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    } else {
        let _ = Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// True if the process identified by `pid` is still running.
pub fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(target_os = "windows")]
    {
        win_alive(pid)
    }
    #[cfg(unix)]
    {
        // SAFETY: kill(pid, 0) only probes for existence, it sends no signal.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
}

/// Windows process-liveness probe via `OpenProcess` / `GetExitCodeProcess`.
#[cfg(target_os = "windows")]
fn win_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// `STILL_ACTIVE` (winbase.h) — a process with this exit code is running.
    const STILL_ACTIVE: u32 = 259;

    unsafe {
        // SAFETY: OpenProcess with a limited query right; the handle is
        // closed on every path.
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut code: u32 = 0;
        let ok = GetExitCodeProcess(handle, &mut code).is_ok();
        let _ = CloseHandle(handle);
        ok && code == STILL_ACTIVE
    }
}

/// Find a process whose command line contains `needle`, returning its pid.
///
/// Used to track the external browser window webui launched (its command line
/// carries `--app=http://localhost:<port>`), so we can detect when the user
/// closes it. Prefer this over webui's own `get_child_process_id`, which
/// relies on the now-removed `wmic`.
pub fn find_process_by_cmdline(needle: &str) -> Option<u32> {
    #[cfg(target_os = "windows")]
    {
        // Restrict to browser binaries so the query never matches its own
        // PowerShell/cmd wrapper (whose command line also contains `needle`).
        let script = format!(
            "(Get-CimInstance Win32_Process | Where-Object {{ \
             $_.Name -match 'msedge|chrome|firefox|chromium|brave|vivaldi|opera|yandex|epic' -and \
             $_.CommandLine -and $_.CommandLine -match [regex]::Escape('{needle}') }} | \
             Select-Object -First 1).ProcessId"
        );
        let mut cmd = Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
        crate::process::run(&mut cmd)
            .ok()
            .and_then(|res| res.stdout.trim().parse::<u32>().ok())
    }
    #[cfg(unix)]
    {
        // pgrep excludes itself and we spawn it directly (no shell wrapper),
        // so the only `-f` match is the browser process.
        let mut cmd = Command::new("pgrep");
        cmd.args(["-f", needle]);
        crate::process::run(&mut cmd).ok().and_then(|res| {
            res.stdout
                .lines()
                .next()
                .and_then(|l| l.trim().parse::<u32>().ok())
        })
    }
}

/// Detect a running dsh instance anywhere on the system (launched manually
/// or supervised by another dshl), returning its pid. Used by the optional
/// `single-instance` mode to refuse starting a second dsh: two processes
/// appending to the same session log corrupt it permanently ("seq gap").
///
/// Matches the bun-compiled `dsh` binary by process name and the node
/// entry (`@deepseek-ai/dsh/lib/bin.js`) by command line, so it covers both
/// `dsh --profile web …` and a manual `dsh web` invocation.
pub fn dsh_instance_running() -> Option<u32> {
    #[cfg(target_os = "windows")]
    {
        let script = concat!(
            "(Get-CimInstance Win32_Process | Where-Object { ",
            "$_.Name -eq 'dsh.exe' -or ",
            "($_.Name -eq 'node.exe' -and $_.CommandLine -match '@deepseek-ai[\\/]dsh[\\/]lib[\\/]bin\\.js') ",
            "} | Select-Object -First 1).ProcessId"
        );
        let mut cmd = Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", script]);
        crate::process::run(&mut cmd)
            .ok()
            .and_then(|res| res.stdout.trim().parse::<u32>().ok())
    }
    #[cfg(unix)]
    {
        // pgrep -x matches the compiled `dsh` binary by exact process name;
        // pgrep -f covers the node entry (`…/dsh/lib/bin.js`).
        let mut cmd = Command::new("pgrep");
        cmd.args(["-x", "dsh"]);
        let direct = crate::process::run(&mut cmd).ok().and_then(|res| {
            res.stdout
                .lines()
                .next()
                .and_then(|l| l.trim().parse::<u32>().ok())
        });
        if direct.is_some() {
            return direct;
        }
        let mut cmd = Command::new("pgrep");
        cmd.args(["-f", "dsh/lib/bin.js"]);
        crate::process::run(&mut cmd).ok().and_then(|res| {
            res.stdout
                .lines()
                .next()
                .and_then(|l| l.trim().parse::<u32>().ok())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_alive_detects_death() {
        #[cfg(target_os = "windows")]
        let mut child = Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("spawn ping");
        #[cfg(unix)]
        let mut child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");

        let pid = child.id();
        assert!(process_alive(pid), "live process should be alive");

        child.kill().expect("kill child");
        let _ = child.wait();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!process_alive(pid), "killed process should be dead");
    }
}
