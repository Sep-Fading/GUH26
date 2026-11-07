//! Starts an approved Windows command suspended, contains its descendants, then resumes it.

// Owns handles and collects bounded output.
use std::{fs::File, io::{self, Read}, os::windows::io::FromRawHandle,
    path::Path, ptr::{null, null_mut}, sync::Arc};
// Calls native creation and containment APIs.
use windows_sys::Win32::{Foundation::*, Security::{Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1}, SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES},
    Storage::FileSystem::*, System::{Console::*, JobObjects::*, Pipes::CreatePipe, Threading::*}};

struct Handle(HANDLE);
// Handles remain valid until their last owner is dropped.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Drop for Handle {
    /// Releases one owned kernel handle.
    fn drop(&mut self) { unsafe { CloseHandle(self.0); } }
}

struct Job(Arc<Handle>, String);
impl Job {
    /// Creates an unpredictable private job owned by the actual user, including elevated callers.
    fn new() -> io::Result<Self> { Self::create(false) }

    /// Creates the job; `breakaway` lets a member start a child outside it, but only one created
    /// with CREATE_BREAKAWAY_FROM_JOB (never silently). Only the Claude sign-in uses it.
    fn create(breakaway: bool) -> io::Result<Self> {
        let owner = crate::windows_network_broker::current_user_sid()?;
        let sddl = wide(&format!("O:{owner}D:P(A;;GA;;;{owner})(A;;GA;;;SY)"))?;
        let mut descriptor = null_mut();
        if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1,
            &mut descriptor, null_mut()) } == 0 { return Err(io::Error::last_os_error()); }
        let security = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor, bInheritHandle: 0 };
        let name = format!("Global\\MiniClaw.Job.{}", uuid::Uuid::new_v4().simple());
        let encoded = wide(&name)?;
        unsafe { SetLastError(ERROR_SUCCESS); }
        let raw = unsafe { CreateJobObjectW(&security, encoded.as_ptr()) };
        let error = unsafe { GetLastError() };
        unsafe { LocalFree(descriptor); }
        if raw.is_null() { return Err(io::Error::from_raw_os_error(error as i32)); }
        let handle = Handle(raw);
        // Never modify a pre-existing kernel object, even on an improbable UUID collision.
        if error == ERROR_ALREADY_EXISTS { return Err(io::ErrorKind::AlreadyExists.into()); }
        let job = Self(Arc::new(handle), name);
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        // A process that crashes ends at once (SEM_NOGPFAULTERRORBOX for every process in the job, its
        // descendants included). For a sandboxed process Windows Error Reporting never finishes, so a crash
        // otherwise hangs until killed. AppContainer processes also have strict handle checks, so a stale
        // handle is such a crash: Git for Windows calls SetConsoleMode at exit on the standard output
        // handle it has just closed (compat/winansi.c reset_std_console after git.c fclose(stdout)), and
        // every Git command typed in the sandboxed terminal hung after printing its output.
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION
            | if breakaway { JOB_OBJECT_LIMIT_BREAKAWAY_OK } else { 0 };
        if unsafe { SetInformationJobObject(job.0.0, JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(), size_of_val(&limits) as u32) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }
}

impl Drop for Job {
    /// Cancellation kills every contained descendant before releasing the job handle.
    fn drop(&mut self) { unsafe { TerminateJobObject(self.0.0, 1); } }
}

struct Attributes { list: LPPROC_THREAD_ATTRIBUTE_LIST, _storage: Vec<usize> }
impl Drop for Attributes {
    /// Releases native attribute allocations before their backing buffer.
    fn drop(&mut self) { unsafe { DeleteProcThreadAttributeList(self.list); } }
}

/// Converts a string to a terminated UTF-16 buffer, rejecting embedded terminators.
fn wide(value: &str) -> io::Result<Vec<u16>> {
    if value.contains('\0') { return Err(io::ErrorKind::InvalidInput.into()); }
    Ok(value.encode_utf16().chain(Some(0)).collect())
}

/// Quotes an argv item using Windows backslash rules, never cmd.exe interpretation.
fn quote(value: &str) -> String {
    if !value.is_empty() && !value.chars().any(|c| c.is_whitespace() || c == '"') {
        return value.to_owned();
    }
    let mut result = String::from("\"");
    let mut slashes = 0;
    for c in value.chars() {
        if c == '\\' { slashes += 1; continue; }
        result.extend(std::iter::repeat_n('\\', if c == '"' { slashes * 2 + 1 } else { slashes }));
        slashes = 0;
        result.push(c);
    }
    result.extend(std::iter::repeat_n('\\', slashes * 2));
    result.push('"');
    result
}

/// Creates one inherited write pipe and a private reader for the parent.
fn output_pipe() -> io::Result<(File, Handle)> {
    let (mut read, mut write) = (null_mut(), null_mut());
    let handle_security = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(), bInheritHandle: 1 };
    unsafe {
        if CreatePipe(&mut read, &mut write, &handle_security, 0) == 0 { return Err(io::Error::last_os_error()); }
        let read = Handle(read);
        let write = Handle(write);
        if SetHandleInformation(read.0, HANDLE_FLAG_INHERIT, 0) == 0 { return Err(io::Error::last_os_error()); }
        let file = File::from_raw_handle(read.0);
        std::mem::forget(read);
        Ok((file, write))
    }
}

/// Borrowed native isolation attributes; their SID storage must outlive process creation.
pub struct SecurityContext<'a> {
    pub capabilities: &'a SECURITY_CAPABILITIES,
    pub low_privilege: bool,
}

/// Owns all values referenced by the native startup attribute list until CreateProcess returns.
struct StartupAttributes {
    attributes: Attributes,
    _policy: Box<u32>,
    _job: Box<HANDLE>,
}

