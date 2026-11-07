//! Runs the narrow SYSTEM network broker and verifies its owner-bound named-pipe peers.
// Reuses authenticated closed request shapes.
use super::{Attest, Attested, BootEpoch, Config, Frame, Hello, Registration, Repair, Request};
#[path = "windows_network_boot.rs"]
mod boot;
#[path = "windows_network_attest.rs"]
mod attest;
// Owns native handles and service state without sharing secret configuration in logs.
use std::{fs::File, io::{self, Read}, os::windows::io::{AsRawHandle}, path::{Path, PathBuf},
    ptr::{null, null_mut}, sync::{Arc, OnceLock, atomic::{AtomicBool, AtomicUsize, Ordering}}, time::Duration};
// Drives bounded asynchronous named-pipe frames and connection cancellation.
use tokio::{io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader}, net::windows::named_pipe::{NamedPipeClient, NamedPipeServer}, sync::Notify};
// Queries actual pipe/process/token identity and manages SCM status only.
use windows_sys::Win32::{Foundation::*, Security::{*, Authorization::*, Isolation::*}, Storage::FileSystem::*,
    System::{JobObjects::*, Pipes::*, Services::*, Threading::*}};

static JOURNAL: OnceLock<PathBuf> = OnceLock::new();
static CONFIG: OnceLock<Config> = OnceLock::new();
static BOOT: OnceLock<BootEpoch> = OnceLock::new();
static STOP: AtomicBool = AtomicBool::new(false);
static CLEANUP_FAILED: AtomicBool = AtomicBool::new(false);
static STATUS: AtomicUsize = AtomicUsize::new(0);
static NOTIFY: OnceLock<Notify> = OnceLock::new();
/// This installation's Winsock catalog entries, and the status file administrators can read.
static WINSOCK: OnceLock<(crate::windows_winsock_acl::Manager, PathBuf)> = OnceLock::new();
/// The last published Winsock state: 0 none, 1 ready, 2 failed.
static WINSOCK_STATE: AtomicUsize = AtomicUsize::new(0);
/// Start parameter that makes the service remove its Winsock catalog entries and stop.
const REMOVE_WINSOCK: &str = "winsock-acl-remove";
/// The well-known `internetClient` capability.
const INTERNET_CLIENT: &str = "S-1-15-3-1";
const FRAME: usize = 8192;
// Read metadata is required for pipe inspection; generic write would allow server creation.
const PIPE_CLIENT_ACCESS: u32 = FILE_GENERIC_READ | FILE_WRITE_DATA;
// Documented Win32 job access and ACE type constants.
const JOB_OBJECT_QUERY: u32 = 0x0004;
const JOB_OBJECT_TERMINATE: u32 = 0x0008;
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
const ACCESS_DENIED_ACE_TYPE: u8 = 1;

struct Handle(HANDLE);
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Drop for Handle {
    /// Releases one verified native object handle.
    fn drop(&mut self) { unsafe { CloseHandle(self.0); } }
}

/// Converts fixed protocol object names without accepting embedded native terminators.
fn wide(value: &str) -> io::Result<Vec<u16>> {
    if value.contains('\0') { return Err(io::ErrorKind::InvalidInput.into()); }
    Ok(value.encode_utf16().chain(Some(0)).collect())
}

/// Formats a native token SID while retaining its allocation until conversion completes.
fn sid_string(sid: PSID) -> io::Result<String> {
    let mut text = null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 { return Err(io::Error::last_os_error()); }
    let mut len = 0;
    unsafe {
        while *text.add(len) != 0 { len += 1; }
        let result = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        LocalFree(text.cast());
        Ok(result)
    }
}

/// Reads aligned token information from an actual kernel process handle.
fn token_info(token: HANDLE, class: TOKEN_INFORMATION_CLASS) -> io::Result<Vec<usize>> {
    let mut bytes = 0;
    unsafe { GetTokenInformation(token, class, null_mut(), 0, &mut bytes); }
    if bytes == 0 || bytes > 65536 { return Err(io::Error::last_os_error()); }
    let mut value = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
    if unsafe { GetTokenInformation(token, class, value.as_mut_ptr().cast(), bytes, &mut bytes) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(value)
}

/// Opens a read-only process token for identity validation, never impersonation or launch.
fn token(process: HANDLE) -> io::Result<Handle> {
    let mut value = null_mut();
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut value) } == 0 { return Err(io::Error::last_os_error()); }
    Ok(Handle(value))
}

/// Returns the real owning user SID from a kernel token.
fn user(token: HANDLE) -> io::Result<String> {
    let info = token_info(token, TokenUser)?;
    sid_string(unsafe { (*(info.as_ptr().cast::<TOKEN_USER>())).User.Sid })
}

/// Gives private kernel objects the real user owner even under an elevated token.
pub(crate) fn current_user_sid() -> io::Result<String> {
    user(token(unsafe { GetCurrentProcess() })?.0)
}

/// Verifies installed secret storage has no allow ACE for any principal besides SYSTEM/admins.
fn administrator_file(file: &File) -> io::Result<()> {
    let mut owner = null_mut();
    let mut acl = null_mut();
    let mut descriptor = null_mut();
    let code = unsafe { GetSecurityInfo(file.as_raw_handle(), SE_FILE_OBJECT,
        OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION, &mut owner, null_mut(), &mut acl, null_mut(), &mut descriptor) };
    if code != 0 { return Err(io::Error::from_raw_os_error(code as i32)); }
    let check = || -> io::Result<()> {
        let mut control = 0;
        let mut revision = 0;
        if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0 || control & SE_DACL_PROTECTED == 0 {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        if !["S-1-5-18", "S-1-5-32-544"].contains(&sid_string(owner)?.as_str()) || acl.is_null() { return Err(io::ErrorKind::PermissionDenied.into()); }
        for index in 0..unsafe { (*acl).AceCount } {
            let mut ace = null_mut();
            if unsafe { GetAce(acl, index as u32, &mut ace) } == 0 { return Err(io::Error::last_os_error()); }
            let header = unsafe { &*ace.cast::<ACE_HEADER>() };
            if header.AceType == ACCESS_ALLOWED_ACE_TYPE {
                let ace = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
                if !["S-1-5-18", "S-1-5-32-544"].contains(&sid_string((&ace.SidStart as *const u32).cast_mut().cast())?.as_str()) {
                    return Err(io::ErrorKind::PermissionDenied.into());
                }
            } else if header.AceType != ACCESS_DENIED_ACE_TYPE { return Err(io::ErrorKind::PermissionDenied.into()); }
        }
        Ok(())
    }();
    unsafe { LocalFree(descriptor); }
    check
}

/// Reads bounded configuration from either the protected service or owner-private client file.
fn config(path: &Path, administrator: bool) -> io::Result<Config> {
    if !path.is_absolute() { return Err(io::ErrorKind::InvalidInput.into()); }
    let file = crate::platform_fs::open_read_nofollow(path)?;
    if administrator { administrator_file(&file)?; }
    else if !crate::platform_fs::private_regular_file(&file)? { return Err(io::ErrorKind::PermissionDenied.into()); }
    let mut bytes = Vec::new();
    file.take((FRAME + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > FRAME { return Err(io::ErrorKind::InvalidData.into()); }
    let config: Config = serde_json::from_slice(&bytes)?;
    config.validate()?;
    if !Path::new(&config.service_exe).is_absolute() { return Err(io::ErrorKind::InvalidInput.into()); }
    Ok(config)
}

/// Creates local-only pipe instances; the user may exchange data but cannot create server instances.
fn pipe(config: &Config, first: bool) -> io::Result<NamedPipeServer> {
    let sddl = wide(&format!("D:P(A;;GA;;;SY)(A;;0x{PIPE_CLIENT_ACCESS:x};;;{})", config.owner_sid))?;
    let mut descriptor = null_mut();
    if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), 1, &mut descriptor, null_mut()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: descriptor, bInheritHandle: 0 };
    let name = wide(&format!("\\\\.\\pipe\\{}", config.pipe_name))?;
    let handle = unsafe { CreateNamedPipeW(name.as_ptr(), PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 },
        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS, 255, FRAME as u32, FRAME as u32, 5000, &attributes) };
    unsafe { LocalFree(descriptor); }
    if handle == INVALID_HANDLE_VALUE { return Err(io::Error::last_os_error()); }
    unsafe { NamedPipeServer::from_raw_handle(handle) }
}

