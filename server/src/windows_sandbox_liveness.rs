//! Proves that a destroyed sandbox job left no running process, without a Windows restart.
// Reuses the run identity derived by recovery.
use super::*;
// Enumerates processes and reads their token identity.
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
        RemoteDesktop::{
            ProcessIdToSessionId, WTS_PROCESS_INFOW, WTSEnumerateProcessesW, WTSFreeMemory,
        },
        Threading::{
            GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
            PROCESS_QUERY_LIMITED_INFORMATION, WaitForSingleObject,
        },
    },
};

struct Owned(HANDLE);
impl Drop for Owned {
    /// Releases one process, token or snapshot handle.
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

enum Seen {
    /// Exited, or running with a readable token that is not this run's.
    Unrelated,
    /// Running with this run's package identity.
    Member,
    /// Could not be inspected; decided by session and account instead.
    Opaque,
}

/// Reads token information into aligned storage.
fn token_info(token: HANDLE, class: TOKEN_INFORMATION_CLASS) -> io::Result<Vec<usize>> {
    unsafe {
        let mut length = 0;
        GetTokenInformation(token, class, ptr::null_mut(), 0, &mut length);
        let mut storage = vec![0usize; (length as usize).div_ceil(size_of::<usize>()).max(1)];
        if GetTokenInformation(token, class, storage.as_mut_ptr().cast(), length, &mut length) == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(storage)
    }
}

/// Classifies one process ID by its exit state and AppContainer identity.
fn inspect(pid: u32, package: PSID) -> Seen {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid);
        if process.is_null() {
            // ERROR_INVALID_PARAMETER: no process has this ID any more.
            return if io::Error::last_os_error().raw_os_error() == Some(87) {
                Seen::Unrelated
            } else {
                Seen::Opaque
            };
        }
        let process = Owned(process);
        if WaitForSingleObject(process.0, 0) == WAIT_OBJECT_0 {
            return Seen::Unrelated;
        }
        let mut token = ptr::null_mut();
        if OpenProcessToken(process.0, TOKEN_QUERY, &mut token) == 0 {
            return Seen::Opaque;
        }
        let token = Owned(token);
        let Ok(info) = token_info(token.0, TokenAppContainerSid) else {
            return Seen::Opaque;
        };
        let sid = (*info.as_ptr().cast::<TOKEN_APPCONTAINER_INFORMATION>()).TokenAppContainer;
        if !sid.is_null() && EqualSid(sid, package) != 0 {
            Seen::Member
        } else {
            Seen::Unrelated
        }
    }
}

/// Lists every process ID and image name on the system; no access to the processes is needed.
fn processes() -> io::Result<Vec<(u32, String)>> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let snapshot = Owned(snapshot);
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snapshot.0, &mut entry) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut ids = Vec::new();
        loop {
            let length = entry.szExeFile.iter().position(|unit| *unit == 0).unwrap_or(0);
            ids.push((entry.th32ProcessID, String::from_utf16_lossy(&entry.szExeFile[..length])));
            if Process32NextW(snapshot.0, &mut entry) == 0 {
                let error = io::Error::last_os_error();
                // ERROR_NO_MORE_FILES ends the snapshot.
                return if error.raw_os_error() == Some(18) {
                    Ok(ids)
                } else {
                    Err(error)
                };
            }
        }
    }
}

/// Reports which uninspectable processes could still be this user's in this session.
///
/// A sandboxed process keeps its launcher's session and account, so a process in another
/// session or under another account cannot belong to the run. A process Windows reports no
/// account for stays undecided.
fn undecided(opaque: &[(u32, String)]) -> io::Result<Vec<String>> {
    unsafe {
        let mut session = 0;
        if ProcessIdToSessionId(GetCurrentProcessId(), &mut session) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut token = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = Owned(token);
        let user = token_info(token.0, TokenUser)?;
        let user = (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid;
        let mut list: *mut WTS_PROCESS_INFOW = ptr::null_mut();
        let mut count = 0;
        // A null server handle is WTS_CURRENT_SERVER_HANDLE.
        if WTSEnumerateProcessesW(ptr::null_mut(), 0, 1, &mut list, &mut count) == 0 {
            return Err(io::Error::last_os_error());
        }
        let listed = std::slice::from_raw_parts(list, count as usize);
        let remaining = opaque
            .iter()
            .filter_map(|(pid, name)| {
                // Absent from this later listing means the process has since exited.
                let item = listed.iter().find(|item| item.ProcessId == *pid)?;
                let same_account = item.pUserSid.is_null() || EqualSid(item.pUserSid, user) != 0;
                (item.SessionId == session && same_account).then(|| {
                    let account = if item.pUserSid.is_null() { "unreported" } else { "this" };
                    format!("{name} (pid {pid}, {account} account)")
                })
            })
            .collect();
        WTSFreeMemory(list.cast());
        Ok(remaining)
    }
}

/// Waits until no process that could carry this run's identity is still running.
///
/// The caller has already found that the run's named job no longer exists. That job was created
/// kill-on-close without breakaway, so Windows began terminating every member when its last
/// handle closed; this confirms the terminations have finished. A process that cannot be
/// inspected and might be the user's keeps the result uncertain, and uncertainty is an error.
pub(super) fn no_survivor(package: PSID, timeout: std::time::Duration) -> io::Result<()> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let mut member = None;
        let mut opaque = Vec::new();
        for (pid, name) in processes()? {
            match inspect(pid, package) {
                Seen::Unrelated => {}
                Seen::Member => member = Some(pid),
                // The idle pseudo-process has no token and runs no program.
                Seen::Opaque if pid == 0 => {}
                Seen::Opaque => opaque.push((pid, name)),
            }
        }
        let opaque = if opaque.is_empty() { Vec::new() } else { undecided(&opaque)? };
        if member.is_none() && opaque.is_empty() {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(io::Error::other(match member {
                Some(pid) => format!("Sandboxed process {pid} is still running after its job closed"),
                None => format!(
                    "These processes in this session could not be inspected, so sandbox termination is unproven: {}",
                    opaque.join(", ")
                ),
            }));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
