#![cfg(windows)]
// Isolated VM-only diagnostic: why language runtimes (Node, Python) do not start inside the LPAC
// sandbox, and which minimal access would let them. Nothing here is production behaviour.
// Registry DACL changes are HKLM-only, recorded durably first and restored exactly; HKCU is never modified.
use miniclaw_server::{platform_fs, seatbelt::Grant, windows_network::Lease, windows_sandbox::Sandbox};
use std::{ffi::c_void, fs, io::Write, path::{Path, PathBuf}, ptr, time::Duration};
use windows_sys::Win32::{Foundation::*, Security::{Authorization::*, *}, System::Registry::*};

/// Stable test capability name; an installer would use one install-specific name instead.
const CUSTOM_CAPABILITY: &str = "miniclawRuntimeRead.diagnostic-7c41a9e2";
const OWNER_GROUP_DACL: u32 = OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
const READ_CONTROL_DAC: u32 = 0x0002_0000 | 0x0004_0000;
const MAXIMUM_ALLOWED_ACCESS: u32 = 0x0200_0000;

/// Phases used when no plan file is supplied: the unchanged production tokens only.
const DEFAULT_PLAN: &str = r"
phase offline-none principal=none caps=none lease=0
run child
phase networked-production principal=none caps=internetClient+winsock lease=1
run child
";

/// Registry keys whose opens the child reports, unless the plan lists its own `probe` lines.
const DEFAULT_PROBES: &[&str] = &["", "SOFTWARE", r"SOFTWARE\Microsoft", r"SOFTWARE\Microsoft\Cryptography",
    r"SOFTWARE\Microsoft\Cryptography\Defaults", r"SOFTWARE\Microsoft\Cryptography\Defaults\Provider",
    r"SOFTWARE\Microsoft\Cryptography\Defaults\Provider\Microsoft Strong Cryptographic Provider",
    r"SOFTWARE\Microsoft\Cryptography\Defaults\Provider Types",
    r"SOFTWARE\Microsoft\Cryptography\Defaults\Provider Types\Type 001",
    r"SOFTWARE\Policies\Microsoft\Cryptography", r"SYSTEM\CurrentControlSet\Control\Lsa\FipsAlgorithmPolicy",
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\SideBySide", r"SOFTWARE\Microsoft\Windows\CurrentVersion\SideBySide\Winners",
    r"SYSTEM\CurrentControlSet\Control\Session Manager", r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options",
    r"SOFTWARE\Microsoft\Wow64", r"SOFTWARE\Microsoft\Wow64\amd64", r"SOFTWARE\Microsoft\Wow64\x86",
    r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters"];

/// Files and folders whose opens the child reports, in addition to `file` lines and discovered assemblies.
const DEFAULT_FILES: &[&str] = &[r"C:\Windows\WinSxS", r"C:\Windows\WinSxS\manifests", r"C:\Windows\WindowsShell.Manifest",
    r"C:\Windows\System32\sxs.dll", r"C:\Windows\System32\rsaenh.dll", r"C:\Windows\System32\cryptsp.dll",
    r"C:\Windows\System32\comctl32.dll", r"C:\Windows\XtaCache", r"C:\Windows\System32\xtajit64.dll",
    r"C:\Windows\System32\xtajit64se.dll", r"C:\Windows\System32\xtajit.dll", r"C:\Windows\apppatch\sysmain.sdb"];

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
// Same discipline as windows_winsock_acl_diagnostic.rs, with its own record location.

#[derive(PartialEq, Debug)]
struct Key(HKEY);
impl Drop for Key { fn drop(&mut self) { unsafe { RegCloseKey(self.0); } } }

/// Opens one existing HKLM key; "" is the hive root. HKCU is never opened for modification.
fn machine_key(path: &str, access: u32) -> Result<Key, u32> {
    let mut key = ptr::null_mut();
    let status = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, wide(path).as_ptr(), 0, access, &mut key) };
    if status == 0 { Ok(Key(key)) } else { Err(status) }
}

/// Returns the self-relative descriptor for exactly this key.
fn descriptor(key: HKEY, information: u32) -> Result<Vec<u8>, u32> {
    unsafe {
        let mut size = 0u32;
        let status = RegGetKeySecurity(key, information, ptr::null_mut(), &mut size);
        if status != ERROR_INSUFFICIENT_BUFFER { return Err(status); }
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        let status = RegGetKeySecurity(key, information, buffer.as_mut_ptr().cast(), &mut size);
        if status != 0 { return Err(status); }
        Ok(std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), size as usize).to_vec())
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

/// Replaces only this key's DACL; nothing propagates to descendants.
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
    let directory = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap()).join("MiniClawRuntimeAclDiagnostic");
    if !directory.exists() { platform_fs::private_dir(&directory, false).unwrap(); }
    directory.join("record.json")
}

/// One requested grant: rights (subset of KEY_READ), ACE flags ("" or "CI"), HKLM path, whole subtree or not.
#[derive(Clone)]
struct Ace { rights: u32, flags: String, path: String, tree: bool }