/// Combines the exact stdio handle whitelist or pseudoconsole with mandatory isolation.
fn attributes(handles: Option<&mut [HANDLE; 3]>, console: Option<HPCON>, security: Option<&SecurityContext<'_>>, job: HANDLE) -> io::Result<StartupAttributes> {
    unsafe {
        let count = 1 + handles.is_some() as u32 + console.is_some() as u32
            + security.map_or(0, |s| 1 + s.low_privilege as u32);
        let mut bytes = 0;
        InitializeProcThreadAttributeList(null_mut(), count, 0, &mut bytes);
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        let list = storage.as_mut_ptr().cast();
        if InitializeProcThreadAttributeList(list, count, 0, &mut bytes) == 0 { return Err(io::Error::last_os_error()); }
        let attrs = StartupAttributes { attributes: Attributes { list, _storage: storage }, _policy: Box::new(1), _job: Box::new(job) };
        let update = |key: u32, value: *const std::ffi::c_void, size: usize| {
            if UpdateProcThreadAttribute(list, 0, key as usize, value, size, null_mut(), null()) == 0 {
                Err(io::Error::last_os_error())
            } else { Ok(()) }
        };
        // Atomic job assignment closes the crash window between CreateProcess and AssignProcessToJobObject.
        update(PROC_THREAD_ATTRIBUTE_JOB_LIST, (&*attrs._job as *const HANDLE).cast(), size_of::<HANDLE>())?;
        if let Some(handles) = handles {
            update(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, handles.as_ptr().cast(), size_of_val(handles))?;
        }
        if let Some(console) = console {
            update(PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, console as *const _, size_of::<HPCON>())?;
        }
        if let Some(security) = security {
            update(PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, (security.capabilities as *const SECURITY_CAPABILITIES).cast(), size_of::<SECURITY_CAPABILITIES>())?;
            if security.low_privilege {
                update(PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY, (&*attrs._policy as *const u32).cast(), size_of::<u32>())?;
            }
        }
        Ok(attrs)
    }
}

/// Retains only OS and lookup variables from an untrusted inherited environment.
fn inherited_environment(values: impl IntoIterator<Item = (String, String)>) -> Vec<(String, String)> {
    let allowed = ["SYSTEMROOT", "WINDIR", "USERPROFILE", "HOME", "TEMP", "TMP", "PATH", "PATHEXT",
        "COMSPEC", "LOCALAPPDATA", "APPDATA", "PROGRAMFILES", "PROGRAMDATA", "LANG"];
    values.into_iter().filter(|(name, _)| allowed.contains(&name.to_uppercase().as_str())).collect()
}

