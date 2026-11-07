//! Test-only, recoverable registration right for the fixed PowerShell ETW provider.
use std::{io::{self, Write}, path::PathBuf, ptr};
use miniclaw_server::platform_fs;
use windows_sys::{core::GUID, Win32::{Foundation::*, Security::{*, Authorization::*},
    System::{Diagnostics::Etw::*, Registry::*}}};
const PROVIDER: GUID = GUID::from_u128(0xf90714a8_5509_434a_bf6d_b1624c8a19a2);

fn journal() -> PathBuf {
    PathBuf::from(std::env::var_os("MINICLAW_DATA_DIR").unwrap()).join("powershell-etw-diagnostic.json")
}
fn query() -> io::Result<Vec<u8>> {
    let mut size = 0;
    let code = unsafe { EventAccessQuery(&PROVIDER, ptr::null_mut(), &mut size) };
    if code != ERROR_MORE_DATA && code != ERROR_SUCCESS { return Err(io::Error::from_raw_os_error(code as i32)); }
    if size == 0 || size > 65536 { return Err(io::Error::other("Invalid ETW security descriptor size")); }
    let mut buffer = vec![0u8; size as usize];
    let code = unsafe { EventAccessQuery(&PROVIDER, buffer.as_mut_ptr().cast(), &mut size) };
    if code != 0 { return Err(io::Error::from_raw_os_error(code as i32)); }
    buffer.truncate(size as usize);
    Ok(buffer)
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Record { before: Vec<u8>, sid: String }

fn save(record: &Record, new: bool) -> io::Result<()> {
    let bytes = serde_json::to_vec(record)?;
    let mut file = platform_fs::private_file(&journal(), new)?;
    file.write_all(&bytes)?;
    file.set_len(bytes.len() as u64)?;
    file.sync_all()
}
pub struct Grant;
impl Grant {
    pub fn new(sid: PSID) -> io::Result<Self> {
        // Never replace a preexisting provider customization in this diagnostic.
        let key: Vec<u16> = "SYSTEM\\CurrentControlSet\\Control\\WMI\\Security\0".encode_utf16().collect();
        let value: Vec<u16> = "{f90714a8-5509-434a-bf6d-b1624c8a19a2}\0".encode_utf16().collect();
        let code = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), value.as_ptr(),
            RRF_RT_ANY, ptr::null_mut(), ptr::null_mut(), ptr::null_mut()) };
        if code != ERROR_FILE_NOT_FOUND { return Err(io::Error::other("ETW diagnostic requires absent explicit provider permissions")); }
        let record = Record { before: query()?, sid: sid_text(sid)? };
        save(&record, true)?;
        let guard = Self;
        let code = unsafe { EventAccessControl(&PROVIDER, EventSecurityAddDACL as u32, sid, TRACELOG_REGISTER_GUIDS, true) };
        if code != 0 { return Err(io::Error::from_raw_os_error(code as i32)); }
        if !only_registration_delta(&record, &query()?)? { return Err(io::Error::other("Unexpected provider ACL delta")); }
        Ok(guard)
    }
}
impl Drop for Grant {
    fn drop(&mut self) {
        if let Err(error) = restore() { eprintln!("ETW diagnostic cleanup retained recovery record: {error}"); }
    }
}
/// Restores only the unchanged descriptor this isolated diagnostic installed.
pub fn restore() -> io::Result<()> {
    let path = journal();
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let record: Record = serde_json::from_slice(&bytes)?;
    let current = query()?;
    if current == record.before { std::fs::remove_file(path)?; return Ok(()); }
    if !only_registration_delta(&record, &current)? {
        return Err(io::Error::other("Provider permissions changed; preserved diagnostic state for exact recovery"));
    }
    let code = unsafe { EventAccessRemove(&PROVIDER) };
    if code != 0 { return Err(io::Error::from_raw_os_error(code as i32)); }
    if query()? != record.before { return Err(io::Error::other("Default provider descriptor did not restore exactly")); }
    std::fs::remove_file(path)
}

