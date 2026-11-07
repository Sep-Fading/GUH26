#![cfg(windows)]
// Isolated VM-only experiment: which exact HKLM read ACEs let a zero-registryRead LPAC start Winsock.
// No production code grants these ACEs; every change is recorded durably first and restored exactly.
use miniclaw_server::{platform_fs, seatbelt::Grant, windows_network::Lease, windows_sandbox::Sandbox};
use std::{ffi::c_void, fs, io::Write, path::PathBuf, ptr, time::Duration};
use windows_sys::Win32::{Foundation::*, Security::{Authorization::*, *}, System::Registry::*};

/// Stable test capability name; an installer would use one install-specific name instead.
const CUSTOM_CAPABILITY: &str = "miniclawWinsockRead.diagnostic-5f0c2d7e";
const OWNER_GROUP_DACL: u32 = OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
const READ_CONTROL_DAC: u32 = 0x0002_0000 | 0x0004_0000;
const MAXIMUM_ALLOWED_ACCESS: u32 = 0x0200_0000;

/// Phases used when no plan file is supplied: the minimal grant found on VM2 (Windows 11 ARM64 26100).
/// Root needs only KEY_QUERY_VALUE (advapi32 opens the predefined HKLM handle before any relative open);
/// the other keys take KEY_READ, non-inheritable. `tree` covers every per-machine catalog entry.
const DEFAULT_PLAN: &str = r"
phase baseline principal=none caps=internetClient+custom
phase minimal principal=cap caps=internetClient+custom assert=final
ace 0x1 - .
ace 0x20019 - SYSTEM\CurrentControlSet\Services\WinSock2\Parameters
ace 0x20019 - SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\Protocol_Catalog9
tree 0x20019 - SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\Protocol_Catalog9\Catalog_Entries64
ace 0x20019 - SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\NameSpace_Catalog5
tree 0x20019 - SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\NameSpace_Catalog5\Catalog_Entries64
ace 0x20019 - SYSTEM\CurrentControlSet\Services\WinSock\Parameters
ace 0x20019 - SYSTEM\CurrentControlSet\Services\WinSock\Setup Migration\Providers
ace 0x20019 - SYSTEM\CurrentControlSet\Services\WinSock\Setup Migration\Providers\Tcpip
ace 0x20019 - SYSTEM\CurrentControlSet\Services\WinSock\Setup Migration\Providers\Tcpip6
";

/// Keys whose opens the child reports step by step, from the hive root down each component.
const DEFAULT_PROBES: &[&str] = &["", "SYSTEM", r"SYSTEM\CurrentControlSet", r"SYSTEM\ControlSet001",
    r"SYSTEM\CurrentControlSet\Services", r"SYSTEM\CurrentControlSet\Services\WinSock2",
    r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters",
    r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\Protocol_Catalog9",
    r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\Protocol_Catalog9\Catalog_Entries64",
    r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\Protocol_Catalog9\Catalog_Entries64\000000000001",
    r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\NameSpace_Catalog5",
    r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\NameSpace_Catalog5\Catalog_Entries64",
    r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\AppId_Catalog",
    r"SYSTEM\CurrentControlSet\Services\WinSock",
    r"SYSTEM\CurrentControlSet\Services\WinSock\Parameters",
    r"SYSTEM\CurrentControlSet\Services\WinSock\Setup Migration",
    r"SYSTEM\CurrentControlSet\Services\Tcpip", r"SYSTEM\CurrentControlSet\Services\Tcpip\Parameters",
    r"SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\Winsock",
    r"SYSTEM\CurrentControlSet\Services\Tcpip6\Parameters",
    r"SYSTEM\CurrentControlSet\Services\Tcpip6\Parameters\Winsock",
    r"SYSTEM\CurrentControlSet\Services\AFD", r"SYSTEM\CurrentControlSet\Services\AFD\Parameters",
    r"SYSTEM\CurrentControlSet\Services\Dnscache\Parameters"];

/// Encodes only local diagnostic names for Win32 APIs.
fn wide(value: &str) -> Vec<u16> { value.encode_utf16().chain(Some(0)).collect() }

/// Copies a LocalAlloc wide string and frees it.
unsafe fn take_text(text: *mut u16) -> String {
    unsafe {
        let mut length = 0;
        while *text.add(length) != 0 { length += 1; }
        let value = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
        LocalFree(text.cast());
        value
    }
}

fn sid_text(sid: PSID) -> String {
    unsafe {
        let mut text = ptr::null_mut();
        assert_ne!(ConvertSidToStringSidW(sid, &mut text), 0);
        take_text(text)
    }
}

/// Reads one token information class into aligned storage.
fn token_information(class: i32) -> Vec<u64> {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = ptr::null_mut();
        assert_ne!(OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token), 0);
        let mut size = 0;
        GetTokenInformation(token, class, ptr::null_mut(), 0, &mut size);
        let mut buffer = vec![0u64; (size as usize).div_ceil(8).max(1)];
        let ok = GetTokenInformation(token, class, buffer.as_mut_ptr().cast(), (buffer.len() * 8) as u32, &mut size);
        CloseHandle(token);
        assert_ne!(ok, 0, "Token information class {class} unavailable");
        buffer
    }
}

/// Uses the actual token user, never the elevated token's possible Administrators default owner.
fn user_sid() -> String {
    let buffer = token_information(TokenUser);
    sid_text(unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid })
}

