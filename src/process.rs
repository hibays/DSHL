//! Process helpers: a native-async child that streams output line by line,
//! plus synchronous capture and (on Windows) a hidden-console spawn so dsh can
//! be stopped gracefully with Ctrl+C.

use std::collections::VecDeque;
use std::future::Future;
use std::io::{self, BufRead, BufReader};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

/// One line of process output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    Stdout(String),
    Stderr(String),
}

/// Result of a synchronously captured command.
#[derive(Debug, Clone)]
pub struct CommandResult {
    pub stdout: String,
    pub stderr: String,
    pub status: Option<ExitStatus>,
}

impl CommandResult {
    pub fn success(&self) -> bool {
        self.status.map(|s| s.success()).unwrap_or(false)
    }

    pub fn code(&self) -> Option<i32> {
        self.status.and_then(|s| s.code())
    }
}

/// Apply a list of `(key, value)` environment variables to a command.
pub fn with_env<'a>(cmd: &'a mut Command, env: &[(String, String)]) -> &'a mut Command {
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd
}

/// Configure a command so its child runs without a console window (Windows)
/// and, on Linux, dies with the launcher (`PR_SET_PDEATHSIG`).
fn prepare_spawn(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(target_os = "linux")]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
}

/// Run a command to completion, capturing stdout/stderr (no shell).
pub fn run(cmd: &mut Command) -> io::Result<CommandResult> {
    prepare_spawn(cmd);
    let output = cmd.output()?;
    Ok(CommandResult {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        status: Some(output.status),
    })
}

/// How the child process handle is held.
#[cfg(windows)]
enum ProcessKind {
    Std(Child),
    /// Raw `HANDLE` stored as `usize` so it is `Send + Sync`.
    Raw(usize),
}
#[cfg(not(windows))]
enum ProcessKind {
    Std(Child),
}

struct Inner {
    /// The spawned pid, kept separately so `pid()`/`kill()` never contend with
    /// the reaper thread's `wait()`.
    pid: u32,
    process: Mutex<Option<ProcessKind>>,
    lines: Mutex<VecDeque<Output>>,
    done: Mutex<bool>,
    /// Exit code (`None` while running).
    code: Mutex<Option<i32>>,
    waker: Mutex<Option<Waker>>,
}

fn wake(inner: &Inner) {
    if let Some(w) = inner.waker.lock().unwrap().take() {
        w.wake();
    }
}

/// A child process whose stdout/stderr can be awaited line by line.
///
/// Two reader threads feed a shared queue while a third thread reaps the
/// process and, only after both streams have been fully drained, marks the
/// child as done. This makes the async contract sound: [`AsyncChild::next_line`]
/// returns `None` exactly once every output line has been delivered.
pub struct AsyncChild {
    inner: Arc<Inner>,
}

impl AsyncChild {
    pub fn spawn(cmd: &mut Command) -> io::Result<Self> {
        prepare_spawn(cmd);

        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .spawn()?;

        // On Windows, assign the child to a kill-on-close job object so it is
        // reaped automatically if the launcher is terminated abruptly.
        #[cfg(windows)]
        win_job::assign(&child);

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let pid = child.id();

        let inner = Arc::new(Inner {
            pid,
            process: Mutex::new(Some(ProcessKind::Std(child))),
            lines: Mutex::new(VecDeque::new()),
            done: Mutex::new(false),
            code: Mutex::new(None),
            waker: Mutex::new(None),
        });

        Self::start_readers(inner, stdout, stderr)
    }

    /// Spawn with a hidden console + new process group (Windows) so the child
    /// can later be stopped gracefully via Ctrl+C. Falls back to [`spawn`] on
    /// non-Windows.
    pub fn spawn_console(cmd: &mut Command) -> io::Result<Self> {
        #[cfg(windows)]
        {
            let spawned = win_proc::spawn_hidden_console(cmd)?;
            win_job::assign_raw(spawned.process);
            let inner = Arc::new(Inner {
                pid: spawned.pid,
                process: Mutex::new(Some(ProcessKind::Raw(spawned.process as usize))),
                lines: Mutex::new(VecDeque::new()),
                done: Mutex::new(false),
                code: Mutex::new(None),
                waker: Mutex::new(None),
            });
            Self::start_readers(inner, spawned.stdout, spawned.stderr)
        }
        #[cfg(not(windows))]
        {
            Self::spawn(cmd)
        }
    }

