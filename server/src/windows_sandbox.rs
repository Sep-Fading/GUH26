//! Windows policy boundaries shared by the native broker and process sandbox.
// Reuses explicit path grants from permission decisions.
use crate::seatbelt::Grant;

/// A deny-first policy over canonical, case-preserving local Windows paths.
pub struct Policy {
    grants: Vec<Grant>,
}

impl Policy {
    /// Accepts paths already normalized by the native guarded path resolver.
    pub fn from_normalized(grants: Vec<Grant>) -> Self {
        Self { grants }
    }

    /// Requires one sufficient grant and no covering block, regardless of order.
    pub fn allows(&self, path: &str, write: bool) -> bool {
        let covers = |grant: &Grant| {
            path == grant.path
                || grant.folder
                    && path.strip_prefix(&grant.path).is_some_and(|suffix| {
                        grant.path.ends_with('\\') || suffix.starts_with('\\')
                    })
        };
        !self
            .grants
            .iter()
            .any(|grant| grant.blocked && covers(grant))
            && self
                .grants
                .iter()
                .any(|grant| !grant.blocked && (!write || grant.write) && covers(grant))
    }
}

/// Matches a canonical path or a complete child component, never a string-prefix sibling.
#[cfg(any(windows, test))]
fn contains(parent: &str, child: &str) -> bool {
    let parent = parent.to_ascii_lowercase();
    let child = child.to_ascii_lowercase();
    parent == child
        || child
            .strip_prefix(&parent)
            .is_some_and(|suffix| parent.ends_with('\\') || suffix.starts_with('\\'))
}

/// The paths a run's own rules block, from its normalized grants (not the protected roots).
#[cfg(any(windows, test))]
fn rule_blocks(grants: &[Grant]) -> Vec<String> {
    grants.iter().filter(|grant| grant.blocked).map(|grant| grant.path.clone()).collect()
}

/// Decides whether one of MiniClaw's own read-only runtime folders (private Node, npm, Python hook) is
/// refused to a run: only by the run's own blocked rules. The protected roots keep tool grants from
/// writing into the installation, whose `runtime\bin` holds these pinned runtimes; a read-only grant of
/// them is what every installed sandbox launch needs.
#[cfg(any(windows, test))]
fn runtime_refused(rules: &[String], root: &str) -> bool {
    rules.iter().any(|blocked| contains(blocked, root) || contains(root, blocked))
}

/// Decides whether a file with several hard links may be granted to a run.
///
/// A grant lands on the file, so it reaches the file under every name. Each name must lie in a
/// root this run was given with at least the requested access, and none under a blocked path.
#[cfg(any(windows, test))]
fn links_within(names: &[String], write: bool, roots: &[Grant], blocked: &[String]) -> bool {
    !names.is_empty()
        && names.iter().all(|name| {
            !blocked.iter().any(|block| contains(block, name))
                && roots.iter().any(|root| {
                    !root.blocked
                        && (root.write || !write)
                        && if root.folder {
                            contains(&root.path, name)
                        } else {
                            root.path.eq_ignore_ascii_case(name)
                        }
                })
        })
}

/// Access-list edits for one run's trustee, on the raw bytes of an `ACL`.
///
/// `SetEntriesInAclW` is not used for these: natively, `REVOKE_ACCESS` left an inherited entry in
/// place on files that another run had created (the count stayed the same and no error came
/// back), and a grant followed by a revoke turned the other entries of an existing file from
/// inherited into explicit ones. Here every other entry is copied byte for byte.
#[cfg(any(windows, test))]
mod acl {
    use std::io;

    const INHERITED_ACE: u8 = 0x10;

    /// Refuses a malformed list with one error kind.
    fn invalid<T>() -> io::Result<T> {
        Err(io::Error::new(io::ErrorKind::InvalidData, "Malformed access list"))
    }

    /// Lists each entry as (start, length) after checking every length against the list's size.
    fn entries(acl: &[u8]) -> io::Result<Vec<(usize, usize)>> {
        if acl.len() < 8 || usize::from(u16::from_le_bytes([acl[2], acl[3]])) != acl.len() {
            return invalid();
        }
        let mut list = Vec::new();
        let mut at = 8;
        for _ in 0..u16::from_le_bytes([acl[4], acl[5]]) {
            if at + 4 > acl.len() { return invalid(); }
            let length = usize::from(u16::from_le_bytes([acl[at + 2], acl[at + 3]]));
            if length < 8 || length % 4 != 0 || at + length > acl.len() { return invalid(); }
            list.push((at, length));
            at += length;
        }
        Ok(list)
    }

