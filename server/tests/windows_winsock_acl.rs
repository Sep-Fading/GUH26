#![cfg(windows)]
// Exercises the Winsock catalog entry manager on a synthetic key tree, and reads the real keys.
use miniclaw_server::{platform_fs, windows_winsock_acl::{self as acl, Manager, Presence}};
// Builds disposable registry trees and private state folders.
use std::{ffi::c_void, path::PathBuf, ptr::{null, null_mut}};
use windows_sys::Win32::{Foundation::*, Security::{Authorization::*, *}, System::Registry::*};

const WHOLE: u32 = OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
const PARAMETERS: &str = r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters";
const PROTOCOLS: &str = r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\Protocol_Catalog9\Catalog_Entries64";
const NAMESPACES: &str = r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\NameSpace_Catalog5\Catalog_Entries64";
const HELPERS: &str = r"SYSTEM\CurrentControlSet\Services\WinSock";

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtDeleteKey(key: HANDLE) -> i32;
}

fn wide(value: &str) -> Vec<u16> { value.encode_utf16().chain(Some(0)).collect() }

/// A synthetic copy of the Winsock key layout under one disposable volatile key.
struct Tree { base: String, state: PathBuf }

impl Tree {
    /// Creates the layout: three protocol entries, two namespace entries, the helper keys, and
    /// keys beside them that are not managed. One catalog key has a protected access list.
    fn new() -> Self {
        let base = format!(r"SOFTWARE\MiniClawWinsockAclTest-{}", uuid::Uuid::new_v4().simple());
        let state = std::env::temp_dir().join(format!("mc-winsock-acl-{}", uuid::Uuid::new_v4().simple()));
        platform_fs::private_dir(&state, false).unwrap();
        let tree = Self { base, state };
        tree.create("", None);
        for path in ["SYSTEM", r"SYSTEM\CurrentControlSet", r"SYSTEM\CurrentControlSet\Services", r"SYSTEM\CurrentControlSet\Services\WinSock2", PARAMETERS] {
            tree.create(path, None);
        }
        tree.create(&format!(r"{PARAMETERS}\Protocol_Catalog9"), Some("D:P(A;CI;KA;;;BA)(A;CI;KA;;;SY)(A;CI;KR;;;BU)"));
        tree.create(&format!(r"{PARAMETERS}\NameSpace_Catalog5"), None);
        tree.create(&format!(r"{PARAMETERS}\AppId_Catalog"), None);
        tree.create(PROTOCOLS, None);
        tree.create(NAMESPACES, None);
        for entry in 1..=3 { tree.create(&format!(r"{PROTOCOLS}\{entry:012}"), None); }
        for entry in 1..=2 { tree.create(&format!(r"{NAMESPACES}\{entry:012}"), None); }
        for path in ["", r"\Parameters", r"\Setup Migration", r"\Setup Migration\Providers", r"\Setup Migration\Providers\Tcpip", r"\Setup Migration\Providers\Tcpip6"] {
            tree.create(&format!("{HELPERS}{path}"), None);
        }
        tree
    }

    fn full(&self, path: &str) -> String {
        if path.is_empty() { self.base.clone() } else { format!(r"{}\{path}", self.base) }
    }

