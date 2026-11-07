//! Read access to the Winsock catalog for networked sandbox runs.
//!
//! A sandboxed run is an LPAC without `registryRead`, so it cannot open the registry keys Winsock
//! reads at startup. The network service grants one install-specific capability SID read access
//! to exactly those keys, keeps a record of each key's descriptor from before its first change,
//! repairs the entries when Windows recreates a key, and removes only its own entries on uninstall.
//! This file holds the decisions that need no Windows API; the native part applies them.

// Serializes the durable before-record.
use serde::{Deserialize, Serialize};
use std::io;

/// `KEY_QUERY_VALUE`: all the hive root needs, so advapi32 can open its predefined handle.
pub const ROOT_RIGHTS: u32 = 0x0001;
/// `KEY_READ`, granted on every other managed key.
pub const READ_RIGHTS: u32 = 0x0002_0019;

const CAPABILITY_PREFIX: &str = "miniclawWinsockRead.";
const SERVICE_PREFIX: &str = "MiniClawNetwork-";
const PARAMETERS: &str = r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters";
const CATALOGS: [&str; 2] = ["Protocol_Catalog9", "NameSpace_Catalog5"];
/// The catalog view 64-bit processes read, including x64 processes emulated on ARM64.
const VIEW: &str = "Catalog_Entries64";
const HELPERS: [&str; 4] = [
    r"SYSTEM\CurrentControlSet\Services\WinSock\Parameters",
    r"SYSTEM\CurrentControlSet\Services\WinSock\Setup Migration\Providers",
    r"SYSTEM\CurrentControlSet\Services\WinSock\Setup Migration\Providers\Tcpip",
    r"SYSTEM\CurrentControlSet\Services\WinSock\Setup Migration\Providers\Tcpip6",
];
/// More entries than this in one catalog is refused rather than granted.
pub const MOST_ENTRIES: usize = 256;
/// Snapshots kept per key: the first one and the most recent ones.
const MOST_SNAPSHOTS: usize = 8;

const ACCESS_ALLOWED: u8 = 0;
const ACCESS_DENIED: u8 = 1;
const INHERITED_ACE: u8 = 0x10;
const SE_DACL_PRESENT: u16 = 0x0004;
const SE_DACL_DEFAULTED: u16 = 0x0008;
const SE_DACL_AUTO_INHERIT_REQ: u16 = 0x0100;
const SE_DACL_AUTO_INHERITED: u16 = 0x0400;
const SE_DACL_PROTECTED: u16 = 0x1000;
const SE_SELF_RELATIVE: u16 = 0x8000;

/// Refuses malformed input with one error kind.
fn invalid<T>(message: &str) -> io::Result<T> {
    Err(io::Error::new(io::ErrorKind::InvalidData, message.to_owned()))
}

/// Accepts exactly 32 lowercase hexadecimal characters.
fn hex_id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Names this installation's capability from its service name, so the backend and the service
/// agree without a second stored value that could name another capability.
pub fn capability_name(service_name: &str) -> io::Result<String> {
    match service_name.strip_prefix(SERVICE_PREFIX) {
        Some(id) if hex_id(id) => Ok(format!("{CAPABILITY_PREFIX}{id}")),
        _ => invalid("Not a MiniClaw network service name"),
    }
}

/// Accepts exactly the shape [`capability_name`] produces, so nothing else can reach a token.
pub fn is_capability_name(name: &str) -> bool {
    name.strip_prefix(CAPABILITY_PREFIX).is_some_and(hex_id)
}

/// One registry key that receives this installation's read entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// Path below the hive root; empty for the root key itself.
    pub path: String,
    pub rights: u32,
    /// A missing required key is an error; a missing helper key is skipped.
    pub required: bool,
}

/// The two keys whose direct children are the per-machine catalog entries.
pub fn entry_parents() -> [String; 2] {
    CATALOGS.map(|catalog| format!(r"{PARAMETERS}\{catalog}\{VIEW}"))
}

/// Catalog entries are numbered keys of twelve digits; nothing else is ever granted.
fn entry_name(name: &str) -> bool {
    name.len() == 12 && name.bytes().all(|b| b.is_ascii_digit())
}

/// Returns the rights for a path inside the fixed list or the entry pattern, and nothing otherwise.
/// Every registry write passes through this check, including paths read back from the record.
pub fn managed(path: &str) -> Option<u32> {
    if path.is_empty() {
        return Some(ROOT_RIGHTS);
    }
    let fixed = std::iter::once(PARAMETERS.to_owned())
        .chain(CATALOGS.iter().map(|catalog| format!(r"{PARAMETERS}\{catalog}")))
        .chain(entry_parents())
        .chain(HELPERS.iter().map(|helper| (*helper).to_owned()));
    for known in fixed {
        if path.eq_ignore_ascii_case(&known) {
            return Some(READ_RIGHTS);
        }
    }
    for parent in entry_parents() {
        let Some(head) = path.get(..parent.len()) else { continue };
        if head.eq_ignore_ascii_case(&parent) {
            if let Some(name) = path[parent.len()..].strip_prefix('\\') {
                if entry_name(name) {
                    return Some(READ_RIGHTS);
                }
            }
        }
    }
    None
}