/// Adds only deliberately supplied server secrets and emits a sorted Windows environment block.
/// A sandboxed process also gets the Node flags it needs and, when its sandbox has the private Node,
/// MiniClaw's Node preload, added to any `NODE_OPTIONS` it was given, and MiniClaw's Python hook
/// first on any `PYTHONPATH` it was given, when its sandbox has the hook.
fn environment(extra: &[(String, String)], sandboxed: bool) -> io::Result<Vec<u16>> {
    let mut entries = std::collections::BTreeMap::new();
    for (name, value) in inherited_environment(std::env::vars_os().filter_map(|(name, value)|
        Some((name.into_string().ok()?, value.into_string().ok()?)))).into_iter().chain(extra.iter().cloned()) {
        if name.is_empty() || name.contains(['=', '\0']) || value.contains('\0') {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        entries.insert(name.to_uppercase(), value);
    }
    if sandboxed {
        crate::windows_runtimes::node_environment(&mut entries);
        crate::windows_runtimes::python_environment(&mut entries);
    }
    entries.remove(crate::windows_runtimes::PYTHON_HOOK_FOLDER);
    entries.remove(crate::windows_runtimes::NODE_PRELOAD_PATH);
    let mut block: Vec<u16> = entries.iter().flat_map(|(name, value)|
        format!("{name}={value}").encode_utf16().chain(Some(0)).collect::<Vec<_>>()).collect();
    block.push(0);
    if entries.is_empty() { block.push(0); }
    Ok(block)
}

struct Running { job: Job, process: Arc<Handle>, input: Option<File>, output: File, errors: File, thread: Option<Handle> }

/// Shares only the verified job lifetime for deferred sandbox-resource cleanup.
#[derive(Clone)]
pub struct TreeGuard(Arc<Handle>);
impl TreeGuard {
    /// Reopens a recorded restricted job only after kernel ownership and token validation.
    pub fn open_verified(name: &str, profile: &str) -> io::Result<Self> {
        use std::os::windows::io::IntoRawHandle;
        let handle = crate::windows_network_broker::recovery_job(name, profile)?;
        Ok(Self(Arc::new(Handle(handle.into_raw_handle()))))
    }

    /// Terminates every descendant and proves the job empty before its resource grants are removed.
    pub fn terminate_and_wait(&self, timeout: std::time::Duration) -> io::Result<()> {
        let deadline = std::time::Instant::now() + timeout;
        let mut terminated = false;
        loop {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
            if unsafe { QueryInformationJobObject(self.0.0, JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(), size_of_val(&info) as u32, null_mut()) } == 0 {
                return Err(io::Error::last_os_error());
            }
            if info.ActiveProcesses == 0 { return Ok(()); }
            if !terminated {
                if unsafe { TerminateJobObject(self.0.0, 1) } == 0 { return Err(io::Error::last_os_error()); }
                terminated = true;
            }
            if std::time::Instant::now() >= deadline { return Err(io::ErrorKind::TimedOut.into()); }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

/// Keeps a native interactive server and all descendants alive only while its connection exists.
pub struct Process { _job: Job, _process: Arc<Handle> }

impl Process {
    /// Lets sandbox resources outlive asynchronous process termination during cancellation.
    pub fn tree_guard(&self) -> TreeGuard { TreeGuard(self._job.0.clone()) }

    /// Captures only a termination handle for a network-disconnect watcher, not job ownership.
    pub fn cancellation(&self) -> impl FnOnce() + Send + use<> {
        let job = self._job.0.clone();
        move || { unsafe { TerminateJobObject(job.0, 1); } }
    }

    /// Identifies this process's private job for the authenticated network-only broker.
    pub fn job_descriptor(&self) -> (String, u64) { (self._job.1.clone(), self._job.0.0 as usize as u64) }

    /// Polls the process handle without treating an active exit-code value as completion.
    pub fn try_wait(&self) -> io::Result<Option<u32>> {
        match unsafe { WaitForSingleObject(self._process.0, 0) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0;
                if unsafe { GetExitCodeProcess(self._process.0, &mut code) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Some(code))
            }
            _ => Err(io::Error::last_os_error()),
        }
    }

    /// Kills the full job immediately while keeping handles valid for a final wait.
    pub fn kill(&self) -> io::Result<()> {
        if unsafe { TerminateJobObject(self._job.0.0, 1) } == 0 { return Err(io::Error::last_os_error()); }
        Ok(())
    }

    /// Watches root exit without extending the job owner lifetime on cancellation.
    pub fn watch_exit(&self) -> tokio::sync::oneshot::Receiver<io::Result<u32>> {
        let process = self._process.clone();
        let job = self._job.0.clone();
        let (send, recv) = tokio::sync::oneshot::channel();
        tokio::task::spawn_blocking(move || unsafe {
            let result = if WaitForSingleObject(process.0, INFINITE) != WAIT_OBJECT_0 {
                Err(io::Error::last_os_error())
            } else {
                let mut code = 0;
                if GetExitCodeProcess(process.0, &mut code) == 0 { Err(io::Error::last_os_error()) } else { Ok(code) }
            };
            TerminateJobObject(job.0, 1);
            let _ = send.send(result);
        });
        recv
    }

    /// Waits for one root process without blocking the async executor.
    pub async fn wait(&self) -> io::Result<u32> {
        let process = self._process.clone();
        tokio::task::spawn_blocking(move || unsafe {
            if WaitForSingleObject(process.0, INFINITE) != WAIT_OBJECT_0 { return Err(io::Error::last_os_error()); }
            let mut code = 0;
            if GetExitCodeProcess(process.0, &mut code) == 0 { return Err(io::Error::last_os_error()); }
            Ok(code)
        }).await.map_err(io::Error::other)?
    }
}

/// Returns interactive native pipes plus the job guard that owns their server lifetime.
pub struct StdioProcess {
    pub process: Process,
    pub input: File,
    pub output: File,
    pub errors: File,
}

/// Creates a private parent writer and inherited child reader for interactive stdin.
fn input_pipe() -> io::Result<(File, Handle)> {
    let (mut read, mut write) = (null_mut(), null_mut());
    let handle_security = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(), bInheritHandle: 1 };
    unsafe {
        if CreatePipe(&mut read, &mut write, &handle_security, 0) == 0 { return Err(io::Error::last_os_error()); }
        let read = Handle(read);
        let write = Handle(write);
        if SetHandleInformation(write.0, HANDLE_FLAG_INHERIT, 0) == 0 { return Err(io::Error::last_os_error()); }
        let file = File::from_raw_handle(write.0);
        std::mem::forget(write);
        Ok((file, read))
    }
}

/// Starts an explicitly unrestricted MCP server with job containment and deliberate secrets only.
pub fn stdio(program: &Path, args: &[String], cwd: &Path, extra: &[(String, String)]) -> io::Result<StdioProcess> {
    stdio_running(spawn(program, args, cwd, extra, true, None, false)?)
}

/// Starts a provider's sign-in like `stdio`, in a job whose members may start a child outside it
/// with CREATE_BREAKAWAY_FROM_JOB: the browser opener uses that so the browser outlives the CLI.
pub fn stdio_allowing_breakaway(program: &Path, args: &[String], cwd: &Path, extra: &[(String, String)]) -> io::Result<StdioProcess> {
    stdio_running(spawn_in(program, args, cwd, extra, true, None, false, true)?)
}

/// Starts an MCP server with mandatory AppContainer attributes and no isolation fallback.
pub fn stdio_restricted(program: &Path, args: &[String], cwd: &Path, extra: &[(String, String)],
    security: &SecurityContext<'_>) -> io::Result<StdioProcess> {
    stdio_running(spawn(program, args, cwd, extra, true, Some(security), false)?)
}

/// Watches a contained interactive process while its connection owns the job lifetime.
fn stdio_running(running: Running) -> io::Result<StdioProcess> {
    let Running { job, process, input, output, errors, .. } = running;
    let watched = process.clone();
    let job_handle = job.0.clone();
    tokio::task::spawn_blocking(move || unsafe {
        if WaitForSingleObject(watched.0, INFINITE) == WAIT_OBJECT_0 {
            // Descendants must not keep a dead provider's pipes open indefinitely.
            TerminateJobObject(job_handle.0, 1);
        }
    });
    Ok(StdioProcess { process: Process { _job: job, _process: process }, input: input.unwrap(), output, errors })
}

/// Retains a suspended primary thread until its network enforcement lease is installed.
pub struct PendingStdio { pub server: StdioProcess, thread: Handle }
impl PendingStdio {
    /// Resumes only after the caller has retained every required security guard.
    pub fn resume(self) -> io::Result<StdioProcess> {
        if unsafe { ResumeThread(self.thread.0) } == u32::MAX { return Err(io::Error::last_os_error()); }
        Ok(self.server)
    }
}

/// Creates a restricted server without running any of its code before broker authentication.
pub fn pending_stdio_restricted(program: &Path, args: &[String], cwd: &Path, extra: &[(String, String)],
    security: &SecurityContext<'_>) -> io::Result<PendingStdio> {
    let mut running = spawn(program, args, cwd, extra, true, Some(security), true)?;
    let thread = running.thread.take().ok_or(io::ErrorKind::Other)?;
    Ok(PendingStdio { server: stdio_running(running)?, thread })
}

/// Captures a resumed restricted server while its owning future keeps job cancellation effective.
pub async fn capture_stdio(server: StdioProcess) -> io::Result<(Vec<u8>, Vec<u8>, u32)> {
    let StdioProcess { process, input, output, errors } = server;
    drop(input);
    let output = tokio::task::spawn_blocking(move || read(output));
    let errors = tokio::task::spawn_blocking(move || read(errors));
    let status = process.wait().await?;
    process.kill()?;
    let (output, errors) = tokio::try_join!(output, errors).map_err(io::Error::other)?;
    let (output, errors) = (output?, errors?);
    if output.len() + errors.len() > 65_536 { return Err(io::ErrorKind::InvalidData.into()); }
    Ok((output, errors, status))
}

/// Creates the child suspended so no command runs before job containment succeeds.
fn spawn(program: &Path, args: &[String], cwd: &Path, extra: &[(String, String)], interactive: bool, security: Option<&SecurityContext<'_>>, suspended: bool) -> io::Result<Running> {
    spawn_in(program, args, cwd, extra, interactive, security, suspended, false)
}

/// Creates the child like `spawn`; `breakaway` allows explicit breakaway from its job.
#[allow(clippy::too_many_arguments)]
fn spawn_in(program: &Path, args: &[String], cwd: &Path, extra: &[(String, String)], interactive: bool, security: Option<&SecurityContext<'_>>, suspended: bool, breakaway: bool) -> io::Result<Running> {
    // CreateProcessW would hand a batch file to cmd.exe, which parses the arguments again.
    crate::provider_process::native_image(program)?;
    let app = wide(&program.to_string_lossy())?;
    let cwd = wide(&cwd.to_string_lossy())?;
    let line = std::iter::once(program.to_string_lossy().into_owned()).chain(args.iter().cloned())
        .map(|item| quote(&item)).collect::<Vec<_>>().join(" ");
    let mut line = wide(&line)?;
    if line.len() > 32_767 { return Err(io::ErrorKind::InvalidInput.into()); }
    let mut env = environment(extra, security.is_some())?;
    // Python asks Windows for its manifest's activation context, which the sandbox cannot build.
    if security.is_some() { crate::windows_runtimes::prime(program); }
    let (output, output_write) = output_pipe()?;
    let (errors, error_write) = output_pipe()?;
    unsafe {
        let job = Job::create(breakaway)?;
        let handle_security = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null_mut(), bInheritHandle: 1 };
        let (parent_input, input) = if interactive {
            let (parent, child) = input_pipe()?;
            (Some(parent), child)
        } else {
            let child = Handle(CreateFileW(wide("NUL")?.as_ptr(), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE,
                &handle_security, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, null_mut()));
            if child.0 == INVALID_HANDLE_VALUE { return Err(io::Error::last_os_error()); }
            (None, child)
        };
        let mut inherited = [input.0, output_write.0, error_write.0];
        let attrs = attributes(Some(&mut inherited), None, security, job.0.0)?;
        let mut startup: STARTUPINFOEXW = std::mem::zeroed();
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = input.0;
        startup.StartupInfo.hStdOutput = output_write.0;
        startup.StartupInfo.hStdError = error_write.0;
        startup.lpAttributeList = attrs.attributes.list;
        let mut process: PROCESS_INFORMATION = std::mem::zeroed();
        if CreateProcessW(app.as_ptr(), line.as_mut_ptr(), null(), null(), 1,
            CREATE_SUSPENDED | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
            env.as_mut_ptr().cast(), cwd.as_ptr(), &startup.StartupInfo, &mut process) == 0 {
            return Err(io::Error::last_os_error());
        }
        let process_handle = Arc::new(Handle(process.hProcess));
        let thread = Handle(process.hThread);
        if !suspended && ResumeThread(thread.0) == u32::MAX { return Err(io::Error::last_os_error()); }
        Ok(Running { job, process: process_handle, input: parent_input, output, errors, thread: if suspended { Some(thread) } else { None } })
    }
}

/// Owns a pseudoconsole; closure runs separately while the output consumer drains final bytes.
#[derive(Clone)]
pub struct Console(Arc<ConsoleHandle>);

struct ConsoleHandle(std::sync::Mutex<HPCON>, Arc<crate::windows_conpty::Api>);
impl Console {
    /// Updates dimensions while preventing concurrent pseudoconsole closure.
    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        let size = dimensions(cols, rows)?;
        let console = self.0.0.lock().map_err(|_| io::Error::other("console lock poisoned"))?;
        if *console == 0 { return Err(io::ErrorKind::BrokenPipe.into()); }
        let result = unsafe { (self.0.1.resize)(*console, size) };
        if result < 0 { return Err(io::Error::from_raw_os_error(result)); }
        Ok(())
    }

    /// Closes once on a dedicated thread while the output consumer continues draining.
    pub fn close(&self) {
        let console = std::mem::take(&mut *self.0.0.lock().unwrap_or_else(|e| e.into_inner()));
        if console != 0 {
            let api = self.0.1.clone();
            std::thread::spawn(move || unsafe { (api.close)(console); });
        }
    }
}
impl Drop for ConsoleHandle {
    /// The final owner closes once even when controllers are dropped concurrently.
    fn drop(&mut self) {
        let console = std::mem::take(self.0.get_mut().unwrap_or_else(|e| e.into_inner()));
        if console != 0 {
            let api = self.1.clone();
            std::thread::spawn(move || unsafe { (api.close)(console); });
        }
    }
}