    /// Creates one volatile key, inheriting its access list or taking the given protected one.
    fn create(&self, path: &str, sddl: Option<&str>) {
        unsafe {
            let mut descriptor = null_mut();
            if let Some(text) = sddl {
                assert_ne!(ConvertStringSecurityDescriptorToSecurityDescriptorW(wide(text).as_ptr(), SDDL_REVISION_1, &mut descriptor, null_mut()), 0);
            }
            let attributes = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: descriptor, bInheritHandle: 0 };
            let mut key = null_mut();
            let mut disposition = 0;
            let status = RegCreateKeyExW(HKEY_LOCAL_MACHINE, wide(&self.full(path)).as_ptr(), 0, null(), REG_OPTION_VOLATILE, KEY_ALL_ACCESS,
                if sddl.is_some() { &attributes } else { null() }, &mut key, &mut disposition);
            if !descriptor.is_null() { LocalFree(descriptor); }
            assert_eq!(status, 0, "create {path}");
            assert_eq!(disposition, REG_CREATED_NEW_KEY, "{path} already existed");
            RegCloseKey(key);
        }
    }

    fn delete(&self, path: &str) {
        assert_eq!(unsafe { RegDeleteKeyW(HKEY_LOCAL_MACHINE, wide(&self.full(path)).as_ptr()) }, 0, "delete {path}");
    }

    /// Owner, group and access list of one key, as stored.
    fn descriptor(&self, path: &str) -> Vec<u8> { descriptor_of(&self.full(path)) }

    /// Every key of the tree, managed or not, with its descriptor.
    fn everything(&self) -> Vec<(String, Vec<u8>)> {
        let mut found = Vec::new();
        let mut pending = vec![String::new()];
        while let Some(path) = pending.pop() {
            for name in children(&self.full(&path)) {
                pending.push(if path.is_empty() { name } else { format!(r"{path}\{name}") });
            }
            found.push((path.clone(), self.descriptor(&path)));
        }
        found.sort();
        found
    }

    fn manager(&self, service: &str, state: &str) -> Manager {
        Manager::synthetic(&self.base, service, &self.state.join(state)).unwrap()
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, wide(&self.base).as_ptr()); }
        let _ = std::fs::remove_dir_all(&self.state);
    }
}

fn open(full: &str, access: u32) -> HKEY {
    let mut key = null_mut();
    assert_eq!(unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, wide(full).as_ptr(), 0, access, &mut key) }, 0, "open {full}");
    key
}

fn descriptor_of(full: &str) -> Vec<u8> {
    unsafe {
        let key = open(full, 0x0002_0000);
        let mut size = 0u32;
        assert_eq!(RegGetKeySecurity(key, WHOLE, null_mut(), &mut size), ERROR_INSUFFICIENT_BUFFER);
        let mut buffer = vec![0u32; (size as usize).div_ceil(4)];
        assert_eq!(RegGetKeySecurity(key, WHOLE, buffer.as_mut_ptr().cast(), &mut size), 0);
        RegCloseKey(key);
        std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), size as usize).to_vec()
    }
}

fn children(full: &str) -> Vec<String> {
    let key = open(full, KEY_ENUMERATE_SUB_KEYS);
    let mut names = Vec::new();
    for index in 0.. {
        let mut name = [0u16; 256];
        let mut length = name.len() as u32;
        if unsafe { RegEnumKeyExW(key, index, name.as_mut_ptr(), &mut length, null(), null_mut(), null_mut(), null_mut()) } != 0 { break; }
        names.push(String::from_utf16_lossy(&name[..length as usize]));
    }
    unsafe { RegCloseKey(key); }
    names
}

/// Text form of a stored descriptor.
fn sddl(descriptor: &[u8]) -> String {
    unsafe {
        let mut aligned = vec![0u32; descriptor.len().div_ceil(4)];
        std::ptr::copy_nonoverlapping(descriptor.as_ptr(), aligned.as_mut_ptr().cast::<u8>(), descriptor.len());
        let mut text = null_mut();
        assert_ne!(ConvertSecurityDescriptorToStringSecurityDescriptorW(aligned.as_mut_ptr().cast(), SDDL_REVISION_1, WHOLE, &mut text, null_mut()), 0);
        let mut length = 0;
        while *text.add(length) != 0 { length += 1; }
        let value = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
        LocalFree(text.cast());
        value
    }
}

/// Replaces one key's access list from text, the way another program or Windows would.
fn set_dacl(full: &str, text: &str) {
    unsafe {
        let mut descriptor = null_mut();
        assert_ne!(ConvertStringSecurityDescriptorToSecurityDescriptorW(wide(text).as_ptr(), SDDL_REVISION_1, &mut descriptor, null_mut()), 0, "{text}");
        // Keep the auto-inherited marker: the kernel drops it unless the request bit accompanies it.
        let control = descriptor.cast::<u8>().add(2).cast::<u16>();
        if control.read_unaligned() & 0x0400 != 0 { control.write_unaligned(control.read_unaligned() | 0x0100); }
        let key = open(full, 0x0006_0000);
        assert_eq!(RegSetKeySecurity(key, DACL_SECURITY_INFORMATION, descriptor), 0);
        RegCloseKey(key);
        LocalFree(descriptor);
    }
}

fn service() -> String { format!("MiniClawNetwork-{}", uuid::Uuid::new_v4().simple()) }