/// Builds the complete key list from the names found under each entry parent, in the order of
/// [`entry_parents`]. An unexpected name or an oversized catalog refuses the whole list.
pub fn targets(entries: &[Vec<String>; 2]) -> io::Result<Vec<Target>> {
    let key = |path: String, required| Target { rights: if path.is_empty() { ROOT_RIGHTS } else { READ_RIGHTS }, path, required };
    let mut list = vec![key(String::new(), true), key(PARAMETERS.to_owned(), true)];
    for (parent, names) in entry_parents().into_iter().zip(entries) {
        if names.len() > MOST_ENTRIES {
            return invalid("The Winsock catalog has more entries than MiniClaw grants");
        }
        let catalog = parent.rsplit_once('\\').map(|(catalog, _)| catalog.to_owned()).unwrap_or_default();
        list.push(key(catalog, true));
        list.push(key(parent.clone(), true));
        let mut seen = std::collections::BTreeSet::new();
        for name in names {
            if !entry_name(name) || !seen.insert(name.clone()) {
                return invalid("Unexpected key in the Winsock catalog");
            }
        }
        list.extend(seen.into_iter().map(|name| key(format!(r"{parent}\{name}"), true)));
    }
    list.extend(HELPERS.iter().map(|helper| key((*helper).to_owned(), false)));
    debug_assert!(list.iter().all(|target| managed(&target.path) == Some(target.rights)));
    Ok(list)
}

/// Accepts the synthetic test base and nothing else; production uses the empty base.
pub fn test_base(base: &str) -> bool {
    base.strip_prefix(r"SOFTWARE\MiniClawWinsockAclTest-").is_some_and(hex_id)
}

/// Checks the kernel's name of an opened key against the path that was asked for, so a link or
/// redirection cannot put an entry on another key. `CurrentControlSet` may resolve to a numbered
/// control set; every other component must match.
pub fn opened_name_matches(base: &str, path: &str, actual: &str) -> bool {
    let mut expected = vec!["", "REGISTRY", "MACHINE"];
    expected.extend(base.split('\\').filter(|part| !part.is_empty()));
    expected.extend(path.split('\\').filter(|part| !part.is_empty()));
    let actual: Vec<&str> = actual.split('\\').collect();
    expected.len() == actual.len()
        && expected.iter().zip(&actual).all(|(want, got)| {
            want.eq_ignore_ascii_case(got)
                || want.eq_ignore_ascii_case("CurrentControlSet")
                    && got.len() == 13
                    && got[..10].eq_ignore_ascii_case("ControlSet")
                    && got[10..].bytes().all(|b| b.is_ascii_digit())
        })
}

/// Accepts only a derived custom capability SID: `S-1-15-3-1024-` and eight hash words.
/// Package SIDs, well-known capabilities, users and groups can never be granted.
pub fn capability_sid(sid: &[u8]) -> bool {
    sid.len() == 48
        && sid[0] == 1
        && sid[1] == 10
        && sid[2..8] == [0, 0, 0, 0, 0, 15]
        && sid[8..12] == 3u32.to_le_bytes()
        && sid[12..16] == 1024u32.to_le_bytes()
}

/// Formats a binary SID for the record and for messages.
pub fn sid_text(sid: &[u8]) -> io::Result<String> {
    if sid.len() < 8 || sid.len() != 8 + 4 * sid[1] as usize {
        return invalid("Malformed SID");
    }
    let authority = sid[2..8].iter().fold(0u64, |value, byte| value << 8 | u64::from(*byte));
    let mut text = format!("S-{}-{authority}", sid[0]);
    for word in sid[8..].chunks_exact(4) {
        text.push_str(&format!("-{}", u32::from_le_bytes([word[0], word[1], word[2], word[3]])));
    }
    Ok(text)
}

/// The parts of a self-relative security descriptor this module reads.
pub struct Descriptor<'a> {
    pub control: u16,
    /// The whole access list, including any unused space Windows keeps at its end.
    pub dacl: &'a [u8],
}

/// Reads a little-endian word at a checked offset.
fn word(bytes: &[u8], at: usize) -> io::Result<u16> {
    match bytes.get(at..at + 2) {
        Some(pair) => Ok(u16::from_le_bytes([pair[0], pair[1]])),
        None => invalid("Truncated security descriptor"),
    }
}

/// Finds the access list of a self-relative descriptor. A descriptor without one is refused:
/// a key that grants everyone everything is never rewritten.
pub fn descriptor(bytes: &[u8]) -> io::Result<Descriptor<'_>> {
    if bytes.len() < 20 || bytes[0] != 1 {
        return invalid("Unsupported security descriptor");
    }
    let control = word(bytes, 2)?;
    let offset = u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]) as usize;
    if control & SE_SELF_RELATIVE == 0 || control & SE_DACL_PRESENT == 0 || offset < 20 {
        return invalid("The key has no access list to extend");
    }
    let size = word(bytes, offset + 2)? as usize;
    match bytes.get(offset..offset + size) {
        Some(dacl) if size >= 8 => Ok(Descriptor { control, dacl }),
        _ => invalid("Truncated access list"),
    }
}

/// One entry of an access list, by position.
struct Ace {
    start: usize,
    len: usize,
    kind: u8,
    flags: u8,
    mask: u32,
}

/// Lists the entries of an access list after checking every length against the list's own size.
fn aces(acl: &[u8]) -> io::Result<Vec<Ace>> {
    if acl.len() < 8 || word(acl, 2)? as usize != acl.len() {
        return invalid("Malformed access list");
    }
    let mut list = Vec::new();
    let mut at = 8;
    for _ in 0..word(acl, 4)? {
        let len = word(acl, at + 2)? as usize;
        if len < 8 || at + len > acl.len() {
            return invalid("Malformed access entry");
        }
        let mask = u32::from_le_bytes([acl[at + 4], acl[at + 5], acl[at + 6], acl[at + 7]]);
        list.push(Ace { start: at, len, kind: acl[at], flags: acl[at + 1], mask });
        at += len;
    }
    Ok(list)
}

