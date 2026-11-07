//! Native Windows storage primitives. No shell, inherited DACL or device namespace.
// Reuses platform-independent validation and native file wrappers.
use super::*;
// Converts checked Windows paths and owned handles.
use std::{
    ffi::c_void,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle},
    },
    ptr,
};
// Creates files with explicit security and reads handle identity.
use windows_sys::Win32::{
    Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE, LocalFree},
    Security::{self, Authorization::*, *},
    Storage::FileSystem::*,
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

struct Descriptor(*mut c_void);
impl Drop for Descriptor {
    /// Releases the descriptor allocated by a native security API.
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

/// Requests data/list access so no-delete sharing actually pins the object's name.
fn metadata_pin_access(directory: bool) -> u32 {
    FILE_READ_ATTRIBUTES
        | READ_CONTROL
        | if directory {
            FILE_LIST_DIRECTORY
        } else {
            FILE_READ_DATA
        }
}

/// Encodes a plain local drive path, refusing all alternate namespaces.
fn wide(path: &Path) -> io::Result<Vec<u16>> {
    // Rust canonicalize returns a verbatim disk prefix. Accept only that internal
    // equivalent of a plain drive path; device and UNC prefixes remain refused.
    let text = path.to_str().ok_or(io::ErrorKind::InvalidInput)?;
    let plain = text.strip_prefix(r"\\?\").unwrap_or(text);
    if !super::plain_windows_path(plain) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(path.as_os_str().encode_wide().chain(Some(0)).collect())
}

/// Reads the current account SID into an aligned token-information allocation.
fn user() -> io::Result<Vec<usize>> {
    unsafe {
        let mut token = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut length = 0;
        GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut length);
        let mut bytes = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        let result = GetTokenInformation(
            token,
            TokenUser,
            bytes.as_mut_ptr().cast(),
            length,
            &mut length,
        );
        CloseHandle(token);
        if result == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(bytes)
    }
}

/// Builds a protected DACL that gives only the current account full access.
fn descriptor() -> io::Result<Descriptor> {
    let token = user()?;
    unsafe {
        let sid = (*(token.as_ptr().cast::<TOKEN_USER>())).User.Sid;
        let mut text = ptr::null_mut();
        if ConvertSidToStringSidW(sid, &mut text) == 0 {
            return Err(io::Error::last_os_error());
        }
        let allocation = Descriptor(text.cast());
        let mut length = 0;
        while *text.add(length) != 0 {
            length += 1;
        }
        let sid = String::from_utf16(std::slice::from_raw_parts(text, length))
            .map_err(io::Error::other)?;
        drop(allocation);
        let sddl: Vec<u16> = format!("O:{sid}D:P(A;OICI;FA;;;{sid})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut output = ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut output,
            ptr::null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Descriptor(output))
    }
}

/// Refuses a missing DACL, a foreign owner, or access allowed to any other account.
fn private_acl(file: &File) -> io::Result<bool> {
    let token = user()?;
    unsafe {
        let sid = (*(token.as_ptr().cast::<TOKEN_USER>())).User.Sid;
        let mut owner = ptr::null_mut();
        let mut dacl: *mut ACL = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        let status = GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        );
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        let _allocation = Descriptor(descriptor);
        if owner.is_null() || dacl.is_null() || EqualSid(owner, sid) == 0 {
            return Ok(false);
        }
        let mut control = 0;
        let mut revision = 0;
        if GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) == 0
            || control & SE_DACL_PROTECTED == 0
            || (*dacl).AceCount != 1
        {
            return Ok(false);
        }
        let mut ace = ptr::null_mut();
        if GetAce(dacl, 0, &mut ace) == 0 {
            return Err(io::Error::last_os_error());
        }
        let allowed = ace.cast::<ACCESS_ALLOWED_ACE>();
        Ok((*allowed).Header.AceType == 0
            && EqualSid(ptr::addr_of!((*allowed).SidStart).cast_mut().cast(), sid) != 0
            && (*allowed).Mask & FILE_ALL_ACCESS == FILE_ALL_ACCESS)
    }
}