/// Native terminal transport; the output must be drained concurrently until console closure.
pub struct PtyProcess {
    pub process: Process,
    pub console: Console,
    pub input: File,
    pub output: Option<File>,
    thread: Option<Handle>,
}

impl PtyProcess {
    /// Resumes a pending terminal only after its network lease has been installed.
    pub fn resume(&mut self) -> io::Result<()> {
        if let Some(thread) = self.thread.take() {
            if unsafe { ResumeThread(thread.0) } == u32::MAX { return Err(io::Error::last_os_error()); }
        }
        Ok(())
    }
}

/// Validates ConPTY's signed positive size contract.
fn dimensions(cols: u16, rows: u16) -> io::Result<COORD> {
    if cols == 0 || rows == 0 || cols > i16::MAX as u16 || rows > i16::MAX as u16 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(COORD { X: cols as i16, Y: rows as i16 })
}

/// Creates noninheritable synchronous pipes for the ConPTY host, never the command child.
fn console_pipe() -> io::Result<(Handle, Handle)> {
    let (mut read, mut write) = (null_mut(), null_mut());
    if unsafe { CreatePipe(&mut read, &mut write, null(), 0) } == 0 { return Err(io::Error::last_os_error()); }
    Ok((Handle(read), Handle(write)))
}

/// Starts a terminal immediately when no external network broker lease is required.
pub fn conpty(program: &Path, args: &[String], cwd: &Path, extra: &[(String, String)], cols: u16, rows: u16,
    security: Option<&SecurityContext<'_>>) -> io::Result<PtyProcess> {
    let mut terminal = conpty_pending(program, args, cwd, extra, cols, rows, security)?;
    terminal.resume()?;
    Ok(terminal)
}