// ---------------------------------------------------------------------------------------------
// Durable record-and-restore of exact HKLM key DACLs (parent, elevated, outside the sandbox).

struct Key(HKEY);
impl Drop for Key { fn drop(&mut self) { unsafe { RegCloseKey(self.0); } } }

/// Opens one existing HKLM key; "" is the hive root. HKCU is never opened for modification.
fn machine_key(path: &str, access: u32) -> Result<Key, u32> {
    let mut key = ptr::null_mut();
    let status = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, wide(path).as_ptr(), 0, access, &mut key) };
    if status == 0 { Ok(Key(key)) } else { Err(status) }
}

/// Returns the self-relative descriptor for exactly this key.
fn descriptor(key: &Key, information: u32) -> Vec<u8> {
    unsafe {
        let mut size = 0u32;
        let status = RegGetKeySecurity(key.0, information, ptr::null_mut(), &mut size);
        assert_eq!(status, ERROR_INSUFFICIENT_BUFFER);
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        assert_eq!(RegGetKeySecurity(key.0, information, buffer.as_mut_ptr().cast(), &mut size), 0);
        std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), size as usize).to_vec()
    }
}

fn sddl(descriptor: &[u8], information: u32) -> String {
    unsafe {
        let aligned: Vec<u64> = descriptor.chunks(8).map(|c| { let mut b = [0u8; 8]; b[..c.len()].copy_from_slice(c); u64::from_ne_bytes(b) }).collect();
        let mut text = ptr::null_mut();
        assert_ne!(ConvertSecurityDescriptorToStringSecurityDescriptorW(aligned.as_ptr() as _, SDDL_REVISION_1, information, &mut text, ptr::null_mut()), 0);
        take_text(text)
    }
}

/// Parses a DACL-only SDDL string and returns its canonical text.
fn canonical_dacl(text: &str) -> String {
    unsafe {
        let mut descriptor = ptr::null_mut();
        assert_ne!(ConvertStringSecurityDescriptorToSecurityDescriptorW(wide(text).as_ptr(), SDDL_REVISION_1, &mut descriptor, ptr::null_mut()), 0, "Invalid SDDL {text}");
        let mut out = ptr::null_mut();
        assert_ne!(ConvertSecurityDescriptorToStringSecurityDescriptorW(descriptor, SDDL_REVISION_1, DACL_SECURITY_INFORMATION, &mut out, ptr::null_mut()), 0);
        LocalFree(descriptor);
        take_text(out)
    }
}

/// Requests that the kernel keep the auto-inherited marker the original descriptor carried.
fn keep_inheritance_marker(descriptor: &mut [u64]) {
    let bytes = unsafe { std::slice::from_raw_parts_mut(descriptor.as_mut_ptr().cast::<u8>(), descriptor.len() * 8) };
    let control = u16::from_le_bytes([bytes[2], bytes[3]]);
    // SE_DACL_AUTO_INHERITED implies SE_DACL_AUTO_INHERIT_REQ for the exact-object update.
    if control & 0x0400 != 0 { bytes[2..4].copy_from_slice(&(control | 0x0100).to_le_bytes()); }
}

/// Replaces only this key's DACL (NtSetSecurityObject); nothing propagates to descendants.
fn set_dacl_sddl(key: &Key, dacl: &str) -> u32 {
    unsafe {
        let mut descriptor = ptr::null_mut();
        let mut size = 0u32;
        assert_ne!(ConvertStringSecurityDescriptorToSecurityDescriptorW(wide(dacl).as_ptr(), SDDL_REVISION_1, &mut descriptor, &mut size), 0);
        let mut copy = vec![0u64; (size as usize).div_ceil(8)];
        ptr::copy_nonoverlapping(descriptor.cast::<u8>(), copy.as_mut_ptr().cast::<u8>(), size as usize);
        LocalFree(descriptor);
        keep_inheritance_marker(&mut copy);
        RegSetKeySecurity(key.0, DACL_SECURITY_INFORMATION, copy.as_mut_ptr().cast())
    }
}

/// Restores from the original binary descriptor so ACL bytes, not only SDDL, match.
fn set_dacl_binary(key: &Key, original: &[u8]) -> u32 {
    let mut copy = vec![0u64; original.len().div_ceil(8)];
    unsafe { ptr::copy_nonoverlapping(original.as_ptr(), copy.as_mut_ptr().cast::<u8>(), original.len()); }
    keep_inheritance_marker(&mut copy);
    unsafe { RegSetKeySecurity(key.0, DACL_SECURITY_INFORMATION, copy.as_mut_ptr().cast()) }
}

/// Inserts one explicit allow ACE after existing explicit ACEs and before inherited ones.
fn with_ace(dacl: &str, ace: &str) -> String {
    assert!(dacl.starts_with("D:") && !dacl.contains("NO_ACCESS_CONTROL") && !dacl.contains("(X"), "Unsupported DACL {dacl}");
    let mut position = dacl.len();
    let mut cursor = 0;
    while let Some(open) = dacl[cursor..].find('(') {
        let start = cursor + open;
        let end = start + dacl[start..].find(')').unwrap();
        if dacl[start + 1..end].split(';').nth(1).unwrap().contains("ID") { position = start; break; }
        cursor = end + 1;
    }
    format!("{}{}{}", &dacl[..position], ace, &dacl[position..])
}