/// Opens a checked path without traversing its final reparse point.
fn native_open(
    path: &Path,
    write: bool,
    directory: bool,
    create: bool,
    private: bool,
) -> io::Result<File> {
    let path = wide(path)?;
    let descriptor = if private { Some(descriptor()?) } else { None };
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.as_ref().map_or(ptr::null_mut(), |item| item.0),
        bInheritHandle: 0,
    };
    let flags = FILE_FLAG_OPEN_REPARSE_POINT
        | if directory {
            FILE_FLAG_BACKUP_SEMANTICS
        } else {
            0
        };
    // Directory pins protect names with no-delete sharing while allowing child-entry updates.
    unsafe {
        let handle = CreateFileW(
            path.as_ptr(),
            if directory {
                metadata_pin_access(true)
            } else {
                GENERIC_READ | if write { GENERIC_WRITE } else { 0 }
            },
            FILE_SHARE_READ | if directory { FILE_SHARE_WRITE } else { 0 }
                | if create { FILE_SHARE_DELETE } else { 0 },
            &attributes,
            if create { CREATE_NEW } else { OPEN_EXISTING },
            flags,
            ptr::null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let file = File::from_raw_handle(handle);
        let metadata = file.metadata()?;
        if super::is_link(&metadata)
            || directory != metadata.is_dir()
            || (!directory && !metadata.is_file())
        {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        if !directory && information(&file)?.nNumberOfLinks != 1 {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(file)
    }
}

/// Opens each ancestor relative to its preceding handle, preventing namespace re-resolution.
fn parents(path: &Path) -> io::Result<Vec<File>> {
    wide(path)?;
    let mut ancestors: Vec<_> = path.ancestors().skip(1).collect();
    ancestors.reverse();
    let mut guards = Vec::new();
    for ancestor in ancestors {
        let file = if let Some(parent) = guards.last() {
            relative_open(
                parent,
                ancestor.file_name().ok_or(io::ErrorKind::InvalidInput)?,
                metadata_pin_access(true),
                true,
                false,
                false,
                false,
            )?
        } else {
            native_open(ancestor, false, true, false, false)?
        };
        guards.push(file);
    }
    Ok(guards)
}

/// Opens one component beneath a native parent handle, never following reparse points.
/// A file with more than one hard link is refused.
fn relative_open(
    parent: &File,
    name: &std::ffi::OsStr,
    access: u32,
    directory: bool,
    create: bool,
    private: bool,
    share_write: bool,
) -> io::Result<File> {
    let file = relative_any(parent, name, access, directory, create, private, share_write)?;
    if !directory && information(&file)?.nNumberOfLinks != 1 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(file)
}

/// As `relative_open`, but leaves the decision about hard links to the caller.
fn relative_any(
    parent: &File,
    name: &std::ffi::OsStr,
    access: u32,
    directory: bool,
    create: bool,
    private: bool,
    share_write: bool,
) -> io::Result<File> {
    use windows_sys::{
        Wdk::{Foundation::OBJECT_ATTRIBUTES, Storage::FileSystem as nt},
        Win32::{
            Foundation::{OBJ_CASE_INSENSITIVE, RtlNtStatusToDosError, UNICODE_STRING},
            System::IO::IO_STATUS_BLOCK,
        },
    };
    let text = name.to_str().ok_or(io::ErrorKind::InvalidInput)?;
    if text.is_empty() || text.contains(['\\', '/', ':', '\0']) || matches!(text, "." | "..") {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut wide: Vec<u16> = name.encode_wide().collect();
    let length = u16::try_from(wide.len() * 2).map_err(|_| io::ErrorKind::InvalidInput)?;
    let mut name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: wide.as_mut_ptr(),
    };
    let descriptor = if private { Some(descriptor()?) } else { None };
    let attrs = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &mut name,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: descriptor.as_ref().map_or(ptr::null(), |d| d.0.cast()),
        SecurityQualityOfService: ptr::null_mut(),
    };
    let mut status: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
    let mut handle = ptr::null_mut();
    let result = unsafe {
        nt::NtCreateFile(
            &mut handle,
            access | SYNCHRONIZE,
            &attrs,
            &mut status,
            ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            FILE_SHARE_READ | if share_write || directory { FILE_SHARE_WRITE } else { 0 },
            if create {
                nt::FILE_CREATE
            } else {
                nt::FILE_OPEN
            },
            nt::FILE_OPEN_REPARSE_POINT
                | nt::FILE_SYNCHRONOUS_IO_NONALERT
                | if directory {
                    nt::FILE_DIRECTORY_FILE
                } else {
                    nt::FILE_NON_DIRECTORY_FILE
                },
            ptr::null(),
            0,
        )
    };
    if result < 0 {
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(result) } as i32,
        ));
    }
    let file = unsafe { File::from_raw_handle(handle) };
    let metadata = file.metadata()?;
    if super::is_link(&metadata) || metadata.is_dir() != directory || (!directory && !metadata.is_file())
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(file)
}

