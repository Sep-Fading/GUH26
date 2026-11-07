//! Platform file primitives. Windows private objects use protected owner-only DACLs.
// Shares native file handles and filesystem errors.
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io,
    path::Path,
};

/// Accepts drive-rooted Win32 names, excluding device aliases, streams and remote shares.
pub fn plain_windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    if bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || !matches!(bytes[2], b'/' | b'\\')
    {
        return false;
    }
    let parts: Vec<_> = path[3..].split(['/', '\\']).collect();
    parts.iter().enumerate().all(|(index, part)| {
        if part.is_empty() {
            return index + 1 == parts.len();
        }
        let base = part
            .split('.')
            .next()
            .unwrap_or_default()
            .trim_end()
            .to_ascii_uppercase();
        !matches!(*part, "." | "..")
            && !part.ends_with(['.', ' '])
            && !part
                .chars()
                .any(|c| c.is_control() || matches!(c, ':' | '<' | '>' | '"' | '|' | '?' | '*'))
            && !matches!(
                base.as_str(),
                "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
            )
            && !(base.starts_with("COM") || base.starts_with("LPT"))
                .then(|| &base[3..])
                .is_some_and(|suffix| {
                    matches!(
                        suffix,
                        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                    )
                })
    })
}

/// Reports whether a name has the shape Windows generates for the 8.3 alias of another file:
/// a base of at most eight characters that ends in `~` and digits, at most one period, an
/// extension of at most three characters and no space ([MS-FSCC] 2.1.5.2.1, "8.3 Filename").
/// Non-ASCII letters are not excluded, which only refuses more.
fn short_alias_shape(name: &str) -> bool {
    let (base, extension) = name.split_once('.').unwrap_or((name, ""));
    let numbered = base.rsplit_once('~')
        .is_some_and(|(_, digits)| !digits.is_empty() && digits.bytes().all(|digit| digit.is_ascii_digit()));
    numbered && base.chars().count() <= 8 && extension.chars().count() <= 3
        && !extension.contains('.') && !name.contains(' ')
}

/// Accepts the name of a file or folder that does not exist yet, in any language. A name
/// that does not exist cannot be compared with the object it may later turn out to alias, so
/// the shape of a generated short name is refused; an existing alias is opened and judged
/// by the identity of what it names. Device names, streams and trailing dots or spaces are
/// refused with the whole path by `plain_windows_path`.
pub fn new_windows_leaf(name: &str) -> bool {
    !name.is_empty() && !name.contains(['\\', '/']) && !short_alias_shape(name)
}

/// Reports whether Windows could treat two names as one. NTFS folds case one UTF-16 unit at a
/// time with a table of its own, so the names must be equally long; two ASCII units match by
/// ASCII case, an ASCII and a non-ASCII unit match when Unicode gives them a common case, and
/// two different non-ASCII units are assumed to match. It errs towards a match: it decides
/// what a blocked name covers, never what is allowed.
pub fn windows_names_may_match(left: &str, right: &str) -> bool {
    let (left, right): (Vec<u16>, Vec<u16>) = (left.encode_utf16().collect(), right.encode_utf16().collect());
    let folded = |unit: u16| char::from_u32(u32::from(unit)).map(|letter|
        (letter.to_uppercase().collect::<String>(), letter.to_lowercase().collect::<String>()));
    left.len() == right.len() && left.iter().zip(&right).all(|(&a, &b)| {
        if a == b { return true; }
        match (a < 0x80, b < 0x80) {
            (true, true) => (a as u8).eq_ignore_ascii_case(&(b as u8)),
            (false, false) => true,
            _ => folded(a).zip(folded(b)).is_some_and(|(a, b)| a.0 == b.0 || a.1 == b.1),
        }
    })
}

/// Whether `path` is `root` or lies beneath it, by whole components. On Windows both are first put
/// in the spelling the permission store keys on (`permission_path`: resolved, with the case on disk),
/// so `c:/work/x` is inside `C:\Work` and `C:\Workshop` is not; a path that cannot be resolved is
/// inside nothing. On macOS this is `Path::starts_with`, unchanged.
pub fn path_within(path: &str, root: &str) -> bool {
    #[cfg(windows)]
    {
        return match (permission_path(Path::new(path)), permission_path(Path::new(root))) {
            (Ok(path), Ok(root)) => Path::new(&path).starts_with(&root),
            _ => false,
        };
    }
    #[cfg(not(windows))]
    Path::new(path).starts_with(root)
}

/// Gives a local disk path in its plain drive form: `\\?\C:\x` becomes `C:\x`. Rust's
/// `canonicalize` answers the verbatim form on Windows, which its documentation warns other
/// applications may not accept; `cmd.exe`, for one, refuses such a working folder. Only the
/// disk prefix is removed; `\\?\UNC\...` and every other form is returned unchanged, so a
/// macOS path is never altered.
pub fn plain_disk_form(path: &str) -> &str {
    match path.strip_prefix(r"\\?\") {
        Some(rest) if matches!(rest.as_bytes(), [drive, b':', b'\\', ..] if drive.is_ascii_alphabetic()) => rest,
        _ => path,
    }
}

