//! Keeps one authenticated boot epoch in an OS-owned volatile registry key.
// Reuses only fixed installation identity and native SID conversion helpers.
use super::{BootEpoch, Config, current_user_sid, sid_string, wide};
use sha2::{Digest, Sha256};
use std::{io, ptr::{null, null_mut}};
use windows_sys::Win32::{Foundation::*, Security::{*, Authorization::*}, System::Registry::*};

struct Key(HKEY);
impl Drop for Key {
    /// Registry handles require RegCloseKey rather than CloseHandle.
    fn drop(&mut self) { unsafe { RegCloseKey(self.0); } }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Record { epoch: String, installation_hash: Vec<u8> }

/// Accepts only a protected SYSTEM-owned key; administrators may inspect but ordinary users cannot change it.
fn secure(key: HKEY) -> io::Result<()> {
    let mut owner = null_mut();
    let mut acl = null_mut();
    let mut descriptor = null_mut();
    let code = unsafe { GetSecurityInfo(key, SE_REGISTRY_KEY, OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
        &mut owner, null_mut(), &mut acl, null_mut(), &mut descriptor) };
    if code != 0 { return Err(io::Error::from_raw_os_error(code as i32)); }
    let checked = (|| -> io::Result<()> {
        let mut control = 0;
        let mut revision = 0;
        if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
            || control & SE_DACL_PROTECTED == 0 || acl.is_null() || sid_string(owner)? != "S-1-5-18" {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        for index in 0..unsafe { (*acl).AceCount } {
            let mut raw = null_mut();
            if unsafe { GetAce(acl, index as u32, &mut raw) } == 0 { return Err(io::Error::last_os_error()); }
            let header = unsafe { &*raw.cast::<ACE_HEADER>() };
            if header.AceType != 0 { return Err(io::ErrorKind::PermissionDenied.into()); }
            let ace = unsafe { &*raw.cast::<ACCESS_ALLOWED_ACE>() };
            let sid = sid_string((&ace.SidStart as *const u32).cast_mut().cast())?;
            if sid != "S-1-5-18" && !(sid == "S-1-5-32-544" && ace.Mask & !KEY_READ == 0) {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
        }
        Ok(())
    })();
    unsafe { LocalFree(descriptor); }
    checked
}

/// Reuses the same epoch after service restarts and creates a new one only after full OS restart.
pub(super) fn initialize(config: &Config) -> io::Result<BootEpoch> {
    config.validate()?;
    if current_user_sid()? != "S-1-5-18" { return Err(io::ErrorKind::PermissionDenied.into()); }
    let suffix = config.service_name.strip_prefix("MiniClawNetwork-").ok_or(io::ErrorKind::InvalidInput)?;
    let path = wide(&format!("SYSTEM\\CurrentControlSet\\Control\\MiniClawBoot-{suffix}"))?;
    let sddl = wide("O:SYD:P(A;;KA;;;SY)(A;;KR;;;BA)")?;
    let mut descriptor = null_mut();
    if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1,
        &mut descriptor, null_mut()) } == 0 { return Err(io::Error::last_os_error()); }
    let attributes = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor, bInheritHandle: 0 };
    let mut key = null_mut();
    let mut disposition = 0;
    // HKLM volatile keys disappear only on a full shutdown, not service restart or user logoff.
    let code = unsafe { RegCreateKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, null_mut(), REG_OPTION_VOLATILE,
        KEY_READ | KEY_WRITE | KEY_WOW64_64KEY, &attributes, &mut key, &mut disposition) };
    unsafe { LocalFree(descriptor); }
    if code != ERROR_SUCCESS { return Err(io::Error::from_raw_os_error(code as i32)); }
    let key = Key(key);
    secure(key.0)?;
    let name = wide("Epoch")?;
    let expected: Vec<u8> = Sha256::digest(serde_json::to_vec(config)?).to_vec();
    if disposition == REG_CREATED_NEW_KEY {
        let record = Record { epoch: uuid::Uuid::new_v4().simple().to_string(), installation_hash: expected.clone() };
        let bytes = serde_json::to_vec(&record)?;
        let code = unsafe { RegSetValueExW(key.0, name.as_ptr(), 0, REG_BINARY, bytes.as_ptr(), bytes.len() as u32) };
        if code != ERROR_SUCCESS { return Err(io::Error::from_raw_os_error(code as i32)); }
    }
    // An existing empty/malformed record is never replaced: it may represent an interrupted initialization.
    let mut bytes = vec![0u8; 2048];
    let mut count = bytes.len() as u32;
    let mut kind = 0;
    let code = unsafe { RegQueryValueExW(key.0, name.as_ptr(), null(), &mut kind, bytes.as_mut_ptr(), &mut count) };
    if code != ERROR_SUCCESS { return Err(io::Error::from_raw_os_error(code as i32)); }
    if kind != REG_BINARY || count == 0 || count as usize > bytes.len() { return Err(io::ErrorKind::InvalidData.into()); }
    let record: Record = serde_json::from_slice(&bytes[..count as usize])?;
    if record.installation_hash != expected { return Err(io::Error::new(io::ErrorKind::PermissionDenied,
        "Boot epoch installation identity changed; existing sandbox records must be preserved")); }
    let epoch = BootEpoch { installation: config.service_name.clone(), epoch: record.epoch };
    if !epoch.valid() { return Err(io::ErrorKind::InvalidData.into()); }
    Ok(epoch)
}
