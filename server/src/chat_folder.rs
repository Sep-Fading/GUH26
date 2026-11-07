//! A conversation's own folder (requirement by user decision, 2026-10-05). A conversation with no
//! project and no agent has no folder until its first tool call that needs one: a command, or
//! writing or editing a file. The folder is then made under `<Documents>/MiniClaw`, named after
//! the conversation's title with a short part of its id, recorded here, and reused from then on.
//! Renaming the conversation keeps the folder; a folder the user removed is made again when next
//! needed. Deleting the conversation leaves the folder unless the user ticks the box that moves it
//! to the Trash.

// Records which folder belongs to which conversation.
use serde_json::{Map, Value};
// Paths, the private record file and its lock.
use std::{path::{Path, PathBuf}, sync::{Mutex, OnceLock}};

/// The record of conversation folders, kept in the data folder.
#[derive(Default)]
pub struct Store { pub path: OnceLock<PathBuf>, gate: Mutex<()> }

/// The folder that holds every conversation folder: `MiniClaw` in the user's Documents folder.
/// Test and debug builds honour `MINICLAW_DOCUMENTS_DIR` (the test harness sets it so tests never
/// touch the user's own Documents); release builds ignore it, so the environment cannot move a
/// folder that becomes a sandbox grant (review F-1).
pub fn root() -> Option<PathBuf> {
    root_in(documents_override(cfg!(any(test, debug_assertions))))
}

/// `MiniClaw` inside `documents`, or inside the user's own Documents folder without one.
fn root_in(documents: Option<PathBuf>) -> Option<PathBuf> {
    documents.or_else(self::documents).map(|documents| documents.join("MiniClaw"))
}

/// The Documents folder `MINICLAW_DOCUMENTS_DIR` names, read only when `honoured`; empty means none.
fn documents_override(honoured: bool) -> Option<PathBuf> {
    if !honoured { return None; }
    std::env::var_os("MINICLAW_DOCUMENTS_DIR").filter(|value| !value.is_empty()).map(PathBuf::from)
}

/// The user's Documents folder. On Windows it is the Documents known folder, which may have been
/// moved, for example into OneDrive (learn.microsoft.com: SHGetKnownFolderPath, FOLDERID_Documents).
#[cfg(windows)]
fn documents() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::{System::Com::CoTaskMemFree, UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath, KF_FLAG_DEFAULT}};
    let mut raw = std::ptr::null_mut();
    // SAFETY: the out pointer is valid; the returned buffer is freed with CoTaskMemFree, as documented,
    // whether or not the call succeeded.
    let result = unsafe { SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT as u32, std::ptr::null_mut(), &mut raw) };
    let path = (result == 0 && !raw.is_null()).then(|| {
        // SAFETY: on success the buffer is a NUL-terminated wide string.
        let length = (0..).take_while(|&index| unsafe { *raw.add(index) } != 0).count();
        PathBuf::from(std::ffi::OsString::from_wide(unsafe { std::slice::from_raw_parts(raw, length) }))
    });
    unsafe { CoTaskMemFree(raw as *const _) };
    path.filter(|path| path.is_absolute())
}

/// The user's Documents folder on macOS.
#[cfg(not(windows))]
fn documents() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).filter(|home| home.is_absolute()).map(|home| home.join("Documents"))
}

/// Names a conversation's folder: its title, made safe as one folder name on both systems, and the
/// first eight characters of its id, so two conversations with one title never share a folder.
pub fn folder_name(title: &str, id: &str) -> String {
    let mut name = String::new();
    for letter in title.chars() {
        let kept = letter.is_alphanumeric() || matches!(letter, ' ' | '-' | '_' | '.' | '(' | ')' | ',' | '\'' | '&' | '+');
        let letter = if kept && !letter.is_control() { letter } else { ' ' };
        // Runs of spaces become one.
        if letter == ' ' && (name.is_empty() || name.ends_with(' ')) { continue; }
        name.push(letter);
        if name.chars().count() >= 60 { break; }
    }
    // Windows drops trailing dots and spaces, and a leading dot hides a folder on macOS.
    let name = name.trim_matches(|letter| letter == ' ' || letter == '.');
    let name = if name.is_empty() { "Conversation" } else { name };
    let short: String = id.chars().filter(char::is_ascii_hexdigit).take(8).collect();
    format!("{name} {short}")
}