fn hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() }
fn unhex(text: &str) -> Vec<u8> { (0..text.len() / 2).map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap()).collect() }

/// Fixed per-user location so a later process can always find an unfinished record.
fn record_path() -> PathBuf {
    let directory = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap()).join("MiniClawWinsockAclDiagnostic");
    if !directory.exists() { platform_fs::private_dir(&directory, false).unwrap(); }
    directory.join("record.json")
}

/// One requested grant: rights (subset of KEY_READ), ACE flags ("" or "CI"), HKLM path, whole subtree or not.
#[derive(Clone)]
struct Ace { rights: u32, flags: String, path: String, tree: bool }

/// Lists a key and all its descendants, bounded like the earlier catalog experiment.
fn expand(ace: &Ace) -> Vec<String> {
    if !ace.tree { return vec![ace.path.clone()]; }
    let mut result = vec![];
    let mut pending = std::collections::VecDeque::from([ace.path.clone()]);
    while let Some(path) = pending.pop_front() {
        assert!(result.len() < 512, "Diagnostic subtree bound exceeded");
        let key = machine_key(&path, KEY_ENUMERATE_SUB_KEYS).unwrap_or_else(|s| panic!("Cannot enumerate HKLM\\{path}: {s}"));
        for index in 0.. {
            let mut name = [0u16; 256];
            let mut length = name.len() as u32;
            if unsafe { RegEnumKeyExW(key.0, index, name.as_mut_ptr(), &mut length, ptr::null(), ptr::null_mut(), ptr::null_mut(), ptr::null_mut()) } != 0 { break; }
            pending.push_back(format!("{path}\\{}", String::from_utf16_lossy(&name[..length as usize])));
        }
        result.push(path);
    }
    result
}

struct AclGuard { active: bool }
impl AclGuard {
    /// Saves every before/after descriptor durably, then mutates each exact key and verifies the delta.
    fn apply(sid: &str, aces: &[Ace]) -> Self {
        let record = record_path();
        assert!(!record.exists(), "Restore the prior Winsock ACL diagnostic before adding permissions");
        assert!(sid.starts_with("S-1-15-3-1024-") || sid.starts_with("S-1-15-2-"), "Expected a capability or package SID");
        let mut entries = vec![];
        let mut seen = std::collections::HashSet::new();
        for ace in aces {
            assert!(ace.rights != 0 && ace.rights & !0x0002_0019 == 0, "Only read rights may be granted");
            assert!(ace.flags.is_empty() || ace.flags == "CI");
            assert!(!ace.path.to_ascii_uppercase().contains("CONTROLSET0"), "Name keys through CurrentControlSet only");
            for path in expand(ace) {
                assert!(seen.insert(path.to_ascii_lowercase()), "Key listed twice: {path}");
                let key = machine_key(&path, READ_CONTROL_DAC).unwrap_or_else(|s| panic!("Cannot open HKLM\\{path} for WRITE_DAC: {s}"));
                let before = descriptor(&key, OWNER_GROUP_DACL);
                let before_dacl = sddl(&before, DACL_SECURITY_INFORMATION);
                assert!(!before_dacl.contains(sid), "Diagnostic SID already present on HKLM\\{path}");
                let after_dacl = canonical_dacl(&with_ace(&before_dacl, &format!("(A;{};0x{:x};;;{sid})", ace.flags, ace.rights)));
                entries.push(serde_json::json!({"path": path, "before": sddl(&before, OWNER_GROUP_DACL), "before_dacl": before_dacl,
                    "after_dacl": after_dacl, "before_hex": hex(&before)}));
            }
        }
        let body = serde_json::to_vec(&serde_json::json!({"version": 1, "sid": sid, "entries": entries})).unwrap();
        let temporary = record.with_extension("tmp");
        let mut file = fs::File::create(&temporary).unwrap();
        file.write_all(&body).unwrap();
        file.sync_all().unwrap();
        drop(file);
        fs::rename(&temporary, &record).unwrap();
        let guard = Self { active: true };
        for entry in &entries {
            let path = entry["path"].as_str().unwrap();
            let key = machine_key(path, READ_CONTROL_DAC).unwrap();
            assert_eq!(hex(&descriptor(&key, OWNER_GROUP_DACL)), entry["before_hex"].as_str().unwrap(), "Registry changed before diagnostic mutation");
            assert_eq!(set_dacl_sddl(&key, entry["after_dacl"].as_str().unwrap()), 0, "Could not set DACL on HKLM\\{path}");
            let now = descriptor(&key, OWNER_GROUP_DACL);
            assert_eq!(sddl(&now, DACL_SECURITY_INFORMATION), entry["after_dacl"].as_str().unwrap(), "Unexpected registry ACL delta on HKLM\\{path}");
            println!("ACL GRANT HKLM\\{path}: {} -> {}", entry["before_dacl"].as_str().unwrap(), entry["after_dacl"].as_str().unwrap());
        }
        println!("Exact Winsock read grants installed for {sid}: {} keys", entries.len());
        guard
    }

    fn restore(&mut self) {
        if self.active { restore_record(); self.active = false; }
    }
}
impl Drop for AclGuard { fn drop(&mut self) { self.restore(); } }