    fn start_readers(
        inner: Arc<Inner>,
        stdout: impl std::io::Read + Send + 'static,
        stderr: impl std::io::Read + Send + 'static,
    ) -> io::Result<Self> {
        let h1 = spawn_reader(inner.clone(), stdout, true);
        let h2 = spawn_reader(inner.clone(), stderr, false);

        {
            let inner = inner.clone();
            std::thread::spawn(move || {
                // Take the process out of the mutex first so `pid()`/`kill()` are
                // never blocked while this thread waits.
                let taken = inner.process.lock().unwrap().take();
                let code = match taken {
                    Some(ProcessKind::Std(mut c)) => c.wait().ok().and_then(|s| s.code()),
                    #[cfg(windows)]
                    Some(ProcessKind::Raw(h)) => {
                        win_proc::wait_handle(h as std::os::windows::io::RawHandle)
                    }
                    None => None,
                };
                // Drain both streams completely before declaring done.
                let _ = h1.join();
                let _ = h2.join();
                *inner.code.lock().unwrap() = code;
                *inner.done.lock().unwrap() = true;
                wake(&inner);
            });
        }

        Ok(Self { inner })
    }

    /// Await the next stdout/stderr line. `None` once the process exited and
    /// all output has been drained.
    pub fn next_line(&self) -> NextLine<'_> {
        NextLine { inner: &self.inner }
    }

    /// Process id of the spawned child.
    pub fn pid(&self) -> Option<u32> {
        Some(self.inner.pid)
    }

    /// Send a graceful stop signal: Ctrl+C on Windows, SIGTERM on Unix.
    pub fn signal_stop(&self) {
        #[cfg(windows)]
        win_proc::send_ctrl_c(self.inner.pid);
        #[cfg(unix)]
        {
            // SAFETY: kill(pid, SIGTERM) sends a catchable termination signal.
            unsafe { libc::kill(self.inner.pid as libc::pid_t, libc::SIGTERM) };
        }
    }

    /// Force-kill the process (best-effort).
    pub fn kill(&self) -> io::Result<()> {
        crate::platform::kill_tree(self.inner.pid);
        Ok(())
    }

    /// Gracefully stop the child (Ctrl+C on Windows / SIGTERM on Unix) and
    /// wait up to `grace_ms` for it to exit on its own, re-sending the stop
    /// signal every few seconds while it is still alive.
    ///
    /// The process is **never** force-killed here: Ctrl+C is the correct way
    /// to close dsh — it commits its session log during shutdown and its own
    /// shutdown logic force-exits at most 5s after the signal. A forced kill
    /// could interrupt that write; and the permanent "corrupt session log:
    /// seq gap" damage comes from TWO processes appending to the same log, so
    /// callers must wait for the child to actually exit (or abort the launch)
    /// before starting a replacement. Returns `true` when the child exited
    /// within the grace period.
    pub fn graceful_kill(&self, grace_ms: u64) -> bool {
        self.signal_stop();
        let start = std::time::Instant::now();
        let mut last_stop = start;
        let re_send = std::time::Duration::from_secs(5);
        while start.elapsed().as_millis() < grace_ms as u128 {
            if !crate::platform::process_alive(self.inner.pid) {
                return true;
            }
            // The first Ctrl+C can be lost (child busy / mid-console-init);
            // re-send periodically so a slow graceful shutdown still happens.
            if last_stop.elapsed() >= re_send {
                last_stop = std::time::Instant::now();
                self.signal_stop();
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        let alive = crate::platform::process_alive(self.inner.pid);
        if alive {
            crate::debug::emit(&format!(
                "graceful_kill: pid {} still alive after {grace_ms}ms; left running (no force kill)",
                self.inner.pid
            ));
        }
        !alive
    }

    /// Exit code once the process has finished (`None` while running).
    pub fn exit_code(&self) -> Option<i32> {
        *self.inner.code.lock().unwrap()
    }

    /// Drain all remaining lines, returning the exit code.
    pub async fn drain(self) -> Option<i32> {
        while self.next_line().await.is_some() {}
        self.exit_code()
    }
}

fn spawn_reader(
    inner: Arc<Inner>,
    stream: impl std::io::Read + Send + 'static,
    is_stdout: bool,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            match line {
                Ok(l) => {
                    let out = if is_stdout {
                        Output::Stdout(l)
                    } else {
                        Output::Stderr(l)
                    };
                    inner.lines.lock().unwrap().push_back(out);
                    wake(&inner);
                }
                Err(_) => break,
            }
        }
    })
}

/// Future returned by [`AsyncChild::next_line`].
pub struct NextLine<'a> {
    inner: &'a Inner,
}

impl Future for NextLine<'_> {
    type Output = Option<Output>;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let line = self.inner.lines.lock().unwrap().pop_front();
        if let Some(l) = line {
            return Poll::Ready(Some(l));
        }
        if *self.inner.done.lock().unwrap() {
            return Poll::Ready(None);
        }
        *self.inner.waker.lock().unwrap() = Some(cx.waker().clone());
        Poll::Pending
    }
}

