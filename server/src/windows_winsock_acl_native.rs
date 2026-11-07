//! Applies the Winsock catalog read entries to the registry and keeps their before-record.
// Reuses the list, access-list and record decisions made without Windows APIs.
use super::{Presence, Record, Snapshot, Target};
// Owns the private state folder and its record.
use std::{ffi::c_void, fs, io::{self, Read, Write}, path::{Path, PathBuf}, ptr::{null, null_mut}};
// Opens exact registry keys and reads or replaces one key's access list.
use windows_sys::Win32::{Foundation::*, Security::{Authorization::*, *}, System::Registry::*};

const READ_KEY: u32 = 0x0002_0000 | KEY_QUERY_VALUE; // READ_CONTROL
const WRITE_KEY: u32 = READ_KEY | 0x0004_0000; // WRITE_DAC
const WHOLE: u32 = OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
const RECORD_LIMIT: u64 = 8 * 1024 * 1024;
const EVENTS_LIMIT: u64 = 1024 * 1024;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryKey(key: HANDLE, class: u32, information: *mut c_void, length: u32, result: *mut u32) -> i32;
}

struct Key(HKEY);
impl Drop for Key {
    /// Registry handles are closed with their own call.
    fn drop(&mut self) { unsafe { RegCloseKey(self.0); } }
}

/// Encodes a checked name for a registry call.
fn wide(value: &str) -> io::Result<Vec<u16>> {
    if value.contains('\0') { return Err(io::ErrorKind::InvalidInput.into()); }
    Ok(value.encode_utf16().chain(Some(0)).collect())
}

/// Turns a registry status into an error that names the key.
fn failed(step: &str, path: &str, code: u32) -> io::Error {
    let error = io::Error::from_raw_os_error(code as i32);
    io::Error::new(error.kind(), format!("{step} HKLM\\{path}: {error}"))
}