impl Store {
    fn read(&self) -> Map<String, Value> {
        self.path.get().and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|value| value.as_object().cloned()).unwrap_or_default()
    }

    fn write(&self, record: &Map<String, Value>) -> std::io::Result<()> {
        let Some(path) = self.path.get() else { return Ok(()); };
        let bytes = serde_json::to_vec_pretty(record)?;
        let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        {
            use std::io::Write;
            let mut file = crate::platform_fs::private_file(&temporary, true)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        std::fs::rename(&temporary, path).inspect_err(|_| { let _ = std::fs::remove_file(&temporary); })
    }

    /// The folder recorded for a conversation, whether or not it still exists.
    pub fn recorded(&self, thread: &str) -> Option<String> {
        let _gate = self.gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.read().get(thread).and_then(Value::as_str).map(str::to_owned)
    }

    /// The conversation's folder, made now if it has none or the user removed it. The folder is
    /// always a real folder directly inside `root`, never a link.
    pub fn ensure(&self, thread: &str, title: &str) -> std::io::Result<String> {
        let _gate = self.gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = root().ok_or(std::io::ErrorKind::NotFound)?;
        let mut record = self.read();
        let folder = match record.get(thread).and_then(Value::as_str) {
            Some(folder) => PathBuf::from(folder),
            None => root.join(folder_name(title, thread)),
        };
        if folder.parent() != Some(root.as_path()) && !inside_root(&folder, &root) {
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
        std::fs::create_dir_all(&root)?;
        match std::fs::create_dir(&folder) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let metadata = std::fs::symlink_metadata(&folder)?;
        if !metadata.is_dir() || crate::platform_fs::is_link(&metadata) { return Err(std::io::ErrorKind::PermissionDenied.into()); }
        // The spelling the sandbox and the permission checks compare: resolved, in plain drive form.
        let resolved = std::fs::canonicalize(&folder)?;
        let resolved = crate::platform_fs::plain_disk_form(resolved.to_str().ok_or(std::io::ErrorKind::InvalidData)?).to_owned();
        if record.get(thread).and_then(Value::as_str) != Some(resolved.as_str()) {
            record.insert(thread.to_owned(), Value::String(resolved.clone()));
            self.write(&record)?;
        }
        Ok(resolved)
    }

    /// Forgets a deleted conversation's folder. With `trash`, the folder is moved there first,
    /// but only when it is still a real folder inside `root` that no other conversation names;
    /// a folder that is already gone is fine. A Trash on another volume gets a copy, and then
    /// the folder is removed (`move_folder`).
    pub fn remove(&self, thread: &str, trash: Option<&Path>) -> std::io::Result<()> {
        self.remove_with(thread, trash, &|from, to| std::fs::rename(from, to))
    }

    /// `remove` with the rename it uses passed in, so a test can make it fail as across volumes.
    fn remove_with(&self, thread: &str, trash: Option<&Path>, rename: &dyn Fn(&Path, &Path) -> std::io::Result<()>)
        -> std::io::Result<()>
    {
        let _gate = self.gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut record = self.read();
        let Some(folder) = record.get(thread).and_then(Value::as_str).map(PathBuf::from) else { return Ok(()); };
        if let Some(trash) = trash {
            let root = root().ok_or(std::io::ErrorKind::NotFound)?;
            let shared = record.iter().any(|(other, path)| other != thread && path.as_str() == folder.to_str());
            match std::fs::symlink_metadata(&folder) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
                Ok(metadata) => {
                    if shared || !metadata.is_dir() || crate::platform_fs::is_link(&metadata) || !inside_root(&folder, &root) {
                        return Err(std::io::ErrorKind::PermissionDenied.into());
                    }
                    std::fs::create_dir_all(trash)?;
                    let name = folder.file_name().ok_or(std::io::ErrorKind::InvalidInput)?.to_string_lossy().into_owned();
                    let target = trash.join(format!("{name} {}", &uuid::Uuid::new_v4().to_string()[..8]));
                    move_folder(&folder, &target, rename, &MOVE_BOUNDS)?;
                }
            }
        }
        record.remove(thread);
        self.write(&record)
    }
}