/// Windows hidden-console process spawn + graceful stop helpers.
#[cfg(windows)]
mod win_proc {
    use std::ffi::OsString;
    use std::fs::File;
    use std::os::raw::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{FromRawHandle, RawHandle};
    use std::process::Command;

    #[repr(C)]
    struct StartupInfoW {
        cb: u32,
        _reserved: *mut u16,
        _desktop: *mut u16,
        _title: *mut u16,
        _x: u32,
        _y: u32,
        _x_size: u32,
        _y_size: u32,
        _x_chars: u32,
        _y_chars: u32,
        _fill: u32,
        flags: u32,
        show_window: u16,
        _reserved2: u16,
        _reserved2_ptr: *mut u8,
        std_input: RawHandle,
        std_output: RawHandle,
        std_error: RawHandle,
    }

    #[repr(C)]
    struct ProcessInformation {
        process: RawHandle,
        thread: RawHandle,
        process_id: u32,
        thread_id: u32,
    }

    const STARTF_USESTDHANDLES: u32 = 0x100;
    const STARTF_USESHOWWINDOW: u32 = 0x1;
    const SW_HIDE: u16 = 0;
    const CREATE_NEW_CONSOLE: u32 = 0x10;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
    const CREATE_UNICODE_ENVIRONMENT: u32 = 0x400;
    const HANDLE_FLAG_INHERIT: u32 = 0x1;
    const CTRL_C_EVENT: u32 = 0;
    const INFINITE: u32 = 0xFFFF_FFFF;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreatePipe(
            read: *mut RawHandle,
            write: *mut RawHandle,
            attrs: *mut c_void,
            size: u32,
        ) -> i32;
        fn SetHandleInformation(h: RawHandle, mask: u32, flags: u32) -> i32;
        fn CreateProcessW(
            app: *const u16,
            cmdline: *mut u16,
            pa: *mut c_void,
            ta: *mut c_void,
            inherit: i32,
            flags: u32,
            env: *mut c_void,
            cwd: *const u16,
            si: *mut StartupInfoW,
            pi: *mut ProcessInformation,
        ) -> i32;
        fn CloseHandle(h: RawHandle) -> i32;
        fn WaitForSingleObject(h: RawHandle, ms: u32) -> u32;
        fn GetExitCodeProcess(h: RawHandle, code: *mut u32) -> i32;
        fn GenerateConsoleCtrlEvent(event: u32, group: u32) -> i32;
        fn AttachConsole(pid: u32) -> i32;
        fn FreeConsole() -> i32;
        fn SetConsoleCtrlHandler(handler: *mut c_void, add: i32) -> i32;
    }

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

    pub fn spawn_hidden_console(cmd: &Command) -> std::io::Result<Spawned> {
        // Create pipes for stdout/stderr.
        let (out_r, out_w) = create_pipe()?;
        let (err_r, err_w) = create_pipe()?;

        let app = wide(&cmd.get_program().to_string_lossy());
        let mut cmdline = build_cmdline(cmd);
        let mut env_block = build_env_block(cmd);

        let mut si = StartupInfoW {
            cb: std::mem::size_of::<StartupInfoW>() as u32,
            _reserved: std::ptr::null_mut(),
            _desktop: std::ptr::null_mut(),
            _title: std::ptr::null_mut(),
            _x: 0,
            _y: 0,
            _x_size: 0,
            _y_size: 0,
            _x_chars: 0,
            _y_chars: 0,
            _fill: 0,
            flags: STARTF_USESTDHANDLES | STARTF_USESHOWWINDOW,
            show_window: SW_HIDE,
            _reserved2: 0,
            _reserved2_ptr: std::ptr::null_mut(),
            std_input: std::ptr::null_mut(),
            std_output: out_w,
            std_error: err_w,
        };
        let mut pi = ProcessInformation {
            process: std::ptr::null_mut(),
            thread: std::ptr::null_mut(),
            process_id: 0,
            thread_id: 0,
        };

        // SAFETY: all pointers are valid for the duration of the call; the
        // pipe handles are closed on error paths.
        let ok = unsafe {
            CreateProcessW(
                app.as_ptr(),
                cmdline.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                1, // inherit handles (for the stdio pipes)
                CREATE_NEW_CONSOLE | CREATE_NEW_PROCESS_GROUP | CREATE_UNICODE_ENVIRONMENT,
                env_block.as_mut_ptr() as *mut c_void,
                std::ptr::null(),
                &mut si,
                &mut pi,
            )
        };

        // Close our copies of the write ends regardless.
        unsafe {
            CloseHandle(out_w);
            CloseHandle(err_w);
        }

        if ok == 0 {
            unsafe {
                CloseHandle(out_r);
                CloseHandle(err_r);
                if !pi.process.is_null() {
                    CloseHandle(pi.process);
                }
                if !pi.thread.is_null() {
                    CloseHandle(pi.thread);
                }
            }
            return Err(std::io::Error::last_os_error());
        }

        unsafe {
            CloseHandle(pi.thread); // we don't need the thread handle
        }

        Ok(Spawned {
            process: pi.process,
            pid: pi.process_id,
            // SAFETY: the read ends are owned by us.
            stdout: unsafe { File::from_raw_handle(out_r) },
            stderr: unsafe { File::from_raw_handle(err_r) },
        })
    }

    fn create_pipe() -> std::io::Result<(RawHandle, RawHandle)> {
        let mut read: RawHandle = std::ptr::null_mut();
        let mut write: RawHandle = std::ptr::null_mut();
        // SAFETY: null attrs/size use the defaults.
        let ok = unsafe { CreatePipe(&mut read, &mut write, std::ptr::null_mut(), 0) };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        // The child must inherit the write end; the parent keeps the read end.
        unsafe {
            SetHandleInformation(write, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
        }
        Ok((read, write))
    }

    /// Wait for the process to exit and return its exit code.
    pub fn wait_handle(handle: RawHandle) -> Option<i32> {
        // SAFETY: handle is a valid process handle.
        unsafe {
            WaitForSingleObject(handle, INFINITE);
            let mut code: u32 = 0;
            if GetExitCodeProcess(handle, &mut code) != 0 {
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
            let _ = SetConsoleCtrlHandler(std::ptr::null_mut(), 1);
            FreeConsole();
            if AttachConsole(pid) != 0 {
                // The ignore flag is PER-CONSOLE: it applied to our previous
                // console above, but we just attached to the CHILD's console.
                // Without re-ignoring here, the broadcast below delivers a
                // CTRL_C to US as well, which the default handler turns into
                // STATUS_CONTROL_C_EXIT (0xc000013a) — the launcher dies
                // "on Ctrl+C" instead of exiting cleanly.
                let _ = SetConsoleCtrlHandler(std::ptr::null_mut(), 1);
                let _ = GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0);
                // Ctrl+C delivery is asynchronous: keep ignoring briefly so
                // the event cannot land after the ignore flag is removed.
                std::thread::sleep(std::time::Duration::from_millis(100));
                let _ = SetConsoleCtrlHandler(std::ptr::null_mut(), 0);
                let _ = FreeConsole();
            }
            let _ = SetConsoleCtrlHandler(std::ptr::null_mut(), 0);
        }
    }
}