/// Derives the one capability SID Windows assigns to a capability name.
pub(crate) fn derive(name: &str) -> io::Result<Vec<u8>> {
    let name = wide(name)?;
    let (mut groups, mut capabilities) = (null_mut(), null_mut());
    let (mut group_count, mut count) = (0, 0);
    if unsafe { DeriveCapabilitySidsFromName(name.as_ptr(), &mut groups, &mut group_count, &mut capabilities, &mut count) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut found = Vec::new();
    unsafe {
        for index in 0..group_count as usize { LocalFree(*groups.add(index)); }
        LocalFree(groups.cast());
        for index in 0..count as usize {
            let sid = *capabilities.add(index);
            if index == 0 && IsValidSid(sid) != 0 {
                found = std::slice::from_raw_parts(sid.cast::<u8>(), GetLengthSid(sid) as usize).to_vec();
            }
            LocalFree(sid);
        }
        LocalFree(capabilities.cast());
    }
    if count != 1 || found.is_empty() { return Err(io::Error::other("Windows returned no single capability SID")); }
    Ok(found)
}

/// What one managed key currently holds for this installation.
#[derive(Debug)]
pub struct KeyState {
    pub path: String,
    pub rights: u32,
    pub presence: Presence,
    /// The key's whole descriptor: owner, group and access list.
    pub descriptor: Vec<u8>,
}

/// The outcome of one verify-and-repair pass.
#[derive(Debug, Default)]
pub struct Report {
    /// Keys checked and found or made correct.
    pub keys: usize,
    /// Keys that received the entry for the first time.
    pub added: Vec<String>,
    /// Keys that had the entry before and lost it, for example after a catalog reset.
    pub repaired: Vec<String>,
}

/// The outcome of removing this installation's entries.
#[derive(Debug, Default)]
pub struct Removal {
    /// Keys an entry was removed from.
    pub stripped: Vec<String>,
    /// Recorded keys whose descriptor now equals the recorded one byte for byte.
    pub exact: Vec<String>,
    /// Recorded keys that were changed by something else since; only our entry was removed.
    pub differing: Vec<String>,
}

/// Manages one installation's read entries on the Winsock catalog keys.
pub struct Manager {
    base: String,
    capability: String,
    sid: Vec<u8>,
    sid_text: String,
    state: PathBuf,
}

impl Manager {
    /// The production manager: the machine hive, the capability named by the installed service.
    pub fn machine(service_name: &str, state: &Path) -> io::Result<Self> {
        Self::build(String::new(), super::capability_name(service_name)?, state)
    }

    /// A manager over a synthetic key tree, for tests. The base must be the fixed test pattern.
    pub fn synthetic(base: &str, service_name: &str, state: &Path) -> io::Result<Self> {
        if !super::test_base(base) { return Err(io::ErrorKind::InvalidInput.into()); }
        Self::build(base.to_owned(), super::capability_name(service_name)?, state)
    }

    fn build(base: String, capability: String, state: &Path) -> io::Result<Self> {
        if !state.is_absolute() { return Err(io::ErrorKind::InvalidInput.into()); }
        let sid = derive(&capability)?;
        if !super::capability_sid(&sid) { return Err(io::Error::other("Windows returned an unexpected capability SID")); }
        let sid_text = super::sid_text(&sid)?;
        Ok(Self { base, capability, sid, sid_text, state: state.to_owned() })
    }

    /// The capability SID as text.
    pub fn sid(&self) -> &str { &self.sid_text }

    /// The capability SID in binary form.
    pub fn sid_bytes(&self) -> &[u8] { &self.sid }

    /// The capability name networked runs carry.
    pub fn capability(&self) -> &str { &self.capability }

    /// Joins the base and a managed path. Refuses any path outside the managed list.
    fn full(&self, path: &str) -> io::Result<String> {
        if super::managed(path).is_none() { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "Not a managed Winsock key")); }
        Ok(match (self.base.is_empty(), path.is_empty()) {
            (true, _) => path.to_owned(),
            (false, true) => self.base.clone(),
            (false, false) => format!("{}\\{path}", self.base),
        })
    }

    /// Opens one managed key and proves the kernel opened the key that was named.
    fn open(&self, path: &str, access: u32) -> io::Result<Key> {
        let full = self.full(path)?;
        let mut key = null_mut();
        let name = wide(&full)?;
        let code = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, name.as_ptr(), 0, access | KEY_WOW64_64KEY, &mut key) };
        if code != 0 { return Err(failed("Cannot open", &full, code)); }
        let key = Key(key);
        // A predefined pseudo-handle would carry none of the requested access.
        if [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER, HKEY_USERS, HKEY_CLASSES_ROOT].contains(&key.0) { return Err(io::Error::other("Windows returned a predefined registry handle")); }
        let mut buffer = vec![0u32; 1024];
        let mut length = 0;
        // KeyNameInformation: a length in bytes, then the name.
        let status = unsafe { NtQueryKey(key.0, 3, buffer.as_mut_ptr().cast(), (buffer.len() * 4) as u32, &mut length) };
        if status < 0 { return Err(io::Error::other(format!("Cannot name HKLM\\{full}: {status:#x}"))); }
        let bytes = buffer[0] as usize;
        if bytes > (buffer.len() - 1) * 4 { return Err(io::ErrorKind::InvalidData.into()); }
        let actual = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(buffer.as_ptr().add(1).cast::<u16>(), bytes / 2) });
        if !super::opened_name_matches(&self.base, path, &actual) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("HKLM\\{full} resolves to another key")));
        }
        Ok(key)
    }

    /// Lists the direct children of the two entry parents and builds the complete key list.
    fn scan(&self) -> io::Result<Vec<Target>> {
        let mut entries = [Vec::new(), Vec::new()];
        for (parent, names) in super::entry_parents().iter().zip(entries.iter_mut()) {
            let key = self.open(parent, READ_KEY | KEY_ENUMERATE_SUB_KEYS)?;
            for index in 0..=super::MOST_ENTRIES as u32 {
                let mut name = [0u16; 256];
                let mut length = name.len() as u32;
                let code = unsafe { RegEnumKeyExW(key.0, index, name.as_mut_ptr(), &mut length, null(), null_mut(), null_mut(), null_mut()) };
                if code == ERROR_NO_MORE_ITEMS { break; }
                if code != 0 { return Err(failed("Cannot list", parent, code)); }
                names.push(String::from_utf16(&name[..length as usize]).map_err(|_| io::ErrorKind::InvalidData)?);
            }
        }
        super::targets(&entries)
    }

    /// Reads the access list of the scanned keys without changing anything.
    pub fn inspect(&self) -> io::Result<Vec<KeyState>> {
        let mut states = Vec::new();
        for target in self.scan()? {
            let key = match self.open(&target.path, READ_KEY) {
                Ok(key) => key,
                Err(error) if error.kind() == io::ErrorKind::NotFound && !target.required => continue,
                Err(error) => return Err(error),
            };
            let descriptor = security(&key, &target.path)?;
            let presence = super::presence(super::descriptor(&descriptor)?.dacl, &self.sid, target.rights)?;
            states.push(KeyState { path: target.path, rights: target.rights, presence, descriptor });
        }
        Ok(states)
    }

    /// Verifies every managed key and adds this installation's entry where it is missing.
    ///
    /// The descriptor of each key about to change is saved to the record, and the record is
    /// flushed, before the first change. Nothing is propagated to subkeys, no owner changes, and
    /// only the keys returned by the scan are opened for writing. Any failure leaves the keys
    /// already corrected in place and returns the error; the caller then refuses network leases.
    pub fn ensure(&self) -> io::Result<Report> {
        let _lock = crate::windows_security_lock::acquire(true)?;
        crate::platform_fs::private_dir(&self.state, true)?;
        let mut record = self.load()?;
        let mut report = Report::default();
        let mut plan = Vec::new();
        let mut unsaved = false;
        for target in self.scan()? {
            let key = match self.open(&target.path, READ_KEY) {
                Ok(key) => key,
                Err(error) if error.kind() == io::ErrorKind::NotFound && !target.required => continue,
                Err(error) => return Err(error),
            };
            let before = security(&key, &target.path)?;
            report.keys += 1;
            let presence = super::presence(super::descriptor(&before)?.dacl, &self.sid, target.rights)?;
            if presence == Presence::Exact { continue; }
            let key = self.open(&target.path, WRITE_KEY)?;
            // A key the record already names had the entry before and lost or changed it.
            let knew = record.knows(&target.path);
            if presence == Presence::Absent {
                unsaved |= record.note(&target.path, snapshot(&before)?);
            }
            plan.push((target, key, before, knew));
        }
        if unsaved { self.save(&record)?; }
        for (target, key, seen, repaired) in plan {
            let current = security(&key, &target.path)?;
            let parsed = super::descriptor(&current)?;
            // Something changed the key between the scan and now: record what is there first.
            if current != seen && super::presence(parsed.dacl, &self.sid, target.rights)? == Presence::Absent
                && record.note(&target.path, snapshot(&current)?) {
                self.save(&record)?;
            }
            let wanted = super::with_ace(parsed.dacl, &self.sid, target.rights)?;
            replace(&key, &target.path, parsed.control, &wanted)?;
            let after = security(&key, &target.path)?;
            if super::descriptor(&after)?.dacl != wanted.as_slice() {
                return Err(io::Error::other(format!("HKLM\\{} did not keep the Winsock read entry", target.path)));
            }
            self.event(if repaired { "repaired" } else { "added" }, &target.path);
            if repaired { report.repaired.push(target.path); } else { report.added.push(target.path); }
        }
        Ok(report)
    }

    /// Removes this installation's entries from every managed key that exists, and verifies it.
    ///
    /// Only entries for this capability SID are deleted; the recorded descriptors are never
    /// written back, so another installation's entry or a later change by Windows stays as it is.
    pub fn remove(&self) -> io::Result<Removal> {
        let _lock = crate::windows_security_lock::acquire(true)?;
        let record = if self.state.exists() { self.load()? } else { Record::new(&self.capability, &self.sid_text) };
        // Keys found now, plus keys the record names that the scan no longer lists.
        let mut paths: Vec<String> = match self.scan() {
            Ok(targets) => targets.into_iter().map(|target| target.path).collect(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => super::targets(&[Vec::new(), Vec::new()])?.into_iter().map(|target| target.path).collect(),
            Err(error) => return Err(error),
        };
        for entry in &record.keys {
            if !paths.iter().any(|path| path.eq_ignore_ascii_case(&entry.path)) { paths.push(entry.path.clone()); }
        }
        let mut removal = Removal::default();
        for path in paths {
            let rights = super::managed(&path).ok_or(io::ErrorKind::PermissionDenied)?;
            let key = match self.open(&path, READ_KEY) {
                Ok(key) => key,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let mut current = security(&key, &path)?;
            if super::presence(super::descriptor(&current)?.dacl, &self.sid, rights)? != Presence::Absent {
                let key = self.open(&path, WRITE_KEY)?;
                current = security(&key, &path)?;
                let parsed = super::descriptor(&current)?;
                let stripped = super::without_sid(parsed.dacl, &self.sid)?;
                replace(&key, &path, parsed.control, &stripped)?;
                current = security(&key, &path)?;
                if super::presence(super::descriptor(&current)?.dacl, &self.sid, rights)? != Presence::Absent {
                    return Err(io::Error::other(format!("HKLM\\{path} still holds the Winsock read entry")));
                }
                self.event("removed", &path);
                removal.stripped.push(path.clone());
            }
            if let Some(before) = record.latest(&path) {
                if before.descriptor == super::hex(&current) { removal.exact.push(path); } else { removal.differing.push(path); }
            }
        }
        if self.state.exists() {
            let summary = serde_json::json!({"capability": self.capability, "sid": self.sid_text, "stripped": removal.stripped,
                "exact": removal.exact, "differing": removal.differing, "record": record});
            self.write("removed.json", &serde_json::to_vec(&summary)?)?;
            match fs::remove_file(self.state.join("record.json")) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
        Ok(removal)
    }

    /// Reads the record, or starts one when this installation has changed nothing yet.
    fn load(&self) -> io::Result<Record> {
        let path = self.state.join("record.json");
        let file = match crate::platform_fs::open_read_nofollow(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Record::new(&self.capability, &self.sid_text)),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take(RECORD_LIMIT + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > RECORD_LIMIT { return Err(io::ErrorKind::InvalidData.into()); }
        Record::parse(&bytes, &self.capability, &self.sid_text)
    }

    /// Flushes the record to a new private file and puts it in place of the old one.
    fn save(&self, record: &Record) -> io::Result<()> {
        self.write("record.json", &serde_json::to_vec(record)?)
    }

    /// Writes one private state file completely before it replaces the previous version.
    fn write(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        let temporary = self.state.join(format!("{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
        let mut file = crate::platform_fs::private_file(&temporary, true)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, self.state.join(name)).inspect_err(|_| { let _ = fs::remove_file(&temporary); })
    }

    /// Appends one line to the private event log. The registry change does not depend on it.
    fn event(&self, what: &str, path: &str) {
        tracing::warn!(key = path, sid = %self.sid_text, "Winsock catalog read entry {what}");
        let log = self.state.join("events.log");
        if fs::metadata(&log).is_ok_and(|meta| meta.len() > EVENTS_LIMIT) {
            let _ = fs::rename(&log, self.state.join("events.old.log"));
        }
        let line = serde_json::json!({"time": now(), "event": what, "key": format!("HKLM\\{}", self.full(path).unwrap_or_default()), "sid": self.sid_text});
        if let Ok(mut file) = fs::OpenOptions::new().append(true).create(true).open(&log) {
            let _ = writeln!(file, "{line}");
        }
    }
}

/// Seconds since 1970 for record and log entries.
fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs())
}

/// Reads owner, group and access list of exactly this key as one self-relative descriptor.
fn security(key: &Key, path: &str) -> io::Result<Vec<u8>> {
    let mut size = 0u32;
    let code = unsafe { RegGetKeySecurity(key.0, WHOLE, null_mut(), &mut size) };
    if code != ERROR_INSUFFICIENT_BUFFER || size == 0 || size > 0x2_0000 { return Err(failed("Cannot read the access list of", path, code)); }
    let mut buffer = vec![0u32; (size as usize).div_ceil(4)];
    let code = unsafe { RegGetKeySecurity(key.0, WHOLE, buffer.as_mut_ptr().cast(), &mut size) };
    if code != 0 { return Err(failed("Cannot read the access list of", path, code)); }
    Ok(unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), size as usize) }.to_vec())
}

