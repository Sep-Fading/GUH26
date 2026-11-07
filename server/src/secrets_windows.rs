//! Generic credentials in the signed-in user's Windows Credential Manager.
// Maps all native failures to opaque unavailability.
use super::{SERVICE, Unavailable};
// Owns returned credentials and last-error codes.
use windows_sys::Win32::{
    Foundation::{ERROR_NOT_FOUND, GetLastError},
    Security::Credentials::*,
};

/// Namespaces an exact credential key without allowing embedded NULs.
fn target(key: &str) -> Result<Vec<u16>, Unavailable> {
    if key.is_empty() || key.contains('\0') {
        return Err(Unavailable);
    }
    let name: Vec<_> = format!("{SERVICE}/{key}")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    if name.len() > CRED_MAX_GENERIC_TARGET_NAME_LENGTH as usize {
        return Err(Unavailable);
    }
    Ok(name)
}

/// Persists bytes on this computer for the current user's future logon sessions.
pub(super) fn native_set(key: &str, value: &str) -> Result<(), Unavailable> {
    let mut name = target(key)?;
    if value.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
        return Err(Unavailable);
    }
    let mut credential: CREDENTIALW = unsafe { std::mem::zeroed() };
    credential.Type = CRED_TYPE_GENERIC;
    credential.TargetName = name.as_mut_ptr();
    credential.CredentialBlobSize = value.len() as u32;
    credential.CredentialBlob = value.as_ptr().cast_mut();
    credential.Persist = CRED_PERSIST_LOCAL_MACHINE;
    if unsafe { CredWriteW(&credential, 0) } == 0 {
        Err(Unavailable)
    } else {
        Ok(())
    }
}

/// Returns only this server's stored value and always frees native allocations.
pub(super) fn native_get(key: &str) -> Result<Option<String>, Unavailable> {
    let name = target(key)?;
    let mut credential = std::ptr::null_mut();
    unsafe {
        if CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) == 0 {
            return if GetLastError() == ERROR_NOT_FOUND {
                Ok(None)
            } else {
                Err(Unavailable)
            };
        }
        let value = if (*credential).CredentialBlobSize == 0 {
            Ok(String::new())
        } else if (*credential).CredentialBlob.is_null() {
            Err(Unavailable)
        } else {
            String::from_utf8(
                std::slice::from_raw_parts(
                    (*credential).CredentialBlob,
                    (*credential).CredentialBlobSize as usize,
                )
                .to_vec(),
            )
            .map_err(|_| Unavailable)
        };
        CredFree(credential.cast());
        value.map(Some)
    }
}

/// Missing credentials count as successful removal.
pub(super) fn native_remove(key: &str) -> Result<(), Unavailable> {
    let name = target(key)?;
    unsafe {
        if CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) != 0
            || GetLastError() == ERROR_NOT_FOUND
        {
            Ok(())
        } else {
            Err(Unavailable)
        }
    }
}