/// Windows job-object helpers: assign spawned children to a kill-on-close job
/// so the OS reaps them automatically when the launcher process exits (even on
/// `TerminateProcess`).
#[cfg(windows)]
mod win_job {
    use std::os::windows::io::RawHandle;
    use std::sync::OnceLock;

    #[repr(C)]
    #[derive(Default)]
    struct JobObjectBasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    /// `IO_COUNTERS` (the second member of the extended limit information).
    #[repr(C)]
    #[derive(Default)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    /// `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` — used instead of the basic
    /// variant: some environments (sandboxes/CI runners) reject
    /// `JobObjectBasicLimitInformation` with ERROR_INVALID_PARAMETER even for
    /// a correct struct, while the extended class works everywhere.
    #[repr(C)]
    #[derive(Default)]
    struct JobObjectExtendedLimitInformation {
        basic: JobObjectBasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(security: *mut core::ffi::c_void, name: *const u16) -> RawHandle;
        fn SetInformationJobObject(
            job: RawHandle,
            class: i32,
            info: *const core::ffi::c_void,
            len: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: RawHandle, process: RawHandle) -> i32;
    }

    static JOB: OnceLock<usize> = OnceLock::new();

    fn job_handle() -> RawHandle {
        let raw = *JOB.get_or_init(|| {
            // SAFETY: all FFI calls are guarded; null input is allowed.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
                if handle.is_null() {
                    crate::debug::emit(&format!(
                        "win_job: CreateJobObjectW failed: {}",
                        std::io::Error::last_os_error()
                    ));
                    return 0usize;
                }
                let info = JobObjectExtendedLimitInformation {
                    basic: JobObjectBasicLimitInformation {
                        limit_flags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let ok = SetInformationJobObject(
                    handle,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JobObjectExtendedLimitInformation>() as u32,
                );
                if ok == 0 {
                    crate::debug::emit(&format!(
                        "win_job: SetInformationJobObject failed: {}",
                        std::io::Error::last_os_error()
                    ));
                    return 0usize;
                }
                handle as usize
            }
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
        let ok = unsafe { AssignProcessToJobObject(job, child.as_raw_handle()) };
        if ok == 0 {
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
        let ok = unsafe { AssignProcessToJobObject(job, process) };
        if ok == 0 {
            crate::debug::emit(&format!(
                "win_job: AssignProcessToJobObject failed: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
}