/// Restores every recorded key in reverse order; refuses to overwrite a third-party change.
fn restore_record() {
    let record = record_path();
    if !record.exists() { println!("No Winsock ACL diagnostic record present"); return; }
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    assert_eq!(value["version"], 1);
    let entries = value["entries"].as_array().unwrap();
    assert!(entries.len() <= 1024);
    for entry in entries.iter().rev() {
        let path = entry["path"].as_str().unwrap();
        let before_hex = entry["before_hex"].as_str().unwrap();
        let key = machine_key(path, READ_CONTROL_DAC).unwrap_or_else(|s| panic!("Cannot reopen HKLM\\{path}: {s}; record preserved"));
        let current = descriptor(&key, OWNER_GROUP_DACL);
        if hex(&current) != before_hex {
            let current_dacl = sddl(&current, DACL_SECURITY_INFORMATION);
            assert!(current_dacl == entry["after_dacl"].as_str().unwrap() || current_dacl == entry["before_dacl"].as_str().unwrap(),
                "HKLM\\{path} changed outside the diagnostic; recovery record preserved");
            assert_eq!(set_dacl_binary(&key, &unhex(before_hex)), 0, "Could not restore HKLM\\{path}; record preserved");
        }
        let restored = descriptor(&key, OWNER_GROUP_DACL);
        assert_eq!(sddl(&restored, OWNER_GROUP_DACL), entry["before"].as_str().unwrap(), "HKLM\\{path} SDDL did not restore exactly; record preserved");
        assert_eq!(hex(&restored), before_hex, "HKLM\\{path} descriptor bytes did not restore exactly; record preserved");
        println!("ACL RESTORED HKLM\\{path}: {}", entry["before"].as_str().unwrap());
    }
    fs::remove_file(&record).unwrap();
    println!("Exact Winsock diagnostic ACL restoration verified: {} keys (SDDL and descriptor bytes equal)", entries.len());
}

// ---------------------------------------------------------------------------------------------
// Synthetic fixtures.

struct Sentinel { name: Vec<u16>, key: HKEY }
impl Sentinel {
    /// Creates a volatile HKLM key: inherited ACL, or the given protected SDDL.
    fn new(name: &str, protected: Option<&str>) -> Self {
        unsafe {
            let mut descriptor = ptr::null_mut();
            if let Some(text) = protected {
                assert_ne!(ConvertStringSecurityDescriptorToSecurityDescriptorW(wide(text).as_ptr(), SDDL_REVISION_1, &mut descriptor, ptr::null_mut()), 0);
            }
            let attributes = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: descriptor, bInheritHandle: 0 };
            let name = wide(name);
            let mut key = ptr::null_mut();
            let mut disposition = 0;
            let status = RegCreateKeyExW(HKEY_LOCAL_MACHINE, name.as_ptr(), 0, ptr::null(), REG_OPTION_VOLATILE, KEY_ALL_ACCESS,
                if protected.is_some() { &attributes } else { ptr::null() }, &mut key, &mut disposition);
            if !descriptor.is_null() { LocalFree(descriptor); }
            assert_eq!(status, 0);
            assert_eq!(disposition, REG_CREATED_NEW_KEY);
            let value = wide("synthetic-private-sentinel");
            assert_eq!(RegSetValueExW(key, wide("sentinel").as_ptr(), 0, REG_SZ, value.as_ptr().cast(), (value.len() * 2) as u32), 0);
            Self { name, key }
        }
    }
}
impl Drop for Sentinel {
    fn drop(&mut self) { unsafe { RegCloseKey(self.key); assert_eq!(RegDeleteKeyW(HKEY_LOCAL_MACHINE, self.name.as_ptr()), 0); } }
}

struct Capabilities { allocations: Vec<PSID>, entries: Vec<SID_AND_ATTRIBUTES>, text: Vec<String> }
impl Capabilities {
    /// Derives fixed capability names, retaining all native allocations.
    fn new(names: &[&str]) -> Self {
        let mut result = Self { allocations: vec![], entries: vec![], text: vec![] };
        for name in names {
            unsafe {
                let (mut groups, mut capabilities) = (ptr::null_mut(), ptr::null_mut());
                let (mut group_count, mut count) = (0, 0);
                assert_ne!(DeriveCapabilitySidsFromName(wide(name).as_ptr(), &mut groups, &mut group_count, &mut capabilities, &mut count), 0);
                for i in 0..group_count { LocalFree(*groups.add(i as usize)); }
                LocalFree(groups.cast());
                for i in 0..count {
                    let sid = *capabilities.add(i as usize);
                    result.allocations.push(sid);
                    result.text.push(sid_text(sid));
                    // SE_GROUP_ENABLED from winnt.h.
                    result.entries.push(SID_AND_ATTRIBUTES { Sid: sid, Attributes: 0x4 });
                }
                LocalFree(capabilities.cast());
            }
        }
        result
    }
}
impl Drop for Capabilities {
    fn drop(&mut self) { for sid in &self.allocations { unsafe { LocalFree(*sid); } } }
}

// ---------------------------------------------------------------------------------------------
// Plan.

struct Phase { name: String, principal: String, caps: Vec<String>, assert_final: bool, aces: Vec<Ace> }