/// Reads a single bounded line without allocating in proportion to peer input.
async fn frame(reader: &mut BufReader<impl tokio::io::AsyncRead + Unpin>) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    loop {
        let ready = reader.fill_buf().await?;
        if ready.is_empty() { return Err(io::ErrorKind::UnexpectedEof.into()); }
        let count = ready.iter().position(|b| *b == b'\n').map_or(ready.len(), |i| i + 1);
        if bytes.len() + count > FRAME { return Err(io::ErrorKind::InvalidData.into()); }
        bytes.extend_from_slice(&ready[..count]);
        reader.consume(count);
        if bytes.last() == Some(&b'\n') { return Ok(bytes); }
    }
}

/// Sends one complete bounded protocol frame.
async fn send(writer: &mut (impl tokio::io::AsyncWrite + Unpin), value: &impl serde::Serialize) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    if bytes.len() > FRAME { return Err(io::ErrorKind::InvalidData.into()); }
    writer.write_all(&bytes).await?;
    writer.flush().await
}

/// Validates a pipe peer from its kernel PID and refuses AppContainer clients themselves.
fn peer(pipe: HANDLE, owner: &str) -> io::Result<Handle> {
    let mut pid = 0;
    if unsafe { GetNamedPipeClientProcessId(pipe, &mut pid) } == 0 { return Err(io::Error::last_os_error()); }
    let process = Handle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_DUP_HANDLE | SYNCHRONIZE, 0, pid) });
    if process.0.is_null() { return Err(io::Error::last_os_error()); }
    let token = token(process.0)?;
    let container = token_info(token.0, TokenIsAppContainer)?;
    if user(token.0)? != owner || unsafe { *container.as_ptr().cast::<u32>() } != 0 { return Err(io::ErrorKind::PermissionDenied.into()); }
    Ok(process)
}

/// Names the failing recovery query while preserving its kind; callers match only the raw
/// not-found code from opening the job, which is never wrapped.
fn stage(name: &str, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("Sandbox job {name}: {error}"))
}

/// Enumerates a bounded caller-owned job and checks every process's restricted token identity.
fn validate_job(job: HANDLE, owner: &str, package: &str, capabilities: &[String]) -> io::Result<()> {
    validate_job_contents(job, owner, package, false, Some(capabilities))
}

/// A networked run may hold `internetClient` and this installation's Winsock capability, nothing
/// else: a token with `registryRead` or any other capability gets no network lease.
fn only_capabilities(token: HANDLE, allowed: &[String]) -> io::Result<()> {
    let info = token_info(token, TokenCapabilities)?;
    let groups = info.as_ptr().cast::<TOKEN_GROUPS>();
    for index in 0..unsafe { (*groups).GroupCount } as usize {
        let sid = unsafe { (*(*groups).Groups.as_ptr().add(index)).Sid };
        if !allowed.contains(&sid_string(sid)?) { return Err(io::ErrorKind::PermissionDenied.into()); }
    }
    Ok(())
}

/// Recovery may inspect an already empty job, but new leases always require a live restricted process.
fn validate_job_contents(job: HANDLE, owner: &str, package: &str, allow_empty: bool, capabilities: Option<&[String]>) -> io::Result<()> {
    let mut descriptor = null_mut();
    let mut sid = null_mut();
    let code = unsafe { GetSecurityInfo(job, SE_KERNEL_OBJECT, OWNER_SECURITY_INFORMATION, &mut sid, null_mut(), null_mut(), null_mut(), &mut descriptor) };
    if code != 0 { return Err(stage("owner query", io::Error::from_raw_os_error(code as i32))); }
    let actual = sid_string(sid);
    unsafe { LocalFree(descriptor); }
    if actual? != owner { return Err(io::ErrorKind::PermissionDenied.into()); }
    let mut buffer = vec![0usize; 4100];
    if unsafe { QueryInformationJobObject(job, JobObjectBasicProcessIdList, buffer.as_mut_ptr().cast(), (buffer.len() * size_of::<usize>()) as u32, null_mut()) } == 0 {
        return Err(stage("process list", io::Error::last_os_error()));
    }
    let list = unsafe { &*buffer.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>() };
    if (!allow_empty && list.NumberOfProcessIdsInList == 0) || list.NumberOfProcessIdsInList != list.NumberOfAssignedProcesses || list.NumberOfProcessIdsInList > 4096 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let ids = unsafe { std::slice::from_raw_parts(list.ProcessIdList.as_ptr(), list.NumberOfProcessIdsInList as usize) };
    for pid in ids {
        let process = Handle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, *pid as u32) });
        if process.0.is_null() { return Err(stage("member process query", io::Error::last_os_error())); }
        let token = token(process.0).map_err(|error| stage("member token query", error))?;
        if user(token.0).map_err(|error| stage("member token user", error))? != owner { return Err(io::ErrorKind::PermissionDenied.into()); }
        let info = token_info(token.0, TokenAppContainerSid).map_err(|error| stage("member token package", error))?;
        let sid = unsafe { (*(info.as_ptr().cast::<TOKEN_APPCONTAINER_INFORMATION>())).TokenAppContainer };
        if sid.is_null() || sid_string(sid)? != package { return Err(io::ErrorKind::PermissionDenied.into()); }
        if let Some(allowed) = capabilities { only_capabilities(token.0, allowed).map_err(|error| stage("member token capabilities", error))?; }
    }
    Ok(())
}