/// The managed paths of the synthetic tree as first created.
fn managed_paths() -> Vec<String> {
    let mut paths = vec![String::new(), PARAMETERS.to_owned(), format!(r"{PARAMETERS}\Protocol_Catalog9"), PROTOCOLS.to_owned()];
    paths.extend((1..=3).map(|entry| format!(r"{PROTOCOLS}\{entry:012}")));
    paths.extend([format!(r"{PARAMETERS}\NameSpace_Catalog5"), NAMESPACES.to_owned()]);
    paths.extend((1..=2).map(|entry| format!(r"{NAMESPACES}\{entry:012}")));
    paths.extend([r"\Parameters", r"\Setup Migration\Providers", r"\Setup Migration\Providers\Tcpip", r"\Setup Migration\Providers\Tcpip6"].map(|path| format!("{HELPERS}{path}")));
    paths
}

#[test]
#[ignore = "requires an elevated Windows token; changes only a disposable synthetic key tree"]
/// The first pass adds one exact entry per managed key and touches nothing else; a second pass changes nothing.
fn entries_land_on_the_managed_keys_only() {
    let tree = Tree::new();
    let before = tree.everything();
    let manager = tree.manager(&service(), "a");
    let report = manager.ensure().unwrap();
    let managed = managed_paths();
    assert_eq!(report.keys, managed.len());
    assert_eq!(report.added, managed);
    assert!(report.repaired.is_empty());
    let sid = manager.sid().to_owned();
    assert!(sid.starts_with("S-1-15-3-1024-"), "{sid}");
    for (path, original) in &before {
        let now = tree.descriptor(path);
        let text = sddl(&now);
        if let Some(position) = managed.iter().position(|known| known == path) {
            // SDDL prints the root's 0x1 (KEY_QUERY_VALUE) as CC and 0x20019 (KEY_READ) as KR.
            let rights = if position == 0 { "(A;;CC;;;" } else { "(A;;KR;;;" };
            assert_eq!(text.matches(&sid).count(), 1, "{path}: {text}");
            assert!(text.contains(&format!("{rights}{sid})")), "{path}: {text}");
            // Owner, group and every other entry are as before.
            assert_eq!(text.replace(&format!("{rights}{sid})"), ""), sddl(original), "{path}");
        } else {
            assert_eq!(&now, original, "Unmanaged key changed: {path}");
            assert!(!text.contains(&sid), "{path}");
        }
    }
    // The protected key stays protected, inherited keys keep the auto-inherited marker.
    assert!(sddl(&tree.descriptor(&format!(r"{PARAMETERS}\Protocol_Catalog9"))).contains("D:P("));
    assert!(sddl(&tree.descriptor(PARAMETERS)).contains("D:AI("));
    for state in manager.inspect().unwrap() {
        assert_eq!(state.presence, Presence::Exact, "{}", state.path);
    }
    let again = manager.ensure().unwrap();
    assert!(again.added.is_empty() && again.repaired.is_empty());
    assert_eq!(again.keys, managed.len());
    // The durable record holds each key's original descriptor.
    let record: serde_json::Value = serde_json::from_slice(&std::fs::read(tree.state.join("a").join("record.json")).unwrap()).unwrap();
    assert_eq!(record["sid"], sid);
    let keys = record["keys"].as_array().unwrap();
    assert_eq!(keys.len(), managed.len());
    for (path, original) in before.iter().filter(|(path, _)| managed.contains(path)) {
        let entry = keys.iter().find(|entry| entry["path"] == path.as_str()).unwrap();
        assert_eq!(entry["before"][0]["descriptor"], acl::hex(original), "{path}");
        assert_eq!(entry["before"][0]["sddl"], sddl(original), "{path}");
    }
}