/// True when a plain allow or deny entry names exactly this SID.
fn names(acl: &[u8], ace: &Ace, sid: &[u8]) -> bool {
    matches!(ace.kind, ACCESS_ALLOWED | ACCESS_DENIED) && acl.get(ace.start + 8..ace.start + 8 + sid.len()) == Some(sid)
        && ace.len == 8 + sid.len()
}

/// What an access list holds for one SID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    /// Exactly one explicit, non-inheritable allow entry with exactly the wanted rights.
    Exact,
    Absent,
    /// Some other entry for the SID: other rights, flags, a deny, an inherited copy or a duplicate.
    Different,
}

/// Classifies this SID's entries in an access list.
pub fn presence(acl: &[u8], sid: &[u8], rights: u32) -> io::Result<Presence> {
    let list = aces(acl)?;
    let ours: Vec<&Ace> = list.iter().filter(|ace| names(acl, ace, sid)).collect();
    Ok(match ours.as_slice() {
        [] => Presence::Absent,
        [ace] if ace.kind == ACCESS_ALLOWED && ace.flags == 0 && ace.mask == rights => Presence::Exact,
        _ => Presence::Different,
    })
}

/// Copies an access list with some entries left out and the header counts corrected.
fn rebuild(acl: &[u8], keep: impl Fn(&Ace) -> bool, insert: Option<(usize, &[u8])>) -> io::Result<Vec<u8>> {
    let list = aces(acl)?;
    let used = list.last().map_or(8, |ace| ace.start + ace.len);
    let mut out = acl[..8].to_vec();
    let mut count = 0u16;
    let position = insert.map(|(position, _)| position);
    for (index, ace) in list.iter().enumerate() {
        if position == Some(index) {
            out.extend_from_slice(insert.map(|(_, bytes)| bytes).unwrap_or_default());
            count += 1;
        }
        if keep(ace) {
            out.extend_from_slice(&acl[ace.start..ace.start + ace.len]);
            count += 1;
        }
    }
    if position == Some(list.len()) {
        out.extend_from_slice(insert.map(|(_, bytes)| bytes).unwrap_or_default());
        count += 1;
    }
    // Unused space at the end of the list is carried over, so removal restores the original bytes.
    out.extend_from_slice(&acl[used..]);
    let size = u16::try_from(out.len()).or_else(|_| invalid("The access list is full"))?;
    out[2..4].copy_from_slice(&size.to_le_bytes());
    out[4..6].copy_from_slice(&count.to_le_bytes());
    Ok(out)
}

/// Removes every explicit entry for this SID and nothing else.
pub fn without_sid(acl: &[u8], sid: &[u8]) -> io::Result<Vec<u8>> {
    if !capability_sid(sid) {
        return invalid("Only a custom capability SID is managed");
    }
    rebuild(acl, |ace| !(names(acl, ace, sid) && ace.flags & INHERITED_ACE == 0), None)
}

/// Returns the list with exactly one explicit, non-inheritable read entry for this SID, placed
/// after the other explicit entries and before inherited ones. Rights beyond `KEY_READ` are refused.
pub fn with_ace(acl: &[u8], sid: &[u8], rights: u32) -> io::Result<Vec<u8>> {
    if rights == 0 || rights & !READ_RIGHTS != 0 {
        return invalid("Only read rights are granted");
    }
    let clean = without_sid(acl, sid)?;
    let list = aces(&clean)?;
    let position = list.iter().position(|ace| ace.flags & INHERITED_ACE != 0).unwrap_or(list.len());
    let mut entry = vec![ACCESS_ALLOWED, 0];
    entry.extend_from_slice(&((8 + sid.len()) as u16).to_le_bytes());
    entry.extend_from_slice(&rights.to_le_bytes());
    entry.extend_from_slice(sid);
    rebuild(&clean, |_| true, Some((position, &entry)))
}

/// Builds the descriptor passed to Windows when replacing one key's access list. Owner, group and
/// audit list are absent, so they cannot change. Protection and inheritance markers are carried
/// over; the kernel keeps the auto-inherited marker only when the request bit accompanies it.
pub fn dacl_descriptor(control: u16, acl: &[u8]) -> Vec<u8> {
    let mut bits = SE_SELF_RELATIVE | SE_DACL_PRESENT | control & (SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED | SE_DACL_DEFAULTED);
    if control & SE_DACL_AUTO_INHERITED != 0 {
        bits |= SE_DACL_AUTO_INHERIT_REQ;
    }
    let mut out = vec![1, 0];
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(&[0; 12]);
    out.extend_from_slice(&20u32.to_le_bytes());
    out.extend_from_slice(acl);
    out
}

/// A key's whole descriptor (owner, group, access list) as it was before an entry was added.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    /// Self-relative descriptor bytes, hexadecimal.
    pub descriptor: String,
    /// The same descriptor as text, for a person reading the record.
    pub sddl: String,
    /// Seconds since 1970.
    pub recorded: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub path: String,
    pub before: Vec<Snapshot>,
}

/// The durable before-record of one installation. It is written before any key changes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub version: u32,
    pub capability: String,
    pub sid: String,
    pub keys: Vec<Entry>,
}

impl Record {
    /// Starts an empty record for one capability.
    pub fn new(capability: &str, sid: &str) -> Self {
        Self { version: 1, capability: capability.to_owned(), sid: sid.to_owned(), keys: Vec::new() }
    }

    /// Reads a stored record and refuses one written for another installation or with paths
    /// outside the managed list.
    pub fn parse(bytes: &[u8], capability: &str, sid: &str) -> io::Result<Self> {
        let record: Self = serde_json::from_slice(bytes)?;
        if record.version != 1 || record.capability != capability || record.sid != sid
            || record.keys.iter().any(|entry| managed(&entry.path).is_none() || entry.before.is_empty())
        {
            return invalid("The Winsock access record belongs to another installation or is damaged");
        }
        Ok(record)
    }