    /// True for a plain allow or deny entry whose trustee is exactly this SID.
    fn names(acl: &[u8], (start, length): (usize, usize), sid: &[u8]) -> bool {
        acl[start] <= 1 && length == 8 + sid.len() && &acl[start + 8..start + length] == sid
    }

    /// Checks that a trustee is a well-formed SID: its length follows from its own count.
    fn well_formed(sid: &[u8]) -> bool {
        sid.len() >= 12 && sid[0] == 1 && sid.len() == 8 + 4 * usize::from(sid[1])
    }

    /// Copies the list without any allow or deny entry for this SID, inherited or not. The
    /// list keeps its size; the freed room becomes unused space at its end.
    pub fn without_trustee(acl: &[u8], sid: &[u8]) -> io::Result<Vec<u8>> {
        if !well_formed(sid) { return invalid(); }
        let list = entries(acl)?;
        let mut out = acl[..8].to_vec();
        let mut count = 0u16;
        for entry in list {
            if names(acl, entry, sid) { continue; }
            out.extend_from_slice(&acl[entry.0..entry.0 + entry.1]);
            count += 1;
        }
        out.resize(acl.len(), 0);
        out[4..6].copy_from_slice(&count.to_le_bytes());
        Ok(out)
    }

    /// Reports whether any allow or deny entry names this SID.
    pub fn names_trustee(acl: &[u8], sid: &[u8]) -> io::Result<bool> {
        Ok(entries(acl)?.into_iter().any(|entry| names(acl, entry, sid)))
    }