/// Lists a key and all its descendants, bounded.
fn expand(ace: &Ace) -> Vec<String> {
    if !ace.tree { return vec![ace.path.clone()]; }
    // A subtree this machine does not have grants nothing; the phase output shows what then fails.
    if machine_key(&ace.path, KEY_ENUMERATE_SUB_KEYS) == Err(ERROR_FILE_NOT_FOUND) { println!("ACL SKIP HKLM\\{}: absent on this machine", ace.path); return vec![]; }
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
        assert!(!record.exists(), "Restore the prior runtime ACL diagnostic before adding permissions");
        assert!(sid.starts_with("S-1-15-3-1024-"), "Expected a custom capability SID");
        let mut entries = vec![];
        let mut seen = std::collections::HashSet::new();
        for ace in aces {
            assert!(ace.rights != 0 && ace.rights & !0x0002_0019 == 0, "Only read rights may be granted");
            assert!(ace.flags.is_empty() || ace.flags == "CI");
            assert!(!ace.path.to_ascii_uppercase().contains("CONTROLSET0"), "Name keys through CurrentControlSet only");
            for path in expand(ace) {
                assert!(seen.insert(path.to_ascii_lowercase()), "Key listed twice: {path}");
                let key = machine_key(&path, READ_CONTROL_DAC).unwrap_or_else(|s| panic!("Cannot open HKLM\\{path} for WRITE_DAC: {s}"));
                let before = descriptor(key.0, OWNER_GROUP_DACL).unwrap();
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
            assert_eq!(hex(&descriptor(key.0, OWNER_GROUP_DACL).unwrap()), entry["before_hex"].as_str().unwrap(), "Registry changed before diagnostic mutation");
            assert_eq!(set_dacl_sddl(&key, entry["after_dacl"].as_str().unwrap()), 0, "Could not set DACL on HKLM\\{path}");
            let now = descriptor(key.0, OWNER_GROUP_DACL).unwrap();
            assert_eq!(sddl(&now, DACL_SECURITY_INFORMATION), entry["after_dacl"].as_str().unwrap(), "Unexpected registry ACL delta on HKLM\\{path}");
            println!("ACL GRANT HKLM\\{path}: {} -> {}", entry["before_dacl"].as_str().unwrap(), entry["after_dacl"].as_str().unwrap());
        }
        println!("Exact runtime read grants installed for {sid}: {} keys", entries.len());
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
    if !record.exists() { println!("No runtime ACL diagnostic record present"); return; }
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    assert_eq!(value["version"], 1);
    let entries = value["entries"].as_array().unwrap();
    assert!(entries.len() <= 1024);
    for entry in entries.iter().rev() {
        let path = entry["path"].as_str().unwrap();
        let before_hex = entry["before_hex"].as_str().unwrap();
        let key = machine_key(path, READ_CONTROL_DAC).unwrap_or_else(|s| panic!("Cannot reopen HKLM\\{path}: {s}; record preserved"));
        let current = descriptor(key.0, OWNER_GROUP_DACL).unwrap();
        if hex(&current) != before_hex {
            let current_dacl = sddl(&current, DACL_SECURITY_INFORMATION);
            assert!(current_dacl == entry["after_dacl"].as_str().unwrap() || current_dacl == entry["before_dacl"].as_str().unwrap(),
                "HKLM\\{path} changed outside the diagnostic; recovery record preserved");
            assert_eq!(set_dacl_binary(&key, &unhex(before_hex)), 0, "Could not restore HKLM\\{path}; record preserved");
        }
        let restored = descriptor(key.0, OWNER_GROUP_DACL).unwrap();
        assert_eq!(sddl(&restored, OWNER_GROUP_DACL), entry["before"].as_str().unwrap(), "HKLM\\{path} SDDL did not restore exactly; record preserved");
        assert_eq!(hex(&restored), before_hex, "HKLM\\{path} descriptor bytes did not restore exactly; record preserved");
        println!("ACL RESTORED HKLM\\{path}: {}", entry["before"].as_str().unwrap());
    }
    fs::remove_file(&record).unwrap();
    println!("Exact runtime diagnostic ACL restoration verified: {} keys (SDDL and descriptor bytes equal)", entries.len());
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
    /// Derives capability SIDs by name, retaining all native allocations.
    fn new(names: &[String]) -> Self {
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

    /// Binary form of every derived SID, for searching raw descriptors.
    fn binary(&self) -> Vec<Vec<u8>> {
        self.allocations.iter().map(|sid| unsafe { std::slice::from_raw_parts((*sid).cast::<u8>(), GetLengthSid(*sid) as usize).to_vec() }).collect()
    }
}
impl Drop for Capabilities {
    fn drop(&mut self) { for sid in &self.allocations { unsafe { LocalFree(*sid); } } }
}

/// Resolves a plan capability word: `custom` and `winsock` are the diagnostic and the installed names.
fn capability_name(word: &str) -> String {
    match word {
        "custom" => CUSTOM_CAPABILITY.to_owned(),
        "winsock" => miniclaw_server::windows_network_broker::winsock_capability().expect("the installed network service names the Winsock capability"),
        other => { assert!(other.chars().all(|c| c.is_ascii_alphanumeric()), "Unexpected capability name {other}"); other.to_owned() }
    }
}

// ---------------------------------------------------------------------------------------------
// Plan.

/// One launch inside a phase's token: the in-token probe child, or a real program.
enum Run { Child(String), Program { variable: String, args: Vec<String>, copy: bool, prime: bool } }

struct Phase { name: String, principal: String, caps: Vec<String>, lease: bool, aces: Vec<Ace>, runs: Vec<Run>, env: Vec<(String, String)>, grants: Vec<String>, pymain: Option<String>, puts: Vec<(String, String)> }

struct Plan { phases: Vec<Phase>, probes: Vec<String>, files: Vec<String>, names: Vec<String> }

/// Line format:
/// `probe <HKLM path>` / `file <path>` / `name <host>` (global lists for the child),
/// `phase <name> principal=none|cap caps=a+b lease=0|1`, then for that phase
/// `ace <hex rights> <-|CI> <HKLM path or .>`, `tree <hex> <-|CI> <path>`, `env NAME=VALUE`,
/// `grant <ENVVAR>` (read access to that program's folder for child runs), `pymain <ENVVAR>`,
/// `put <relative path> <text>` (a file written into each run's work folder; `\n` is a newline),
/// `run child [sections]` and `run program <ENVVAR> <args…>`. `%WORK%` expands in env and args.
/// `run copy <ENVVAR> <args…>` runs a fresh copy of the program and its libraries from the work folder, a
/// path Windows has never built an activation context for; `run primed …` does the same after the
/// unsandboxed parent has asked Windows for the activation contexts of that copy; `run primedprogram …`
/// asks for those of the installed program and its libraries, then runs it in place.
fn plan() -> Plan {
    let text = match std::env::var("MC_RT_PLAN") { Ok(path) => fs::read_to_string(path).unwrap(), Err(_) => DEFAULT_PLAN.to_owned() };
    let mut plan = Plan { phases: vec![], probes: vec![], files: vec![], names: vec![] };
    for line in text.lines().map(|l| l.trim().trim_start_matches('\u{feff}')).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let (kind, rest) = line.split_once(' ').map_or((line, ""), |(kind, rest)| (kind, rest.trim()));
        match kind {
            "probe" => plan.probes.push(if rest == "." { String::new() } else { rest.to_owned() }),
            "file" => plan.files.push(rest.to_owned()),
            "name" => plan.names.push(rest.to_owned()),
            "phase" => {
                let mut words = rest.split_whitespace();
                let mut phase = Phase { name: words.next().unwrap().to_owned(), principal: "none".into(), caps: vec![], lease: false, aces: vec![], runs: vec![], env: vec![], grants: vec![], pymain: None, puts: vec![] };
                for word in words {
                    let (key, value) = word.split_once('=').unwrap();
                    match key {
                        "principal" => phase.principal = value.to_owned(),
                        "caps" => phase.caps = value.split('+').filter(|c| *c != "none").map(str::to_owned).collect(),
                        "lease" => phase.lease = value == "1",
                        _ => panic!("Unknown phase option {key}"),
                    }
                }
                assert!(["none", "cap"].contains(&phase.principal.as_str()));
                plan.phases.push(phase);
            }
            "ace" | "tree" => {
                let mut words = rest.splitn(3, ' ');
                let rights = u32::from_str_radix(words.next().unwrap().trim_start_matches("0x"), 16).unwrap();
                let flags = words.next().unwrap().replace('-', "");
                let path = words.next().unwrap().trim();
                plan.phases.last_mut().unwrap().aces.push(Ace { rights, flags, path: if path == "." { String::new() } else { path.to_owned() }, tree: kind == "tree" });
            }
            "env" => { let (name, value) = rest.split_once('=').unwrap(); plan.phases.last_mut().unwrap().env.push((name.to_owned(), value.to_owned())); }
            "grant" => plan.phases.last_mut().unwrap().grants.push(rest.to_owned()),
            "put" => {
                let (name, text) = rest.split_once(' ').unwrap_or((rest, ""));
                assert!(!name.contains("..") && !name.contains(':') && !name.starts_with(['\\', '/']), "put names a file inside the work folder");
                plan.phases.last_mut().unwrap().puts.push((name.to_owned(), text.replace("\\n", "\n")));
            }
            "pymain" => plan.phases.last_mut().unwrap().pymain = Some(rest.to_owned()),
            "run" => {
                let mut words = rest.split_whitespace();
                let run = match words.next().unwrap() {
                    "child" => Run::Child(words.collect::<Vec<_>>().join(",")),
                    kind @ ("program" | "copy" | "primed" | "primedprogram") => Run::Program { variable: words.next().unwrap().to_owned(), args: words.map(str::to_owned).collect(), copy: kind == "copy" || kind == "primed", prime: kind.starts_with("primed") },
                    other => panic!("Unknown run kind {other}"),
                };
                plan.phases.last_mut().unwrap().runs.push(run);
            }
            _ => panic!("Unknown plan line {line}"),
        }
    }
    plan
}

/// Proves on synthetic keys that the exact-object update and the binary restore round-trip.
fn self_test() {
    let sid = Capabilities::new(&[CUSTOM_CAPABILITY.to_owned()]).text[0].clone();
    let inherited = format!("SOFTWARE\\MiniClawRuntimeAclSelfTest-{}", uuid::Uuid::new_v4());
    let protected = format!("SOFTWARE\\MiniClawRuntimeAclSelfTestP-{}", uuid::Uuid::new_v4());
    let _a = Sentinel::new(&inherited, None);
    let _b = Sentinel::new(&protected, Some("D:P(A;CI;KA;;;BA)(A;CI;KA;;;SY)(A;CI;KR;;;BU)"));
    let before: Vec<_> = [&inherited, &protected].iter().map(|p| descriptor(machine_key(p, READ_CONTROL_DAC).unwrap().0, OWNER_GROUP_DACL).unwrap()).collect();
    let mut guard = AclGuard::apply(&sid, &[Ace { rights: 0x20019, flags: String::new(), path: inherited.clone(), tree: false },
        Ace { rights: 0x1, flags: "CI".into(), path: protected.clone(), tree: false }]);
    guard.restore();
    for (path, before) in [&inherited, &protected].iter().zip(before) {
        assert_eq!(descriptor(machine_key(path, READ_CONTROL_DAC).unwrap().0, OWNER_GROUP_DACL).unwrap(), before);
    }
    println!("SELF TEST exact set/restore verified on inherited and protected synthetic keys");
}

#[test]
#[ignore = "reviewed recovery entry: restores any unfinished runtime ACL diagnostic record"]
/// Run after every experiment, also after a crash; a missing record is success.
fn runtime_acl_restore() { restore_record(); }

/// The program a test variable names, when it exists on this machine.
fn program(variable: &str) -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os(variable)?);
    path.is_file().then_some(path)
}