/// Parses the small line format documented at DEFAULT_PLAN; "." names the HKLM root key.
fn plan() -> Vec<Phase> {
    let text = match std::env::var("MC_WINSOCK_ACL_PLAN") { Ok(path) => fs::read_to_string(path).unwrap(), Err(_) => DEFAULT_PLAN.to_owned() };
    let mut phases: Vec<Phase> = vec![];
    for line in text.lines().map(|l| l.trim().trim_start_matches('\u{feff}')).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let mut parts = line.splitn(2, ' ');
        let kind = parts.next().unwrap();
        let rest = parts.next().unwrap_or("").trim();
        if kind == "phase" {
            let mut words = rest.split_whitespace();
            let mut phase = Phase { name: words.next().unwrap().to_owned(), principal: "none".into(), caps: vec![], assert_final: false, aces: vec![] };
            for word in words {
                let (key, value) = word.split_once('=').unwrap();
                match key {
                    "principal" => phase.principal = value.to_owned(),
                    "caps" => phase.caps = value.split('+').filter(|c| *c != "none").map(str::to_owned).collect(),
                    "assert" => phase.assert_final = value == "final",
                    _ => panic!("Unknown phase option {key}"),
                }
            }
            assert!(["none", "cap", "package"].contains(&phase.principal.as_str()));
            phases.push(phase);
        } else {
            assert!(kind == "ace" || kind == "tree", "Unknown plan line {line}");
            let mut words = rest.splitn(3, ' ');
            let rights = u32::from_str_radix(words.next().unwrap().trim_start_matches("0x"), 16).unwrap();
            let flags = words.next().unwrap().replace('-', "");
            let path = words.next().unwrap().trim();
            phases.last_mut().unwrap().aces.push(Ace { rights, flags, path: if path == "." { String::new() } else { path.to_owned() }, tree: kind == "tree" });
        }
    }
    phases
}

/// Proves on synthetic keys that the exact-object update and the binary restore round-trip.
fn self_test() {
    let sid = Capabilities::new(&[CUSTOM_CAPABILITY]).text[0].clone();
    let inherited = format!("SOFTWARE\\MiniClawWinsockAclSelfTest-{}", uuid::Uuid::new_v4());
    let protected = format!("SOFTWARE\\MiniClawWinsockAclSelfTestP-{}", uuid::Uuid::new_v4());
    let _a = Sentinel::new(&inherited, None);
    let _b = Sentinel::new(&protected, Some("D:P(A;CI;KA;;;BA)(A;CI;KA;;;SY)(A;CI;KR;;;BU)"));
    let before: Vec<_> = [&inherited, &protected].iter().map(|p| descriptor(&machine_key(p, READ_CONTROL_DAC).unwrap(), OWNER_GROUP_DACL)).collect();
    let mut guard = AclGuard::apply(&sid, &[Ace { rights: 0x20019, flags: String::new(), path: inherited.clone(), tree: false },
        Ace { rights: 0x1, flags: "CI".into(), path: protected.clone(), tree: false }]);
    guard.restore();
    for (path, before) in [&inherited, &protected].iter().zip(before) {
        assert_eq!(descriptor(&machine_key(path, READ_CONTROL_DAC).unwrap(), OWNER_GROUP_DACL), before);
    }
    println!("SELF TEST exact set/restore verified on inherited and protected synthetic keys");
}

#[test]
#[ignore = "reviewed recovery entry: restores any unfinished Winsock ACL diagnostic record"]
/// Run after every experiment, also after a crash; a missing record is success.
fn winsock_acl_restore() { restore_record(); }