/// How much a move to a Trash on another volume copies at most. A larger folder is kept where it
/// is and the user is told, rather than holding up MiniClaw while gigabytes are copied.
struct Bounds { entries: usize, bytes: u64, depth: usize }
const MOVE_BOUNDS: Bounds = Bounds { entries: 20_000, bytes: 1 << 30, depth: 64 };

/// Whether a failed rename failed only because the target is on another volume: EXDEV on Unix,
/// ERROR_NOT_SAME_DEVICE (17) on Windows. A folder cannot be renamed across volumes on either
/// (rename(2); MoveFileExW: "the destination must be on the same drive" for a directory).
fn cross_device(error: &std::io::Error) -> bool {
    #[cfg(windows)]
    let code = windows_sys::Win32::Foundation::ERROR_NOT_SAME_DEVICE as i32;
    #[cfg(unix)]
    let code = libc::EXDEV;
    error.kind() == std::io::ErrorKind::CrossesDevices || error.raw_os_error() == Some(code)
}

/// Moves a checked conversation folder to `target` in the Trash. A rename is tried first; when it
/// fails because the Trash is on another volume, the folder is copied within `bounds`, never
/// through a link, and removed only once the whole copy is written. On any failure the folder
/// stays, and a half-made copy is removed again.
fn move_folder(folder: &Path, target: &Path, rename: &dyn Fn(&Path, &Path) -> std::io::Result<()>, bounds: &Bounds)
    -> std::io::Result<()>
{
    match rename(folder, target) {
        Err(error) if cross_device(&error) => {}
        done => return done,
    }
    std::fs::create_dir(target)?;
    if let Err(error) = copy_contents(folder, target, bounds) {
        let _ = std::fs::remove_dir_all(target);
        return Err(error);
    }
    // The folder copied must still be a real folder. `remove_dir_all` does not follow links
    // inside it (Rust's fix for CVE-2022-21658 on Unix; reparse points on Windows).
    let metadata = std::fs::symlink_metadata(folder)?;
    if !metadata.is_dir() || crate::platform_fs::is_link(&metadata) { return Err(std::io::ErrorKind::PermissionDenied.into()); }
    std::fs::remove_dir_all(folder)
}

/// Copies what is inside `from` into the new, empty folder `to`: folders and regular files only,
/// each opened without following a link, within `bounds`. A link, or anything else, refuses the copy.
fn copy_contents(from: &Path, to: &Path, bounds: &Bounds) -> std::io::Result<()> {
    use std::io::Read;
    let too_large = || std::io::Error::from(std::io::ErrorKind::FileTooLarge);
    let (mut entries, mut bytes) = (0usize, 0u64);
    let mut pending = vec![(from.to_path_buf(), to.to_path_buf(), 0usize)];
    while let Some((source, copy, depth)) = pending.pop() {
        for entry in std::fs::read_dir(&source)? {
            let entry = entry?;
            entries += 1;
            if entries > bounds.entries { return Err(too_large()); }
            let (from, to) = (entry.path(), copy.join(entry.file_name()));
            let metadata = std::fs::symlink_metadata(&from)?;
            if crate::platform_fs::is_link(&metadata) { return Err(std::io::ErrorKind::PermissionDenied.into()); }
            if metadata.is_dir() {
                if depth + 1 > bounds.depth { return Err(too_large()); }
                std::fs::create_dir(&to)?;
                pending.push((from, to, depth + 1));
            } else if metadata.is_file() {
                let reader = crate::platform_fs::open_read_nofollow(&from)?;
                let mut writer = crate::platform_fs::create_new(&to, false)?;
                let room = bounds.bytes - bytes;
                let copied = std::io::copy(&mut reader.take(room + 1), &mut writer)?;
                if copied > room { return Err(too_large()); }
                bytes += copied;
                let _ = writer.set_modified(metadata.modified()?);
                #[cfg(unix)]
                let permissions = { use std::os::unix::fs::PermissionsExt; std::fs::Permissions::from_mode(metadata.permissions().mode() & 0o777) };
                #[cfg(windows)]
                let permissions = metadata.permissions();
                writer.sync_all()?;
                writer.set_permissions(permissions)?;
            } else {
                return Err(std::io::ErrorKind::PermissionDenied.into());
            }
        }
    }
    Ok(())
}