/// Process IDs currently in a job owned by this process.
fn job_pids(handle: u64) -> Vec<usize> {
    use windows_sys::Win32::System::JobObjects::{JobObjectBasicProcessIdList, QueryInformationJobObject};
    #[repr(C)]
    struct List { assigned: u32, listed: u32, ids: [usize; 32] }
    let mut list = List { assigned: 0, listed: 0, ids: [0; 32] };
    let ok = unsafe { QueryInformationJobObject(handle as usize as HANDLE, JobObjectBasicProcessIdList, (&mut list as *mut List).cast(), std::mem::size_of::<List>() as u32, ptr::null_mut()) };
    if ok == 0 { return vec![]; }
    list.ids[..(list.listed as usize).min(32)].to_vec()
}

/// Finds side-by-side assemblies a runtime's manifest names, so the child can report their opens.
fn discovered_files() -> Vec<String> {
    let mut found = vec![];
    let Ok(entries) = fs::read_dir(r"C:\Windows\WinSxS\manifests") else { return found };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        if (lower.starts_with("arm64_microsoft.windows.common-controls_6595b64144ccf1df_6.0.") || lower.starts_with("amd64_microsoft.windows.common-controls_6595b64144ccf1df_6.0."))
            && lower.ends_with(".manifest") {
            let stem = name[..name.len() - ".manifest".len()].to_owned();
            found.push(format!(r"C:\Windows\WinSxS\manifests\{name}"));
            found.push(format!(r"C:\Windows\WinSxS\{stem}"));
            found.push(format!(r"C:\Windows\WinSxS\{stem}\comctl32.dll"));
        }
    }
    found
}

const MANIFEST_EMPTY: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0"><assemblyIdentity type="win32" name="MiniClaw.RuntimeDiagnostic" version="1.0.0.0"/></assembly>"#;
const MANIFEST_SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0" xmlns:asmv3="urn:schemas-microsoft-com:asm.v3"><assemblyIdentity type="win32" name="MiniClaw.RuntimeDiagnostic" version="1.0.0.0"/>
<compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1"><application><supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/></application></compatibility>
<asmv3:application><asmv3:windowsSettings xmlns:ws2="http://schemas.microsoft.com/SMI/2016/WindowsSettings"><ws2:longPathAware>true</ws2:longPathAware></asmv3:windowsSettings></asmv3:application></assembly>"#;
const MANIFEST_COMCTL: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0"><assemblyIdentity type="win32" name="MiniClaw.RuntimeDiagnostic" version="1.0.0.0"/>
<dependency><dependentAssembly><assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/></dependentAssembly></dependency></assembly>"#;

const NODE_PROBE: &str = r#"
const net = require('net'), dgram = require('dgram'), dns = require('dns'), fs = require('fs'), crypto = require('crypto');
console.log('NODEPROBE started', process.version, process.arch, 'random', crypto.randomBytes(8).length);
const ports = (process.env.MC_RT_PORTS || '').split(':');
const targets = [['proxy', '127.0.0.1', ports[0]], ['second loopback port', '127.0.0.1', ports[1]], ['127.0.0.2', '127.0.0.2', ports[2]], ['::1', '::1', ports[3]], ['lan', ports[4], ports[5]], ['public', '1.1.1.1', 443]];
let pending = 0;
const done = () => { if (--pending === 0) { console.log('NODEPROBE complete'); process.exit(0); } };
for (const [label, host, port] of targets) {
  pending++;
  try {
    const socket = net.connect({ host, port: Number(port), timeout: 2000 });
    socket.on('connect', () => { console.log('NODEPROBE connect ' + label + ': CONNECTED'); socket.destroy(); done(); });
    socket.on('timeout', () => { console.log('NODEPROBE connect ' + label + ': timeout'); socket.destroy(); done(); });
    socket.on('error', error => { console.log('NODEPROBE connect ' + label + ': ' + error.code); done(); });
  } catch (error) { console.log('NODEPROBE connect ' + label + ': threw ' + error.code); done(); }
}
for (const target of (process.env.MC_RT_UDP || '').split(',').filter(Boolean)) {
  pending++;
  try {
    const [host, port] = target.split(':');
    const socket = dgram.createSocket('udp4');
    socket.on('error', error => { console.log('NODEPROBE udp ' + target + ': ' + error.code); });
    socket.send('miniclaw-udp-probe', Number(port), host, error => { console.log('NODEPROBE udp ' + target + ': ' + (error ? error.code : 'sent')); try { socket.close(); } catch {} done(); });
  } catch (error) { console.log('NODEPROBE udp ' + target + ': threw ' + error.code); done(); }
}
for (const name of (process.env.MC_RT_NAMES || '').split(',').filter(Boolean)) {
  pending++;
  dns.lookup(name, { all: true }, (error, list) => { console.log('NODEPROBE resolve ' + name + ': ' + (error ? error.code : 'RESOLVED count=' + list.length)); done(); });
}
pending++;
try { fs.readFileSync(process.env.MC_PRIVATE_FILE); console.log('NODEPROBE outside-grant file: EXPOSED'); } catch (error) { console.log('NODEPROBE outside-grant file: ' + error.code); }
done();
"#;

const PYTHON_PROBE: &str = r#"
import os, sys
print("PYPROBE started", sys.version.split()[0], sys.executable, "urandom", len(os.urandom(8)), flush=True)
for name in ("json", "ctypes", "sqlite3", "hashlib", "ssl", "_overlapped", "asyncio", "subprocess"):
    try:
        __import__(name); print("PYPROBE import", name, "ok")
    except BaseException as error:
        print("PYPROBE import", name, "FAILED", type(error).__name__, error)
try:
    import ssl; print("PYPROBE ssl.RAND_bytes", len(ssl.RAND_bytes(16)), ssl.OPENSSL_VERSION)
except BaseException as error:
    print("PYPROBE ssl.RAND_bytes FAILED", type(error).__name__, error)
try:
    import socket
except BaseException as error:
    print("PYPROBE import socket FAILED", type(error).__name__, error); print("PYPROBE complete"); sys.exit(0)
ports = os.environ.get("MC_RT_PORTS", "").split(":")
for label, host, port in (("proxy", "127.0.0.1", ports[0]), ("second loopback port", "127.0.0.1", ports[1]), ("127.0.0.2", "127.0.0.2", ports[2]), ("lan", ports[4], ports[5]), ("public", "1.1.1.1", 443)):
    try:
        socket.create_connection((host, int(port)), timeout=2).close(); print("PYPROBE connect", label, "CONNECTED")
    except BaseException as error:
        print("PYPROBE connect", label, type(error).__name__, getattr(error, "winerror", None) or error)
for name in filter(None, os.environ.get("MC_RT_NAMES", "").split(",")):
    try:
        print("PYPROBE resolve", name, "RESOLVED count=%d" % len(socket.getaddrinfo(name, 80)))
    except BaseException as error:
        print("PYPROBE resolve", name, type(error).__name__, getattr(error, "errno", None))
try:
    open(os.environ["MC_PRIVATE_FILE"], "rb").read(); print("PYPROBE outside-grant file: EXPOSED")
except BaseException as error:
    print("PYPROBE outside-grant file:", type(error).__name__)
print("PYPROBE complete")
"#;

