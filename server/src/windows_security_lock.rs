//! Serializes security descriptor changes across MiniClaw processes and services.
// Owns native locks without poisoning future cleanup.
use std::{io, ptr};
// Restricts named mutex access to trusted accounts.
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    System::Threading::*,
};

pub(crate) struct Guard(HANDLE);
impl Drop for Guard {
    /// Releases the acquired mutex and closes its process handle.
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}
struct Allocation(*mut std::ffi::c_void);
impl Drop for Allocation {
    /// Frees a native security allocation on every exit path.
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

/// Acquires the per-user ACL lock or the shared admin-only loopback-configuration lock.
pub(crate) fn acquire(admin: bool) -> io::Result<Guard> {
    let user = if admin { "BA".into() } else { user_sid()? };
    let name = if admin {
        "Global\\MiniClaw.NetworkIsolation".into()
    } else {
        format!("Global\\MiniClaw.SandboxAcl.{user}")
    };
    let sddl = if admin {
        "O:BAD:P(A;;GA;;;SY)(A;;GA;;;BA)".into()
    } else {
        format!("O:{user}D:P(A;;GA;;;{user})")
    };
    let mut descriptor = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide(&sddl).as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let _descriptor = Allocation(descriptor);
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let handle = unsafe { CreateMutexW(&attributes, 0, wide(&name).as_ptr()) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    if let Err(error) = validate(handle, &user, admin) {
        unsafe {
            CloseHandle(handle);
        }
        return Err(error);
    }
    let status = unsafe { WaitForSingleObject(handle, 30_000) };
    if matches!(status, WAIT_OBJECT_0 | WAIT_ABANDONED) {
        Ok(Guard(handle))
    } else {
        unsafe {
            CloseHandle(handle);
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Security update lock unavailable",
        ))
    }
}

/// Rejects a precreated named object that grants access to an unexpected account.
fn validate(handle: HANDLE, user: &str, admin: bool) -> io::Result<()> {
    unsafe {
        let mut owner = ptr::null_mut();
        let mut dacl = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        let status = GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        );
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        let _descriptor = Allocation(descriptor);
        let mut control = 0;
        let mut revision = 0;
        if dacl.is_null()
            || owner.is_null()
            || GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) == 0
            || control & SE_DACL_PROTECTED == 0
        {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let allowed = if admin {
            vec!["S-1-5-18", "S-1-5-32-544"]
        } else {
            vec![user]
        };
        if !allowed.contains(&sid_string(owner)?.as_str())
            || (*dacl).AceCount as usize != allowed.len()
        {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        for index in 0..(*dacl).AceCount {
            let mut ace = ptr::null_mut();
            if GetAce(dacl, index as u32, &mut ace) == 0 {
                return Err(io::Error::last_os_error());
            }
            let ace = &*ace.cast::<ACCESS_ALLOWED_ACE>();
            if ace.Header.AceType != 0
                || !allowed
                    .contains(&sid_string(ptr::addr_of!(ace.SidStart).cast_mut().cast())?.as_str())
            {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
        }
        Ok(())
    }
}

/// Reads the current user SID without accessing any saved credential data.
fn user_sid() -> io::Result<String> {
    unsafe {
        let mut token = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut length = 0;
        GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut length);
        let mut storage = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        let success = GetTokenInformation(
            token,
            TokenUser,
            storage.as_mut_ptr().cast(),
            length,
            &mut length,
        );
        CloseHandle(token);
        if success == 0 {
            return Err(io::Error::last_os_error());
        }
        sid_string((*storage.as_ptr().cast::<TOKEN_USER>()).User.Sid)
    }
}

/// Converts an OS-validated SID into its stable textual account identifier.
fn sid_string(sid: PSID) -> io::Result<String> {
    unsafe {
        let mut text = ptr::null_mut();
        if ConvertSidToStringSidW(sid, &mut text) == 0 {
            return Err(io::Error::last_os_error());
        }
        let _text = Allocation(text.cast());
        let mut length = 0;
        while *text.add(length) != 0 {
            length += 1;
        }
        String::from_utf16(std::slice::from_raw_parts(text, length)).map_err(io::Error::other)
    }
}

/// Encodes validated text for a synchronous native call.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    // Verifies independent owners serialize actual native mutations.
    use super::*;

    #[test]
    /// Two backend threads cannot hold the same user's security update lock concurrently.
    fn serializes_user_updates() {
        let first = acquire(false).unwrap();
        let (attempted, attempt) = std::sync::mpsc::channel();
        let (entered, entry) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            attempted.send(()).unwrap();
            let _second = acquire(false).unwrap();
            entered.send(()).unwrap();
        });
        attempt.recv().unwrap();
        assert!(
            entry
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err()
        );
        drop(first);
        entry
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        worker.join().unwrap();
    }
}