#[tokio::test]
#[ignore = "isolated elevated VM experiment that temporarily changes HKLM key DACLs; not production acceptance"]
/// Runs each planned grant set against a zero-registryRead LPAC child under the unchanged WFP policy.
async fn winsock_acl_experiment() {
    use std::net::TcpListener;
    assert!(!record_path().exists(), "Unfinished Winsock ACL record; run winsock_acl_restore first");
    self_test();
    let root = std::env::temp_dir().join(format!("mc-winsock-acl-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&root, false).unwrap();
    let outside = root.join("outside-secret");
    fs::write(&outside, "synthetic-file-secret").unwrap();
    let work = root.join("work");
    fs::create_dir(&work).unwrap();
    let owner = user_sid();
    let private_name = format!("SOFTWARE\\MiniClawWinsockAclPrivate-{}", uuid::Uuid::new_v4());
    let inherited_name = format!("SOFTWARE\\MiniClawWinsockAclInherited-{}", uuid::Uuid::new_v4());
    let _private = Sentinel::new(&private_name, Some(&format!("O:{owner}D:P(A;;KA;;;{owner})")));
    let _inherited = Sentinel::new(&inherited_name, None);
    let program = std::env::current_exe().unwrap();
    let custom_sid = Capabilities::new(&[CUSTOM_CAPABILITY]).text[0].clone();
    println!("CUSTOM CAPABILITY {CUSTOM_CAPABILITY} = {custom_sid}");
    for phase in plan() {
        let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
        let denied = TcpListener::bind("127.0.0.1:0").unwrap();
        let other = TcpListener::bind("127.0.0.2:0").unwrap();
        let ipv6 = TcpListener::bind("[::1]:0").unwrap();
        // Live receivers plus an unsandboxed positive connection rule out unavailable services.
        for receiver in [&proxy, &denied, &other, &ipv6] {
            let control = std::net::TcpStream::connect_timeout(&receiver.local_addr().unwrap(), Duration::from_secs(2)).unwrap();
            drop(receiver.accept().unwrap());
            drop(control);
            receiver.set_nonblocking(true).unwrap();
        }
        let port = proxy.local_addr().unwrap().port();
        let mut sandbox = Sandbox::new(&[
            Grant { path: work.to_string_lossy().into_owned(), write: true, folder: true, blocked: false },
            Grant { path: program.to_string_lossy().into_owned(), write: false, folder: false, blocked: false },
        ], &work).unwrap();
        let package = sandbox.package_sid().unwrap();
        let names: Vec<&str> = phase.caps.iter().map(|c| if c == "custom" { CUSTOM_CAPABILITY } else {
            assert!(["internetClient", "registryRead"].contains(&c.as_str()));
            c.as_str()
        }).collect();
        let mut capabilities = Capabilities::new(&names);
        println!("\nPHASE {} principal={} caps={:?} capability_sids={:?} package={package}", phase.name, phase.principal, phase.caps, capabilities.text);
        let native = SECURITY_CAPABILITIES { AppContainerSid: sandbox.security().capabilities.AppContainerSid,
            Capabilities: if capabilities.entries.is_empty() { ptr::null_mut() } else { capabilities.entries.as_mut_ptr() },
            CapabilityCount: capabilities.entries.len() as u32, Reserved: 0 };
        let security = miniclaw_server::windows_process::SecurityContext { capabilities: &native, low_privilege: true };
        let mut env: Vec<_> = sandbox.environment().into_iter().map(|(k, v)| (k.into_string().unwrap(), v.into_string().unwrap())).collect();
        env.extend([("MC_WINSOCK_ACL_CHILD".into(), format!("{port}:{}:{}:{}", denied.local_addr().unwrap().port(),
                other.local_addr().unwrap().port(), ipv6.local_addr().unwrap().port())),
            ("MC_PRIVATE_KEY".into(), private_name.clone()), ("MC_INHERITED_KEY".into(), inherited_name.clone()),
            ("MC_PRIVATE_FILE".into(), outside.to_string_lossy().into_owned())]);
        if let Ok(probes) = std::env::var("MC_WINSOCK_ACL_PROBES") { env.push(("MC_WINSOCK_ACL_PROBES".into(), probes)); }
        let mut guard = match phase.principal.as_str() {
            "cap" => Some(AclGuard::apply(&custom_sid, &phase.aces)),
            "package" => Some(AclGuard::apply(&package, &phase.aces)),
            _ => { assert!(phase.aces.is_empty(), "A phase without a principal cannot grant"); None }
        };
        let pending = miniclaw_server::windows_process::pending_stdio_restricted(
            &program, &["--exact".into(), "winsock_acl_child".into(), "--nocapture".into()], &work, &env, &security).unwrap();
        sandbox.bind_process(&pending.server.process).unwrap();
        let tree = pending.server.process.tree_guard();
        let mut lease = Lease::install(&package, port).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(40), miniclaw_server::windows_process::capture_stdio(pending.resume().unwrap())).await;
        tree.terminate_and_wait(Duration::from_secs(10)).unwrap();
        lease.revoke().unwrap();
        if let Some(guard) = guard.as_mut() { guard.restore(); }
        let (output, errors, code) = result.unwrap().unwrap();
        let output = String::from_utf8_lossy(&output).into_owned();
        println!("{output}\n{}", String::from_utf8_lossy(&errors));
        tokio::time::sleep(Duration::from_millis(200)).await;
        let reached = proxy.accept().is_ok();
        let leaked: Vec<_> = [("blocked-port", &denied), ("127.0.0.2", &other), ("::1", &ipv6)].iter()
            .filter(|(_, receiver)| receiver.accept().is_ok()).map(|(name, _)| *name).collect();
        println!("PARENT phase={} child_exit={code} proxy_accepted={reached} denied_receivers_reached={leaked:?}", phase.name);
        assert_eq!(code, 0);
        assert!(leaked.is_empty(), "A non-approved endpoint received a connection");
        assert_eq!(fs::read_to_string(&outside).unwrap(), "synthetic-file-secret");
        if phase.assert_final {
            assert!(!phase.caps.iter().any(|c| c == "registryRead"));
            assert!(output.contains("WSAStartup=0") && output.contains("PROXY reachable=true") && reached, "Winsock did not work with the planned grant");
            assert!(!output.contains("EXPOSED"), "A negative control was exposed");
            assert!(output.contains("NEG complete"));
            println!("FINAL phase={} Winsock works and every negative control passed", phase.name);
        }
        drop(sandbox);
        miniclaw_server::windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    }
    fs::remove_dir_all(root).unwrap();
}

// ---------------------------------------------------------------------------------------------
// Child (runs inside the LPAC). Reads no real values: only open statuses and synthetic sentinels.

#[repr(C)]
struct UnicodeString { length: u16, maximum: u16, buffer: *const u16 }
#[repr(C)]
struct ObjectAttributes { length: u32, root: HANDLE, name: *const UnicodeString, attributes: u32, descriptor: *const c_void, quality: *const c_void }
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtOpenKey(handle: *mut HANDLE, access: u32, attributes: *const ObjectAttributes) -> i32;
    fn NtQueryObject(handle: HANDLE, class: u32, buffer: *mut c_void, length: u32, returned: *mut u32) -> i32;
    fn NtClose(handle: HANDLE) -> i32;
}