#[tokio::test]
#[ignore = "isolated elevated VM experiment that may temporarily change HKLM key DACLs; not production acceptance"]
/// Runs each planned token (capabilities, optional recorded read ACEs) against the probe child and real runtimes.
async fn runtime_experiment() {
    use std::net::{TcpListener, UdpSocket};
    assert!(!record_path().exists(), "Unfinished runtime ACL record; run runtime_acl_restore first");
    self_test();
    let root = std::env::temp_dir().join(format!("mc-runtime-diag-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&root, false).unwrap();
    let outside = root.join("outside-secret");
    fs::write(&outside, "synthetic-file-secret").unwrap();
    let owner = user_sid();
    let private_name = format!("SOFTWARE\\MiniClawRuntimeDiagPrivate-{}", uuid::Uuid::new_v4());
    let inherited_name = format!("SOFTWARE\\MiniClawRuntimeDiagInherited-{}", uuid::Uuid::new_v4());
    let _private = Sentinel::new(&private_name, Some(&format!("O:{owner}D:P(A;;KA;;;{owner})")));
    let _inherited = Sentinel::new(&inherited_name, None);
    let test_program = std::env::current_exe().unwrap();
    let custom_sid = Capabilities::new(&[CUSTOM_CAPABILITY.to_owned()]).text[0].clone();
    println!("CUSTOM CAPABILITY {CUSTOM_CAPABILITY} = {custom_sid}");
    let plan = plan();
    let probes = if plan.probes.is_empty() { DEFAULT_PROBES.iter().map(|p| if p.is_empty() { ".".to_owned() } else { (*p).to_owned() }).collect::<Vec<_>>() }
        else { plan.probes.iter().map(|p| if p.is_empty() { ".".to_owned() } else { p.clone() }).collect() };
    let mut files: Vec<String> = DEFAULT_FILES.iter().map(|f| (*f).to_owned()).chain(plan.files.iter().cloned()).chain(discovered_files()).collect();
    for variable in ["MC_RT_NODE", "MC_RT_PYTHON_ARM64", "MC_RT_NODE_X64", "MC_RT_PYTHON_X64"] {
        match program(variable) { Some(path) => { println!("PROGRAM {variable} = {}", path.display()); files.push(path.to_string_lossy().into_owned()); } None => println!("PROGRAM {variable} not available") }
    }
    let mut names = vec!["localhost".to_owned(), format!("mc-{}.invalid", uuid::Uuid::new_v4().simple())];
    names.extend(plan.names.iter().cloned());
    for name in &names {
        let resolved = std::net::ToSocketAddrs::to_socket_addrs(&(name.as_str(), 80));
        println!("CONTROL unsandboxed resolve {name}: ok={} count={} error={:?}", resolved.is_ok(), resolved.as_ref().map_or(0, |list| list.clone().count()), resolved.err().and_then(|e| e.raw_os_error()));
    }
    let lan = UdpSocket::bind("0.0.0.0:0").and_then(|socket| { socket.connect("192.0.2.1:9")?; socket.local_addr() }).unwrap().ip();
    let mut number = 0;
    for phase in &plan.phases {
        let names_of_caps: Vec<String> = phase.caps.iter().map(|c| capability_name(c)).collect();
        let mut capabilities = Capabilities::new(&names_of_caps);
        println!("\nPHASE {} principal={} caps={:?} capability_sids={:?} lease={}", phase.name, phase.principal, phase.caps, capabilities.text, phase.lease);
        assert!(!phase.lease || phase.caps.iter().any(|c| c == "internetClient"), "A lease is only meaningful with internetClient");
        let mut guard = match phase.principal.as_str() {
            "cap" => { assert!(phase.caps.iter().any(|c| c == "custom"), "ACEs for a capability the token does not hold prove nothing"); Some(AclGuard::apply(&custom_sid, &phase.aces)) }
            _ => { assert!(phase.aces.is_empty(), "A phase without a principal cannot grant"); None }
        };
        for run in &phase.runs {
            number += 1;
            let work = root.join(format!("work-{number}"));
            fs::create_dir(&work).unwrap();
            for (name, body) in [("m-empty.manifest", MANIFEST_EMPTY), ("m-settings.manifest", MANIFEST_SETTINGS), ("m-comctl.manifest", MANIFEST_COMCTL),
                ("netprobe.js", NODE_PROBE), ("pyprobe.py", PYTHON_PROBE)] { fs::write(work.join(name), body).unwrap(); }
            for (name, body) in &phase.puts {
                let path = work.join(name);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, body).unwrap();
            }
            let expand = |text: &str| text.replace("%WORK%", &work.to_string_lossy());
            let (label, target, args, mut folders): (String, PathBuf, Vec<String>, Vec<PathBuf>) = match run {
                Run::Child(sections) => (format!("child[{sections}]"), test_program.clone(), ["--exact", "runtime_child", "--nocapture"].map(str::to_owned).to_vec(),
                    phase.grants.iter().filter_map(|variable| program(variable)).map(|path| path.parent().unwrap().to_owned()).collect()),
                Run::Program { variable, args, copy, prime } => {
                    let Some(path) = program(variable) else { println!("RUN phase={} {variable}: SKIPPED, program not available", phase.name); let _ = fs::remove_dir_all(&work); continue };
                    let label = format!("{}{variable} {}", match (*copy, *prime) { (true, true) => "primed-copy ", (true, false) => "copy ", (false, true) => "primed ", _ => "" }, args.join(" "));
                    if *copy {
                        let folder = work.join("copy");
                        fs::create_dir(&folder).unwrap();
                        let target = folder.join(path.file_name().unwrap());
                        fs::copy(&path, &target).unwrap();
                        let mut libraries = vec![];
                        for entry in fs::read_dir(path.parent().unwrap()).unwrap().flatten() {
                            if entry.path().extension().is_some_and(|e| e.eq_ignore_ascii_case("dll")) { let to = folder.join(entry.file_name()); fs::copy(entry.path(), &to).unwrap(); libraries.push(to); }
                        }
                        if *prime {
                            println!("PRIME unsandboxed CreateActCtx manifest 1 of {}: error={}", target.display(), activation_context(&target.to_string_lossy(), 1));
                            for library in &libraries { println!("PRIME unsandboxed CreateActCtx manifest 2 of {}: error={}", library.display(), activation_context(&library.to_string_lossy(), 2)); }
                        }
                        (label, target, args.iter().map(|a| expand(a)).collect(), vec![])
                    } else {
                        if *prime {
                            println!("PRIME unsandboxed CreateActCtx manifest 1 of {}: error={}", path.display(), activation_context(&path.to_string_lossy(), 1));
                            for entry in fs::read_dir(path.parent().unwrap()).unwrap().flatten().filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("dll"))) {
                                println!("PRIME unsandboxed CreateActCtx manifest 2 of {}: error={}", entry.path().display(), activation_context(&entry.path().to_string_lossy(), 2));
                            }
                        }
                        (label, path.clone(), args.iter().map(|a| expand(a)).collect(), vec![path.parent().unwrap().to_owned()])
                    }
                }
            };
            let python_library = phase.pymain.as_deref().and_then(program).and_then(|path| {
                let folder = path.parent().unwrap().to_owned();
                let library = fs::read_dir(&folder).ok()?.flatten().map(|e| e.path()).find(|p| {
                    let name = p.file_name().unwrap().to_string_lossy().to_ascii_lowercase();
                    name.starts_with("python3") && name.ends_with(".dll") && name.len() > "python3.dll".len()
                })?;
                Some((folder, library))
            });
            if let (Run::Child(_), Some((folder, _))) = (run, &python_library) { if !folders.contains(folder) { folders.push(folder.clone()); } }
            let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
            let second = TcpListener::bind("127.0.0.1:0").unwrap();
            let other = TcpListener::bind("127.0.0.2:0").unwrap();
            let ipv6 = TcpListener::bind("[::1]:0").unwrap();
            let lan_tcp = TcpListener::bind((lan, 0)).unwrap();
            // Live receivers plus an unsandboxed positive connection rule out unavailable services.
            for receiver in [&proxy, &second, &other, &ipv6, &lan_tcp] {
                let control = std::net::TcpStream::connect_timeout(&receiver.local_addr().unwrap(), Duration::from_secs(2)).unwrap();
                drop(receiver.accept().unwrap());
                drop(control);
                receiver.set_nonblocking(true).unwrap();
            }
            let receivers: Vec<UdpSocket> = [std::net::IpAddr::from([127, 0, 0, 1]), lan].into_iter().map(|address| UdpSocket::bind((address, 0)).unwrap()).collect();
            let mut datagram = [0u8; 64];
            for receiver in &receivers {
                receiver.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                UdpSocket::bind("0.0.0.0:0").unwrap().send_to(b"control", receiver.local_addr().unwrap()).unwrap();
                assert_eq!(receiver.recv(&mut datagram).unwrap(), 7, "The datagram receiver on {} does not work", receiver.local_addr().unwrap());
                receiver.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
            }
            let port = proxy.local_addr().unwrap().port();
            let mut grants = vec![
                Grant { path: work.to_string_lossy().into_owned(), write: true, folder: true, blocked: false },
                Grant { path: test_program.to_string_lossy().into_owned(), write: false, folder: false, blocked: false },
            ];
            for folder in &folders { grants.push(Grant { path: folder.to_string_lossy().into_owned(), write: false, folder: true, blocked: false }); }
            let started = std::time::Instant::now();
            let mut sandbox = match Sandbox::new(&grants, &work) {
                Ok(sandbox) => sandbox,
                Err(error) => { println!("RUN phase={} run={label}: sandbox preparation failed: {error:?}", phase.name); let _ = fs::remove_dir_all(&work); continue }
            };
            let package = sandbox.package_sid().unwrap();
            let native = SECURITY_CAPABILITIES { AppContainerSid: sandbox.security().capabilities.AppContainerSid,
                Capabilities: if capabilities.entries.is_empty() { ptr::null_mut() } else { capabilities.entries.as_mut_ptr() },
                CapabilityCount: capabilities.entries.len() as u32, Reserved: 0 };
            let security = miniclaw_server::windows_process::SecurityContext { capabilities: &native, low_privilege: true };
            let mut env: Vec<_> = sandbox.environment().into_iter().map(|(k, v)| (k.into_string().unwrap(), v.into_string().unwrap())).collect();
            env.extend([("MC_RT_PORTS".into(), format!("{port}:{}:{}:{}:{}:{}", second.local_addr().unwrap().port(), other.local_addr().unwrap().port(),
                    ipv6.local_addr().unwrap().port(), lan, lan_tcp.local_addr().unwrap().port())),
                ("MC_RT_UDP".into(), receivers.iter().map(|r| r.local_addr().unwrap().to_string()).chain(Some("1.1.1.1:53".to_owned())).collect::<Vec<_>>().join(",")),
                ("MC_RT_NAMES".into(), names.join(",")),
                ("MC_PRIVATE_KEY".into(), private_name.clone()), ("MC_INHERITED_KEY".into(), inherited_name.clone()),
                ("MC_PRIVATE_FILE".into(), outside.to_string_lossy().into_owned()),
                ("MC_PROTECTED_FILE".into(), std::env::var("MINICLAW_NETWORK_CONFIG").unwrap_or_default())]);
            if let Run::Child(sections) = run {
                env.extend([("MC_RT_CHILD".into(), if sections.is_empty() { "all".to_owned() } else { sections.clone() }),
                    ("MC_RT_PROBES".into(), probes.join(";")), ("MC_RT_FILES".into(), files.join(";")),
                    ("MC_RT_ACTCTX".into(), folders.iter().flat_map(|folder| ["MC_RT_NODE", "MC_RT_PYTHON_ARM64", "MC_RT_NODE_X64", "MC_RT_PYTHON_X64"].into_iter()
                        .filter_map(program).filter(move |path| path.parent() == Some(folder.as_path()))).map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join(";"))]);
                if let Some((folder, library)) = &python_library {
                    env.extend([("MC_RT_PYDLL".into(), library.to_string_lossy().into_owned()), ("PYTHONHOME".into(), folder.to_string_lossy().into_owned()),
                        ("PYTHONDONTWRITEBYTECODE".into(), "1".into())]);
                }
            }
            env.extend(phase.env.iter().map(|(name, value)| (name.clone(), expand(value))));
            let pending = match miniclaw_server::windows_process::pending_stdio_restricted(&target, &args, &work, &env, &security) {
                Ok(pending) => pending,
                Err(error) => {
                    println!("RUN phase={} run={label}: CreateProcess failed: {error:?} (os error {:?})", phase.name, error.raw_os_error());
                    drop(sandbox);
                    miniclaw_server::windows_sandbox::drain_cleanup(Duration::from_secs(60)).unwrap();
                    let _ = fs::remove_dir_all(&work);
                    continue
                }
            };
            sandbox.bind_process(&pending.server.process).unwrap();
            let pids = job_pids(pending.server.process.job_descriptor().1);
            println!("RUN phase={} run={label} PID={} package={package} prepared_ms={}", phase.name, pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(","), started.elapsed().as_millis());
            let tree = pending.server.process.tree_guard();
            let mut lease = if phase.lease { Some(Lease::install(&package, port).unwrap()) } else { None };
            let result = tokio::time::timeout(Duration::from_secs(60), miniclaw_server::windows_process::capture_stdio(pending.resume().unwrap())).await;
            tree.terminate_and_wait(Duration::from_secs(10)).unwrap();
            if let Some(lease) = lease.as_mut() { lease.revoke().unwrap(); }
            match result {
                Ok(Ok((output, errors, code))) => {
                    let clip = |bytes: &[u8]| { let text = String::from_utf8_lossy(bytes).into_owned(); if text.len() > 24_000 { format!("{}…[clipped]", text.chars().take(24_000).collect::<String>()) } else { text } };
                    println!("EXIT phase={} run={label} code={code} (0x{code:08X})\n--- stdout\n{}\n--- stderr\n{}\n--- end", phase.name, clip(&output), clip(&errors));
                }
                Ok(Err(error)) => println!("EXIT phase={} run={label} capture failed: {error:?}", phase.name),
                Err(_) => println!("EXIT phase={} run={label} TIMEOUT after 60 s", phase.name),
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
            let reached = proxy.accept().is_ok();
            let mut leaked: Vec<String> = [("second loopback port", &second), ("127.0.0.2", &other), ("::1", &ipv6), ("lan tcp", &lan_tcp)].iter()
                .filter(|(_, receiver)| receiver.accept().is_ok()).map(|(name, _)| (*name).to_owned()).collect();
            if reached && !phase.lease { leaked.push("loopback listener without a lease".into()); }
            for receiver in &receivers { if receiver.recv(&mut datagram).is_ok() { leaked.push(format!("udp {}", receiver.local_addr().unwrap())); } }
            println!("PARENT phase={} run={label} proxy_accepted={reached} unapproved_receivers_reached={leaked:?}{}", phase.name, if leaked.is_empty() { "" } else { " LEAK" });
            assert_eq!(fs::read_to_string(&outside).unwrap(), "synthetic-file-secret", "The outside-grant file was changed");
            drop(sandbox);
            miniclaw_server::windows_sandbox::drain_cleanup(Duration::from_secs(60)).unwrap();
            let _ = fs::remove_dir_all(&work);
        }
        if let Some(guard) = guard.as_mut() { guard.restore(); }
    }
    fs::remove_dir_all(root).unwrap();
}