/// Replaces the access list of exactly this key. `RegSetKeySecurity` sets the one object named by
/// the handle; it does not walk subkeys.
fn replace(key: &Key, path: &str, control: u16, acl: &[u8]) -> io::Result<()> {
    let bytes = super::dacl_descriptor(control, acl);
    let mut aligned = vec![0u32; bytes.len().div_ceil(4)];
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), aligned.as_mut_ptr().cast::<u8>(), bytes.len()); }
    let code = unsafe { RegSetKeySecurity(key.0, DACL_SECURITY_INFORMATION, aligned.as_mut_ptr().cast()) };
    if code != 0 { return Err(failed("Cannot change the access list of", path, code)); }
    Ok(())
}

/// Describes a descriptor for the record: exact bytes plus readable text.
fn snapshot(descriptor: &[u8]) -> io::Result<Snapshot> {
    let mut aligned = vec![0u32; descriptor.len().div_ceil(4)];
    unsafe { std::ptr::copy_nonoverlapping(descriptor.as_ptr(), aligned.as_mut_ptr().cast::<u8>(), descriptor.len()); }
    let mut text = null_mut();
    if unsafe { ConvertSecurityDescriptorToStringSecurityDescriptorW(aligned.as_mut_ptr().cast(), SDDL_REVISION_1, WHOLE, &mut text, null_mut()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut length = 0;
    let sddl = unsafe {
        while *text.add(length) != 0 { length += 1; }
        let value = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
        LocalFree(text.cast());
        value
    };
    Ok(Snapshot { descriptor: super::hex(descriptor), sddl, recorded: now() })
}