/// Opens an absolute NT registry path; returns the status and, on success, the granted access mask.
fn nt_open(path: &str, access: u32, open_link: bool) -> (i32, u32) {
    let text: Vec<u16> = path.encode_utf16().collect();
    let name = UnicodeString { length: (text.len() * 2) as u16, maximum: (text.len() * 2) as u16, buffer: text.as_ptr() };
    // OBJ_CASE_INSENSITIVE, plus OBJ_OPENLINK to open a symbolic link key itself.
    let attributes = ObjectAttributes { length: std::mem::size_of::<ObjectAttributes>() as u32, root: ptr::null_mut(), name: &name,
        attributes: if open_link { 0x140 } else { 0x40 }, descriptor: ptr::null(), quality: ptr::null() };
    let mut handle = ptr::null_mut();
    let status = unsafe { NtOpenKey(&mut handle, access, &attributes) };
    if status != 0 { return (status, 0); }
    let mut basic = [0u32; 14];
    unsafe { NtQueryObject(handle, 0, basic.as_mut_ptr().cast(), 56, ptr::null_mut()); NtClose(handle); }
    (status, basic[1])
}

fn win32_open(hive: HKEY, path: &str, access: u32) -> u32 {
    let mut key = ptr::null_mut();
    let status = unsafe { RegOpenKeyExW(hive, wide(path).as_ptr(), 0, access, &mut key) };
    if status == 0 { unsafe { RegCloseKey(key); } }
    status
}

/// Reports whether a key can be opened by any path or mask, and whether its first subkey can be listed.
fn negative(label: &str, hive: HKEY, path: &str, nt_path: Option<String>) {
    let mut exposed = vec![];
    for access in [KEY_QUERY_VALUE, KEY_ENUMERATE_SUB_KEYS, KEY_READ, MAXIMUM_ALLOWED_ACCESS] {
        let status = win32_open(hive, path, access);
        if status != ERROR_ACCESS_DENIED && status != ERROR_FILE_NOT_FOUND { exposed.push(format!("win32 access=0x{access:x} status={status}")); }
        if let Some(nt) = &nt_path {
            let (status, granted) = nt_open(nt, access, false);
            // STATUS_ACCESS_DENIED or STATUS_OBJECT_NAME_NOT_FOUND.
            if status as u32 != 0xC000_0022 && status as u32 != 0xC000_0034 { exposed.push(format!("nt access=0x{access:x} status=0x{:x} granted=0x{granted:x}", status as u32)); }
        }
    }
    let mut key = ptr::null_mut();
    if unsafe { RegOpenKeyExW(hive, wide(path).as_ptr(), 0, KEY_ENUMERATE_SUB_KEYS, &mut key) } == 0 {
        let mut name = [0u16; 256];
        let mut length = name.len() as u32;
        // Only the status is reported; the enumerated name is discarded.
        let status = unsafe { RegEnumKeyExW(key, 0, name.as_mut_ptr(), &mut length, ptr::null(), ptr::null_mut(), ptr::null_mut(), ptr::null_mut()) };
        unsafe { RegCloseKey(key); }
        exposed.push(format!("enumeration status={status}"));
    }
    if exposed.is_empty() { println!("NEG {label}: denied"); } else { println!("NEG {label}: EXPOSED {}", exposed.join("; ")); }
}