    /// Adds a snapshot for a key that is about to change. Returns whether the record changed and
    /// must be saved first. A key recreated by Windows gets a new snapshot; the first is kept.
    pub fn note(&mut self, path: &str, snapshot: Snapshot) -> bool {
        let position = self.keys.iter().position(|entry| entry.path.eq_ignore_ascii_case(path));
        let Some(position) = position else {
            self.keys.push(Entry { path: path.to_owned(), before: vec![snapshot] });
            return true;
        };
        let before = &mut self.keys[position].before;
        if before.last().is_some_and(|last| last.descriptor == snapshot.descriptor) {
            return false;
        }
        if before.len() >= MOST_SNAPSHOTS {
            before.remove(1);
        }
        before.push(snapshot);
        true
    }

    /// The most recent descriptor recorded for a key before this installation changed it.
    pub fn latest(&self, path: &str) -> Option<&Snapshot> {
        self.keys.iter().find(|entry| entry.path.eq_ignore_ascii_case(path)).and_then(|entry| entry.before.last())
    }

    /// Whether this installation ever changed the key.
    pub fn knows(&self, path: &str) -> bool {
        self.latest(path).is_some()
    }
}

/// Encodes bytes for the record.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The message a refused networked run carries to the user. The detail names the failing key or step.
pub fn refusal(detail: &str) -> String {
    let detail: String = detail.chars().filter(|c| !c.is_control()).take(240).collect();
    format!("Network access for sandboxed commands is unavailable: the MiniClaw network service could not confirm its read access to the Windows socket catalog ({detail}). The command was not started. Restart the service or repair the MiniClaw installation.")
}

/// What the page says about the service's access entries, read from its status file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AccessStatus {
    /// `healthy`, `repaired` (the last check put back entries Windows had removed) or `failed`; the
    /// gate reports `unknown` instead while the service is stopped (`windows_gate::access_report`).
    pub state: &'static str,
    /// Seconds since 1970 of the check that published this status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<u64>,
    /// Why the entries could not be confirmed, for a failed status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Reads the service's published status. Removed entries and a failed check are failures; a
/// check that repaired entries says so, with its time. Anything else is not a status.
pub fn access_status(value: &serde_json::Value) -> Option<AccessStatus> {
    let time = value["time"].as_u64();
    let clean = |text: &str| -> String { text.chars().filter(|c| !c.is_control()).take(240).collect() };
    if value["removed"] == true {
        return Some(AccessStatus { state: "failed", time, reason: Some("its entries were removed".into()) });
    }
    match value["ok"].as_bool()? {
        true if value["repaired"].as_array().is_some_and(|keys| !keys.is_empty()) => Some(AccessStatus { state: "repaired", time, reason: None }),
        true => Some(AccessStatus { state: "healthy", time, reason: None }),
        false => Some(AccessStatus { state: "failed", time, reason: Some(clean(value["error"].as_str().unwrap_or("no status"))) }),
    }
}

#[cfg(windows)]
#[path = "windows_winsock_acl_native.rs"]
mod native;
#[cfg(windows)]
pub use native::{KeyState, Manager, Removal, Report};
#[cfg(windows)]
pub(crate) use native::derive;

#[cfg(test)]
mod tests {
    use super::*;

    /// A capability SID with recognisable hash words.
    fn cap(seed: u32) -> Vec<u8> {
        let mut sid = vec![1, 10, 0, 0, 0, 0, 0, 15];
        for value in [3, 1024, seed, 2, 3, 4, 5, 6, 7, 8] {
            sid.extend_from_slice(&u32::to_le_bytes(value));
        }
        sid
    }

    /// A short SID such as SYSTEM (S-1-5-18) or Users (S-1-5-32-545).
    fn sid(authority: u8, parts: &[u32]) -> Vec<u8> {
        let mut sid = vec![1, parts.len() as u8, 0, 0, 0, 0, 0, authority];
        for part in parts {
            sid.extend_from_slice(&part.to_le_bytes());
        }
        sid
    }

    /// One access entry.
    fn ace(kind: u8, flags: u8, mask: u32, sid: &[u8]) -> Vec<u8> {
        let mut entry = vec![kind, flags];
        entry.extend_from_slice(&((8 + sid.len()) as u16).to_le_bytes());
        entry.extend_from_slice(&mask.to_le_bytes());
        entry.extend_from_slice(sid);
        entry
    }

    /// An access list with optional unused space at its end.
    fn acl(entries: &[Vec<u8>], slack: usize) -> Vec<u8> {
        let size = 8 + entries.iter().map(Vec::len).sum::<usize>() + slack;
        let mut list = vec![2, 0];
        list.extend_from_slice(&(size as u16).to_le_bytes());
        list.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        list.extend_from_slice(&[0, 0]);
        for entry in entries {
            list.extend_from_slice(entry);
        }
        list.extend(std::iter::repeat_n(0, slack));
        list
    }

    /// A typical catalog key: explicit administrators entry, then inherited entries.
    fn typical(slack: usize) -> Vec<u8> {
        acl(&[ace(0, 0, 0xf003f, &sid(5, &[32, 544])), ace(0, 0x12, 0x20019, &sid(5, &[32, 545])),
            ace(0, 0x12, 0xf003f, &sid(5, &[18])), ace(0, 0x1a, 0x1000_0000, &sid(3, &[0]))], slack)
    }