// ---------------------------------------------------------------------------------------------
// Capability exposure survey (parent, elevated, read-only): which objects name a capability SID.

/// Returns the DACL ACEs in SDDL form that name one of the SIDs.
fn aces_naming(dacl: &str, sids: &[String]) -> Vec<String> {
    dacl.split('(').skip(1).filter_map(|part| part.split(')').next()).filter(|ace| sids.iter().any(|sid| ace.ends_with(sid.as_str()))).map(str::to_owned).collect()
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool { haystack.windows(needle.len()).any(|window| window == needle) }

struct Survey { sids: Vec<Vec<u8>>, text: Vec<String>, visited: u64, unreadable: u64, hits: u64, printed: u64, files: u64 }
impl Survey {
    /// Walks keys below an open key. Only descriptors are read, never a value.
    fn keys(&mut self, key: HKEY, path: &str, parent_hit: bool, depth: usize) {
        self.visited += 1;
        let hit = match descriptor(key, DACL_SECURITY_INFORMATION) {
            Ok(bytes) => {
                let hit = self.sids.iter().any(|sid| contains_bytes(&bytes, sid));
                if hit {
                    self.hits += 1;
                    // A subtree that inherits the entry is reported once, at its top.
                    if !parent_hit && self.printed < 600 { self.printed += 1; println!("SURVEY KEY {path}: {}", aces_naming(&sddl(&bytes, DACL_SECURITY_INFORMATION), &self.text).join(" ")); }
                }
                hit
            }
            Err(_) => { self.unreadable += 1; false }
        };
        if depth > 48 { return; }
        let mut names = vec![];
        for index in 0.. {
            let mut name = [0u16; 256];
            let mut length = name.len() as u32;
            if unsafe { RegEnumKeyExW(key, index, name.as_mut_ptr(), &mut length, ptr::null(), ptr::null_mut(), ptr::null_mut(), ptr::null_mut()) } != 0 { break; }
            names.push(String::from_utf16_lossy(&name[..length as usize]));
        }
        for name in names {
            // The numbered control sets repeat CurrentControlSet.
            if depth == 1 && path.ends_with("SYSTEM") && name.to_ascii_lowercase().starts_with("controlset") { continue; }
            let mut child = ptr::null_mut();
            if unsafe { RegOpenKeyExW(key, wide(&name).as_ptr(), 0, KEY_ENUMERATE_SUB_KEYS | 0x0002_0000, &mut child) } != 0 { self.unreadable += 1; continue; }
            self.keys(child, &format!("{path}\\{name}"), hit, depth + 1);
            unsafe { RegCloseKey(child); }
        }
    }

    /// Walks a folder to a fixed depth without following reparse points. Only descriptors are read.
    fn files(&mut self, path: &Path, parent_hit: bool, depth: usize) {
        use std::os::windows::fs::MetadataExt;
        if self.files > 400_000 { return; }
        self.files += 1;
        self.visited += 1;
        let mut buffer = vec![0u64; 2048];
        let mut needed = 0u32;
        let name = wide(&path.to_string_lossy());
        let ok = unsafe { GetFileSecurityW(name.as_ptr(), DACL_SECURITY_INFORMATION, buffer.as_mut_ptr().cast(), (buffer.len() * 8) as u32, &mut needed) };
        let hit = if ok == 0 { self.unreadable += 1; false } else {
            let bytes = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), (needed as usize).min(buffer.len() * 8)) };
            let hit = self.sids.iter().any(|sid| contains_bytes(bytes, sid));
            if hit {
                self.hits += 1;
                if !parent_hit && self.printed < 600 { self.printed += 1; println!("SURVEY FILE {}: {}", path.display(), aces_naming(&sddl(bytes, DACL_SECURITY_INFORMATION), &self.text).join(" ")); }
            }
            hit
        };
        if depth == 0 { return; }
        let Ok(entries) = fs::read_dir(path) else { return };
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else { self.unreadable += 1; continue };
            // FILE_ATTRIBUTE_REPARSE_POINT.
            if metadata.file_attributes() & 0x400 != 0 { continue; }
            if metadata.is_dir() { self.files(&entry.path(), hit, depth - 1); } else { self.files(&entry.path(), hit, 0); }
        }
    }
}

