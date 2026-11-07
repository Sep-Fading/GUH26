//! Answers, for the SYSTEM broker, whether a finished sandbox run left any running process.
// Reuses the broker's handle wrapper and token readers.
use super::{Handle, token, token_info, wide};
// Carries the validated names of the run in question.
use crate::windows_network_broker::Attest;
// Reports native failures to the connection handler.
use std::{io, ptr::{null, null_mut}};
// Opens jobs, lists processes and reads their tokens.
use windows_sys::Win32::{
    Foundation::*,
    Security::{Isolation::DeriveAppContainerSidFromAppContainerName, *},
    Storage::FileSystem::SYNCHRONIZE,
    System::{
        Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS},
        JobObjects::OpenJobObjectW,
        Threading::{GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION, WaitForSingleObject},
    },
};

/// Pause between the two process listings that must both come back clean.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(100);

struct Package(PSID);
impl Drop for Package {
    /// Releases the SID derived from the profile name.
    fn drop(&mut self) { unsafe { FreeSid(self.0); } }
}

/// Turns on the debug privilege LocalSystem holds, so every process on the machine can be opened.
fn enable_debug() -> io::Result<()> {
    let name = wide("SeDebugPrivilege")?;
    let mut privileges = TOKEN_PRIVILEGES { PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES { Luid: LUID { LowPart: 0, HighPart: 0 }, Attributes: SE_PRIVILEGE_ENABLED }] };
    if unsafe { LookupPrivilegeValueW(null(), name.as_ptr(), &mut privileges.Privileges[0].Luid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut raw = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Handle(raw);
    // The call reports success even when the token lacks the privilege; the last error tells.
    if unsafe { AdjustTokenPrivileges(token.0, 0, &privileges, 0, null_mut(), null_mut()) } == 0
        || unsafe { GetLastError() } != ERROR_SUCCESS {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// True only when Windows reports that no object has this name; any other outcome is not proof.
fn job_absent(name: &[u16]) -> bool {
    // Documented job right; windows-sys does not export the constant.
    const JOB_OBJECT_QUERY: u32 = 0x0004;
    let job = unsafe { OpenJobObjectW(JOB_OBJECT_QUERY, 0, name.as_ptr()) };
    if !job.is_null() {
        drop(Handle(job));
        return false;
    }
    unsafe { GetLastError() == ERROR_FILE_NOT_FOUND }
}

/// Lists the ID of every process on the machine.
fn processes() -> io::Result<Vec<u32>> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE { return Err(io::Error::last_os_error()); }
    let snapshot = Handle(snapshot);
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    if unsafe { Process32FirstW(snapshot.0, &mut entry) } == 0 { return Err(io::Error::last_os_error()); }
    let mut ids = Vec::new();
    loop {
        ids.push(entry.th32ProcessID);
        if unsafe { Process32NextW(snapshot.0, &mut entry) } == 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) { Ok(ids) } else { Err(error) };
        }
    }
}

/// True when this process ID is gone, has exited, or runs under a token that is not the run's.
/// A running process whose token cannot be read counts against the proof.
fn unrelated(pid: u32, package: PSID) -> bool {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
    if process.is_null() {
        // No process has this ID any more; the idle pseudo-process (ID 0) reports the same code.
        return unsafe { GetLastError() } == ERROR_INVALID_PARAMETER;
    }
    let process = Handle(process);
    let exited = || unsafe { WaitForSingleObject(process.0, 0) } == WAIT_OBJECT_0;
    if exited() { return true; }
    let read = || -> io::Result<bool> {
        let token = token(process.0)?;
        let info = token_info(token.0, TokenAppContainerSid)?;
        let sid = unsafe { (*info.as_ptr().cast::<TOKEN_APPCONTAINER_INFORMATION>()).TokenAppContainer };
        Ok(sid.is_null() || unsafe { EqualSid(sid, package) } == 0)
    };
    // A process that exited while it was being read no longer runs anything.
    read().unwrap_or_else(|_| exited())
}

/// Decides whether the named run is dead: its job name is gone and no process carries its identity.
///
/// The job was created kill-on-close without breakaway, so a missing name means its last handle
/// closed and Windows terminated every member. Two clean listings confirm those terminations
/// finished, and the name is checked again afterwards. Anything unreadable answers `false`.
pub(super) fn dead(attest: &Attest) -> io::Result<bool> {
    attest.validate()?;
    enable_debug()?;
    let job = wide(&attest.job_name)?;
    let profile = wide(&attest.profile)?;
    let mut package = null_mut();
    if unsafe { DeriveAppContainerSidFromAppContainerName(profile.as_ptr(), &mut package) } < 0 {
        return Err(io::Error::other("Invalid AppContainer profile"));
    }
    let package = Package(package);
    if !job_absent(&job) { return Ok(false); }
    for pass in 0..2 {
        if pass != 0 { std::thread::sleep(SETTLE); }
        if !processes()?.into_iter().all(|pid| unrelated(pid, package.0)) { return Ok(false); }
    }
    Ok(job_absent(&job))
}