/// Includes junctions and other Windows reparse points in the rejected link class.
pub fn is_link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

/// Produces a high-resolution modification stamp for transfer consistency checks.
pub fn modified_ns(metadata: &Metadata) -> io::Result<u128> {
    metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .map_err(io::Error::other)
}

/// Opens one existing regular file without following its final link.
pub fn open_read_nofollow(path: &Path) -> io::Result<File> {
    open_existing(path, false, false)
}

/// Opens an existing object; Windows pins and rejects reparse-point ancestors.
pub fn open_existing(path: &Path, write: bool, directory: bool) -> io::Result<File> {
    #[cfg(windows)]
    {
        windows::open_existing(path, write, directory)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let file = OpenOptions::new()
            .read(true)
            .write(write)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        let metadata = file.metadata()?;
        if directory != metadata.is_dir() || (!directory && !metadata.is_file()) {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(file)
    }
}

/// Creates a file atomically without replacing an existing entry.
pub fn create_new(path: &Path, private: bool) -> io::Result<File> {
    #[cfg(windows)]
    {
        windows::create_file(path, private, true)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(if private { 0o600 } else { 0o644 })
            .open(path)
    }
}

/// Opens private storage, validating an existing file before truncation.
pub fn private_file(path: &Path, new: bool) -> io::Result<File> {
    let file = if new {
        create_new(path, true)?
    } else {
        private_lock_file(path)?
    };
    if !new {
        file.set_len(0)?;
    }
    Ok(file)
}

/// Opens or creates a private lock without changing existing contents.
pub fn private_lock_file(path: &Path) -> io::Result<File> {
    match create_new(path, true) {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let file = open_existing(path, true, false)?;
            if !private_regular_file(&file)? {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            Ok(file)
        }
        Err(error) => Err(error),
    }
}

/// Checks owner-only permissions, a regular file and exactly one hard link.
pub fn private_regular_file(file: &File) -> io::Result<bool> {
    #[cfg(windows)]
    {
        windows::private_regular_file(file)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        Ok(metadata.is_file()
            && metadata.mode() & 0o077 == 0
            && metadata.uid() == rustix::process::geteuid().as_raw()
            && metadata.nlink() == 1)
    }
}

/// Creates private directories without inheriting a public Windows DACL.
pub fn private_dir(path: &Path, recursive: bool) -> io::Result<()> {
    #[cfg(windows)]
    {
        windows::private_dir(path, recursive)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(recursive)
            .mode(0o700)
            .create(path)
    }
}

/// Returns volume and file identifiers from the already-open object.
pub fn file_identity(file: &File) -> io::Result<(u64, u128)> {
    #[cfg(windows)]
    {
        windows::file_identity(file)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        Ok((metadata.dev(), metadata.ino() as u128))
    }
}

/// Reports available bytes on the caller's target volume.
pub fn free_space(path: &Path) -> io::Result<u64> {
    #[cfg(windows)]
    {
        windows::free_space(path)
    }
    #[cfg(unix)]
    {
        let stats = rustix::fs::statvfs(path)?;
        Ok(stats.f_bavail.saturating_mul(stats.f_frsize))
    }
}

/// Flushes parent-directory changes where the platform supports directory fsync.
pub fn sync_parent(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path.parent().ok_or(io::ErrorKind::InvalidInput)?)?.sync_all()
    }
    // Windows File::sync_all flushes file data; directory FlushFileBuffers is unsupported.
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(windows)]
#[path = "platform_fs_windows.rs"]
mod windows;

/// Canonicalizes Windows policy names while rejecting aliases the policy cannot represent.
#[cfg(windows)]
pub fn permission_path(path: &Path) -> io::Result<String> {
    windows::permission_path(path)
}

/// Keeps SQLite's private database and Windows ancestor names pinned for its lifetime.
pub struct DatabaseGuard {
    #[allow(dead_code)]
    files: Vec<File>,
}