#[test]
#[ignore = "read-only elevated VM survey of which registry keys and files name given capability SIDs"]
/// Lists, per capability in MC_RT_SURVEY_CAPS, the topmost keys and files whose DACL names its SID.
fn capability_acl_survey() {
    let names: Vec<String> = std::env::var("MC_RT_SURVEY_CAPS").unwrap_or_else(|_| "lpacCryptoServices".into()).split(',').map(str::to_owned).collect();
    let user = user_sid();
    for name in names {
        let capabilities = Capabilities::new(&[capability_name(&name)]);
        println!("\nSURVEY capability {name} sids={:?}", capabilities.text);
        let mut survey = Survey { sids: capabilities.binary(), text: capabilities.text.clone(), visited: 0, unreadable: 0, hits: 0, printed: 0, files: 0 };
        let started = std::time::Instant::now();
        for (label, hive, path) in [("HKLM", HKEY_LOCAL_MACHINE, String::new()), ("HKCU", HKEY_USERS, user.clone()), ("HKCU classes", HKEY_USERS, format!("{user}_Classes")), ("HKU .DEFAULT", HKEY_USERS, ".DEFAULT".to_owned())] {
            let mut key = ptr::null_mut();
            let status = unsafe { RegOpenKeyExW(hive, wide(&path).as_ptr(), 0, KEY_ENUMERATE_SUB_KEYS | 0x0002_0000, &mut key) };
            if status != 0 { println!("SURVEY {label}: cannot open ({status})"); continue; }
            let (visited, hits) = (survey.visited, survey.hits);
            survey.keys(key, label, false, 0);
            unsafe { RegCloseKey(key); }
            println!("SURVEY {name} {label}: keys visited={} naming the SID={} (topmost printed above)", survey.visited - visited, survey.hits - hits);
        }
        println!("SURVEY {name} registry total: visited={} unreadable={} hits={} seconds={}", survey.visited, survey.unreadable, survey.hits, started.elapsed().as_secs());
        let profile = PathBuf::from(std::env::var_os("USERPROFILE").unwrap());
        let (visited, hits) = (survey.visited, survey.hits);
        for (folder, depth) in [(PathBuf::from(r"C:\"), 1), (PathBuf::from(r"C:\Windows"), 2), (PathBuf::from(r"C:\Windows\System32"), 2), (PathBuf::from(r"C:\ProgramData"), 4),
            (PathBuf::from(r"C:\Program Files"), 2), (PathBuf::from(r"C:\Users"), 1), (profile.clone(), 2), (profile.join(r"AppData\Roaming\Microsoft"), 3), (profile.join(r"AppData\Local\Microsoft"), 2),
            (profile.join(r"AppData\LocalLow"), 2)] {
            survey.files(&folder, false, depth);
        }
        println!("SURVEY {name} files: visited={} naming the SID={} unreadable so far={} seconds={}", survey.visited - visited, survey.hits - hits, survey.unreadable, started.elapsed().as_secs());
    }
}

// ---------------------------------------------------------------------------------------------
// Child (runs inside the LPAC). Reads no real values: only statuses and synthetic sentinels.

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
#[link(name = "advapi32")]
unsafe extern "system" {
    fn SystemFunction036(buffer: *mut u8, length: u32) -> u8;
    fn CryptAcquireContextW(provider: *mut usize, container: *const u16, name: *const u16, kind: u32, flags: u32) -> i32;
    fn CryptGenRandom(provider: usize, length: u32, buffer: *mut u8) -> i32;
    fn CryptReleaseContext(provider: usize, flags: u32) -> i32;
    fn CryptGetDefaultProviderW(kind: u32, reserved: *mut u32, flags: u32, name: *mut u16, length: *mut u32) -> i32;
    fn CryptEnumProvidersW(index: u32, reserved: *mut u32, flags: u32, kind: *mut u32, name: *mut u16, length: *mut u32) -> i32;
}
#[link(name = "bcrypt")]
unsafe extern "system" {
    fn BCryptGenRandom(algorithm: *mut c_void, buffer: *mut u8, length: u32, flags: u32) -> i32;
}
#[repr(C)]
struct ActivationContext { size: u32, flags: u32, source: *const u16, architecture: u16, language: u16, directory: *const u16, resource: *const u16, application: *const u16, module: *mut c_void }
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateActCtxW(context: *const ActivationContext) -> HANDLE;
    fn ReleaseActCtx(context: HANDLE);
}

/// Opens an absolute NT registry path; returns the status and, on success, the granted access mask.
fn nt_open(path: &str, access: u32) -> (i32, u32) {
    let text: Vec<u16> = path.encode_utf16().collect();
    let name = UnicodeString { length: (text.len() * 2) as u16, maximum: (text.len() * 2) as u16, buffer: text.as_ptr() };
    // OBJ_CASE_INSENSITIVE.
    let attributes = ObjectAttributes { length: std::mem::size_of::<ObjectAttributes>() as u32, root: ptr::null_mut(), name: &name,
        attributes: 0x40, descriptor: ptr::null(), quality: ptr::null() };
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
            let (status, granted) = nt_open(nt, access);
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

/// Opens a file or folder three ways and returns the Win32 error of each (0 is success).
fn file_open(path: &str) -> [u32; 3] {
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING};
    // GENERIC_READ; FILE_READ_ATTRIBUTES; FILE_EXECUTE | SYNCHRONIZE. FILE_FLAG_BACKUP_SEMANTICS opens folders.
    [0x8000_0000u32, 0x80, 0x0010_0020].map(|access| unsafe {
        let handle = CreateFileW(wide(path).as_ptr(), access, 7, ptr::null(), OPEN_EXISTING, 0x0200_0000, ptr::null_mut());
        if handle == INVALID_HANDLE_VALUE { GetLastError() } else { CloseHandle(handle); 0 }
    })
}

/// Creates an activation context from a manifest file (resource 0) or an image's embedded manifest
/// resource (1 for a program, 2 for a library); returns the Win32 error.
fn activation_context(source: &str, resource: usize) -> u32 {
    let source = wide(source);
    // ACTCTX_FLAG_RESOURCE_NAME_VALID.
    let context = ActivationContext { size: std::mem::size_of::<ActivationContext>() as u32, flags: if resource != 0 { 0x8 } else { 0 }, source: source.as_ptr(),
        architecture: 0, language: 0, directory: ptr::null(), resource: resource as *const u16, application: ptr::null(), module: ptr::null_mut() };
    unsafe {
        let handle = CreateActCtxW(&context);
        if handle == INVALID_HANDLE_VALUE { GetLastError() } else { ReleaseActCtx(handle); 0 }
    }
}

fn section_token() {
    let container = token_information(TokenIsAppContainer)[0] as u32;
    let groups = token_information(TokenCapabilities);
    let capabilities: Vec<String> = unsafe {
        let groups = groups.as_ptr().cast::<TOKEN_GROUPS>();
        (0..(*groups).GroupCount as usize).map(|i| sid_text((*(*groups).Groups.as_ptr().add(i)).Sid)).collect()
    };
    println!("TOKEN appcontainer={container} capabilities={capabilities:?}");
}

fn section_registry() {
    for probe in std::env::var("MC_RT_PROBES").unwrap_or_default().split(';').filter(|p| !p.is_empty()) {
        let probe = if probe == "." { "" } else { probe };
        let nt = if probe.is_empty() { r"\Registry\Machine".to_owned() } else { format!(r"\Registry\Machine\{probe}") };
        let (read, _) = nt_open(&nt, KEY_READ);
        let (query, _) = nt_open(&nt, KEY_QUERY_VALUE);
        let (maximum, granted) = nt_open(&nt, MAXIMUM_ALLOWED_ACCESS);
        println!("PROBE HKLM\\{probe}: nt_read=0x{:x} nt_query=0x{:x} nt_max=0x{:x} granted=0x{granted:x} win32_read={} win32_query={}",
            read as u32, query as u32, maximum as u32, win32_open(HKEY_LOCAL_MACHINE, probe, KEY_READ), win32_open(HKEY_LOCAL_MACHINE, probe, KEY_QUERY_VALUE));
    }
}

/// Reproduces what OpenSSL's legacy Windows seeding calls, one step at a time.
fn section_crypto() {
    use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
    let mut bytes = [0u8; 32];
    // BCRYPT_USE_SYSTEM_PREFERRED_RNG.
    println!("CRYPTO BCryptGenRandom(system preferred)=0x{:x} RtlGenRandom={}", unsafe { BCryptGenRandom(ptr::null_mut(), bytes.as_mut_ptr(), 32, 2) } as u32, unsafe { SystemFunction036(bytes.as_mut_ptr(), 32) });
    for library in ["cryptsp.dll", "rsaenh.dll", "cryptbase.dll", "bcryptprimitives.dll"] {
        let handle = unsafe { LoadLibraryW(wide(library).as_ptr()) };
        println!("CRYPTO LoadLibrary {library}: {}", if handle.is_null() { format!("failed {}", unsafe { GetLastError() }) } else { "ok".into() });
    }
    // CRYPT_MACHINE_DEFAULT then CRYPT_USER_DEFAULT for PROV_RSA_FULL. Provider names are public constants.
    for (label, flags) in [("machine", 1u32), ("user", 2)] {
        let mut name = [0u16; 256];
        let mut length = (name.len() * 2) as u32;
        let ok = unsafe { CryptGetDefaultProviderW(1, ptr::null_mut(), flags, name.as_mut_ptr(), &mut length) };
        println!("CRYPTO CryptGetDefaultProvider({label}, PROV_RSA_FULL): {}", if ok != 0 { format!("ok {:?}", String::from_utf16_lossy(&name[..(length as usize / 2).saturating_sub(1)])) } else { format!("failed 0x{:x}", unsafe { GetLastError() }) });
    }
    let (mut kind, mut name, mut length) = (0u32, [0u16; 256], 512u32);
    let listed = unsafe { CryptEnumProvidersW(0, ptr::null_mut(), 0, &mut kind, name.as_mut_ptr(), &mut length) };
    println!("CRYPTO CryptEnumProviders(0): {}", if listed != 0 { "ok".to_owned() } else { format!("failed 0x{:x}", unsafe { GetLastError() }) });
    // CRYPT_VERIFYCONTEXT | CRYPT_SILENT, exactly as OpenSSL's rand_win.c and then with explicit providers.
    for (label, provider, kind) in [("default PROV_RSA_FULL", None, 1u32), ("Microsoft Base Cryptographic Provider v1.0", Some("Microsoft Base Cryptographic Provider v1.0"), 1),
        ("Microsoft Enhanced Cryptographic Provider v1.0", Some("Microsoft Enhanced Cryptographic Provider v1.0"), 1), ("Microsoft Strong Cryptographic Provider", Some("Microsoft Strong Cryptographic Provider"), 1),
        ("default PROV_RSA_AES", None, 24), ("Intel Hardware Cryptographic Service Provider (PROV_INTEL_SEC)", Some("Intel Hardware Cryptographic Service Provider"), 22)] {
        let name = provider.map(wide);
        let mut handle = 0usize;
        let ok = unsafe { CryptAcquireContextW(&mut handle, ptr::null(), name.as_ref().map_or(ptr::null(), |n| n.as_ptr()), kind, 0xF000_0040) };
        if ok == 0 { println!("CRYPTO CryptAcquireContext {label}: failed 0x{:x}", unsafe { GetLastError() }); continue; }
        let random = unsafe { CryptGenRandom(handle, 32, bytes.as_mut_ptr()) };
        println!("CRYPTO CryptAcquireContext {label}: ok CryptGenRandom={random}");
        unsafe { CryptReleaseContext(handle, 0); }
    }
}

/// Winsock start, socket creation per family, then every receiver the parent listens on.
fn section_winsock() {
    use std::net::{SocketAddr, TcpStream};
    use windows_sys::Win32::Networking::WinSock::{closesocket, socket, WSAGetLastError, WSAStartup, INVALID_SOCKET};
    let mut data = unsafe { std::mem::zeroed() };
    let startup = unsafe { WSAStartup(0x0202, &mut data) };
    println!("WSAStartup={startup}");
    if startup != 0 { println!("NET no Winsock: no socket and no name resolution is possible"); return; }
    let mut any = false;
    // AF_INET 2, AF_INET6 23; SOCK_STREAM 1, SOCK_DGRAM 2, SOCK_RAW 3.
    for (label, family, kind) in [("IPv4 stream", 2, 1), ("IPv4 datagram", 2, 2), ("IPv6 stream", 23, 1), ("IPv6 datagram", 23, 2), ("IPv4 raw", 2, 3)] {
        let handle = unsafe { socket(family, kind, 0) };
        if handle == INVALID_SOCKET { println!("NET socket {label}: failed {}", unsafe { WSAGetLastError() }); } else { any = true; println!("NET socket {label}: created"); unsafe { closesocket(handle); } }
    }
    let ports: Vec<String> = std::env::var("MC_RT_PORTS").unwrap().split(':').map(str::to_owned).collect();
    if any {
        let connect = |address: SocketAddr| TcpStream::connect_timeout(&address, Duration::from_secs(2));
        let port = |index: usize| ports[index].parse::<u16>().unwrap();
        let lan: std::net::IpAddr = ports[4].parse().unwrap();
        for (label, address) in [("proxy or first loopback port", SocketAddr::from(([127, 0, 0, 1], port(0)))), ("second loopback port", SocketAddr::from(([127, 0, 0, 1], port(1)))),
            ("127.0.0.2", SocketAddr::from(([127, 0, 0, 2], port(2)))), ("::1", SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port(3)))),
            ("this machine's network address", SocketAddr::new(lan, port(5))), ("public address", SocketAddr::from(([1, 1, 1, 1], 443)))] {
            match connect(address) {
                Ok(_) => println!("NET connect {label}: CONNECTED"),
                Err(error) => println!("NET connect {label}: refused ({:?})", error.raw_os_error()),
            }
        }
        for address in ["127.0.0.1:0", "0.0.0.0:0", "[::1]:0"] {
            match std::net::TcpListener::bind(address) {
                Ok(_) => println!("NET listen {address}: BOUND"),
                Err(error) => println!("NET listen {address}: refused ({:?})", error.raw_os_error()),
            }
        }
        // A datagram send can report success and still be dropped; the parent's receivers decide.
        for target in std::env::var("MC_RT_UDP").unwrap().split(',') {
            let sent = std::net::UdpSocket::bind("0.0.0.0:0").and_then(|socket| socket.send_to(b"miniclaw-udp-probe", target));
            println!("NET udp send to {target}: {:?}", sent.map_err(|error| error.raw_os_error()));
        }
    } else { println!("NET no socket of any family could be created"); }
    // Name resolution does not need a socket of this process: the DNS client service sends the query.
    for name in std::env::var("MC_RT_NAMES").unwrap().split(',') {
        let resolved = std::net::ToSocketAddrs::to_socket_addrs(&(name, 80));
        match resolved {
            Ok(list) => println!("NET resolve {name}: RESOLVED count={}", list.count()),
            Err(error) => println!("NET resolve {name}: failed ({:?})", error.raw_os_error()),
        }
    }
}