/// Starts a terminal suspended and contained, with the caller's explicit isolation attributes.
pub fn conpty_pending(program: &Path, args: &[String], cwd: &Path, extra: &[(String, String)], cols: u16, rows: u16,
    security: Option<&SecurityContext<'_>>) -> io::Result<PtyProcess> {
    let size = dimensions(cols, rows)?;
    // The same rule as `spawn`: a batch file is never started through cmd.exe.
    crate::provider_process::native_image(program)?;
    let app = wide(&program.to_string_lossy())?;
    let cwd = wide(&cwd.to_string_lossy())?;
    let line = std::iter::once(program.to_string_lossy().into_owned()).chain(args.iter().cloned())
        .map(|item| quote(&item)).collect::<Vec<_>>().join(" ");
    let mut line = wide(&line)?;
    if line.len() > 32_767 { return Err(io::ErrorKind::InvalidInput.into()); }
    let mut env = environment(extra, security.is_some())?;
    if security.is_some() { crate::windows_runtimes::prime(program); }
    let (input_read, input_write) = console_pipe()?;
    let (output_read, output_write) = console_pipe()?;
    unsafe {
        let mut console = 0;
        let api = crate::windows_conpty::runtime()?;
        let result = (api.create)(size, input_read.0, output_write.0, 0, &mut console);
        if result < 0 { return Err(io::Error::from_raw_os_error(result)); }
        let console_handle = console;
        let console = Console(Arc::new(ConsoleHandle(std::sync::Mutex::new(console), api)));
        let job = Job::new()?;
        let attrs = attributes(None, Some(console_handle), security, job.0.0)?;
        let mut startup: STARTUPINFOEXW = std::mem::zeroed();
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        // Null explicit streams make Windows connect the child to ConPTY even when our streams are redirected.
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.lpAttributeList = attrs.attributes.list;
        let mut process: PROCESS_INFORMATION = std::mem::zeroed();
        if CreateProcessW(app.as_ptr(), line.as_mut_ptr(), null(), null(), 0,
            CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
            env.as_mut_ptr().cast(), cwd.as_ptr(), &startup.StartupInfo, &mut process) == 0 {
            return Err(io::Error::last_os_error());
        }
        let process_handle = Arc::new(Handle(process.hProcess));
        let thread = Handle(process.hThread);
        drop(input_read);
        drop(output_write);
        let input = File::from_raw_handle(input_write.0);
        let output = File::from_raw_handle(output_read.0);
        std::mem::forget(input_write);
        std::mem::forget(output_read);
        Ok(PtyProcess { process: Process { _job: job, _process: process_handle }, console, input, output: Some(output), thread: Some(thread) })
    }
}

/// Reads all pipe data while retaining bounded output so verbose children cannot deadlock.
fn read(mut file: File) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 { break; }
        let keep = count.min(65_537usize.saturating_sub(output.len()));
        output.extend_from_slice(&buffer[..keep]);
    }
    Ok(output)
}

/// Captures one approved command; dropping this future kills its contained process tree.
pub async fn capture(program: &Path, args: &[String], cwd: &Path) -> io::Result<(Vec<u8>, Vec<u8>, u32)> {
    capture_running(spawn(program, args, cwd, &[], false, None, false)?).await
}

/// Captures a process using explicit AppContainer isolation; the caller retains its profile guard.
pub fn capture_restricted(program: &Path, args: &[String], cwd: &Path, extra: &[(String, String)],
    security: &SecurityContext<'_>) -> impl std::future::Future<Output = io::Result<(Vec<u8>, Vec<u8>, u32)>> + Send + use<> {
    let running = spawn(program, args, cwd, extra, false, Some(security), false);
    async move { capture_running(running?).await }
}