/// Validates the database while permitting SQLite's read/write handles, never deletion.
pub fn database_guard(path: &Path) -> io::Result<DatabaseGuard> {
    #[cfg(windows)]
    {
        windows::database_guard(path).map(|files| DatabaseGuard { files })
    }
    #[cfg(unix)]
    {
        let file = open_read_nofollow(path)?;
        if !private_regular_file(&file)? {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(DatabaseGuard { files: vec![file] })
    }
}

/// Retains every ancestor and the leaf throughout a brokered filesystem operation.
#[cfg(windows)]
pub struct GuardedPath {
    pub file: File,
    pub path: std::path::PathBuf,
    pub key: String,
    _parents: Vec<File>,
}

/// Opens a stable local path and rejects reparse or case-sensitive directories.
#[cfg(windows)]
pub fn guarded_path(path: &Path, write: bool, directory: bool) -> io::Result<GuardedPath> {
    windows::guarded_path(path, write, directory)
}

/// Creates an absent leaf underneath an already-pinned parent after policy approval.
#[cfg(windows)]
pub fn guarded_create(parent: &GuardedPath, name: &std::ffi::OsStr) -> io::Result<GuardedPath> {
    windows::guarded_create(parent, name)
}

/// Pins a policy object while allowing the container to open its data for writing.
#[cfg(windows)]
pub fn guarded_acl_path(path: &Path, directory: bool) -> io::Result<GuardedPath> {
    windows::guarded_acl_path(path, directory)
}

/// Pins a regular file for an ACL grant and lists every hard-link name it has.
#[cfg(windows)]
pub(crate) fn guarded_linked_file(path: &Path) -> io::Result<(GuardedPath, Vec<String>)> {
    windows::guarded_linked_file(path)
}

/// Reports whether a listed hard-link name opens the same file as an open handle.
#[cfg(windows)]
pub(crate) fn names_file(file: &File, name: &str) -> bool {
    windows::names_file(file, name)
}

#[cfg(windows)]
impl GuardedPath {
    /// Returns stable volume/file identifiers for the leaf and every pinned ancestor.
    pub fn identities(&self) -> io::Result<Vec<(u64, u128)>> {
        self._parents
            .iter()
            .chain(std::iter::once(&self.file))
            .map(file_identity)
            .collect()
    }
}

/// Pins a regular Windows file for recoverable deletion without path re-resolution.
#[cfg(windows)]
pub(crate) fn guarded_delete_path(path: &Path) -> io::Result<GuardedPath> {
    windows::guarded_delete_path(path)
}

/// Marks the exact open Windows object for deletion after its replacement is durable.
#[cfg(windows)]
pub(crate) fn delete_open_file(file: &File) -> io::Result<()> {
    windows::delete_open_file(file)
}

#[cfg(test)]
mod plain_disk_form_tests {
    use super::plain_disk_form;

    /// Only the verbatim disk prefix goes; every other spelling stays as it was.
    #[test]
    fn only_the_verbatim_disk_prefix_is_removed() {
        assert_eq!(plain_disk_form(r"\\?\C:\Users\S\chat"), r"C:\Users\S\chat");
        assert_eq!(plain_disk_form(r"\\?\d:\"), r"d:\");
        assert_eq!(plain_disk_form(r"C:\Users\S\chat"), r"C:\Users\S\chat");
        assert_eq!(plain_disk_form(r"\\?\UNC\server\share"), r"\\?\UNC\server\share");
        assert_eq!(plain_disk_form(r"\\?\Volume{0}\x"), r"\\?\Volume{0}\x");
        assert_eq!(plain_disk_form(r"\\?\C:"), r"\\?\C:");
        assert_eq!(plain_disk_form(r"\\.\C:\x"), r"\\.\C:\x");
        assert_eq!(plain_disk_form("/private/var/folders/x/chat"), "/private/var/folders/x/chat");
    }
}

#[cfg(test)]
mod path_within_tests {
    use super::path_within;

    /// A folder contains itself and what is beneath it, never a sibling that shares its prefix.
    #[test]
    fn whole_components_only() {
        let root = std::env::temp_dir().join(format!("mc-within-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("Work").join("sub")).unwrap();
        std::fs::create_dir_all(root.join("Workshop")).unwrap();
        let work = root.join("Work");
        let work = work.to_str().unwrap();
        assert!(path_within(work, work));
        assert!(path_within(root.join("Work").join("sub").to_str().unwrap(), work));
        assert!(path_within(root.join("Work").join("new.txt").to_str().unwrap(), work), "a file not created yet");
        assert!(!path_within(root.join("Workshop").to_str().unwrap(), work));
        assert!(!path_within(root.to_str().unwrap(), work));
        // Windows names one folder in many spellings: forward slashes, another case.
        #[cfg(windows)] {
            let other = root.join("work").join("SUB").to_str().unwrap().replace('\\', "/");
            assert!(path_within(&other, work), "{other}");
            assert!(path_within(&format!("\\\\?\\{}", root.join("Work").join("sub").display()), work));
        }
        std::fs::remove_dir_all(&root).unwrap();
    }
}

/// MiniClaw's data folder: `MINICLAW_DATA_DIR` when setup passed one, otherwise the platform's
/// per-user application storage.
pub fn data_directory() -> std::path::PathBuf {
    use std::{env, path::PathBuf};
    if let Some(path) = env::var_os("MINICLAW_DATA_DIR") { return PathBuf::from(path); }
    #[cfg(windows)]
    { return env::var_os("LOCALAPPDATA").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env::var_os("USERPROFILE").unwrap_or_default())
            .join("AppData/Local")).join("MiniClaw"); }
    #[cfg(target_os = "linux")]
    { return env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|path| path.is_absolute())
        .unwrap_or_else(|| PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".local/share"))
        .join("miniclaw"); }
    #[allow(unreachable_code)]
    PathBuf::from(env::var_os("HOME").unwrap_or_default()).join("Library/Application Support/MiniClaw")
}