/// File opens the loader, the side-by-side resolver and the x64 emulator need, then activation contexts.
fn section_files() {
    for path in std::env::var("MC_RT_FILES").unwrap_or_default().split(';').filter(|p| !p.is_empty()) {
        let [read, attributes, execute] = file_open(path);
        let listing = if Path::new(path).is_dir() || read == 0 && fs::metadata(path).map_or(false, |m| m.is_dir()) { format!(" list={}", fs::read_dir(path).map_or_else(|e| format!("{:?}", e.raw_os_error()), |_| "0".into())) } else { String::new() };
        println!("FILE {path}: read={read} attributes={attributes} execute={execute}{listing}");
    }
}

fn section_sxs() {
    for name in ["m-empty.manifest", "m-settings.manifest", "m-comctl.manifest"] { println!("SXS CreateActCtx file {name}: error={}", activation_context(name, 0)); }
    for image in std::env::var("MC_RT_ACTCTX").unwrap_or_default().split(';').filter(|p| !p.is_empty()) {
        println!("SXS CreateActCtx embedded manifest 1 of {image}: error={}", activation_context(image, 1));
    }
    if let Ok(library) = std::env::var("MC_RT_PYDLL") { println!("SXS CreateActCtx embedded manifest 2 of {library}: error={}", activation_context(&library, 2)); }
    println!("SXS CreateActCtx embedded manifest 1 of this test program: error={}", activation_context(&std::env::current_exe().unwrap().to_string_lossy(), 1));
}