/// Reopens only a generated, caller-owned job whose live processes have the recorded package SID.
/// Missing objects remain an error: namespace removal alone is not used as a death certificate.
pub(crate) fn recovery_job(name: &str, profile: &str) -> io::Result<std::os::windows::io::OwnedHandle> {
    if !super::identifier(name, "Global\\MiniClaw.Job.") || !super::identifier(profile, "MiniClaw.Run.") {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    recovery_job_for(name, profile, &current_user_sid()?)
}

/// The service uses its configured authenticated user, never a caller-supplied owner.
fn recovery_job_for(name: &str, profile: &str, owner: &str) -> io::Result<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    if !super::identifier(name, "Global\\MiniClaw.Job.") || !super::identifier(profile, "MiniClaw.Run.") {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let name = wide(name)?;
    let job = Handle(unsafe { OpenJobObjectW(JOB_OBJECT_QUERY | JOB_OBJECT_TERMINATE | READ_CONTROL, 0, name.as_ptr()) });
    if job.0.is_null() {
        let error = io::Error::last_os_error();
        return Err(if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) { error } else { stage("open", error) });
    }
    let profile = wide(profile)?;
    let mut package = null_mut();
    let code = unsafe { DeriveAppContainerSidFromAppContainerName(profile.as_ptr(), &mut package) };
    if code < 0 { return Err(io::Error::other("Invalid AppContainer profile")); }
    let package_text = sid_string(package);
    unsafe { FreeSid(package); }
    let package_text = package_text?;
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { QueryInformationJobObject(job.0, JobObjectExtendedLimitInformation,
        (&mut limits as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(), size_of_val(&limits) as u32, null_mut()) } == 0 {
        return Err(stage("limit query", io::Error::last_os_error()));
    }
    let flags = limits.BasicLimitInformation.LimitFlags;
    if flags & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE == 0 || flags & (JOB_OBJECT_LIMIT_BREAKAWAY_OK | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK) != 0 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    validate_job_contents(job.0, owner, &package_text, true, None)?;
    let raw = job.0;
    std::mem::forget(job);
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

struct BoundJob(Handle);
impl Drop for BoundJob {
    /// An interrupted service session can only terminate the verified caller-owned job.
    fn drop(&mut self) { unsafe { TerminateJobObject(self.0.0, 1); } }
}
impl BoundJob {
    /// Waits for all descendants to leave before any persistent enforcement is removed.
    async fn stop(&self) -> io::Result<()> {
        if unsafe { TerminateJobObject(self.0.0, 1) } == 0 { return Err(io::Error::last_os_error()); }
        for _ in 0..100 {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
            if unsafe { QueryInformationJobObject(self.0.0, JobObjectBasicAccountingInformation, (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(), size_of_val(&info) as u32, null_mut()) } == 0 {
                return Err(io::Error::last_os_error());
            }
            if info.ActiveProcesses == 0 { return Ok(()); }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err(io::ErrorKind::TimedOut.into())
    }
}

/// Duplicates and validates the caller's native objects entirely before any await point.
fn bind_job(process: HANDLE, config: &Config, request: &Request) -> io::Result<(BoundJob, String)> {
    let mut job = null_mut();
    if unsafe { DuplicateHandle(process, request.job_handle as usize as HANDLE, GetCurrentProcess(), &mut job, JOB_OBJECT_QUERY | JOB_OBJECT_TERMINATE | READ_CONTROL, 0, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // Do not wrap in the terminating guard until the handle's ownership has been verified.
    let job = Handle(job);
    let name = wide(&request.job_name)?;
    let named = Handle(unsafe { OpenJobObjectW(JOB_OBJECT_QUERY, 0, name.as_ptr()) });
    if named.0.is_null() || unsafe { CompareObjectHandles(job.0, named.0) } == 0 { return Err(io::ErrorKind::PermissionDenied.into()); }
    let mut package = null_mut();
    let profile = wide(&request.profile)?;
    let code = unsafe { DeriveAppContainerSidFromAppContainerName(profile.as_ptr(), &mut package) };
    if code < 0 { return Err(io::Error::other("Invalid AppContainer profile")); }
    let package_text = sid_string(package);
    unsafe { FreeSid(package); }
    let package_text = package_text?;
    let winsock = crate::windows_winsock_acl::derive(&crate::windows_winsock_acl::capability_name(&config.service_name)?)?;
    let allowed = [INTERNET_CLIENT.to_owned(), crate::windows_winsock_acl::sid_text(&winsock)?];
    validate_job(job.0, &config.owner_sid, &package_text, &allowed)?;
    Ok((BoundJob(job), package_text))
}

/// Replaces the small status file beside the service configuration. Administrators read it after
/// installation and removal; it holds no secret and decides nothing inside the service.
fn publish_winsock(value: serde_json::Value) {
    let Some((_, path)) = WINSOCK.get() else { return };
    let temporary = path.with_extension("tmp");
    let written = serde_json::to_vec(&value).map_err(io::Error::other)
        .and_then(|bytes| std::fs::write(&temporary, bytes))
        .and_then(|_| std::fs::rename(&temporary, path));
    if let Err(error) = written { tracing::warn!(%error, "Could not publish the Winsock access status"); }
}

/// Verifies this installation's Winsock catalog entries and repairs any that Windows dropped.
/// On failure returns the text a refused networked run shows; nothing is opened up instead.
fn winsock_ready() -> Result<(), String> {
    let Some((manager, _)) = WINSOCK.get() else { return Err(crate::windows_winsock_acl::refusal("the service is not initialized")) };
    let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs());
    match manager.ensure() {
        Ok(report) => {
            let changed = !report.added.is_empty() || !report.repaired.is_empty();
            if changed || WINSOCK_STATE.swap(1, Ordering::SeqCst) != 1 {
                WINSOCK_STATE.store(1, Ordering::SeqCst);
                publish_winsock(serde_json::json!({"ok": true, "sid": manager.sid(), "capability": manager.capability(), "keys": report.keys,
                    "added": report.added, "repaired": report.repaired, "time": time}));
            }
            Ok(())
        }
        Err(error) => {
            tracing::error!(%error, "Winsock catalog access could not be verified; refusing network leases");
            WINSOCK_STATE.store(2, Ordering::SeqCst);
            publish_winsock(serde_json::json!({"ok": false, "sid": manager.sid(), "capability": manager.capability(), "error": error.to_string(), "time": time}));
            Err(crate::windows_winsock_acl::refusal(&error.to_string()))
        }
    }
}

/// Removes this installation's Winsock catalog entries; run only by the uninstall start parameter.
fn winsock_remove() -> io::Result<()> {
    let (manager, _) = WINSOCK.get().ok_or(io::ErrorKind::NotConnected)?;
    let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs());
    match manager.remove() {
        Ok(removal) => {
            publish_winsock(serde_json::json!({"ok": true, "removed": true, "sid": manager.sid(), "capability": manager.capability(),
                "stripped": removal.stripped, "exact": removal.exact, "differing": removal.differing, "time": time}));
            Ok(())
        }
        Err(error) => {
            publish_winsock(serde_json::json!({"ok": false, "removed": false, "sid": manager.sid(), "capability": manager.capability(),
                "error": error.to_string(), "time": time}));
            Err(error)
        }
    }
}

/// Runs one job-death scan at a time off the async workers, so repeated questions cannot pile up.
async fn attested(question: Attest) -> io::Result<bool> {
    static SCAN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _one = SCAN.lock().await;
    tokio::task::spawn_blocking(move || attest::dead(&question)).await.map_err(io::Error::other)?
}

/// Serves one authenticated lease, or answers one job-death question and closes.
/// Dropping a lease connection terminates the associated job.
async fn connection(stream: NamedPipeServer, config: Arc<Config>) -> io::Result<()> {
    let process = peer(stream.as_raw_handle(), &config.owner_sid)?;
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    let challenge = uuid::Uuid::new_v4().simple().to_string();
    send(&mut write, &Hello::new(&config, challenge.clone(), BOOT.get().ok_or(io::ErrorKind::NotConnected)?.clone())?).await?;
    let bytes = tokio::time::timeout(Duration::from_secs(5), frame(&mut read)).await.map_err(|_| io::ErrorKind::TimedOut)??;
    let signed = match serde_json::from_slice(&bytes)? {
        Frame::Lease(signed) => signed,
        Frame::Attest(question) => {
            question.verify(&config, &challenge)?;
            return send(&mut write, &Attested { ok: true, dead: attested(question.attest).await? }).await;
        }
        Frame::Repair(request) => {
            // The owner's backend asks for the check the service otherwise makes at its next lease.
            request.verify(&config, &challenge)?;
            let answer = match tokio::task::spawn_blocking(winsock_ready).await.map_err(io::Error::other)? {
                Ok(()) => serde_json::json!({"ok": true}),
                Err(message) => serde_json::json!({"ok": false, "error": message}),
            };
            return send(&mut write, &answer).await;
        }
    };
    signed.verify(&config, &challenge)?;
    let (job, package_text) = bind_job(process.0, &config, &signed.request)?;
    // The run's token can use Winsock only through this installation's catalog entries. They are
    // checked, and repaired if Windows recreated a key, before any network policy is installed.
    if let Err(message) = tokio::task::spawn_blocking(winsock_ready).await.map_err(io::Error::other)? {
        let _ = send(&mut write, &serde_json::json!({"ok": false, "error": message})).await;
        job.stop().await?;
        return Err(io::Error::other(message));
    }
    let journal = JOURNAL.get().unwrap().join(format!("{}.json", uuid::Uuid::new_v4().simple()));
    let save = |network: &crate::windows_network::Record| -> io::Result<()> {
        use std::io::Write;
        let mut file = crate::platform_fs::private_file(&journal, true)?;
        file.write_all(&serde_json::to_vec(&serde_json::json!({"owner_sid":config.owner_sid,
            "profile":signed.request.profile,"job_name":signed.request.job_name,"network":network, "boot":BOOT.get().unwrap()}))?)?;
        file.sync_all()
    };
    let mut lease = match crate::windows_network::Lease::install_ports_recorded(&package_text, signed.request.proxy_port,
        &signed.request.connect_ports, &signed.request.listen_ports, save) {
        Ok(lease) => lease,
        Err(error) => {
            // Native installation may have persisted enforcement before a later API failed.
            CLEANUP_FAILED.store(true, Ordering::SeqCst);
            let _ = job.stop().await;
            return Err(error);
        }
    };
    if !signed.request.listen_ports.is_empty() {
        if let Err(error) = lease.start_inbound(&signed.request.profile) {
            CLEANUP_FAILED.store(true, Ordering::SeqCst);
            if job.stop().await.is_ok() { let _ = lease.revoke(); }
            return Err(error);
        }
    }
    // If sending fails, the guard terminates the job and persistent filters remain fail-closed.
    let ready = send(&mut write, &serde_json::json!({"ok":true})).await;
    let mut byte = [0u8; 1];
    while ready.is_ok() {
        tokio::select! {
            _ = read.read(&mut byte) => break,
            _ = NOTIFY.get().unwrap().notified() => if STOP.load(Ordering::SeqCst) { break; },
            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                if STOP.load(Ordering::SeqCst) || unsafe { WaitForSingleObject(process.0, 0) } == WAIT_OBJECT_0 { break; }
                if !lease.installed().unwrap_or(false) { CLEANUP_FAILED.store(true, Ordering::SeqCst); break; }
                if !signed.request.listen_ports.is_empty() && !matches!(lease.inbound_status(), Ok(Some((None, _)))) {
                    CLEANUP_FAILED.store(true, Ordering::SeqCst); break;
                }
            }
        }
    }
    if let Err(error) = job.stop().await { CLEANUP_FAILED.store(true, Ordering::SeqCst); return Err(error); }
    if let Err(error) = lease.revoke() { CLEANUP_FAILED.store(true, Ordering::SeqCst); return Err(error); }
    if let Err(error) = std::fs::remove_file(&journal) { CLEANUP_FAILED.store(true, Ordering::SeqCst); return Err(error); }
    ready
}

/// Publishes SCM state without advertising unsupported pause controls.
fn status(state: u32, error: u32) {
    let handle = STATUS.load(Ordering::SeqCst) as SERVICE_STATUS_HANDLE;
    if handle.is_null() { return; }
    let value = SERVICE_STATUS { dwServiceType: SERVICE_WIN32_OWN_PROCESS, dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING { SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN } else { 0 },
        dwWin32ExitCode: error, dwServiceSpecificExitCode: 0, dwCheckPoint: 0, dwWaitHint: if state == SERVICE_STOP_PENDING { 10000 } else { 0 } };
    unsafe { SetServiceStatus(handle, &value); }
}

/// Accepts only SCM stop/shutdown and signals all bound sessions to terminate their own jobs.
unsafe extern "system" fn control(code: u32, _: u32, _: *mut std::ffi::c_void, _: *mut std::ffi::c_void) -> u32 {
    if code == SERVICE_CONTROL_STOP || code == SERVICE_CONTROL_SHUTDOWN {
        status(SERVICE_STOP_PENDING, 0);
        STOP.store(true, Ordering::SeqCst);
        if let Some(notify) = NOTIFY.get() { notify.notify_waiters(); }
    }
    0
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryRecord {
    owner_sid: String,
    profile: String,
    job_name: String,
    network: crate::windows_network::Record,
    boot: BootEpoch,
}

/// Restores persistent enforcement only after proving that the recorded job cannot still execute.
async fn recover_leases(config: &Config, boot: &BootEpoch) -> io::Result<()> {
    use std::os::windows::io::IntoRawHandle;
    for entry in std::fs::read_dir(JOURNAL.get().ok_or(io::ErrorKind::NotConnected)?)? {
        let path = entry?.path();
        let mut file = crate::platform_fs::open_read_nofollow(&path)?;
        administrator_file(&file)?;
        if file.metadata()?.len() > 65536 { return Err(io::ErrorKind::InvalidData.into()); }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let record: RecoveryRecord = serde_json::from_slice(&bytes)?;
        if record.owner_sid != config.owner_sid || record.boot.installation != config.service_name
            || !record.boot.valid() || !super::identifier(&record.profile, "MiniClaw.Run.")
            || !super::identifier(&record.job_name, "Global\\MiniClaw.Job.") {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        match recovery_job_for(&record.job_name, &record.profile, &config.owner_sid) {
            Ok(handle) => BoundJob(Handle(handle.into_raw_handle())).stop().await?,
            Err(error) if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) && boot.supersedes(&record.boot) => {},
            Err(_) => return Err(io::Error::other("Sandbox job death is uncertain. Use Windows Restart before recovering this network service.")),
        }
        let mut lease = crate::windows_network::Lease::recover(record.network)?;
        lease.revoke()?;
        drop(file);
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Keeps an instance available before handling each authenticated connection.
async fn serve(config: Arc<Config>) -> io::Result<()> {
    BOOT.set(boot::initialize(&config)?).map_err(|_| io::Error::other("Boot epoch already initialized"))?;
    recover_leases(&config, BOOT.get().unwrap()).await?;
    // A failure here leaves the service running for job attestation; each lease checks again.
    let _ = tokio::task::spawn_blocking(winsock_ready).await;
    // Client authentication relies on this order: RUNNING is reported only after this process owns
    // the first pipe instance, so a RUNNING service never coexists with an older squatter instance.
    let mut listener = pipe(&config, true)?;
    let mut tasks = tokio::task::JoinSet::new();
    status(SERVICE_RUNNING, 0);
    loop {
        tokio::select! {
            result = listener.connect() => {
                result?;
                let next = pipe(&config, false)?;
                let connected = std::mem::replace(&mut listener, next);
                let config = config.clone();
                if tasks.len() < 64 { tasks.spawn(async move { connection(connected, config).await }); }
            }
            _ = tokio::time::sleep(Duration::from_millis(250)) => if STOP.load(Ordering::SeqCst) { break; },
            result = tasks.join_next(), if !tasks.is_empty() => {
                if matches!(result, Some(Err(_))) { CLEANUP_FAILED.store(true, Ordering::SeqCst); }
            },
        }
    }
    while let Some(result) = tasks.join_next().await {
        if result.is_err() { CLEANUP_FAILED.store(true, Ordering::SeqCst); }
    }
    if CLEANUP_FAILED.load(Ordering::SeqCst) { Err(io::Error::other("One or more network jobs could not be cleanly stopped")) } else { Ok(()) }
}

/// Runs the SCM callback on its service thread, with a separate async runtime.
unsafe extern "system" fn service_main(count: u32, arguments: *mut *mut u16) {
    let config = CONFIG.get().unwrap().clone();
    let name = match wide(&config.service_name) { Ok(name) => name, Err(_) => return };
    let handle = unsafe { RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(control), null_mut()) };
    if handle.is_null() { return; }
    STATUS.store(handle as usize, Ordering::SeqCst);
    status(SERVICE_START_PENDING, 0);
    // Start parameters follow the service name. Only administrators and SYSTEM may start a service.
    let parameters: Vec<Option<String>> = (1..count as usize).map(|index| scm_text(unsafe { *arguments.add(index) }).ok()).collect();
    if !parameters.is_empty() {
        let result = if parameters == [Some(REMOVE_WINSOCK.to_owned())] { winsock_remove() } else { Err(io::ErrorKind::InvalidInput.into()) };
        return status(SERVICE_STOPPED, if result.is_ok() { 0 } else { ERROR_INVALID_FUNCTION });
    }
    let result = tokio::runtime::Builder::new_multi_thread().enable_all().build()
        .and_then(|runtime| runtime.block_on(serve(Arc::new(config))));
    status(SERVICE_STOPPED, u32::from(result.is_err()));
}

/// Opens the validated private journal on both initial startup and service restart.
fn journal_directory(path: &Path) -> io::Result<std::path::PathBuf> {
    let journal = path.parent().ok_or(io::ErrorKind::InvalidInput)?.join("leases");
    crate::platform_fs::private_dir(&journal, true)?;
    Ok(journal)
}

/// Starts only the configured SYSTEM broker; clients authenticate it through the SCM, so its
/// process DACL is left untouched and grants the owner no handle rights.
pub fn service(path: &Path) -> io::Result<()> {
    let config = config(path, true)?;
    let journal = journal_directory(path)?;
    JOURNAL.set(journal).map_err(|_| io::Error::other("Service already initialized"))?;
    let name = wide(&config.service_name)?;
    let folder = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    let manager = crate::windows_winsock_acl::Manager::machine(&config.service_name, &folder.join("winsock-acl"))?;
    WINSOCK.set((manager, folder.join("winsock-acl-status.json"))).map_err(|_| io::Error::other("Service already initialized"))?;
    CONFIG.set(config).map_err(|_| io::Error::other("Service already initialized"))?;
    NOTIFY.set(Notify::new()).map_err(|_| io::Error::other("Service already initialized"))?;
    let table = [SERVICE_TABLE_ENTRYW { lpServiceName: name.as_ptr().cast_mut(), lpServiceProc: Some(service_main) },
        SERVICE_TABLE_ENTRYW { lpServiceName: null_mut(), lpServiceProc: None }];
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 { return Err(io::Error::last_os_error()); }
    Ok(())
}

/// Keeps the verified pipe open; closing it asks the service to terminate this job and revoke its lease.
pub struct ClientLease { watcher: tokio::task::JoinHandle<()> }
impl Drop for ClientLease {
    /// Closing the lease connection triggers broker teardown and never leaves a detached watcher.
    fn drop(&mut self) { self.watcher.abort(); }
}

struct ServiceHandle(SC_HANDLE);
impl Drop for ServiceHandle {
    /// Closes SCM metadata handles using their required release API.
    fn drop(&mut self) { unsafe { CloseServiceHandle(self.0); } }
}

/// Copies one bounded SCM configuration string.
fn scm_text(value: *const u16) -> io::Result<String> {
    if value.is_null() { return Err(io::ErrorKind::PermissionDenied.into()); }
    let mut len = 0;
    unsafe { while *value.add(len) != 0 { len += 1; if len > 32768 { return Err(io::ErrorKind::InvalidData.into()); } } }
    String::from_utf16(unsafe { std::slice::from_raw_parts(value, len) }).map_err(|_| io::ErrorKind::InvalidData.into())
}

/// Reads the kernel-recorded owner of the connected pipe; non-administrators cannot assign SYSTEM/Administrators.
fn pipe_owner(pipe: HANDLE) -> io::Result<String> {
    let mut owner = null_mut();
    let mut descriptor = null_mut();
    let code = unsafe { GetSecurityInfo(pipe, SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION,
        &mut owner, null_mut(), null_mut(), null_mut(), &mut descriptor) };
    if code != 0 { return Err(io::Error::from_raw_os_error(code as i32)); }
    let text = if owner.is_null() { Err(io::ErrorKind::PermissionDenied.into()) } else { sid_string(owner) };
    unsafe { LocalFree(descriptor); }
    text
}

/// Reads the installed service's type, account and command line, then its live state and PID.
fn registration(config: &Config) -> io::Result<Registration> {
    let manager = ServiceHandle(unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT) });
    if manager.0.is_null() { return Err(io::Error::last_os_error()); }
    let name = wide(&config.service_name)?;
    let service = ServiceHandle(unsafe { OpenServiceW(manager.0, name.as_ptr(), SERVICE_QUERY_STATUS | SERVICE_QUERY_CONFIG) });
    if service.0.is_null() { return Err(io::Error::last_os_error()); }
    let mut needed = 0;
    unsafe { QueryServiceConfigW(service.0, null_mut(), 0, &mut needed); }
    if needed == 0 || needed > 65536 { return Err(io::ErrorKind::InvalidData.into()); }
    let mut bytes = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    if unsafe { QueryServiceConfigW(service.0, bytes.as_mut_ptr().cast(), needed, &mut needed) } == 0 { return Err(io::Error::last_os_error()); }
    let installed = unsafe { &*bytes.as_ptr().cast::<QUERY_SERVICE_CONFIGW>() };
    let (kind, account, command) = (installed.dwServiceType, scm_text(installed.lpServiceStartName)?, scm_text(installed.lpBinaryPathName)?);
    // Status is read last so the PID comparison uses the freshest SCM state.
    let mut status: SERVICE_STATUS_PROCESS = unsafe { std::mem::zeroed() };
    if unsafe { QueryServiceStatusEx(service.0, SC_STATUS_PROCESS_INFO, (&mut status as *mut SERVICE_STATUS_PROCESS).cast(), size_of_val(&status) as u32, &mut needed) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if status.dwServiceType != kind { return Err(io::ErrorKind::PermissionDenied.into()); }
    Ok(Registration { kind, state: status.dwCurrentState, pid: status.dwProcessId, account, command })
}

/// Binds the pipe's kernel-recorded server PID and owner to the SCM's running LocalSystem service.
/// No process handle is opened: a standard user has no rights on the SYSTEM broker process.
fn server(pipe: HANDLE, config: &Config) -> io::Result<()> {
    let mut pid = 0;
    if unsafe { GetNamedPipeServerProcessId(pipe, &mut pid) } == 0 { return Err(io::Error::last_os_error()); }
    let owner = pipe_owner(pipe)?;
    super::authenticate_server(config, pid, &owner, &registration(config)?)
}

/// Checks the installed service and shared key without authorizing or creating a job.
/// A short cache keeps status rendering cheap; each execution authenticates a fresh connection.
pub fn available() -> bool {
    use std::sync::Mutex;
    use std::time::Instant;
    static CACHE: OnceLock<Mutex<Option<(std::ffi::OsString, Instant, bool)>>> = OnceLock::new();
    let Some(path) = std::env::var_os("MINICLAW_NETWORK_CONFIG") else { return false; };
    let Ok(mut cache) = CACHE.get_or_init(|| Mutex::new(None)).lock() else { return false; };
    if let Some((previous, time, ready)) = cache.as_ref() {
        if previous == &path && time.elapsed() < Duration::from_secs(1) { return *ready; }
    }
    let ready = probe(Path::new(&path)).is_ok();
    *cache = Some((path, Instant::now(), ready));
    ready
}

/// Looks at the installed service without authorizing anything, and names the first thing wrong.
/// The sandbox gate words each state for the user; nothing here is cached.
pub fn readiness(path: Option<&Path>) -> crate::windows_gate::Service {
    use crate::windows_gate::Service;
    let Some(path) = path else { return Service::NotConfigured; };
    let config = match config(path, false) {
        Ok(config) => config,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Service::NotConfigured,
        Err(_) => return Service::Unverified,
    };
    match registration(&config) {
        Err(error) if error.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST as i32) => return Service::NotInstalled,
        Err(_) => return Service::Unverified,
        Ok(service) if service.state != super::SERVICE_RUNNING => return Service::Stopped,
        Ok(_) => {}
    }
    // Every pipe instance can be taken for a moment; the service is there, so ask again.
    for _ in 0..5 {
        match probe(path) {
            Ok(_) => return Service::Ready,
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                tracing::debug!(%error, "The network service did not authenticate");
                return Service::Unverified;
            }
        }
    }
    Service::Unverified
}

/// Reads the status the service last published about its Winsock catalog access, as the page
/// shows it. `None` when nothing readable is there.
pub fn winsock_status(path: &Path) -> Option<crate::windows_winsock_acl::AccessStatus> {
    let config = config(path, false).ok()?;
    let service = registration(&config).ok()?;
    let prefix = format!("\"{}\" --network-service \"", config.service_exe);
    let command = service.command;
    if !command.get(..prefix.len()).is_some_and(|start| start.eq_ignore_ascii_case(&prefix)) { return None; }
    let argument = command[prefix.len()..].strip_suffix('"')?;
    let status = Path::new(argument).parent()?.join("winsock-acl-status.json");
    let file = crate::platform_fs::open_read_nofollow(&status).ok()?;
    let mut bytes = Vec::new();
    file.take(65537).read_to_end(&mut bytes).ok()?;
    if bytes.len() > 65536 { return None; }
    crate::windows_winsock_acl::access_status(&serde_json::from_slice(&bytes).ok()?)
}

/// Asks the running service to check its Windows access entries now and repair what Windows
/// removed. Needs no elevation: the service runs as SYSTEM and repairs only its own entries.
/// The error carries the service's reason when it could not repair them.
pub fn repair_access() -> io::Result<()> {
    use std::io::Write;
    let path = std::env::var_os("MINICLAW_NETWORK_CONFIG").ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected,
        "The Windows network service is not configured"))?;
    let (mut pipe, config, hello) = greeted(Path::new(&path))?;
    let mut bytes = serde_json::to_vec(&Repair::access().sign(&config, &hello.challenge)?)?;
    bytes.push(b'\n');
    pipe.write_all(&bytes)?;
    super::lease_answer(&frame_within(&mut pipe, Duration::from_secs(60))?)
}