#[test]
/// Reproduces Winsock's registry opens component by component, then sockets and negative controls.
fn winsock_acl_child() {
    use std::net::{SocketAddr, TcpStream};
    let Ok(ports) = std::env::var("MC_WINSOCK_ACL_CHILD") else { return };
    println!("CHILD PID {}", std::process::id());
    let container = token_information(TokenIsAppContainer)[0] as u32;
    let groups = token_information(TokenCapabilities);
    let capabilities: Vec<String> = unsafe {
        let groups = groups.as_ptr().cast::<TOKEN_GROUPS>();
        (0..(*groups).GroupCount as usize).map(|i| sid_text((*(*groups).Groups.as_ptr().add(i)).Sid)).collect()
    };
    println!("TOKEN appcontainer={container} capabilities={capabilities:?}");
    let probes: Vec<String> = match std::env::var("MC_WINSOCK_ACL_PROBES") {
        Ok(list) => list.split(';').map(|p| if p == "." { String::new() } else { p.to_owned() }).collect(),
        Err(_) => DEFAULT_PROBES.iter().map(|p| (*p).to_owned()).collect(),
    };
    for probe in &probes {
        let nt = if probe.is_empty() { r"\Registry\Machine".to_owned() } else { format!(r"\Registry\Machine\{probe}") };
        let (read, _) = nt_open(&nt, KEY_READ, false);
        let (query, _) = nt_open(&nt, KEY_QUERY_VALUE, false);
        let (maximum, granted) = nt_open(&nt, MAXIMUM_ALLOWED_ACCESS, false);
        let link = if probe.ends_with("CurrentControlSet") { format!(" nt_openlink_max=0x{:x}", nt_open(&nt, MAXIMUM_ALLOWED_ACCESS, true).0 as u32) } else { String::new() };
        println!("PROBE HKLM\\{probe}: nt_read=0x{:x} nt_query=0x{:x} nt_max=0x{:x} granted=0x{granted:x}{link} win32_read={} win32_max={}",
            read as u32, query as u32, maximum as u32, win32_open(HKEY_LOCAL_MACHINE, probe, KEY_READ), win32_open(HKEY_LOCAL_MACHINE, probe, MAXIMUM_ALLOWED_ACCESS));
    }
    let mut data = unsafe { std::mem::zeroed() };
    let startup = unsafe { windows_sys::Win32::Networking::WinSock::WSAStartup(0x0202, &mut data) };
    println!("WSAStartup={startup}");
    let ports: Vec<u16> = ports.split(':').map(|p| p.parse().unwrap()).collect();
    if startup == 0 {
        let connect = |address: SocketAddr| TcpStream::connect_timeout(&address, Duration::from_secs(2));
        let allowed = connect(SocketAddr::from(([127, 0, 0, 1], ports[0])));
        println!("PROXY reachable={} error={:?}", allowed.is_ok(), allowed.as_ref().err().and_then(|e| e.raw_os_error()));
        for (label, address) in [("blocked loopback port", SocketAddr::from(([127, 0, 0, 1], ports[1]))),
            ("127.0.0.2", SocketAddr::from(([127, 0, 0, 2], ports[2]))),
            ("::1", SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, ports[3])))] {
            match connect(address) {
                Ok(_) => println!("NEG connect {label}: EXPOSED"),
                Err(error) => println!("NEG connect {label}: denied ({:?})", error.raw_os_error()),
            }
        }
        match std::net::TcpListener::bind("127.0.0.1:0") {
            Ok(_) => println!("NEG listen 127.0.0.1:0: EXPOSED"),
            Err(error) => println!("NEG listen 127.0.0.1:0: denied ({:?})", error.raw_os_error()),
        }
        let resolved = std::net::ToSocketAddrs::to_socket_addrs("localhost:80");
        println!("INFO resolve localhost ok={} error={:?}", resolved.is_ok(), resolved.err().and_then(|e| e.raw_os_error()));
        unsafe { windows_sys::Win32::Networking::WinSock::WSACleanup(); }
    }
    let user = user_sid();
    for (label, path) in [("HKCU root", ""), ("HKCU Environment", "Environment"), ("HKCU Software", "SOFTWARE"),
        ("HKCU Software Classes", r"SOFTWARE\Classes"), ("HKCU Software Microsoft", r"SOFTWARE\Microsoft"), ("HKCU Volatile Environment", "Volatile Environment")] {
        let nt = if path.is_empty() { format!(r"\Registry\User\{user}") } else { format!(r"\Registry\User\{user}\{path}") };
        negative(label, HKEY_CURRENT_USER, path, Some(nt));
    }
    negative("HKU .DEFAULT", HKEY_USERS, ".DEFAULT", Some(r"\Registry\User\.DEFAULT".into()));
    for (label, path) in [("HKLM SOFTWARE", "SOFTWARE"), ("HKLM SOFTWARE Windows NT CurrentVersion", r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"),
        ("HKLM SOFTWARE Classes", r"SOFTWARE\Classes"), ("HKLM SYSTEM", "SYSTEM"), ("HKLM SYSTEM Control", r"SYSTEM\CurrentControlSet\Control"),
        ("HKLM Session Manager Environment", r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment"),
        ("HKLM Services", r"SYSTEM\CurrentControlSet\Services"), ("HKLM Services LanmanServer", r"SYSTEM\CurrentControlSet\Services\LanmanServer"),
        ("HKLM SAM", "SAM"), ("HKLM SECURITY", "SECURITY"), ("HKLM HARDWARE", "HARDWARE")] {
        negative(label, HKEY_LOCAL_MACHINE, path, Some(format!(r"\Registry\Machine\{path}")));
    }
    for (label, variable) in [("HKLM owner-only sentinel", "MC_PRIVATE_KEY"), ("HKLM inherited sentinel", "MC_INHERITED_KEY")] {
        let path = std::env::var(variable).unwrap();
        negative(label, HKEY_LOCAL_MACHINE, &path, Some(format!(r"\Registry\Machine\{path}")));
    }
    // The root key itself may be granted; report exactly what that grant allows there.
    let mut key = ptr::null_mut();
    let opened = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, ptr::null(), 0, KEY_ENUMERATE_SUB_KEYS, &mut key) };
    let mut listing = String::from("not opened");
    if opened == 0 {
        let mut name = [0u16; 256];
        let mut length = name.len() as u32;
        listing = format!("first subkey enumeration status={}", unsafe { RegEnumKeyExW(key, 0, name.as_mut_ptr(), &mut length, ptr::null(), ptr::null_mut(), ptr::null_mut(), ptr::null_mut()) });
        unsafe { RegCloseKey(key); }
    }
    let mut created = ptr::null_mut();
    let create = unsafe { RegCreateKeyExW(HKEY_LOCAL_MACHINE, wide(r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\MiniClawWriteProbe").as_ptr(), 0, ptr::null(),
        REG_OPTION_VOLATILE, KEY_WRITE, ptr::null(), &mut created, ptr::null_mut()) };
    if create == 0 { unsafe { RegCloseKey(created); } }
    println!("INFO HKLM root enumerate-open={opened} ({listing}); catalog subkey create={create}");
    if create == 0 { println!("NEG catalog write: EXPOSED"); } else { println!("NEG catalog write: denied"); }
    let secret = std::env::var("MC_PRIVATE_FILE").unwrap();
    let (read, write) = (fs::read(&secret).is_ok(), fs::write(&secret, "changed").is_ok());
    if read || write { println!("NEG outside-grant file: EXPOSED read={read} write={write}"); } else { println!("NEG outside-grant file: denied"); }
    fs::write("positive-control", "writable").unwrap();
    assert_eq!(fs::read_to_string("positive-control").unwrap(), "writable");
    println!("NEG complete");
}