/// Loads the Python runtime library into this manifest-free process and runs a script through Py_Main.
fn section_python() {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW};
    let Ok(library) = std::env::var("MC_RT_PYDLL") else { println!("PYMAIN not requested"); return };
    // LOAD_WITH_ALTERED_SEARCH_PATH: dependencies beside the library.
    let module = unsafe { LoadLibraryExW(wide(&library).as_ptr(), ptr::null_mut(), 0x8) };
    if module.is_null() { println!("PYMAIN LoadLibrary {library}: failed {}", unsafe { GetLastError() }); return; }
    println!("PYMAIN LoadLibrary {library}: ok");
    let Some(version) = (unsafe { GetProcAddress(module, c"Py_GetVersion".as_ptr().cast()) }) else { println!("PYMAIN Py_GetVersion missing"); return };
    let version: unsafe extern "C" fn() -> *const std::ffi::c_char = unsafe { std::mem::transmute(version) };
    println!("PYMAIN Py_GetVersion: {:?}", unsafe { std::ffi::CStr::from_ptr(version()) });
    let Some(main) = (unsafe { GetProcAddress(module, c"Py_Main".as_ptr().cast()) }) else { println!("PYMAIN Py_Main missing"); return };
    let main: unsafe extern "C" fn(i32, *mut *mut u16) -> i32 = unsafe { std::mem::transmute(main) };
    let mut arguments: Vec<Vec<u16>> = ["python", "-I", "-B", "pyprobe.py"].iter().map(|a| wide(a)).collect();
    let mut pointers: Vec<*mut u16> = arguments.iter_mut().map(|a| a.as_mut_ptr()).chain(Some(ptr::null_mut())).collect();
    std::io::stdout().flush().unwrap();
    let code = unsafe { main(4, pointers.as_mut_ptr()) };
    println!("PYMAIN Py_Main exit={code}");
}

fn section_negative() {
    let user = user_sid();
    for (label, path) in [("HKCU root", ""), ("HKCU Environment", "Environment"), ("HKCU Software", "SOFTWARE"),
        ("HKCU Software Classes", r"SOFTWARE\Classes"), ("HKCU Software Microsoft", r"SOFTWARE\Microsoft"), ("HKCU Software Microsoft Cryptography", r"SOFTWARE\Microsoft\Cryptography"),
        ("HKCU Software Microsoft SystemCertificates", r"SOFTWARE\Microsoft\SystemCertificates"), ("HKCU Volatile Environment", "Volatile Environment")] {
        let nt = if path.is_empty() { format!(r"\Registry\User\{user}") } else { format!(r"\Registry\User\{user}\{path}") };
        negative(label, HKEY_CURRENT_USER, path, Some(nt));
    }
    negative("HKU .DEFAULT", HKEY_USERS, ".DEFAULT", Some(r"\Registry\User\.DEFAULT".into()));
    for (label, path) in [("HKLM SOFTWARE", "SOFTWARE"), ("HKLM SOFTWARE Microsoft", r"SOFTWARE\Microsoft"), ("HKLM SOFTWARE Windows NT CurrentVersion", r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"),
        ("HKLM SOFTWARE Windows CurrentVersion", r"SOFTWARE\Microsoft\Windows\CurrentVersion"), ("HKLM SOFTWARE Windows CurrentVersion Uninstall", r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
        ("HKLM SOFTWARE Classes", r"SOFTWARE\Classes"), ("HKLM SOFTWARE Policies", r"SOFTWARE\Policies"), ("HKLM SOFTWARE Microsoft Cryptography", r"SOFTWARE\Microsoft\Cryptography"),
        ("HKLM SOFTWARE Microsoft SystemCertificates", r"SOFTWARE\Microsoft\SystemCertificates"),
        ("HKLM SYSTEM", "SYSTEM"), ("HKLM SYSTEM Control", r"SYSTEM\CurrentControlSet\Control"),
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
    let mut listing = 5;
    if opened == 0 {
        let mut name = [0u16; 256];
        let mut length = name.len() as u32;
        listing = unsafe { RegEnumKeyExW(key, 0, name.as_mut_ptr(), &mut length, ptr::null(), ptr::null_mut(), ptr::null_mut(), ptr::null_mut()) };
        unsafe { RegCloseKey(key); }
    }
    if listing == 0 { println!("NEG HKLM root listing: EXPOSED"); } else { println!("NEG HKLM root listing: denied (open={opened} listing={listing})"); }
    for path in [r"SOFTWARE\Microsoft\Cryptography\MiniClawWriteProbe", r"SOFTWARE\MiniClawWriteProbe", r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\MiniClawWriteProbe"] {
        let mut created = ptr::null_mut();
        let create = unsafe { RegCreateKeyExW(HKEY_LOCAL_MACHINE, wide(path).as_ptr(), 0, ptr::null(), REG_OPTION_VOLATILE, KEY_WRITE, ptr::null(), &mut created, ptr::null_mut()) };
        if create == 0 { unsafe { RegCloseKey(created); } println!("NEG registry write {path}: EXPOSED"); } else { println!("NEG registry write {path}: denied ({create})"); }
    }
    let secret = std::env::var("MC_PRIVATE_FILE").unwrap();
    let (read, write) = (fs::read(&secret).is_ok(), fs::write(&secret, "changed").is_ok());
    if read || write { println!("NEG outside-grant file: EXPOSED read={read} write={write}"); } else { println!("NEG outside-grant file: denied"); }
    let protected = std::env::var("MC_PROTECTED_FILE").unwrap_or_default();
    if !protected.is_empty() { if fs::File::open(&protected).is_ok() { println!("NEG protected state: EXPOSED"); } else { println!("NEG protected state: denied"); } }
    // Folders a capability might open: only whether a listing starts is reported.
    let profile = std::env::var("MC_PRIVATE_FILE").ok().and_then(|path| path.split(r"\AppData\").next().map(str::to_owned)).unwrap_or_default();
    for folder in [profile.clone(), format!(r"{profile}\Documents"), format!(r"{profile}\AppData\Roaming"), format!(r"{profile}\AppData\Roaming\Microsoft\Crypto"),
        format!(r"{profile}\AppData\Roaming\Microsoft\SystemCertificates"), r"C:\Users".to_owned(), r"C:\ProgramData".to_owned(), r"C:\ProgramData\Microsoft\Crypto".to_owned()] {
        if folder.is_empty() || folder.starts_with('\\') { continue; }
        match fs::read_dir(&folder) { Ok(_) => println!("NEG folder listing {folder}: EXPOSED"), Err(error) => println!("NEG folder listing {folder}: denied ({:?})", error.raw_os_error()) }
    }
    fs::write("positive-control", "writable").unwrap();
    assert_eq!(fs::read_to_string("positive-control").unwrap(), "writable");
    println!("NEG complete");
}

#[test]
/// Reproduces, inside the sandbox token, the calls the runtimes make while starting.
fn runtime_child() {
    let Ok(sections) = std::env::var("MC_RT_CHILD") else { return };
    println!("CHILD PID {}", std::process::id());
    let wanted = |name: &str| sections == "all" || sections.split(',').any(|s| s == name);
    if wanted("token") { section_token(); }
    if wanted("registry") { section_registry(); }
    if wanted("crypto") { section_crypto(); }
    if wanted("winsock") { section_winsock(); }
    if wanted("files") { section_files(); }
    if wanted("sxs") { section_sxs(); }
    if wanted("negative") { section_negative(); }
    // Last: the interpreter finalizes process-wide state.
    if wanted("python") { section_python(); }
    println!("CHILD complete");
}