    /// Copies the list with an explicit allow entry for this SID that holds at least `rights`.
    /// An explicit allow entry of the SID with the same inheritance flags takes the rights as
    /// well as its own, as a second grant of the same object did before; otherwise a new entry
    /// goes after the other explicit entries and before the inherited ones, where Windows
    /// expects it. No other entry changes. `flags` are the entry's inheritance flags.
    pub fn with_trustee(acl: &[u8], sid: &[u8], rights: u32, flags: u8) -> io::Result<Vec<u8>> {
        if !well_formed(sid) || rights == 0 || flags & INHERITED_ACE != 0 { return invalid(); }
        let list = entries(acl)?;
        if let Some((start, _)) = list.iter().copied().find(|entry| names(acl, *entry, sid) && acl[entry.0] == 0 && acl[entry.0 + 1] == flags) {
            let mut out = acl.to_vec();
            let held = u32::from_le_bytes([acl[start + 4], acl[start + 5], acl[start + 6], acl[start + 7]]);
            out[start + 4..start + 8].copy_from_slice(&(held | rights).to_le_bytes());
            return Ok(out);
        }
        let mut out = acl[..8].to_vec();
        let mut entry = vec![0, flags];
        entry.extend_from_slice(&((8 + sid.len()) as u16).to_le_bytes());
        entry.extend_from_slice(&rights.to_le_bytes());
        entry.extend_from_slice(sid);
        let mut count = 0u16;
        let mut placed = false;
        for kept in list {
            if !placed && acl[kept.0 + 1] & INHERITED_ACE != 0 {
                out.extend_from_slice(&entry);
                count += 1;
                placed = true;
            }
            out.extend_from_slice(&acl[kept.0..kept.0 + kept.1]);
            count += 1;
        }
        if !placed {
            out.extend_from_slice(&entry);
            count += 1;
        }
        let size = u16::try_from(out.len()).map_err(|_| io::Error::other("The access list is full"))?;
        out[2..4].copy_from_slice(&size.to_le_bytes());
        out[4..6].copy_from_slice(&count.to_le_bytes());
        Ok(out)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// A SID with the given last sub-authority.
        fn sid(last: u32) -> Vec<u8> {
            let mut bytes = vec![1, 2, 0, 0, 0, 0, 0, 15];
            bytes.extend_from_slice(&2u32.to_le_bytes());
            bytes.extend_from_slice(&last.to_le_bytes());
            bytes
        }

        /// One allow (0) or deny (1) entry.
        fn ace(kind: u8, flags: u8, mask: u32, sid: &[u8]) -> Vec<u8> {
            let mut bytes = vec![kind, flags];
            bytes.extend_from_slice(&((8 + sid.len()) as u16).to_le_bytes());
            bytes.extend_from_slice(&mask.to_le_bytes());
            bytes.extend_from_slice(sid);
            bytes
        }

        /// A list of entries with some unused room at its end.
        fn list(aces: &[Vec<u8>], spare: usize) -> Vec<u8> {
            let body: Vec<u8> = aces.concat();
            let mut bytes = vec![2, 0];
            bytes.extend_from_slice(&((8 + body.len() + spare) as u16).to_le_bytes());
            bytes.extend_from_slice(&(aces.len() as u16).to_le_bytes());
            bytes.extend_from_slice(&[0, 0]);
            bytes.extend_from_slice(&body);
            bytes.resize(8 + body.len() + spare, 0);
            bytes
        }

        #[test]
        fn removal_takes_inherited_and_explicit_entries_and_copies_the_rest_exactly() {
            let (run, user, system) = (sid(700), sid(1001), sid(18));
            let others = [ace(1, 0, 2, &user), ace(0, 0x10, 0x1f01ff, &system), ace(0, 0x13, 0x1f01ff, &user)];
            // The run's entries in every shape a run leaves: explicit, inheritable, inherited, deny.
            let before = list(&[others[0].clone(), ace(0, 0, 0x1301bf, &run), ace(0, 3, 0x1301bf, &run),
                others[1].clone(), ace(0, 0x10, 0x1301bf, &run), ace(1, 0x13, 1, &run), others[2].clone()], 12);
            assert!(names_trustee(&before, &run).unwrap());
            let after = without_trustee(&before, &run).unwrap();
            assert!(!names_trustee(&after, &run).unwrap());
            assert_eq!(after.len(), before.len(), "the list keeps its size");
            assert_eq!(&after[..8 + others.concat().len()], &list(&others, 0)[..8 + others.concat().len()].iter().enumerate()
                .map(|(index, byte)| if (2..4).contains(&index) { before[index] } else { *byte }).collect::<Vec<u8>>()[..]);
            assert!(after[8 + others.concat().len()..].iter().all(|byte| *byte == 0));
            // A second removal changes nothing, and a list without the run is returned as it was.
            assert_eq!(without_trustee(&after, &run).unwrap(), after);
            let clean = list(&others, 0);
            assert_eq!(without_trustee(&clean, &run).unwrap(), clean);
        }

        #[test]
        fn a_grant_goes_after_explicit_entries_and_before_inherited_ones() {
            let (run, user, system) = (sid(700), sid(1001), sid(18));
            let explicit = [ace(1, 0, 2, &user), ace(0, 0, 0x1f01ff, &user)];
            let inherited = [ace(0, 0x10, 0x1f01ff, &system), ace(0, 0x13, 0x1200a9, &user)];
            let before = list(&[explicit.to_vec(), inherited.to_vec()].concat(), 0);
            let after = with_trustee(&before, &run, 0x1301bf, 3).unwrap();
            let expected = list(&[explicit.to_vec(), vec![ace(0, 3, 0x1301bf, &run)], inherited.to_vec()].concat(), 0);
            assert_eq!(after, expected, "every other entry keeps its bytes, flags and place");
            // A second grant with the same flags adds its rights to the entry; nothing is lost.
            assert_eq!(with_trustee(&after, &run, 0x40, 3).unwrap(),
                list(&[explicit.to_vec(), vec![ace(0, 3, 0x1301ff, &run)], inherited.to_vec()].concat(), 0));
            assert_eq!(with_trustee(&after, &run, 0x1200a9, 3).unwrap(), after, "rights already held change nothing");
            // A grant with other inheritance flags is its own entry, after the explicit ones.
            assert_eq!(with_trustee(&after, &run, 0x1200a9, 0).unwrap(),
                list(&[explicit.to_vec(), vec![ace(0, 3, 0x1301bf, &run), ace(0, 0, 0x1200a9, &run)], inherited.to_vec()].concat(), 0));
            // An inherited entry of the run is never the one that is widened.
            let stale = list(&[vec![ace(0, 0x10, 1, &run)], inherited.to_vec()].concat(), 0);
            assert_eq!(with_trustee(&stale, &run, 2, 0).unwrap(),
                list(&[vec![ace(0, 0, 2, &run), ace(0, 0x10, 1, &run)], inherited.to_vec()].concat(), 0));
            // Grant then removal gives back the original entries.
            assert_eq!(&without_trustee(&after, &run).unwrap()[8..before.len()], &before[8..]);
            // A list of inherited entries only, and an empty one, take the entry first.
            let only = list(&inherited, 0);
            assert_eq!(with_trustee(&only, &run, 1, 0).unwrap(), list(&[vec![ace(0, 0, 1, &run)], inherited.to_vec()].concat(), 0));
            assert_eq!(with_trustee(&list(&[], 0), &run, 1, 0).unwrap(), list(&[ace(0, 0, 1, &run)], 0));
        }

        #[test]
        fn malformed_lists_and_trustees_are_refused() {
            let run = sid(700);
            let good = list(&[ace(0, 0, 1, &run)], 0);
            let mut short = good.clone();
            short.truncate(good.len() - 4);
            let mut overrun = good.clone();
            overrun[10] = 200;
            for bad in [&short[..], &overrun[..], &[2, 0, 4, 0][..]] {
                assert!(without_trustee(bad, &run).is_err());
                assert!(with_trustee(bad, &run, 1, 0).is_err());
                assert!(names_trustee(bad, &run).is_err());
            }
            assert!(without_trustee(&good, &run[..run.len() - 1]).is_err());
            assert!(with_trustee(&good, &run, 0, 0).is_err(), "no rights is not a grant");
            assert!(with_trustee(&good, &run, 1, 0x10).is_err(), "an entry is never written as inherited");
            // Another SID that only starts like the run's is not the run's.
            let longer = { let mut bytes = run.clone(); bytes[1] = 3; bytes.extend_from_slice(&9u32.to_le_bytes()); bytes };
            let mixed = list(&[ace(0, 0, 1, &longer)], 0);
            assert_eq!(without_trustee(&mixed, &run).unwrap(), mixed);
        }
    }
}