/// Reads what the service last published about its Winsock catalog access, from the status file
/// beside its registered configuration. `None` when nothing readable is there; the service checks
/// again before every lease, so this only lets the page say early that network access will fail.
pub fn winsock_health(path: &Path) -> Option<Result<(), String>> {
    let status = winsock_status(path)?;
    match status.reason {
        Some(reason) if status.state == "failed" => Some(Err(crate::windows_winsock_acl::refusal(&reason))),
        _ => Some(Ok(())),
    }
}

/// Reads one bounded frame, taking only bytes already present so a silent service cannot block.
fn frame_within(pipe: &mut File, limit: Duration) -> io::Result<Vec<u8>> {
    let started = std::time::Instant::now();
    let mut bytes = Vec::new();
    while started.elapsed() < limit {
        let mut available = 0;
        if unsafe { PeekNamedPipe(pipe.as_raw_handle(), null_mut(), 0, null_mut(), &mut available, null_mut()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if available == 0 { std::thread::sleep(Duration::from_millis(5)); continue; }
        let mut part = vec![0u8; (available as usize).min(FRAME + 1 - bytes.len())];
        let read = pipe.read(&mut part)?;
        if read == 0 { return Err(io::ErrorKind::UnexpectedEof.into()); }
        bytes.extend_from_slice(&part[..read]);
        if bytes.len() > FRAME { return Err(io::ErrorKind::InvalidData.into()); }
        if bytes.last() == Some(&b'\n') { return Ok(bytes); }
    }
    Err(io::ErrorKind::TimedOut.into())
}

/// Opens a fresh connection to the installed service and verifies its identity and greeting.
fn greeted(path: &Path) -> io::Result<(File, Config, Hello)> {
    use std::os::windows::io::FromRawHandle;
    let config = config(path, false)?;
    if user(token(unsafe { GetCurrentProcess() })?.0)? != config.owner_sid { return Err(io::ErrorKind::PermissionDenied.into()); }
    let path = wide(&format!("\\\\.\\pipe\\{}", config.pipe_name))?;
    let handle = unsafe { CreateFileW(path.as_ptr(), PIPE_CLIENT_ACCESS, 0, null(), OPEN_EXISTING,
        SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION, null_mut()) };
    if handle == INVALID_HANDLE_VALUE { return Err(io::Error::last_os_error()); }
    let mut pipe = unsafe { File::from_raw_handle(handle) };
    server(pipe.as_raw_handle(), &config)?;
    let hello: Hello = serde_json::from_slice(&frame_within(&mut pipe, Duration::from_millis(250))?)?;
    hello.verify(&config)?;
    Ok((pipe, config, hello))
}

/// Authenticates the service without authorizing anything, and returns its boot epoch.
fn probe(path: &Path) -> io::Result<BootEpoch> {
    Ok(greeted(path)?.2.boot)
}

/// Asks the service once whether the named run is dead.
fn attest_once(path: &Path, profile: &str, job: &str) -> io::Result<bool> {
    use std::io::Write;
    let (mut pipe, config, hello) = greeted(path)?;
    let question = Attest { profile: profile.to_owned(), job_name: job.to_owned() }.sign(&config, &hello.challenge)?;
    let mut bytes = serde_json::to_vec(&question)?;
    bytes.push(b'\n');
    pipe.write_all(&bytes)?;
    let answer: Attested = serde_json::from_slice(&frame_within(&mut pipe, Duration::from_secs(10))?)?;
    if !answer.ok { return Err(io::ErrorKind::InvalidData.into()); }
    Ok(answer.dead)
}

/// Polls the service until it attests the run dead, or the time runs out.
///
/// `ResourceBusy` means the service answered and still sees the job, a process with the run's
/// identity, or a process it cannot inspect. Any other error means no answer was obtained.
pub(crate) fn attest_within(profile: &str, job: &str, limit: Duration) -> io::Result<()> {
    let path = std::env::var_os("MINICLAW_NETWORK_CONFIG").ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected,
        "The Windows network service is not configured"))?;
    let deadline = std::time::Instant::now() + limit;
    loop {
        match attest_once(Path::new(&path), profile, job) {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            // Every pipe instance is momentarily taken; the service is there, so ask again.
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) && std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(error) => return Err(error),
        }
        if std::time::Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::ResourceBusy, "The Windows network service still sees this sandbox run"));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Obtains the SYSTEM service's attestation that a sandbox run's job is gone and none of its