fn sid_text(sid: PSID) -> io::Result<String> {
    let mut text = ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 { return Err(io::Error::last_os_error()); }
    let mut size = 0;
    unsafe {
        while *text.add(size) != 0 { size += 1; }
        let value = String::from_utf16_lossy(std::slice::from_raw_parts(text, size));
        LocalFree(text.cast());
        Ok(value)
    }
}
fn without_dacl(bytes: &[u8]) -> io::Result<String> {
    let mut text = ptr::null_mut();
    let mut size = 0;
    if unsafe { ConvertSecurityDescriptorToStringSecurityDescriptorW(bytes.as_ptr().cast_mut().cast(),
        SDDL_REVISION_1, OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | SACL_SECURITY_INFORMATION,
        &mut text, &mut size) } == 0 { return Err(io::Error::last_os_error()); }
    let value = unsafe { String::from_utf16_lossy(std::slice::from_raw_parts(text, size.saturating_sub(1) as usize)) };
    unsafe { LocalFree(text.cast()); }
    Ok(value)
}
fn aces(bytes: &[u8], remove_sid: Option<&str>) -> io::Result<Vec<Vec<u8>>> {
    let mut acl = ptr::null_mut();
    let mut present = 0;
    let mut defaulted = 0;
    if unsafe { GetSecurityDescriptorDacl(bytes.as_ptr().cast_mut().cast(), &mut present, &mut acl, &mut defaulted) } == 0
        || present == 0 || acl.is_null() { return Err(io::Error::other("Missing provider DACL")); }
    let mut result = Vec::new();
    let mut removed = 0;
    for index in 0..unsafe { (*acl).AceCount } {
        let mut ace = ptr::null_mut();
        if unsafe { GetAce(acl, index as u32, &mut ace) } == 0 { return Err(io::Error::last_os_error()); }
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        if header.AceType == 0 && header.AceFlags == 0 {
            let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
            let sid = sid_text((&allowed.SidStart as *const u32).cast_mut().cast())?;
            if remove_sid == Some(sid.as_str()) && allowed.Mask == TRACELOG_REGISTER_GUIDS {
                removed += 1;
                continue;
            }
        }
        result.push(unsafe { std::slice::from_raw_parts(ace.cast::<u8>(), header.AceSize as usize).to_vec() });
    }
    if remove_sid.is_some() && removed != 1 { return Err(io::Error::other("Expected exactly one diagnostic registration ACE")); }
    Ok(result)
}
/// Accepts only the recorded exact SID/right addition, even if interrupted before post-mutation save.
fn only_registration_delta(record: &Record, current: &[u8]) -> io::Result<bool> {
    let control = |bytes: &[u8]| -> io::Result<u16> {
        let mut control = 0;
        let mut revision = 0;
        if unsafe { GetSecurityDescriptorControl(bytes.as_ptr().cast_mut().cast(), &mut control, &mut revision) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(control)
    };
    Ok(control(&record.before)? == control(current)?
        && without_dacl(&record.before)? == without_dacl(current)?
        && aces(&record.before, None)? == aces(current, Some(&record.sid))?)
}

#[test]
fn portable_etw_delta_rejects_unrelated_permissions() {
    fn descriptor(value: &str) -> Vec<u8> {
        let value: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
        let mut descriptor = ptr::null_mut();
        let mut size = 0;
        assert_ne!(unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(value.as_ptr(),
            SDDL_REVISION_1, &mut descriptor, &mut size) }, 0);
        let bytes = unsafe { std::slice::from_raw_parts(descriptor.cast::<u8>(), size as usize).to_vec() };
        unsafe { LocalFree(descriptor); }
        bytes
    }
    let sid = "S-1-5-21-1-2-3-1001";
    let baseline = "O:SYG:SYD:(A;;GA;;;SY)";
    let record = Record { before: descriptor(baseline), sid: sid.into() };
    let added = format!("{baseline}(A;;0x800;;;{sid})");
    assert!(only_registration_delta(&record, &descriptor(&added)).unwrap());
    for altered in [format!("{baseline}(A;;GA;;;{sid})"),
        format!("{added}(A;;GR;;;WD)"), added.replace("O:SY", "O:BA")] {
        assert!(!only_registration_delta(&record, &descriptor(&altered)).unwrap_or(false));
    }
}