#[test]
#[ignore = "requires an elevated Windows token; changes only a disposable synthetic key tree"]
/// A catalog reset deletes and recreates entry keys; the next pass restores the entry and reports it.
fn recreated_and_new_entries_are_repaired() {
    let tree = Tree::new();
    let manager = tree.manager(&service(), "a");
    manager.ensure().unwrap();
    let recreated = format!(r"{PROTOCOLS}\000000000002");
    tree.delete(&recreated);
    tree.create(&recreated, None);
    let added = format!(r"{NAMESPACES}\000000000003");
    tree.create(&added, None);
    // Windows also rewrites an access list in place: the entry is gone but the key is the same.
    let rewritten = format!("{HELPERS}\\Parameters");
    set_dacl(&tree.full(&rewritten), "D:P(A;CI;KA;;;BA)(A;CI;KA;;;SY)");
    let missing: Vec<String> = manager.inspect().unwrap().into_iter().filter(|state| state.presence == Presence::Absent).map(|state| state.path).collect();
    assert_eq!(missing, vec![recreated.clone(), added.clone(), rewritten.clone()]);
    let report = manager.ensure().unwrap();
    assert_eq!(report.repaired, vec![recreated, rewritten]);
    assert_eq!(report.added, vec![added]);
    assert!(manager.inspect().unwrap().iter().all(|state| state.presence == Presence::Exact));
    let log = std::fs::read_to_string(tree.state.join("a").join("events.log")).unwrap();
    assert_eq!(log.lines().filter(|line| line.contains(r#""event":"repaired""#)).count(), 2, "{log}");
    // An entry with the wrong rights or flags is replaced by the exact one.
    let wrong = format!(r"{PROTOCOLS}\000000000001");
    let text = sddl(&tree.descriptor(&wrong));
    let dacl = &text[text.find("D:").unwrap()..];
    set_dacl(&tree.full(&wrong), &dacl.replace(&format!("(A;;KR;;;{})", manager.sid()), &format!("(A;CI;KA;;;{})", manager.sid())));
    assert_eq!(manager.inspect().unwrap().iter().find(|state| state.path == wrong).unwrap().presence, Presence::Different);
    assert_eq!(manager.ensure().unwrap().repaired, vec![wrong.clone()]);
    assert_eq!(sddl(&tree.descriptor(&wrong)), text);
}

#[test]
#[ignore = "requires an elevated Windows token; changes only a disposable synthetic key tree"]
/// Removal leaves every key byte-identical to its descriptor from before the installation.
fn removal_restores_every_descriptor_exactly() {
    let tree = Tree::new();
    let before = tree.everything();
    let manager = tree.manager(&service(), "a");
    manager.ensure().unwrap();
    assert_ne!(tree.everything(), before);
    let removal = manager.remove().unwrap();
    let managed = managed_paths();
    assert_eq!(removal.stripped, managed);
    assert_eq!(removal.exact, managed);
    assert!(removal.differing.is_empty());
    let after = tree.everything();
    for ((path, original), (_, now)) in before.iter().zip(&after) {
        assert_eq!(sddl(now), sddl(original), "{path}");
        assert_eq!(now, original, "{path}: descriptor bytes differ");
    }
    assert!(!tree.state.join("a").join("record.json").exists());
    assert!(tree.state.join("a").join("removed.json").exists());
    // Removing again, or with no state at all, finds nothing and succeeds.
    assert!(manager.remove().unwrap().stripped.is_empty());
    assert!(tree.manager(&service(), "never-installed").remove().unwrap().stripped.is_empty());
    assert_eq!(tree.everything(), before);
    // A later installation starts a fresh record.
    assert_eq!(manager.ensure().unwrap().added, managed);
    assert_eq!(manager.remove().unwrap().exact, managed);
    assert_eq!(tree.everything(), before);
}

#[test]
#[ignore = "requires an elevated Windows token; changes only a disposable synthetic key tree"]
/// Two installations hold separate entries; removing one leaves the other complete.
fn two_installations_are_independent() {
    let tree = Tree::new();
    let before = tree.everything();
    let (first, second) = (tree.manager(&service(), "a"), tree.manager(&service(), "b"));
    assert_ne!(first.sid(), second.sid());
    first.ensure().unwrap();
    second.ensure().unwrap();
    let managed = managed_paths();
    assert_eq!(first.remove().unwrap().stripped, managed);
    for state in second.inspect().unwrap() { assert_eq!(state.presence, Presence::Exact, "{}", state.path); }
    for state in first.inspect().unwrap() { assert_eq!(state.presence, Presence::Absent, "{}", state.path); }
    for path in &managed {
        let text = sddl(&tree.descriptor(path));
        assert!(!text.contains(first.sid()) && text.matches(second.sid()).count() == 1, "{path}: {text}");
    }
    // The second installation recorded keys that already held the first one's entry, so its
    // record differs from the final state; it still removes only its own entry.
    let removal = second.remove().unwrap();
    assert_eq!(removal.stripped, managed);
    assert_eq!(removal.differing, managed);
    assert_eq!(tree.everything(), before, "The last removal restores the original descriptors");
    // Reverse order of removal.
    first.ensure().unwrap();
    second.ensure().unwrap();
    second.remove().unwrap();
    assert!(first.inspect().unwrap().iter().all(|state| state.presence == Presence::Exact));
    assert_eq!(first.remove().unwrap().exact, managed);
    assert_eq!(tree.everything(), before);
}

#[test]
#[ignore = "requires an elevated Windows token; changes only a disposable synthetic key tree"]
/// A key changed by something else after installation keeps that change; only our entry goes.
fn later_changes_by_others_survive_removal() {
    let tree = Tree::new();
    let manager = tree.manager(&service(), "a");
    manager.ensure().unwrap();
    let changed = format!(r"{NAMESPACES}\000000000001");
    let text = sddl(&tree.descriptor(&changed));
    let dacl = text[text.find("D:").unwrap()..].replacen('(', "(A;;KR;;;S-1-5-32-546)(", 1);
    set_dacl(&tree.full(&changed), &dacl);
    let deleted = format!(r"{PROTOCOLS}\000000000003");
    tree.delete(&deleted);
    let removal = manager.remove().unwrap();
    assert_eq!(removal.differing, vec![changed.clone()]);
    assert!(!removal.stripped.contains(&deleted) && !removal.exact.contains(&deleted));
    let now = sddl(&tree.descriptor(&changed));
    assert!(now.contains("(A;;KR;;;S-1-5-32-546)") || now.contains("(A;;KR;;;BG)"), "{now}");
    assert!(!now.contains(manager.sid()));
    assert_eq!(removal.exact.len(), managed_paths().len() - 2);
}

#[test]
#[ignore = "requires an elevated Windows token; changes only a disposable synthetic key tree"]
/// When a key cannot be repaired the pass fails with the key's name, and the refusal text carries it.
fn unrepairable_keys_fail_the_pass() {
    // The owner is limited to reading, so even an administrator who owns the key cannot rewrite it.
    let tree = Tree::new();
    let manager = tree.manager(&service(), "a");
    let locked = format!(r"{NAMESPACES}\000000000002");
    tree.delete(&locked);
    tree.create(&locked, Some("D:P(A;;KR;;;OW)(A;;0x30019;;;BA)(A;;KA;;;SY)"));
    let before = tree.descriptor(&locked);
    let error = manager.ensure().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied, "{error}");
    assert!(error.to_string().contains("000000000002"), "{error}");
    let refusal = acl::refusal(&error.to_string());
    assert!(refusal.contains("Network access for sandboxed commands is unavailable") && refusal.contains("000000000002"), "{refusal}");
    assert_eq!(tree.descriptor(&locked), before);
    // Once the key can be written the next pass completes.
    tree.delete(&locked);
    tree.create(&locked, None);
    manager.ensure().unwrap();
    assert!(manager.inspect().unwrap().iter().all(|state| state.presence == Presence::Exact));

    // A missing catalog, and a key of an unexpected name inside a catalog, refuse the whole pass.
    let tree = Tree::new();
    let manager = tree.manager(&service(), "a");
    tree.create(&format!(r"{PROTOCOLS}\Unexpected"), None);
    let before = tree.everything();
    assert!(manager.ensure().is_err());
    assert_eq!(tree.everything(), before, "A refused key list changes nothing");
    tree.delete(&format!(r"{PROTOCOLS}\Unexpected"));
    for entry in 1..=2 { tree.delete(&format!(r"{NAMESPACES}\{entry:012}")); }
    tree.delete(NAMESPACES);
    let error = manager.ensure().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound, "{error}");
    // A missing helper key is skipped.
    let tree = Tree::new();
    tree.delete(&format!(r"{HELPERS}\Setup Migration\Providers\Tcpip6"));
    assert_eq!(tree.manager(&service(), "a").ensure().unwrap().keys, managed_paths().len() - 1);
}

#[test]
#[ignore = "requires an elevated Windows token; changes only disposable synthetic keys"]
/// An entry name that is a registry link to another key is refused, and the other key is untouched.
fn link_to_another_key_is_refused() {
    let tree = Tree::new();
    let manager = tree.manager(&service(), "a");
    let outside = format!(r"SOFTWARE\MiniClawWinsockAclOutside-{}", uuid::Uuid::new_v4().simple());
    let entry = format!(r"{PROTOCOLS}\000000000002");
    tree.delete(&entry);
    unsafe {
        let mut target = null_mut();
        assert_eq!(RegCreateKeyExW(HKEY_LOCAL_MACHINE, wide(&outside).as_ptr(), 0, null(), REG_OPTION_VOLATILE, KEY_ALL_ACCESS, null(), &mut target, null_mut()), 0);
        RegCloseKey(target);
        let mut link = null_mut();
        assert_eq!(RegCreateKeyExW(HKEY_LOCAL_MACHINE, wide(&tree.full(&entry)).as_ptr(), 0, null(), REG_OPTION_VOLATILE | REG_OPTION_CREATE_LINK,
            KEY_ALL_ACCESS | KEY_CREATE_LINK, null(), &mut link, null_mut()), 0);
        let destination: Vec<u16> = format!(r"\Registry\Machine\{outside}").encode_utf16().collect();
        assert_eq!(RegSetValueExW(link, wide("SymbolicLinkValue").as_ptr(), 0, REG_LINK, destination.as_ptr().cast(), (destination.len() * 2) as u32), 0);
        let before = descriptor_of(&outside);
        let result = manager.ensure();
        let after = descriptor_of(&outside);
        // The link key itself is deleted through its own handle before anything is asserted.
        assert!(NtDeleteKey(link as *mut c_void) >= 0);
        RegCloseKey(link);
        assert_eq!(RegDeleteKeyW(HKEY_LOCAL_MACHINE, wide(&outside).as_ptr()), 0);
        let error = result.unwrap_err();
        assert!(error.to_string().contains("resolves to another key"), "{error}");
        assert_eq!(after, before, "The link target must not change");
        assert!(!sddl(&after).contains(manager.sid()));
    }
}

#[test]
#[ignore = "requires the installed network service; reads the real Winsock keys and changes nothing"]
/// The running service's entries are present, exact, explicit and not inheritable on the real keys,
/// and no key beside them names this installation's SID.
fn machine_keys_carry_exactly_the_service_entries() {
    let config: serde_json::Value = serde_json::from_slice(&std::fs::read(std::env::var_os("MINICLAW_NETWORK_CONFIG").unwrap()).unwrap()).unwrap();
    let state = std::env::temp_dir().join(format!("mc-winsock-inspect-{}", uuid::Uuid::new_v4().simple()));
    let manager = Manager::machine(config["service_name"].as_str().unwrap(), &state).unwrap();
    assert_eq!(manager.capability(), miniclaw_server::windows_network_broker::winsock_capability().unwrap());
    let states = manager.inspect().unwrap();
    println!("WINSOCK capability={} sid={} keys={}", manager.capability(), manager.sid(), states.len());
    assert!(states.len() >= 10);
    for state in &states {
        let text = sddl(&state.descriptor);
        println!("WINSOCK KEY HKLM\\{} rights={:#x} {:?}", state.path, state.rights, state.presence);
        assert_eq!(state.presence, Presence::Exact, "HKLM\\{}: {text}", state.path);
        let rights = if state.path.is_empty() { "CC" } else { "KR" };
        assert!(text.contains(&format!("(A;;{rights};;;{})", manager.sid())), "HKLM\\{}: {text}", state.path);
        assert_eq!(text.matches(manager.sid()).count(), 1);
    }
    assert_eq!(states[0].rights, 0x1);
    // Keys on the way to the catalog and beside it hold nothing for this installation.
    for path in ["SYSTEM", "SOFTWARE", r"SYSTEM\CurrentControlSet\Services", r"SYSTEM\CurrentControlSet\Services\WinSock2",
        r"SYSTEM\CurrentControlSet\Services\WinSock", r"SYSTEM\CurrentControlSet\Services\WinSock\Setup Migration",
        r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\AppId_Catalog", r"SYSTEM\CurrentControlSet\Services\Tcpip\Parameters",
        r"SYSTEM\CurrentControlSet\Services\AFD", r"SYSTEM\CurrentControlSet\Control"] {
        assert!(!sddl(&descriptor_of(path)).contains(manager.sid()), "HKLM\\{path} names the capability");
    }
    assert!(!state.exists(), "Inspection writes no state");
}