/// processes is running. Terminations started by the job's destruction get 15 seconds to finish.
pub(crate) fn attest_job_dead(profile: &str, job: &str) -> io::Result<()> {
    attest_within(profile, job, Duration::from_secs(15))
}

/// Retrieves a fresh authenticated OS-lifetime epoch; cached availability is never a recovery proof.
pub fn verified_boot_epoch() -> io::Result<BootEpoch> {
    let path = std::env::var_os("MINICLAW_NETWORK_CONFIG").ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected,
        "The Windows network service must be configured before sandbox recovery"))?;
    probe(Path::new(&path))
}

/// Names the capability a networked run carries so Winsock can read its catalog: the one the
/// installed service granted, taken from the same private configuration that authenticates it.
pub fn winsock_capability() -> io::Result<String> {
    let path = std::env::var_os("MINICLAW_NETWORK_CONFIG").ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "Windows network service is not configured"))?;
    crate::windows_winsock_acl::capability_name(&config(Path::new(&path), false)?.service_name)
}

/// Authenticates one pending sandbox job before its primary thread is resumed.
pub async fn connect(process: &crate::windows_process::Process, profile: &str, port: u16) -> io::Result<ClientLease> {
    connect_ports(process, profile, port, &[], &[]).await
}

/// Authorizes only declared local TCP ports in addition to the host-filtering egress proxy.
pub async fn connect_ports(process: &crate::windows_process::Process, profile: &str, port: u16,
    connect_ports: &[u16], listen_ports: &[u16]) -> io::Result<ClientLease> {
    // Inbound loopback to a sandboxed listener is dropped by Windows outside MiniClaw's filters, so a
    // declared listener would start and never receive a connection. It is refused before any lease.
    if !listen_ports.is_empty() { return Err(io::Error::new(io::ErrorKind::Unsupported, super::LISTEN_UNSUPPORTED)); }
    let path = std::env::var_os("MINICLAW_NETWORK_CONFIG").ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "Windows network service is not configured"))?;
    let config = config(Path::new(&path), false)?;
    if user(token(unsafe { GetCurrentProcess() })?.0)? != config.owner_sid { return Err(io::ErrorKind::PermissionDenied.into()); }
    let path = wide(&format!("\\\\.\\pipe\\{}", config.pipe_name))?;
    let handle = unsafe { CreateFileW(path.as_ptr(), PIPE_CLIENT_ACCESS, 0, null(), OPEN_EXISTING,
        FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION, null_mut()) };
    if handle == INVALID_HANDLE_VALUE { return Err(io::Error::last_os_error()); }
    let pipe = unsafe { NamedPipeClient::from_raw_handle(handle)? };
    server(pipe.as_raw_handle(), &config)?;
    let mut reader = BufReader::new(pipe);
    let challenge = tokio::time::timeout(Duration::from_secs(5), frame(&mut reader)).await.map_err(|_| io::ErrorKind::TimedOut)??;
    let hello: Hello = serde_json::from_slice(&challenge)?;
    hello.verify(&config)?;
    let challenge = &hello.challenge;
    let (job_name, job_handle) = process.job_descriptor();
    let signed = Request { profile: profile.to_owned(), job_name, job_handle, proxy_port: port, connect_ports: connect_ports.to_vec(), listen_ports: listen_ports.to_vec() }.sign(&config, challenge)?;
    send(reader.get_mut(), &signed).await?;
    let response = tokio::time::timeout(Duration::from_secs(10), frame(&mut reader)).await.map_err(|_| io::ErrorKind::TimedOut)??;
    super::lease_answer(&response)?;
    let mut pipe = reader.into_inner();
    let cancel = process.cancellation();
    let watcher = tokio::spawn(async move {
        let _ = pipe.read(&mut [0u8; 1]).await;
        cancel();
    });
    Ok(ClientLease { watcher })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Owner clients can inspect/read the pipe but cannot impersonate its server.
    #[tokio::test]
    async fn pipe_allows_client_metadata_without_server_creation() {
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let config = Config {
            owner_sid: current_user_sid().unwrap(),
            pipe_name: format!("MiniClawNetwork-{suffix}"),
            service_name: format!("MiniClawNetwork-{suffix}"),
            service_exe: std::env::current_exe().unwrap().to_string_lossy().into_owned(),
            secret_hex: "12".repeat(32),
        };
        let _server = pipe(&config, true).unwrap();
        let name = wide(&format!("\\\\.\\pipe\\{}", config.pipe_name)).unwrap();
        let raw = unsafe { CreateFileW(name.as_ptr(), FILE_GENERIC_READ | FILE_WRITE_DATA,
            0, null(), OPEN_EXISTING, SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION, null_mut()) };
        assert_ne!(raw, INVALID_HANDLE_VALUE, "client open: {}", io::Error::last_os_error());
        let client = Handle(raw);
        let mut pid = 0;
        assert_ne!(unsafe { GetNamedPipeServerProcessId(client.0, &mut pid) }, 0,
            "server PID: {}", io::Error::last_os_error());
        assert_eq!(pid, std::process::id());
        let mut available = 0;
        assert_ne!(unsafe { PeekNamedPipe(client.0, null_mut(), 0, null_mut(), &mut available, null_mut()) },
            0, "peek: {}", io::Error::last_os_error());
        let mut acl = null_mut();
        let mut descriptor = null_mut();
        let code = unsafe { GetSecurityInfo(client.0, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION,
            null_mut(), null_mut(), &mut acl, null_mut(), &mut descriptor) };
        assert_eq!(code, 0);
        assert!(!acl.is_null());
        let mut owner_aces = 0;
        for index in 0..unsafe { (*acl).AceCount } {
            let mut raw = null_mut();
            assert_ne!(unsafe { GetAce(acl, index as u32, &mut raw) }, 0);
            let ace = unsafe { &*raw.cast::<ACCESS_ALLOWED_ACE>() };
            let sid = sid_string((&ace.SidStart as *const u32).cast_mut().cast()).unwrap();
            if sid == config.owner_sid {
                owner_aces += 1;
                assert_eq!(ace.Mask & FILE_CREATE_PIPE_INSTANCE, 0, "owner client ACE must not permit server creation");
            }
        }
        unsafe { LocalFree(descriptor); }
        assert_eq!(owner_aces, 1, "expected one owner client ACE");
        let other = unsafe { CreateNamedPipeW(name.as_ptr(), PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            255, FRAME as u32, FRAME as u32, 5000, null()) };
        let error = io::Error::last_os_error();
        if other != INVALID_HANDLE_VALUE { unsafe { CloseHandle(other); } }
        assert_eq!(other, INVALID_HANDLE_VALUE, "client must not create a server instance");
        assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
    }

    /// A restarted broker reopens its private journal without losing recovery records.
    #[test]
    fn journal_directory_survives_restart() {
        let root = std::env::temp_dir().join(format!("mc-service-restart-{}", uuid::Uuid::new_v4()));
        crate::platform_fs::private_dir(&root, false).unwrap();
        let path = root.join("service.json");
        let first = journal_directory(&path).unwrap();
        std::fs::write(first.join("record"), b"recovery fixture").unwrap();
        let reopened = journal_directory(&path).unwrap();
        assert_eq!(std::fs::read(reopened.join("record")).unwrap(), b"recovery fixture");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn authenticated_boot_epoch_is_stable_for_running_service() {
        let first = verified_boot_epoch().expect("Native acceptance requires an installed owner-bound broker");
        let second = verified_boot_epoch().unwrap();
        assert_eq!(first, second);
        assert!(!second.supersedes(&first));
    }

    /// A pipe squatted by this non-service process must fail against the real installed registration.
    #[tokio::test]
    async fn squatted_pipe_is_rejected_for_installed_service() {
        let path = std::env::var_os("MINICLAW_NETWORK_CONFIG").expect("Native acceptance requires an installed owner-bound broker");
        let installed = config(Path::new(&path), false).unwrap();
        let mut squatted = installed.clone();
        squatted.pipe_name = format!("MiniClawNetwork-{}", uuid::Uuid::new_v4().simple());
        let _server = pipe(&squatted, true).unwrap();
        let name = wide(&format!("\\\\.\\pipe\\{}", squatted.pipe_name)).unwrap();
        let client = Handle(unsafe { CreateFileW(name.as_ptr(), PIPE_CLIENT_ACCESS, 0, null(), OPEN_EXISTING,
            SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION, null_mut()) });
        assert_ne!(client.0, INVALID_HANDLE_VALUE, "client open: {}", io::Error::last_os_error());
        let mut pid = 0;
        assert_ne!(unsafe { GetNamedPipeServerProcessId(client.0, &mut pid) }, 0);
        assert_eq!(pid, std::process::id());
        let service = registration(&installed).unwrap();
        assert_ne!(service.pid, pid, "the real broker is a different live process");
        let error = server(client.0, &squatted).expect_err("a squatted pipe must not authenticate as the service");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        // The squatter's own PID alone never satisfies the check, whatever owner the pipe reports.
        assert!(super::super::authenticate_server(&installed, pid, "S-1-5-18", &service).is_err());
    }

    /// Reports authentication stages for the disposable standard-user broker and proves that the
    /// account authenticates the service while holding no rights at all on the SYSTEM broker process.
    #[test]
    #[ignore = "diagnostic under the preserved disposable standard account"]
    fn standard_user_boot_auth_diagnostic() {
        fn checked(stage: &str, ok: bool) {
            let error = io::Error::last_os_error();
            println!("AUTH {stage}: success={ok}, error={:?}", if ok { None } else { error.raw_os_error() });
            assert!(ok, "{stage}: {error}");
        }
        let path = std::env::var_os("MINICLAW_NETWORK_CONFIG").unwrap();
        let config = config(Path::new(&path), false).expect("AUTH private config");
        println!("AUTH private config: success=true");
        assert_eq!(current_user_sid().unwrap(), config.owner_sid, "AUTH configured owner mismatch");
        println!("AUTH configured owner: success=true");
        let name = wide(&format!("\\\\.\\pipe\\{}", config.pipe_name)).unwrap();
        let pipe = Handle(unsafe { CreateFileW(name.as_ptr(), PIPE_CLIENT_ACCESS, 0, null(), OPEN_EXISTING,
            SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION, null_mut()) });
        checked("CreateFile pipe", pipe.0 != INVALID_HANDLE_VALUE);
        let mut pid = 0;
        checked("GetNamedPipeServerProcessId", unsafe { GetNamedPipeServerProcessId(pipe.0, &mut pid) } != 0);
        let owner = pipe_owner(pipe.0).expect("AUTH pipe owner");
        println!("AUTH pipe owner: {owner}");
        let service = registration(&config).expect("AUTH service registration");
        println!("AUTH service registration: kind={:#x}, state={}, pid_matches={}, account={}",
            service.kind, service.state, service.pid == pid, service.account);
        for (name, rights) in [("limited query", PROCESS_QUERY_LIMITED_INFORMATION), ("synchronize", SYNCHRONIZE),
            ("duplicate handle", PROCESS_DUP_HANDLE), ("read memory", PROCESS_VM_READ),
            ("write memory", PROCESS_VM_WRITE), ("create thread", PROCESS_CREATE_THREAD),
            ("terminate", PROCESS_TERMINATE), ("write DACL", WRITE_DAC)] {
            let forbidden = unsafe { OpenProcess(rights, 0, pid) };
            let error = io::Error::last_os_error();
            if !forbidden.is_null() { unsafe { CloseHandle(forbidden); } }
            assert!(forbidden.is_null(), "Standard account obtained forbidden broker right: {name}");
            assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32), "Unexpected {name} failure");
            println!("AUTH forbidden {name}: denied");
        }
        server(pipe.0, &config).expect("AUTH complete server identity");
        println!("AUTH server identity: success=true");
        drop(pipe);
        verified_boot_epoch().expect("AUTH complete authenticated epoch");
        println!("AUTH authenticated epoch: success=true");
    }

    /// A matching user and named job do not authorize an ordinary unsandboxed process.
    #[tokio::test]
    async fn rejects_non_appcontainer_job_without_terminating_it() {
        let program = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let args = crate::powershell::arguments("Start-Sleep -Seconds 120").unwrap();
        let child = crate::windows_process::stdio(&program, &args, &std::env::temp_dir(), &[]).unwrap();
        let (job_name, job_handle) = child.process.job_descriptor();
        assert!(crate::windows_process::TreeGuard::open_verified(&job_name,
            &format!("MiniClaw.Run.{}", uuid::Uuid::new_v4().simple())).is_err());
        assert!(child.process.try_wait().unwrap().is_none(), "Rejected recovery must not terminate unrelated processes");
        let owner = user(token(unsafe { GetCurrentProcess() }).unwrap().0).unwrap();
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let config = Config { owner_sid: owner, pipe_name: format!("MiniClawNetwork-{suffix}"),
            service_name: format!("MiniClawNetwork-{suffix}"), service_exe: std::env::current_exe().unwrap().to_string_lossy().into_owned(), secret_hex: "12".repeat(32) };
        let request = Request { profile: format!("MiniClaw.Run.{suffix}"), job_name, job_handle, proxy_port: 8123,
            connect_ports: Vec::new(), listen_ports: Vec::new() };
        let denied = bind_job(unsafe { GetCurrentProcess() }, &config, &request);
        assert!(denied.is_err());
        assert!(child.process.try_wait().unwrap().is_none(), "An unverified job must never enter the terminating guard");
        drop(child);
    }
}