/// Collects a contained process while preserving cancellation cleanup on the owning future.
async fn capture_running(running: Running) -> io::Result<(Vec<u8>, Vec<u8>, u32)> {
    let Running { job, process, output, errors, .. } = running;
    let job_handle = job.0.clone();
    let output = tokio::task::spawn_blocking(move || read(output));
    let errors = tokio::task::spawn_blocking(move || read(errors));
    let status = tokio::task::spawn_blocking(move || unsafe {
        if WaitForSingleObject(process.0, INFINITE) != WAIT_OBJECT_0 { return Err(io::Error::last_os_error()); }
        let mut code = 0;
        if GetExitCodeProcess(process.0, &mut code) == 0 { return Err(io::Error::last_os_error()); }
        TerminateJobObject(job_handle.0, 1);
        Ok(code)
    });
    let (output, errors, status) = tokio::try_join!(output, errors, status).map_err(io::Error::other)?;
    let (output, errors, status) = (output?, errors?, status?);
    drop(job);
    if output.len() + errors.len() > 65_536 { return Err(io::ErrorKind::InvalidData.into()); }
    Ok((output, errors, status))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pseudoconsole handles input and resize without inheriting unrelated streams.
    #[tokio::test]
    async fn pseudoconsole_input_resize_and_exit() {
        use std::io::{Read, Write};
        let program = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32").join("cmd.exe");
        let mut terminal = conpty(&program, &["/d".into(), "/q".into()], &std::env::temp_dir(), &[], 80, 24, None).unwrap();
        terminal.console.resize(100, 30).unwrap();
        assert!(terminal.console.resize(0, 30).is_err());
        let output = terminal.output.take().unwrap();
        let read = tokio::task::spawn_blocking(move || {
            let mut bytes = Vec::new();
            output.take(65536).read_to_end(&mut bytes).unwrap();
            bytes
        });
        terminal.input.write_all(b"echo CONPTY_OK\r\nexit 7\r\n").unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(15), terminal.process.wait()).await;
        terminal.process.kill().ok();
        drop(terminal);
        let output = tokio::time::timeout(std::time::Duration::from_secs(5), read).await.unwrap().unwrap();
        assert_eq!(result.unwrap().unwrap(), 7, "ConPTY output: {:?}", String::from_utf8_lossy(&output));
        assert!(String::from_utf8_lossy(&output).contains("CONPTY_OK"));
    }

    /// Private shell markers must survive ConPTY in order before agent capture is enabled.
    #[tokio::test]
    async fn pseudoconsole_preserves_shell_markers() {
        use std::io::Read;
        let program = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let args = crate::powershell::arguments("[Console]::Write(\"$([char]27)]7331;start;c1$([char]7)\"); [Console]::Write('MARKED_OUTPUT'); [Console]::Write(\"$([char]27)]7331;end;c1;0$([char]7)\")").unwrap();
        let mut terminal = conpty(&program, &args, &std::env::temp_dir(), &[], 80, 24, None).unwrap();
        let mut output = terminal.output.take().unwrap();
        let read = tokio::task::spawn_blocking(move || {
            let mut bytes = Vec::new();
            output.read_to_end(&mut bytes).unwrap();
            bytes
        });
        let exit = tokio::time::timeout(std::time::Duration::from_secs(15), terminal.process.wait()).await;
        terminal.console.close();
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), read).await;
        drop(terminal);
        assert_eq!(exit.unwrap().unwrap(), 0);
        let text = String::from_utf8(result.unwrap().unwrap()).unwrap();
        let start = text.find("\x1b]7331;start;c1\x07").expect("ConPTY must forward private start OSC");
        let content = text.find("MARKED_OUTPUT").unwrap();
        let end = text.find("\x1b]7331;end;c1;0\x07").expect("ConPTY must forward private end OSC");
        assert!(start < content && content < end, "ConPTY must preserve marker/output ordering: {text:?}");
    }

    /// Closing the terminal controller terminates its running root before returning pipe EOF.
    #[tokio::test]
    async fn pseudoconsole_drop_terminates_process() {
        let program = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let args = crate::powershell::arguments("Start-Sleep -Seconds 120").unwrap();
        let mut terminal = conpty(&program, &args, &std::env::temp_dir(), &[], 80, 24, None).unwrap();
        let root = terminal.process._process.clone();
        let mut output = terminal.output.take().unwrap();
        let read = tokio::task::spawn_blocking(move || std::io::copy(&mut output, &mut std::io::sink()));
        assert_eq!(unsafe { WaitForSingleObject(root.0, 0) }, WAIT_TIMEOUT);
        drop(terminal);
        assert_eq!(unsafe { WaitForSingleObject(root.0, 5000) }, WAIT_OBJECT_0);
        tokio::time::timeout(std::time::Duration::from_secs(5), read).await.unwrap().unwrap().unwrap();
    }

    #[test]
    fn empty_job_has_actual_user_owner_for_recovery_even_when_elevated() {
        let job = Job::new().unwrap();
        let profile = format!("MiniClaw.Run.{}", uuid::Uuid::new_v4().simple());
        let guard = TreeGuard::open_verified(&job.1, &profile).unwrap();
        guard.terminate_and_wait(std::time::Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn quotes_trailing_slashes_and_embedded_quotes() {
        assert_eq!(quote(r"C:\some folder\"), "\"C:\\some folder\\\\\"");
        assert_eq!(quote("a\"b"), "\"a\\\"b\"");
    }

    /// A batch file is refused before any process exists, however its name is dressed up, so
    /// cmd.exe never parses an argument as a command.
    #[tokio::test]
    async fn batch_files_are_refused_and_never_run() {
        let folder = std::env::temp_dir().join(format!("mc-batch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&folder).unwrap();
        let marker = folder.join("ran.txt");
        for name in ["shim.cmd", "shim.bat"] {
            std::fs::write(folder.join(name), "@echo off\r\necho ran>\"%~dp0ran.txt\"\r\n").unwrap();
        }
        // The injected argument would start a second command if cmd.exe saw it.
        let args = vec!["\"&echo injected>\"ran.txt\"&rem ".to_owned()];
        for name in ["shim.cmd", "shim.bat", "shim.CMD", "shim.cmd.", "shim.cmd ", "shim.bat. ."] {
            let program = folder.join(name);
            for refused in [capture(&program, &args, &folder).await.map(|_| ()),
                stdio(&program, &args, &folder, &[]).map(|_| ()),
                conpty(&program, &args, &folder, &[], 80, 24, None).map(|_| ())] {
                assert_eq!(refused.unwrap_err().kind(), io::ErrorKind::InvalidInput, "{name}");
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert!(!marker.exists(), "a batch file ran");
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[tokio::test]
    async fn captures_unicode_and_native_exit_code() {
        let program = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let args = crate::powershell::arguments("Write-Output 'hello ✓ & $'; exit 7").unwrap();
        let (output, _, code) = capture(&program, &args, &std::env::temp_dir()).await.unwrap();
        assert!(String::from_utf8(output).unwrap().contains("hello ✓ & $"));
        assert_eq!(code, 7);
    }

    /// Environment filtering is independent of process-global mutation and parallel tests.
    #[test]
    fn inherited_secrets_are_omitted() {
        let values = inherited_environment([
            ("Path".into(), "C:\\tools".into()), ("SystemRoot".into(), "C:\\Windows".into()),
            ("OPENAI_API_KEY".into(), "private".into()), ("MINICLAW_TOOL_TOKEN".into(), "private".into()),
        ]);
        assert_eq!(values.len(), 2);
        assert!(values.iter().all(|(name, _)| matches!(name.as_str(), "Path" | "SystemRoot")));
        let block = environment(&[("MCP_TEST_KEY".into(), "deliberate".into())], false).unwrap();
        assert!(String::from_utf16_lossy(&block).contains("MCP_TEST_KEY=deliberate\0"));
        assert!(!String::from_utf16_lossy(&block).contains("NODE_OPTIONS"), "An unrestricted process gets nothing added");
    }

    /// A sandboxed process gets Node's flags after any `NODE_OPTIONS` it was deliberately given.
    #[test]
    fn sandboxed_node_options_are_appended() {
        let block = String::from_utf16_lossy(&environment(&[], true).unwrap());
        assert!(block.contains("NODE_OPTIONS=--preserve-symlinks --preserve-symlinks-main\0"), "{block}");
        let block = String::from_utf16_lossy(&environment(&[("node_options".into(), "--max-old-space-size=512".into())], true).unwrap());
        assert!(block.contains("NODE_OPTIONS=--max-old-space-size=512 --preserve-symlinks --preserve-symlinks-main\0"), "{block}");
        assert_eq!(block.matches("NODE_OPTIONS=").count(), 1);
    }

    /// The private Node's preload reaches a sandboxed process through `NODE_OPTIONS` only; the
    /// variable that names it never reaches any process, and an unrestricted one gets nothing.
    #[test]
    fn sandboxed_node_gets_the_preload() {
        let preload = [(crate::windows_runtimes::NODE_PRELOAD_PATH.to_owned(), r"C:\M C\bin\node\child-pipes.cjs".to_owned())];
        let block = String::from_utf16_lossy(&environment(&preload, true).unwrap());
        assert!(block.contains(r#"NODE_OPTIONS=--preserve-symlinks --preserve-symlinks-main --require "C:\\M C\\bin\\node\\child-pipes.cjs""#), "{block}");
        assert!(!block.contains(crate::windows_runtimes::NODE_PRELOAD_PATH), "{block}");
        let block = String::from_utf16_lossy(&environment(&preload, false).unwrap());
        assert!(!block.contains("NODE_OPTIONS") && !block.contains(crate::windows_runtimes::NODE_PRELOAD_PATH), "{block}");
    }

    /// A sandboxed process whose sandbox has the Python hook gets it first on `PYTHONPATH`; an
    /// unrestricted process never gets it, even when the variable naming it is passed along.
    #[test]
    fn sandboxed_python_path_starts_with_the_hook() {
        let hook = ("MINICLAW_PYTHON_HOOK".to_owned(), r"C:\MiniClaw\bin\python".to_owned());
        let block = String::from_utf16_lossy(&environment(&[hook.clone(), ("PythonPath".into(), r"C:\proj".into())], true).unwrap());
        assert!(block.contains("PYTHONPATH=C:\\MiniClaw\\bin\\python;C:\\proj\0") && block.contains("MINICLAW_PYTHON_SANDBOX=1\0"), "{block}");
        assert!(!block.contains("MINICLAW_PYTHON_HOOK="), "{block}");
        let block = String::from_utf16_lossy(&environment(&[hook], false).unwrap());
        assert!(!block.contains("MINICLAW_PYTHON") && !block.contains("PYTHONPATH"), "{block}");
        assert!(!String::from_utf16_lossy(&environment(&[], true).unwrap()).contains("PYTHONPATH"));
    }

    /// PowerShell errors remain readable text instead of encoded CLIXML records.
    #[tokio::test]
    async fn plain_stderr() {
        let program = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let args = crate::powershell::arguments("Write-Error 'plain failure'; exit 3").unwrap();
        let (_, errors, code) = capture(&program, &args, &std::env::temp_dir()).await.unwrap();
        let errors = String::from_utf8_lossy(&errors);
        assert!(errors.contains("plain failure"));
        assert!(!errors.contains("CLIXML"));
        assert_eq!(code, 3);
    }

    /// Interactive pipes deliver a request and only the explicitly supplied MCP credential.
    #[tokio::test]
    async fn interactive_stdio() {
        use std::io::{BufRead, Write};
        let program = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let args = crate::powershell::arguments("$line=[Console]::ReadLine(); [Console]::WriteLine($line + ':' + $env:MCP_TEST_KEY)").unwrap();
        let server = stdio(&program, &args, &std::env::temp_dir(), &[("MCP_TEST_KEY".into(), "deliberate".into())]).unwrap();
        let mut input = server.input;
        writeln!(input, "request").unwrap();
        input.flush().unwrap();
        let read = tokio::task::spawn_blocking(move || {
            let mut reply = String::new();
            std::io::BufReader::new(server.output).read_line(&mut reply).unwrap();
            reply
        });
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), read).await;
        drop(server.process);
        assert_eq!(result.unwrap().unwrap().trim(), "request:deliberate");
    }

    /// Cancelling a running capture kills a child that outlives its PowerShell launcher.
    #[tokio::test]
    async fn cancellation_kills_descendants() {
        let directory = std::env::temp_dir().join(format!("mc-job-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let marker = directory.join("child.pid");
        let program = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let child_args = crate::powershell::arguments("Start-Sleep -Seconds 120").unwrap()
            .into_iter().map(|value| format!("'{}'", value.replace('\'', "''"))).collect::<Vec<_>>().join(",");
        let script = format!("$child=Start-Process -FilePath '{}' -ArgumentList @({child_args}) -PassThru; [IO.File]::WriteAllText('{}', [string]$child.Id); Start-Sleep -Seconds 120",
            program.to_string_lossy().replace('\'', "''"), marker.to_string_lossy().replace('\'', "''"));
        let args = crate::powershell::arguments(&script).unwrap();
        let capture_dir = directory.clone();
        let task = tokio::spawn(async move { capture(&program, &args, &capture_dir).await });
        let found = tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                if let Ok(text) = std::fs::read_to_string(&marker) {
                    if let Ok(pid) = text.parse::<u32>() { break pid; }
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }).await;
        let pid = match found {
            Ok(pid) => pid,
            Err(error) => { task.abort(); let _ = task.await; panic!("Child did not start: {error}"); }
        };
        let child = unsafe { Handle(OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid)) };
        let was_alive = !child.0.is_null() && unsafe { WaitForSingleObject(child.0, 0) } == WAIT_TIMEOUT;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(was_alive, "The descendant must be running before cancellation");
        assert_eq!(unsafe { WaitForSingleObject(child.0, 5000) }, WAIT_OBJECT_0);
        let _ = std::fs::remove_dir_all(directory);
    }


    /// Concurrent long-lived launchers cannot inherit a short command's output handles.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_launches_do_not_share_pipes() {
        let program = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        for _ in 0..4 {
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let ready = barrier.clone();
            let long_program = program.clone();
            let long = tokio::task::spawn_blocking(move || {
                let args = crate::powershell::arguments("Start-Sleep -Seconds 120").unwrap();
                ready.wait();
                stdio(&long_program, &args, &std::env::temp_dir(), &[])
            });
            let args = crate::powershell::arguments("Write-Output 'short completed'").unwrap();
            barrier.wait();
            let short = tokio::time::timeout(std::time::Duration::from_secs(10),
                capture(&program, &args, &std::env::temp_dir())).await;
            let long = long.await.unwrap().unwrap();
            let long_was_alive = long.process.try_wait().unwrap().is_none();
            drop(long);
            assert!(long_was_alive, "The unrelated process must still hold its own pipes");
            let (output, _, code) = short.expect("Short process must reach EOF independently").unwrap();
            assert_eq!(code, 0);
            assert!(String::from_utf8_lossy(&output).contains("short completed"));
        }
    }


    /// The child half of `sandboxed_bad_handle_and_crash_end_promptly`: misbehaves as the variable says.
    #[test]
    fn sandboxed_misbehaving_child() {
        match std::env::var("MINICLAW_MISBEHAVE").as_deref() {
            // What Git for Windows does at exit: a console call on the standard handle it has just closed.
            Ok("bad-handle") => unsafe {
                let stale: HANDLE = 0x7ff0 as HANDLE;
                let mut mode = 0;
                GetConsoleMode(stale, &mut mode);
                println!("not stopped by the stale handle");
            },
            Ok("crash") => unsafe { std::ptr::write_volatile(8usize as *mut u32, 1) },
            _ => {}
        }
    }

    /// A sandboxed program that touches a stale handle, or crashes, ends at once with the exception as its
    /// exit code. AppContainer processes get strict handle checks and Windows Error Reporting never
    /// finishes for them, so both used to hang until killed: every Git command typed in the terminal
    /// printed its output and never exited (VM1, installed release, 2026-10-05).
    #[tokio::test]
    async fn sandboxed_bad_handle_and_crash_end_promptly() {
        let folder = std::env::temp_dir().join(format!("mc-misbehave-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&folder).unwrap();
        let cwd = folder.to_string_lossy().into_owned();
        let program = std::env::current_exe().unwrap();
        let grants = [crate::seatbelt::Grant { path: cwd, write: true, folder: true, blocked: false },
            crate::seatbelt::Grant { path: program.to_string_lossy().into_owned(), write: false, folder: false, blocked: false }];
        let sandbox = crate::windows_sandbox::Sandbox::new(&grants, &folder).unwrap();
        let args = ["--exact", "windows_process::tests::sandboxed_misbehaving_child", "--nocapture"].map(str::to_owned);
        for (mode, exception) in [("bad-handle", 0xC000_0008u32), ("crash", 0xC000_0005)] {
            let mut env: Vec<_> = sandbox.environment().into_iter()
                .map(|(k, v)| (k.into_string().unwrap(), v.into_string().unwrap())).collect();
            env.push(("MINICLAW_MISBEHAVE".into(), mode.into()));
            let started = std::time::Instant::now();
            let ran = tokio::time::timeout(std::time::Duration::from_secs(30),
                capture_restricted(&program, &args, &folder, &env, &sandbox.security())).await;
            let (output, errors, code) = ran.unwrap_or_else(|_| panic!("{mode}: still running after 30 s")).unwrap();
            let text = format!("{mode}: code {code:#x} after {:?} {} {}", started.elapsed(),
                String::from_utf8_lossy(&output), String::from_utf8_lossy(&errors));
            assert_eq!(code, exception, "{text}");
            assert!(started.elapsed() < std::time::Duration::from_secs(10), "{text}");
        }
        drop(sandbox);
        crate::windows_sandbox::drain_cleanup(std::time::Duration::from_secs(35)).unwrap();
        let _ = std::fs::remove_dir_all(folder);
    }

    /// The provider wrapper cannot inherit another launch's temporarily inheritable pipe.
    #[tokio::test]
    async fn provider_excludes_unrelated_inheritable_handle() {
        let program = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let (mut unrelated, writer) = output_pipe().unwrap();
        let mut command = tokio::process::Command::new(program);
        command.args(crate::powershell::arguments("Start-Sleep -Seconds 120").unwrap());
        let mut child = crate::provider_process::spawn(&mut command).unwrap();
        drop(writer);
        let read = tokio::task::spawn_blocking(move || {
            let mut bytes = Vec::new();
            unrelated.read_to_end(&mut bytes)
        });
        let eof = tokio::time::timeout(std::time::Duration::from_secs(3), read).await;
        let still_running = child.try_wait().unwrap().is_none();
        child.kill().await.unwrap();
        assert!(still_running);
        assert_eq!(eof.expect("Unrelated pipe must reach EOF while provider runs").unwrap().unwrap(), 0);
    }

}