/// Whether `folder` is a folder directly inside `root`, compared resolved where both exist.
fn inside_root(folder: &Path, root: &Path) -> bool {
    let resolved = |path: &Path| std::fs::canonicalize(path).ok()
        .and_then(|path| path.to_str().map(|text| PathBuf::from(crate::platform_fs::plain_disk_form(text))));
    match (folder.parent().and_then(resolved), resolved(root)) {
        (Some(parent), Some(root)) => parent == root,
        _ => folder.parent() == Some(root),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A title becomes one safe folder name; the id keeps names apart.
    #[test]
    fn names_are_safe_and_distinct() {
        let id = "3f2a9c1d-0000-4000-8000-000000000000";
        assert_eq!(folder_name("Fix the build", id), "Fix the build 3f2a9c1d");
        assert_eq!(folder_name("../../etc/passwd", id), "etc passwd 3f2a9c1d");
        assert_eq!(folder_name("a:b*c?\"d<e>f|g\\h/i", id), "a b c d e f g h i 3f2a9c1d");
        assert_eq!(folder_name("  ...  ", id), "Conversation 3f2a9c1d");
        assert_eq!(folder_name("CON", id), "CON 3f2a9c1d", "a reserved device name is not reserved with a suffix");
        assert_eq!(folder_name("Ünïcode café", id), "Ünïcode café 3f2a9c1d");
        assert_eq!(folder_name("tab\there\nnewline", id), "tab here newline 3f2a9c1d");
        assert!(folder_name(&"x".repeat(300), id).chars().count() <= 69);
        assert_eq!(folder_name("trailing dot.", id), "trailing dot 3f2a9c1d");
    }

    /// Serializes the tests here that set `MINICLAW_DOCUMENTS_DIR`, which the whole test binary shares.
    static DOCUMENTS: Mutex<()> = Mutex::new(());

    /// Takes the Documents lock, whether or not an earlier test failed while holding it.
    fn documents_lock() -> std::sync::MutexGuard<'static, ()> {
        DOCUMENTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A new empty folder for one test.
    fn scratch(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("mc-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    /// A rename that fails the way a move to another volume does: EXDEV on macOS,
    /// ERROR_NOT_SAME_DEVICE (17) on Windows.
    fn other_volume(_: &Path, _: &Path) -> std::io::Result<()> {
        #[cfg(windows)]
        let code = windows_sys::Win32::Foundation::ERROR_NOT_SAME_DEVICE as i32;
        #[cfg(unix)]
        let code = libc::EXDEV;
        Err(std::io::Error::from_raw_os_error(code))
    }

    /// A conversation folder with a file at the top and one two folders down.
    fn filled(base: &Path) -> PathBuf {
        let folder = base.join("Documents").join("MiniClaw").join("Chat 3f2a9c1d");
        std::fs::create_dir_all(folder.join("sub").join("deeper")).unwrap();
        std::fs::write(folder.join("a.txt"), "top").unwrap();
        std::fs::write(folder.join("sub").join("deeper").join("b.txt"), "deep").unwrap();
        folder
    }

    /// Release builds never move the root of conversation folders through the environment
    /// (review F-1); test and debug builds do, so tests never touch the user's own Documents.
    #[test]
    fn the_documents_override_is_for_tests_only() {
        let _documents = documents_lock();
        let elsewhere = std::env::temp_dir().join(format!("mc-elsewhere-{}", uuid::Uuid::new_v4()));
        // SAFETY: the tests that touch this variable hold the Documents lock.
        unsafe { std::env::set_var("MINICLAW_DOCUMENTS_DIR", &elsewhere); }
        assert_eq!(documents_override(false), None, "the release path ignores the variable");
        assert_eq!(root_in(documents_override(false)), documents().map(|documents| documents.join("MiniClaw")));
        assert_eq!(documents_override(true), Some(elsewhere.clone()));
        assert_eq!(root(), Some(elsewhere.join("MiniClaw")), "this test build honours it");
        unsafe { std::env::set_var("MINICLAW_DOCUMENTS_DIR", ""); }
        assert_eq!(documents_override(true), None, "an empty value means the real Documents folder");
        unsafe { std::env::remove_var("MINICLAW_DOCUMENTS_DIR"); }
    }

    /// Only an error that says "another volume" counts as one.
    #[test]
    fn a_cross_volume_failure_is_recognised() {
        assert!(cross_device(&other_volume(Path::new("a"), Path::new("b")).unwrap_err()));
        assert!(!cross_device(&std::io::ErrorKind::PermissionDenied.into()));
        assert!(!cross_device(&std::io::ErrorKind::NotFound.into()));
    }

    /// A folder on another volume than the Trash (review F-2: Documents in OneDrive or on another
    /// drive, the Trash under the data folder) is copied whole and then removed.
    #[test]
    fn a_cross_volume_move_copies_then_removes() {
        let base = scratch("cross-volume");
        let folder = filled(&base);
        std::fs::create_dir_all(base.join("Trash")).unwrap();
        let target = base.join("Trash").join("Chat 3f2a9c1d 0a1b2c3d");
        move_folder(&folder, &target, &other_volume, &MOVE_BOUNDS).unwrap();
        assert!(!folder.exists(), "the folder left Documents");
        assert_eq!(std::fs::read_to_string(target.join("a.txt")).unwrap(), "top");
        assert_eq!(std::fs::read_to_string(target.join("sub").join("deeper").join("b.txt")).unwrap(), "deep");
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// Any other failed rename copies nothing and keeps the folder as it was.
    #[test]
    fn only_a_cross_volume_failure_falls_back_to_a_copy() {
        let base = scratch("rename-refused");
        let folder = filled(&base);
        let target = base.join("Trash 0a1b2c3d");
        let refused = |_: &Path, _: &Path| -> std::io::Result<()> { Err(std::io::ErrorKind::PermissionDenied.into()) };
        assert!(move_folder(&folder, &target, &refused, &MOVE_BOUNDS).is_err());
        assert!(!target.exists(), "nothing was copied");
        assert_eq!(std::fs::read_to_string(folder.join("sub").join("deeper").join("b.txt")).unwrap(), "deep");
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// The copy stops at too many entries, too many bytes or too deep a tree, and refuses a link
    /// inside the folder. Each time the folder stays whole and no half copy is left in the Trash.
    #[test]
    fn a_cross_volume_copy_is_bounded_and_refuses_links() {
        let base = scratch("cross-volume-bounds");
        let folder = filled(&base);
        let target = base.join("Chat 3f2a9c1d 0a1b2c3d");
        let check = |bounds: Bounds, why: &str| {
            assert!(move_folder(&folder, &target, &other_volume, &bounds).is_err(), "{why}");
            assert!(!target.exists(), "{why}: the half copy is removed");
            assert_eq!(std::fs::read_to_string(folder.join("a.txt")).unwrap(), "top", "{why}");
            assert_eq!(std::fs::read_to_string(folder.join("sub").join("deeper").join("b.txt")).unwrap(), "deep", "{why}");
        };
        check(Bounds { entries: 3, bytes: 1 << 20, depth: 8 }, "four entries");
        check(Bounds { entries: 100, bytes: 6, depth: 8 }, "seven bytes");
        check(Bounds { entries: 100, bytes: 1 << 20, depth: 1 }, "two levels down");
        #[cfg(unix)]
        {
            let outside = base.join("outside.txt");
            std::fs::write(&outside, "secret").unwrap();
            std::os::unix::fs::symlink(&outside, folder.join("sub").join("link.txt")).unwrap();
            check(Bounds { entries: 100, bytes: 1 << 20, depth: 8 }, "a link to a file");
            assert!(std::fs::symlink_metadata(folder.join("sub").join("link.txt")).unwrap().file_type().is_symlink());
            assert_eq!(std::fs::read_to_string(&outside).unwrap(), "secret");
            std::fs::remove_file(folder.join("sub").join("link.txt")).unwrap();
        }
        move_folder(&folder, &target, &other_volume, &Bounds { entries: 4, bytes: 7, depth: 2 }).unwrap();
        assert!(!folder.exists() && target.join("a.txt").is_file(), "the same folder moves within its bounds");
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// Deleting a conversation with the box ticked moves its folder even when the Trash is on
    /// another volume, and forgets the folder.
    #[test]
    fn a_deleted_conversation_folder_reaches_a_trash_on_another_volume() {
        let _documents = documents_lock();
        let base = scratch("chat-folder-volume");
        let store = Store::default();
        let _ = store.path.set(base.join("record.json"));
        // SAFETY: the tests that touch this variable hold the Documents lock.
        unsafe { std::env::set_var("MINICLAW_DOCUMENTS_DIR", base.join("Documents")); }
        let thread = "3f2a9c1d-2222-4000-8000-000000000000";
        let folder = store.ensure(thread, "Far away").unwrap();
        std::fs::write(Path::new(&folder).join("hi.txt"), "hi").unwrap();
        let trash = base.join("Trash");
        store.remove_with(thread, Some(&trash), &other_volume).unwrap();
        assert!(!Path::new(&folder).exists());
        let moved: Vec<_> = std::fs::read_dir(&trash).unwrap().map(|entry| entry.unwrap().path()).collect();
        assert_eq!(moved.len(), 1);
        assert_eq!(std::fs::read_to_string(moved[0].join("hi.txt")).unwrap(), "hi");
        assert_eq!(store.recorded(thread), None);
        unsafe { std::env::remove_var("MINICLAW_DOCUMENTS_DIR"); }
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// Made at first need, reused, made again when removed, kept when renamed, and moved to the
    /// Trash only on request and only from inside the root.
    #[test]
    fn a_folder_is_made_once_and_kept() {
        let _documents = documents_lock();
        let base = std::env::temp_dir().join(format!("mc-chat-folder-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        let store = Store::default();
        let _ = store.path.set(base.join("record.json"));
        // SAFETY: the tests that touch this variable hold the Documents lock.
        unsafe { std::env::set_var("MINICLAW_DOCUMENTS_DIR", base.join("Documents")); }
        let thread = "3f2a9c1d-1111-4000-8000-000000000000";
        assert_eq!(store.recorded(thread), None);
        assert!(!base.join("Documents").exists(), "nothing exists before it is needed");
        let first = store.ensure(thread, "My chat").unwrap();
        assert!(first.ends_with("My chat 3f2a9c1d"), "{first}");
        assert!(Path::new(&first).is_dir());
        assert_eq!(store.ensure(thread, "Renamed").unwrap(), first, "a rename keeps the folder");
        std::fs::remove_dir(&first).unwrap();
        assert_eq!(store.ensure(thread, "Renamed").unwrap(), first, "a removed folder is made again");
        std::fs::write(Path::new(&first).join("hi.txt"), "hi").unwrap();
        store.remove(thread, None).unwrap();
        assert!(Path::new(&first).join("hi.txt").is_file(), "deleting without the box keeps the files");
        assert_eq!(store.recorded(thread), None);
        let second = store.ensure(thread, "Renamed").unwrap();
        let trash = base.join("Trash");
        store.remove(thread, Some(&trash)).unwrap();
        assert!(!Path::new(&second).exists());
        assert_eq!(std::fs::read_dir(&trash).unwrap().count(), 1);
        // Gone already: nothing to move, and no error.
        let third = store.ensure(thread, "x").unwrap();
        std::fs::remove_dir_all(&third).unwrap();
        store.remove(thread, Some(&trash)).unwrap();
        // A record pointing outside the root is never moved.
        let outside = base.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let mut record = Map::new();
        record.insert(thread.into(), Value::String(outside.to_string_lossy().into_owned()));
        store.write(&record).unwrap();
        assert!(store.remove(thread, Some(&trash)).is_err());
        assert!(outside.is_dir());
        unsafe { std::env::remove_var("MINICLAW_DOCUMENTS_DIR"); }
        std::fs::remove_dir_all(&base).unwrap();
    }
}