/// Uses a pinned parent for every leaf except the already-open drive root itself.
fn leaf(
    path: &Path,
    parents: &[File],
    access: u32,
    directory: bool,
    create: bool,
    private: bool,
    share_write: bool,
) -> io::Result<File> {
    if let Some(parent) = parents.last() {
        relative_open(
            parent,
            path.file_name().ok_or(io::ErrorKind::InvalidInput)?,
            access,
            directory,
            create,
            private,
            share_write,
        )
    } else {
        if !directory || create {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        native_open(path, false, true, false, false)
    }
}

/// Opens a leaf while ancestor handles prevent junction replacement and directory renames.
pub(super) fn open_existing(path: &Path, write: bool, directory: bool) -> io::Result<File> {
    let _parents = parents(path)?;
    native_open(path, write, directory, false, false)
}

/// Creates a new file with a private DACL when requested.
pub(super) fn create_file(path: &Path, private: bool, _new: bool) -> io::Result<File> {
    let _parents = parents(path)?;
    let file = native_open(path, true, false, true, private)?;
    if private && !private_regular_file(&file)? {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(file)
}

/// Reads file identity and hard-link count from an open handle.
fn information(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    unsafe {
        let mut info = std::mem::zeroed();
        if GetFileInformationByHandle(file.as_raw_handle(), &mut info) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(info)
    }
}

/// Verifies a regular private marker without trusting path metadata.
pub(super) fn private_regular_file(file: &File) -> io::Result<bool> {
    let metadata = file.metadata()?;
    Ok(metadata.is_file()
        && !super::is_link(&metadata)
        && information(file)?.nNumberOfLinks == 1
        && private_acl(file)?)
}

/// Creates each missing private directory with its final DACL at creation time.
pub(super) fn private_dir(path: &Path, recursive: bool) -> io::Result<()> {
    if recursive && !path.parent().is_some_and(Path::exists) {
        private_dir(path.parent().ok_or(io::ErrorKind::InvalidInput)?, true)?;
    }
    let _parents = parents(path)?;
    let descriptor = descriptor()?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    unsafe {
        if CreateDirectoryW(wide(path)?.as_ptr(), &attributes) == 0 {
            let error = io::Error::last_os_error();
            if !recursive || error.kind() != io::ErrorKind::AlreadyExists {
                return Err(error);
            }
        }
    }
    let file = native_open(path, false, true, false, false)?;
    if !private_acl(&file)? {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(())
}

/// Identifies a file using the full volume serial and 128-bit filesystem ID.
pub(super) fn file_identity(file: &File) -> io::Result<(u64, u128)> {
    let mut info: FILE_ID_INFO = unsafe { std::mem::zeroed() };
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok((
        info.VolumeSerialNumber,
        u128::from_le_bytes(info.FileId.Identifier),
    ))
}

/// Reads available space for the current account on a local drive.
pub(super) fn free_space(path: &Path) -> io::Result<u64> {
    let mut free = 0;
    unsafe {
        if GetDiskFreeSpaceExW(
            wide(path)?.as_ptr(),
            &mut free,
            ptr::null_mut(),
            ptr::null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(free)
}

/// Refuses per-directory case sensitivity before case-normalizing permission keys.
fn case_insensitive(file: &File) -> io::Result<()> {
    let mut info = FILE_CASE_SENSITIVE_INFO { Flags: 0 };
    unsafe {
        if GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileCaseSensitiveInfo,
            (&mut info as *mut FILE_CASE_SENSITIVE_INFO).cast(),
            size_of::<FILE_CASE_SENSITIVE_INFO>() as u32,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    if info.Flags != 0 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(())
}

/// Returns the normalized long drive path from a pinned handle.
fn final_path(file: &File) -> io::Result<String> {
    let mut buffer = vec![0u16; 32768];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
        )
    };
    if length == 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize >= buffer.len() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let text = String::from_utf16(&buffer[..length as usize]).map_err(io::Error::other)?;
    let text = text
        .strip_prefix(r"\\?\")
        .ok_or(io::ErrorKind::InvalidInput)?;
    if !super::plain_windows_path(text) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(text.to_owned())
}

/// Normalizes existing aliases, or a new plain suffix beneath a pinned canonical parent.
pub(super) fn permission_path(path: &Path) -> io::Result<String> {
    wide(path)?;
    let mut existing = path.to_path_buf();
    let mut suffix = Vec::new();
    loop {
        let metadata = fs::symlink_metadata(&existing);
        match metadata {
            Ok(metadata) => {
                let parents = parents(&existing)?;
                for parent in &parents {
                    case_insensitive(parent)?;
                }
                if super::is_link(&metadata) {
                    return Err(io::ErrorKind::PermissionDenied.into());
                }
                let file = leaf(
                    &existing,
                    &parents,
                    metadata_pin_access(metadata.is_dir()),
                    metadata.is_dir(),
                    false,
                    false,
                    true,
                )?;
                if metadata.is_dir() {
                    case_insensitive(&file)?;
                }
                if !suffix.is_empty() && !metadata.is_dir() {
                    return Err(io::ErrorKind::NotADirectory.into());
                }
                let mut resolved = std::path::PathBuf::from(final_path(&file)?);
                for name in suffix.iter().rev() {
                    resolved.push(name);
                }
                return Ok(resolved
                    .to_str()
                    .ok_or(io::ErrorKind::InvalidInput)?
                    .replace('/', "\\")
                    .to_owned());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let name = existing
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or(io::ErrorKind::InvalidInput)?;
                // Any language is a name; only the shape of a generated 8.3 alias is refused.
                if !super::new_windows_leaf(name) {
                    return Err(io::ErrorKind::Unsupported.into());
                }
                suffix.push(name.to_owned());
                existing = existing
                    .parent()
                    .ok_or(io::ErrorKind::InvalidInput)?
                    .to_path_buf();
            }
            Err(error) => return Err(error),
        }
    }
}

/// Pins SQLite's database and ancestors while permitting SQLite's independent data handles.
pub(super) fn database_guard(path: &Path) -> io::Result<Vec<File>> {
    let mut guards = parents(path)?;
    unsafe {
        let handle = CreateFileW(
            wide(path)?.as_ptr(),
            metadata_pin_access(false),
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let file = File::from_raw_handle(handle);
        if !private_regular_file(&file)? {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        guards.push(file);
    }
    Ok(guards)
}

/// Holds every path component while deriving the actual opened object's policy name.
pub(super) fn guarded_path(path: &Path, write: bool, directory: bool) -> io::Result<GuardedPath> {
    let guards = parents(path)?;
    for guard in &guards {
        case_insensitive(guard)?;
    }
    let file = leaf(
        path,
        &guards,
        if directory {
            metadata_pin_access(true)
        } else {
            GENERIC_READ | if write { GENERIC_WRITE } else { 0 }
        },
        directory,
        false,
        false,
        false,
    )?;
    if directory {
        case_insensitive(&file)?;
    }
    let resolved = final_path(&file)?;
    let key = resolved.replace('/', "\\").to_owned();
    Ok(GuardedPath {
        file,
        path: resolved.into(),
        key,
        _parents: guards,
    })
}

/// Holds the exact regular file and ancestors for a recoverable user-requested deletion.
pub(super) fn guarded_delete_path(path: &Path) -> io::Result<GuardedPath> {
    let guards = parents(path)?;
    for guard in &guards {
        case_insensitive(guard)?;
    }
    let file = leaf(
        path,
        &guards,
        GENERIC_READ | DELETE,
        false,
        false,
        false,
        false,
    )?;
    let key = final_path(&file)?;
    Ok(GuardedPath {
        file,
        path: key.clone().into(),
        key,
        _parents: guards,
    })
}

/// Deletes only the already-open object after its recovery copy has been synced.
pub(super) fn delete_open_file(file: &File) -> io::Result<()> {
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&info as *const FILE_DISPOSITION_INFO).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Creates only a plain child of the pinned directory, without replacing any existing file.
pub(super) fn guarded_create(
    parent: &GuardedPath,
    name: &std::ffi::OsStr,
) -> io::Result<GuardedPath> {
    let name = name.to_str().ok_or(io::ErrorKind::InvalidInput)?;
    if !super::new_windows_leaf(name) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    case_insensitive(&parent.file)?;
    let path = parent.path.join(name);
    wide(&path)?;
    let file = relative_open(
        &parent.file,
        name.as_ref(),
        GENERIC_READ | GENERIC_WRITE | DELETE,
        false,
        true,
        false,
        false,
    )?;
    // The new object must carry exactly the requested name. Anything else means the name
    // was an alias of some kind, and the file just created here is taken away again.
    let resolved = final_path(&file);
    let named = resolved
        .as_ref()
        .is_ok_and(|resolved| Path::new(resolved).file_name().and_then(|leaf| leaf.to_str()) == Some(name));
    if !named {
        delete_open_file(&file)?;
        return Err(resolved.err().unwrap_or_else(|| io::ErrorKind::PermissionDenied.into()));
    }
    let resolved = resolved?;
    let mut guards = Vec::with_capacity(parent._parents.len() + 1);
    for guard in &parent._parents {
        guards.push(guard.try_clone()?);
    }
    guards.push(parent.file.try_clone()?);
    Ok(GuardedPath {
        file,
        key: resolved.to_owned(),
        path: resolved.into(),
        _parents: guards,
    })
}

/// Retains namespace identity without excluding separately authorized data writers.
pub(super) fn guarded_acl_path(path: &Path, directory: bool) -> io::Result<GuardedPath> {
    let guards = parents(path)?;
    for guard in &guards {
        case_insensitive(guard)?;
    }
    let file = leaf(
        path,
        &guards,
        metadata_pin_access(directory),
        directory,
        false,
        false,
        true,
    )?;
    let metadata = file.metadata()?;
    if super::is_link(&metadata)
        || metadata.is_dir() != directory
        || (!directory && (!metadata.is_file() || information(&file)?.nNumberOfLinks != 1))
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    if directory {
        case_insensitive(&file)?;
    }
    let resolved = final_path(&file)?;
    Ok(GuardedPath {
        file,
        key: resolved.to_owned(),
        path: resolved.into(),
        _parents: guards,
    })
}

/// Copies the text before the first NUL of a buffer Windows filled.
fn terminated(buffer: &[u16]) -> io::Result<String> {
    let length = buffer.iter().position(|unit| *unit == 0).ok_or(io::ErrorKind::InvalidData)?;
    String::from_utf16(&buffer[..length]).map_err(io::Error::other)
}

/// Lists every name of a pinned file as a plain drive path, its own name included.
///
/// Windows reports each name relative to the volume; it is joined to the mount point of the
/// pinned path. The list must match the link count read from the handle and contain the
/// pinned path. A volume that cannot list names, or any disagreement, is a refusal.
fn link_names(file: &File, key: &str) -> io::Result<Vec<String>> {
    let count = information(file)?.nNumberOfLinks as usize;
    if count == 1 {
        return Ok(vec![key.to_owned()]);
    }
    let refused = || {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Windows could not list every name of a hard-linked file, so it cannot be granted",
        )
    };
    let path = wide(Path::new(key))?;
    let mut buffer = vec![0u16; 32768];
    if unsafe { GetVolumePathNameW(path.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) } == 0 {
        return Err(refused());
    }
    let volume = terminated(&buffer)?;
    let volume = volume.trim_end_matches('\\');
    buffer.fill(0);
    let mut length = buffer.len() as u32;
    let find = unsafe { FindFirstFileNameW(path.as_ptr(), 0, &mut length, buffer.as_mut_ptr()) };
    if find == INVALID_HANDLE_VALUE {
        return Err(refused());
    }
    let mut names = Vec::new();
    let listed = loop {
        let Ok(name) = terminated(&buffer) else { break false };
        // 1023 other names is the NTFS limit; anything beyond the handle's count is wrong.
        if !name.starts_with('\\') || names.len() >= count {
            break false;
        }
        names.push(format!("{volume}{name}"));
        buffer.fill(0);
        length = buffer.len() as u32;
        if unsafe { FindNextFileNameW(find, &mut length, buffer.as_mut_ptr()) } == 0 {
            // ERROR_HANDLE_EOF ends the list.
            break io::Error::last_os_error().raw_os_error() == Some(38);
        }
    };
    unsafe {
        FindClose(find);
    }
    let plain = names.iter().all(|name| super::plain_windows_path(name));
    let own = names.iter().filter(|name| name.eq_ignore_ascii_case(key)).count() == 1;
    if !listed || !plain || !own || names.len() != count {
        return Err(refused());
    }
    Ok(names)
}

/// Pins a regular file for an ACL grant and returns every hard-link name it has.
///
/// One name means an ordinary file. With more, the caller must decide from the names whether
/// the grant stays inside what the user approved; this function refuses nothing on that ground.
pub(super) fn guarded_linked_file(path: &Path) -> io::Result<(GuardedPath, Vec<String>)> {
    let guards = parents(path)?;
    for guard in &guards {
        case_insensitive(guard)?;
    }
    let parent = guards.last().ok_or(io::ErrorKind::InvalidInput)?;
    let name = path.file_name().ok_or(io::ErrorKind::InvalidInput)?;
    let file = relative_any(parent, name, metadata_pin_access(false), false, false, false, true)?;
    let resolved = final_path(&file)?;
    let names = link_names(&file, &resolved)?;
    Ok((
        GuardedPath {
            file,
            key: resolved.to_owned(),
            path: resolved.into(),
            _parents: guards,
        },
        names,
    ))
}

/// Confirms that a listed link name opens the pinned file itself.
pub(super) fn names_file(file: &File, name: &str) -> bool {
    let Ok(path) = wide(Path::new(name)) else { return false };
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return false;
    }
    let other = unsafe { File::from_raw_handle(handle) };
    matches!((file_identity(file), file_identity(&other)), (Ok(left), Ok(right)) if left == right)
}