    #[test]
    fn capability_follows_the_service_name_only() {
        let id = "0123456789abcdef0123456789abcdef";
        assert_eq!(capability_name(&format!("MiniClawNetwork-{id}")).unwrap(), format!("miniclawWinsockRead.{id}"));
        let other = capability_name(&format!("MiniClawNetwork-{}", "f".repeat(32))).unwrap();
        assert_ne!(other, capability_name(&format!("MiniClawNetwork-{id}")).unwrap(), "Each installation has its own capability");
        for name in ["registryRead", "MiniClawNetwork-", "MiniClawNetwork-ABCDEF0123456789abcdef0123456789", "miniclawnetwork-0123456789abcdef0123456789abcdef",
            "MiniClawNetwork-0123456789abcdef0123456789abcdef0", "MiniClawNetwork-0123456789abcdef0123456789abcde\0", "internetClient", ""] {
            assert!(capability_name(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn key_list_is_the_fixed_keys_plus_every_entry() {
        let entries = [vec!["000000000002".to_owned(), "000000000001".to_owned()], vec!["000000000001".to_owned()]];
        let list = targets(&entries).unwrap();
        let paths: Vec<&str> = list.iter().map(|target| target.path.as_str()).collect();
        assert_eq!(paths, vec![
            "",
            r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters",
            r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\Protocol_Catalog9",
            r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\Protocol_Catalog9\Catalog_Entries64",
            r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\Protocol_Catalog9\Catalog_Entries64\000000000001",
            r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\Protocol_Catalog9\Catalog_Entries64\000000000002",
            r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\NameSpace_Catalog5",
            r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\NameSpace_Catalog5\Catalog_Entries64",
            r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\NameSpace_Catalog5\Catalog_Entries64\000000000001",
            r"SYSTEM\CurrentControlSet\Services\WinSock\Parameters",
            r"SYSTEM\CurrentControlSet\Services\WinSock\Setup Migration\Providers",
            r"SYSTEM\CurrentControlSet\Services\WinSock\Setup Migration\Providers\Tcpip",
            r"SYSTEM\CurrentControlSet\Services\WinSock\Setup Migration\Providers\Tcpip6",
        ]);
        // The root gets query-value only; everything else exactly KEY_READ.
        assert_eq!(list[0].rights, 0x1);
        assert!(list[1..].iter().all(|target| target.rights == 0x20019));
        // The experiment's machine: 14 protocol and 5 namespace entries make 29 keys.
        let numbered = |count: usize| (1..=count).map(|n| format!("{n:012}")).collect::<Vec<_>>();
        assert_eq!(targets(&[numbered(14), numbered(5)]).unwrap().len(), 29);
        assert_eq!(targets(&[vec![], vec![]]).unwrap().len(), 10);
        // Only the helper keys may be absent on a machine.
        assert!(list.iter().all(|target| target.required != target.path.contains(r"Services\WinSock\")));
    }

    #[test]
    fn unexpected_catalog_contents_refuse_the_whole_list() {
        for name in ["", "1", "00000000000a", "0000000000001", r"000000000001\x", "..", "AppId_Catalog", "000000000001 "] {
            assert!(targets(&[vec![name.to_owned()], vec![]]).is_err(), "{name:?}");
        }
        let twice = vec!["000000000001".to_owned(), "000000000001".to_owned()];
        assert!(targets(&[vec![], twice]).is_err());
        let many: Vec<String> = (0..=MOST_ENTRIES).map(|n| format!("{n:012}")).collect();
        assert!(targets(&[many, vec![]]).is_err());
    }

    #[test]
    fn nothing_outside_the_list_is_managed() {
        assert_eq!(managed(""), Some(ROOT_RIGHTS));
        assert_eq!(managed(r"system\currentcontrolset\services\winsock2\parameters"), Some(READ_RIGHTS));
        assert_eq!(managed(r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\NameSpace_Catalog5\Catalog_Entries64\000000000007"), Some(READ_RIGHTS));
        for path in ["SYSTEM", "SOFTWARE", r"SYSTEM\CurrentControlSet", r"SYSTEM\CurrentControlSet\Services", r"SYSTEM\CurrentControlSet\Services\WinSock2",
            r"SYSTEM\CurrentControlSet\Services\WinSock", r"SYSTEM\CurrentControlSet\Services\WinSock\Setup Migration",
            r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\AppId_Catalog",
            r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\Protocol_Catalog9\Catalog_Entries",
            r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\Protocol_Catalog9\Catalog_Entries\000000000001",
            r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\Protocol_Catalog9\Catalog_Entries64\000000000001\Sub",
            r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\Protocol_Catalog9\Catalog_Entries64\..\..\..",
            r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\Protocol_Catalog9\Catalog_Entries64\",
            r"SYSTEM\ControlSet001\Services\WinSock2\Parameters", r"SYSTEM\CurrentControlSet\Services\Tcpip\Parameters",
            r"\SYSTEM\CurrentControlSet\Services\WinSock2\Parameters", r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\",
            r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment", "Environment", "SAM", "SECURITY"] {
            assert_eq!(managed(path), None, "{path}");
        }
    }

    #[test]
    fn opened_key_must_be_the_key_that_was_named() {
        let path = r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters";
        assert!(opened_name_matches("", "", r"\REGISTRY\MACHINE"));
        assert!(opened_name_matches("", path, r"\REGISTRY\MACHINE\SYSTEM\ControlSet001\Services\WinSock2\Parameters"));
        assert!(opened_name_matches("", path, r"\Registry\Machine\System\ControlSet002\Services\Winsock2\Parameters"));
        assert!(opened_name_matches("", path, r"\REGISTRY\MACHINE\SYSTEM\CurrentControlSet\Services\WinSock2\Parameters"));
        let base = format!(r"SOFTWARE\MiniClawWinsockAclTest-{}", "a".repeat(32));
        assert!(opened_name_matches(&base, path, &format!(r"\REGISTRY\MACHINE\{base}\SYSTEM\CurrentControlSet\Services\WinSock2\Parameters")));
        assert!(opened_name_matches(&base, "", &format!(r"\REGISTRY\MACHINE\{base}")));
        for actual in [r"\REGISTRY\MACHINE\SOFTWARE\Elsewhere", r"\REGISTRY\USER\S-1-5-21-1\Environment",
            r"\REGISTRY\MACHINE\SYSTEM\ControlSet001\Services\WinSock2", r"\REGISTRY\MACHINE\SYSTEM\ControlSet001\Services\WinSock2\Parameters\Extra",
            r"\REGISTRY\MACHINE\SYSTEM\ControlSetABC\Services\WinSock2\Parameters", r"\REGISTRY\MACHINE\SYSTEM\Select\Services\WinSock2\Parameters",
            r"\REGISTRY\MACHINE\SOFTWARE\ControlSet001\Services\WinSock2\Parameters", r"\REGISTRY\MACHINE", ""] {
            assert!(!opened_name_matches("", path, actual), "{actual}");
        }
        assert!(!opened_name_matches("", "", r"\REGISTRY\MACHINE\SYSTEM"));
        assert!(!opened_name_matches("", "", r"\REGISTRY\USER"));
        assert!(test_base(&base));
        for base in ["", "SOFTWARE", r"SOFTWARE\MiniClawWinsockAclTest-", r"SYSTEM\MiniClawWinsockAclTest-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            r"SOFTWARE\MiniClawWinsockAclTest-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\x", r"SOFTWARE\MiniClawWinsockAclTest-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"] {
            assert!(!test_base(base), "{base}");
        }
    }

    #[test]
    fn only_a_custom_capability_sid_is_ever_granted() {
        assert!(capability_sid(&cap(7)));
        assert_eq!(sid_text(&cap(7)).unwrap(), "S-1-15-3-1024-7-2-3-4-5-6-7-8");
        // internetClient, registryRead-style short capabilities, ALL APPLICATION PACKAGES, a package, a user.
        let mut package = cap(7);
        package[8..12].copy_from_slice(&2u32.to_le_bytes());
        for other in [sid(15, &[3, 1]), sid(15, &[2, 1]), sid(15, &[2, 2]), package, sid(5, &[21, 1, 2, 3, 1001]), sid(5, &[18]), sid(1, &[0]), Vec::new()] {
            assert!(!capability_sid(&other));
            if !other.is_empty() {
                assert!(with_ace(&typical(0), &other, READ_RIGHTS).is_err());
                assert!(without_sid(&typical(0), &other).is_err());
            }
        }
    }

    #[test]
    fn entry_is_added_once_explicit_and_not_inheritable() {
        for slack in [0, 12] {
            let original = typical(slack);
            let ours = cap(1);
            assert_eq!(presence(&original, &ours, READ_RIGHTS).unwrap(), Presence::Absent);
            let added = with_ace(&original, &ours, READ_RIGHTS).unwrap();
            assert_eq!(presence(&added, &ours, READ_RIGHTS).unwrap(), Presence::Exact);
            assert_eq!(added.len(), original.len() + 56);
            let list = aces(&added).unwrap();
            assert_eq!(list.len(), 5);
            // After the explicit administrators entry, before the first inherited one.
            assert!(names(&added, &list[1], &ours));
            assert_eq!((list[1].kind, list[1].flags, list[1].mask), (0, 0, READ_RIGHTS));
            assert!(list[2..].iter().all(|ace| ace.flags & INHERITED_ACE != 0));
            // Adding again changes nothing.
            assert_eq!(with_ace(&added, &ours, READ_RIGHTS).unwrap(), added);
            // Removal gives back the original bytes, unused space included.
            assert_eq!(without_sid(&added, &ours).unwrap(), original);
            assert_eq!(without_sid(&original, &ours).unwrap(), original);
        }
        // A list with only inherited entries, and an empty list.
        let inherited = acl(&[ace(0, 0x10, 0x20019, &sid(5, &[32, 545]))], 0);
        let added = with_ace(&inherited, &cap(1), ROOT_RIGHTS).unwrap();
        assert!(names(&added, &aces(&added).unwrap()[0], &cap(1)));
        let empty = acl(&[], 0);
        assert_eq!(without_sid(&with_ace(&empty, &cap(1), ROOT_RIGHTS).unwrap(), &cap(1)).unwrap(), empty);
    }

    #[test]
    fn only_read_rights_can_be_added() {
        for rights in [0, 0x2, 0x4, 0x20006, 0xf003f, 0x1000_0000, 0x8000_0000, READ_RIGHTS | 0x4_0000, 0x1_0000] {
            assert!(with_ace(&typical(0), &cap(1), rights).is_err(), "{rights:#x}");
        }
        for rights in [ROOT_RIGHTS, READ_RIGHTS] {
            assert!(with_ace(&typical(0), &cap(1), rights).is_ok());
        }
    }

    #[test]
    fn wrong_entries_for_the_sid_are_replaced_by_the_exact_one() {
        let ours = cap(1);
        let users = ace(0, 0x12, 0x20019, &sid(5, &[32, 545]));
        for wrong in [vec![ace(0, 0, 0xf003f, &ours)], vec![ace(0, 0x02, READ_RIGHTS, &ours)], vec![ace(1, 0, READ_RIGHTS, &ours)],
            vec![ace(0, 0, READ_RIGHTS, &ours), ace(0, 0, READ_RIGHTS, &ours)], vec![ace(0, 0, ROOT_RIGHTS, &ours)]] {
            let mut entries = wrong.clone();
            entries.push(users.clone());
            let list = acl(&entries, 0);
            assert_eq!(presence(&list, &ours, READ_RIGHTS).unwrap(), Presence::Different);
            let fixed = with_ace(&list, &ours, READ_RIGHTS).unwrap();
            assert_eq!(presence(&fixed, &ours, READ_RIGHTS).unwrap(), Presence::Exact);
            assert_eq!(aces(&fixed).unwrap().len(), 2);
            assert_eq!(without_sid(&fixed, &ours).unwrap(), acl(&[users.clone()], 0));
        }
        // An inherited copy cannot be removed here; it stays visible as a difference.
        let inherited = acl(&[ace(0, 0x10, READ_RIGHTS, &ours)], 0);
        assert_eq!(without_sid(&inherited, &ours).unwrap(), inherited);
        assert_eq!(presence(&inherited, &ours, READ_RIGHTS).unwrap(), Presence::Different);
    }

    #[test]
    fn two_installations_do_not_touch_each_other() {
        let (first, second) = (cap(1), cap(2));
        let original = typical(4);
        let one = with_ace(&original, &first, READ_RIGHTS).unwrap();
        let both = with_ace(&one, &second, READ_RIGHTS).unwrap();
        assert_eq!(presence(&both, &first, READ_RIGHTS).unwrap(), Presence::Exact);
        assert_eq!(presence(&both, &second, READ_RIGHTS).unwrap(), Presence::Exact);
        let without_first = without_sid(&both, &first).unwrap();
        assert_eq!(presence(&without_first, &first, READ_RIGHTS).unwrap(), Presence::Absent);
        assert_eq!(presence(&without_first, &second, READ_RIGHTS).unwrap(), Presence::Exact);
        assert_eq!(without_first, with_ace(&original, &second, READ_RIGHTS).unwrap());
        assert_eq!(without_sid(&without_first, &second).unwrap(), original, "The last removal restores the original list");
        // Removing in the other order restores it as well.
        assert_eq!(without_sid(&without_sid(&both, &second).unwrap(), &first).unwrap(), original);
    }

    #[test]
    fn other_changes_to_a_key_survive_removal() {
        let ours = cap(1);
        let added = with_ace(&typical(0), &ours, READ_RIGHTS).unwrap();
        // Someone else adds an entry and an object entry of a kind this module does not parse.
        let mut entries: Vec<Vec<u8>> = aces(&added).unwrap().iter().map(|ace| added[ace.start..ace.start + ace.len].to_vec()).collect();
        entries.insert(0, ace(1, 0, 0x6, &sid(5, &[32, 546])));
        let mut object = ace(5, 0, 0x1, &[0u8; 40]);
        object[8..12].copy_from_slice(&1u32.to_le_bytes());
        entries.push(object);
        let changed = acl(&entries, 0);
        let removed = without_sid(&changed, &ours).unwrap();
        entries.retain(|entry| entry != &ace(0, 0, READ_RIGHTS, &ours));
        assert_eq!(removed, acl(&entries, 0));
    }

    #[test]
    fn malformed_lists_are_refused() {
        let good = typical(0);
        let mut short_size = good.clone();
        short_size[2..4].copy_from_slice(&((good.len() - 4) as u16).to_le_bytes());
        let mut long_ace = good.clone();
        long_ace[10..12].copy_from_slice(&0xfff0u16.to_le_bytes());
        let mut tiny_ace = good.clone();
        tiny_ace[10..12].copy_from_slice(&4u16.to_le_bytes());
        let mut many = good.clone();
        many[4..6].copy_from_slice(&40u16.to_le_bytes());
        for bad in [good[..7].to_vec(), short_size, long_ace, tiny_ace, many, Vec::new()] {
            assert!(presence(&bad, &cap(1), READ_RIGHTS).is_err());
            assert!(with_ace(&bad, &cap(1), READ_RIGHTS).is_err());
            assert!(without_sid(&bad, &cap(1)).is_err());
        }
        // A list that cannot hold one more entry.
        let filler = ace(0, 0, 1, &[0u8; 1000]);
        let full = acl(&vec![filler; 65], 0);
        assert!(with_ace(&full, &cap(1), READ_RIGHTS).is_err());
    }

    /// A self-relative descriptor: owner and group after the header, then the access list.
    fn self_relative(control: u16, list: Option<&[u8]>) -> Vec<u8> {
        let owner = sid(5, &[18]);
        let mut bytes = vec![1, 0];
        bytes.extend_from_slice(&(control | SE_SELF_RELATIVE).to_le_bytes());
        bytes.extend_from_slice(&20u32.to_le_bytes());
        bytes.extend_from_slice(&32u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&if list.is_some() { 44u32 } else { 0 }.to_le_bytes());
        bytes.extend_from_slice(&owner);
        bytes.extend_from_slice(&owner);
        bytes.extend_from_slice(list.unwrap_or_default());
        bytes
    }

    #[test]
    fn descriptor_without_an_access_list_is_never_rewritten() {
        let list = typical(0);
        let whole = self_relative(SE_DACL_PRESENT | SE_DACL_AUTO_INHERITED, Some(&list));
        let parsed = descriptor(&whole).unwrap();
        assert_eq!(parsed.dacl, &list[..]);
        assert_eq!(parsed.control & SE_DACL_AUTO_INHERITED, SE_DACL_AUTO_INHERITED);
        assert!(descriptor(&self_relative(0, None)).is_err(), "No access list");
        assert!(descriptor(&self_relative(SE_DACL_PRESENT, None)).is_err(), "A present but null list grants everything");
        let mut absolute = self_relative(SE_DACL_PRESENT, Some(&list));
        absolute[3] &= 0x7f;
        assert!(descriptor(&absolute).is_err());
        let whole = self_relative(SE_DACL_PRESENT, Some(&list));
        assert!(descriptor(&whole[..whole.len() - 1]).is_err());
        assert!(descriptor(&[]).is_err());
    }

    #[test]
    fn replacement_descriptor_carries_only_the_access_list() {
        let list = typical(0);
        for (control, expected) in [(SE_DACL_PRESENT, 0x8004u16), (SE_DACL_PRESENT | SE_DACL_PROTECTED, 0x9004),
            (SE_DACL_PRESENT | SE_DACL_AUTO_INHERITED, 0x8504), (SE_DACL_PRESENT | SE_DACL_AUTO_INHERITED | SE_DACL_PROTECTED | 0x0003 | 0x0010 | 0x0800, 0x9504)] {
            let built = dacl_descriptor(control, &list);
            assert_eq!(u16::from_le_bytes([built[2], built[3]]), expected, "{control:#x}");
            // No owner, group or audit list: those can never be changed by this descriptor.
            assert_eq!(&built[4..16], &[0; 12]);
            assert_eq!(descriptor(&built).unwrap().dacl, &list[..]);
        }
    }

    /// A snapshot with recognisable content.
    fn snapshot(tag: &str) -> Snapshot {
        Snapshot { descriptor: hex(tag.as_bytes()), sddl: format!("D:({tag})"), recorded: 1 }
    }

    #[test]
    fn record_keeps_the_first_and_latest_descriptor_of_each_key() {
        let mut record = Record::new("miniclawWinsockRead.x", "S-1-15-3-1024-1");
        let path = r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters";
        assert!(!record.knows(path));
        assert!(record.note(path, snapshot("first")));
        assert!(!record.note(path, snapshot("first")), "The same descriptor is not recorded twice");
        assert!(record.note(&path.to_lowercase(), snapshot("recreated")), "A recreated key gets a new snapshot");
        assert_eq!(record.keys.len(), 1);
        for index in 0..20 {
            record.note(path, snapshot(&format!("later-{index}")));
        }
        let before = &record.keys[0].before;
        assert_eq!(before.len(), MOST_SNAPSHOTS);
        assert_eq!(before[0], snapshot("first"), "The original descriptor is never dropped");
        assert_eq!(record.latest(path), Some(&snapshot("later-19")));
        assert!(record.note("", snapshot("root")));
        assert_eq!(record.keys.len(), 2);
    }

    #[test]
    fn record_of_another_installation_or_with_other_paths_is_refused() {
        let (capability, sid) = ("miniclawWinsockRead.0123456789abcdef0123456789abcdef", "S-1-15-3-1024-1-2-3-4-5-6-7-8");
        let mut record = Record::new(capability, sid);
        record.note("", snapshot("root"));
        let bytes = serde_json::to_vec(&record).unwrap();
        assert_eq!(Record::parse(&bytes, capability, sid).unwrap(), record);
        assert!(Record::parse(&bytes, "miniclawWinsockRead.ffffffffffffffffffffffffffffffff", sid).is_err());
        assert!(Record::parse(&bytes, capability, "S-1-15-3-1024-9-2-3-4-5-6-7-8").is_err());
        let text = String::from_utf8(bytes).unwrap();
        for altered in [text.replace(r#""path":"""#, r#""path":"SOFTWARE""#), text.replace(r#""path":"""#, r#""path":"SYSTEM\\CurrentControlSet\\Control""#),
            text.replace(r#""version":1"#, r#""version":2"#), text.replace(r#""version":1"#, r#""version":1,"extra":true"#),
            text.replace(r#""before":[{"#, r#""before":[],"x":[{"#), "{}".to_owned(), String::new()] {
            assert!(Record::parse(altered.as_bytes(), capability, sid).is_err(), "{altered}");
        }
    }

    #[test]
    fn published_status_is_healthy_repaired_or_failed() {
        use serde_json::json;
        assert_eq!(access_status(&json!({"ok": true, "keys": 29, "added": [], "repaired": [], "time": 5})),
            Some(AccessStatus { state: "healthy", time: Some(5), reason: None }));
        assert_eq!(access_status(&json!({"ok": true, "added": ["x"], "repaired": [], "time": 5})).unwrap().state, "healthy");
        assert_eq!(access_status(&json!({"ok": true, "repaired": [r"SYSTEM\x"], "time": 7})),
            Some(AccessStatus { state: "repaired", time: Some(7), reason: None }));
        let failed = access_status(&json!({"ok": false, "error": "Cannot open HKLM\\x: Access is denied.\n", "time": 9})).unwrap();
        assert_eq!((failed.state, failed.time), ("failed", Some(9)));
        assert_eq!(failed.reason.as_deref(), Some("Cannot open HKLM\\x: Access is denied."));
        assert_eq!(access_status(&json!({"ok": false})).unwrap().reason.as_deref(), Some("no status"));
        assert_eq!(access_status(&json!({"ok": true, "removed": true})).unwrap().state, "failed");
        assert_eq!(access_status(&json!({"error": "x"})), None);
        assert_eq!(access_status(&json!("ok")), None);
        let long = access_status(&json!({"ok": false, "error": "e".repeat(5000)})).unwrap();
        assert_eq!(long.reason.unwrap().len(), 240);
    }

    #[test]
    fn refusal_names_the_cause_and_stays_bounded() {
        let message = refusal("HKLM\\SYSTEM\\...\\000000000003: access denied\r\n");
        assert!(message.contains("000000000003: access denied"));
        assert!(message.contains("was not started"));
        assert!(!message.contains('\n') && !message.contains('\r'));
        assert!(refusal(&"x".repeat(5000)).len() < 600);
    }
}