/// Why a sandboxed run was not started, in words for the user. It names the item at fault
/// (a path inside a folder the user granted, or a record of an earlier run), so callers that
/// otherwise keep reasons to the log show this text as it is.
#[derive(Debug)]
pub struct Refusal(pub String);

impl std::fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Refusal {}

/// Wraps a refusal for the user in an I/O error that keeps its words.
pub fn refused(message: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::PermissionDenied, Refusal(message))
}

/// The user's words of a refusal, if this error carries one.
pub fn refusal(error: &std::io::Error) -> Option<&str> {
    error.get_ref()?.downcast_ref::<Refusal>().map(|refusal| refusal.0.as_str())
}

/// Says that a granted folder holds an item this Windows user cannot manage. Such an item cannot
/// be given the run's access entry, and the run's cleanup could not prove it clean afterwards, so
/// the launch is refused before anything is recorded or changed.
pub fn unmanageable(path: &std::path::Path) -> std::io::Error {
    let path = path.to_string_lossy();
    let path = path.strip_prefix(r"\\?\").unwrap_or(&path);
    refused(format!(
        "The folder contains items the current Windows user cannot manage, starting with {path} (access denied). \
         MiniClaw cannot share that folder with the sandbox and changed nothing. Choose a narrower folder that \
         leaves this item out, or have an administrator give your account control of it."
    ))
}

/// What the last recovery of earlier runs' records left unresolved, in words for the user, or
/// `None` once it succeeded. While it is set the gate is closed for sandboxed work only; the
/// backend keeps serving chat (`windows_gate`).
static RECOVERY: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Records the outcome of a recovery of earlier runs' records.
pub fn set_recovery(problem: Option<String>) {
    *RECOVERY.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = problem;
}

/// What the last recovery left unresolved, if anything.
pub fn recovery_problem() -> Option<String> {
    RECOVERY.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
}

#[cfg(windows)]
#[path = "windows_sandbox_native.rs"]
mod native;
#[cfg(any(windows, test))]
#[path = "windows_sandbox_cleanup.rs"]
mod cleanup;
#[cfg(windows)]
pub use native::{Sandbox, drain_cleanup, recover};

#[cfg(test)]
mod tests {
    use super::*;

    /// A refusal keeps its words through an I/O error; other errors carry none.
    #[test]
    fn refusals_keep_their_words_for_the_user() {
        let error = unmanageable(std::path::Path::new(r"\\?\C:\Users\S\AppData\Local\Roblox\EBWebView\hyphen-data\120.0.6050.0"));
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        let words = refusal(&error).expect("the refusal is recognised");
        assert_eq!(words, error.to_string(), "Display shows the same words");
        assert!(words.starts_with("The folder contains items the current Windows user cannot manage, starting with C:\\Users\\S\\AppData\\Local\\Roblox\\EBWebView\\hyphen-data\\120.0.6050.0 (access denied)."), "{words}");
        assert!(words.contains("changed nothing"), "{words}");
        assert!(refusal(&std::io::Error::from_raw_os_error(5)).is_none());
        assert!(refusal(&std::io::Error::other("plain")).is_none());
    }

    /// Builds one normalized root.
    fn root(path: &str, write: bool, folder: bool) -> Grant {
        Grant { path: path.into(), write, folder, blocked: false }
    }

    /// Owned names for one linked file.
    fn names(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    // An installation keeps MiniClaw's private Node, npm and Python hook in `<install>\runtime\bin`, which
    // the protected roots cover so that no tool grant can write there. Granting those runtimes read-only
    // to a run is refused only by the run's own blocked rules (VM2, installed release, 2026-10-05: every
    // sandbox launch of an installed build failed with PermissionDenied).
    #[test]
    fn installed_runtimes_are_refused_by_the_runs_rules_not_by_the_protected_roots() {
        let protected = crate::seatbelt::windows_protected_roots(|key| match key {
            "USERPROFILE" => Some(r"C:\Users\Ana".into()),
            "MINICLAW_DATA_DIR" => Some(r"C:\Users\Ana\AppData\Local\MiniClaw Install\data".into()),
            _ => None,
        });
        let node = r"C:\Users\Ana\AppData\Local\MiniClaw Install\runtime\bin\node";
        assert!(protected.iter().any(|root| contains(root, node)), "the installation's runtime is protected");
        assert!(!runtime_refused(&rule_blocks(&[root(r"C:\work", true, true)]), node));
        let blocking = [root(r"C:\work", true, true), Grant { path: r"C:\Users\Ana\AppData\Local\MiniClaw Install".into(), write: false, folder: true, blocked: true }];
        assert!(runtime_refused(&rule_blocks(&blocking), node));
        let inner = [Grant { path: format!(r"{node}\npm-cache"), write: false, folder: true, blocked: true }];
        assert!(runtime_refused(&rule_blocks(&inner), node), "a rule inside the runtime folder refuses it too");
    }

    #[test]
    fn linked_file_needs_every_name_inside_granted_roots() {
        let roots = [root(r"C:\work", true, true), root(r"C:\docs", false, true), root(r"C:\tools\sh.exe", false, false)];
        let inside = names(&[r"C:\work\a.txt", r"C:\work\deep\b.txt"]);
        assert!(links_within(&inside, true, &roots, &[]));
        assert!(links_within(&inside, false, &roots, &[]));
        // One name outside every root refuses the grant, whatever access is asked.
        for outside in [r"C:\other\a.txt", r"C:\workspace\a.txt", r"C:\a.txt", r"D:\work\a.txt", r"C:\tools\other.exe"] {
            let pair = names(&[r"C:\work\a.txt", outside]);
            assert!(!links_within(&pair, false, &roots, &[]), "{outside}");
        }
        assert!(!links_within(&names(&[r"C:\work\a.txt", r"C:\workspace\a.txt"]), true, &roots, &[]));
        assert!(!links_within(&[], false, &roots, &[]));
    }

    #[test]
    fn linked_file_needs_the_same_access_under_every_name() {
        let roots = [root(r"C:\work", true, true), root(r"C:\docs", false, true), root(r"C:\tools\sh.exe", false, false)];
        let mixed = names(&[r"C:\work\a.txt", r"C:\docs\a.txt"]);
        assert!(links_within(&mixed, false, &roots, &[]), "Read is granted under both names");
        assert!(!links_within(&mixed, true, &roots, &[]), "Write would reach a read-only folder");
        // A single-file root covers exactly that name, without regard to letter case.
        let tool = names(&[r"C:\work\sh.exe", r"c:\TOOLS\sh.exe"]);
        assert!(links_within(&tool, false, &roots, &[]));
        assert!(!links_within(&tool, true, &roots, &[]));
        assert!(!links_within(&names(&[r"C:\work\sh.exe", r"C:\tools\sh.exe\x"]), false, &roots, &[]));
        // A blocked rule is never a root, even if it says write.
        let blocked_rule = [Grant { path: r"C:\work".into(), write: true, folder: true, blocked: true }];
        assert!(!links_within(&names(&[r"C:\work\a.txt"]), false, &blocked_rule, &[]));
    }

    #[test]
    fn linked_file_under_a_blocked_path_is_refused() {
        let roots = [root(r"C:\Users\me", true, true)];
        let blocked = [r"C:\Users\me\.codex".to_owned(), r"C:\Users\me\.claude.json".to_owned()];
        assert!(links_within(&names(&[r"C:\Users\me\p\a", r"C:\Users\me\p\b"]), true, &roots, &blocked));
        for secret in [r"C:\Users\me\.codex\auth.json", r"C:\Users\me\.CODEX\auth.json", r"C:\Users\me\.claude.json"] {
            let pair = names(&[r"C:\Users\me\p\a", secret]);
            assert!(!links_within(&pair, false, &roots, &blocked), "{secret}");
        }
        // A sibling that only shares a prefix with a blocked path is not blocked.
        assert!(links_within(&names(&[r"C:\Users\me\.codex-notes\a"]), true, &roots, &blocked));
    }
}
