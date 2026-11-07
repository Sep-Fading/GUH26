//! Removes one run's ACL entries through verified handles, whatever the run left behind.
// Reuses journal identities and the exact-object ACL writer.
use super::*;
// Queues children and remembers recorded identities.
use std::{
    collections::{HashSet, VecDeque},
    os::windows::ffi::OsStringExt,
    rc::Rc,
};
// Opens children relative to a verified parent handle.
use windows_sys::{
    Wdk::{Foundation::OBJECT_ATTRIBUTES, Storage::FileSystem as nt},
    Win32::{
        Foundation::{GetLastError, RtlNtStatusToDosError, UNICODE_STRING},
        System::IO::IO_STATUS_BLOCK,
    },
};

/// Record lines a journal may hold before discovered children stop being added.
const RECORD_LINES: usize = 250_000;
/// Bytes of records a journal may hold; the reader accepts strictly more than this.
pub(super) const RECORD_BYTES: u64 = 64 * 1024 * 1024;
/// Children handled between journal flushes and ACL-lock acquisitions.
const BATCH: usize = 512;

const FILE_ATTRIBUTE_DIRECTORY_BIT: u32 = 0x10;
const FILE_ATTRIBUTE_REPARSE_BIT: u32 = 0x400;

/// Knows which identities a record file already holds, so retries never repeat them.
#[derive(Default)]
pub(super) struct Ledger {
    known: HashSet<((u64, u128), bool)>,
    lines: usize,
    limit: Option<usize>,
    saturated: bool,
}

impl Ledger {
    /// Starts from the records and line count of an existing journal.
    pub(super) fn resume(records: &[Entry], lines: usize) -> Self {
        Self {
            known: records
                .iter()
                .map(|entry| (entry.identity, entry.inheritance_root))
                .collect(),
            lines,
            ..Self::default()
        }
    }

    /// Counts a control line such as the header or a job binding.
    pub(super) fn line(&mut self) {
        self.lines += 1;
    }

    /// Lowers the discovered-record budget so a test can reach it cheaply.
    #[cfg(test)]
    pub(super) fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Flushes unseen identities once, before the caller changes any of those objects.
    ///
    /// A required record that does not fit is an error and nothing is written. Optional records
    /// (children found during revocation) are skipped instead: their retained root finds them
    /// again, and an oversized journal would otherwise become unreadable.
    pub(super) fn append(
        &mut self,
        file: &mut File,
        entries: &[Entry],
        required: bool,
    ) -> io::Result<()> {
        let mut size = file.metadata()?.len();
        let mut bytes = Vec::new();
        let mut added = Vec::new();
        for entry in entries {
            let key = (entry.identity, entry.inheritance_root);
            if self.known.contains(&key) || added.contains(&key) {
                continue;
            }
            let start = bytes.len();
            serde_json::to_writer(&mut bytes, entry)?;
            bytes.push(b'\n');
            let fits = bytes.len() - start <= 32768
                && size + bytes.len() as u64 <= RECORD_BYTES
                && (required
                    || self.lines + added.len() < self.limit.unwrap_or(RECORD_LINES));
            if fits {
                added.push(key);
                continue;
            }
            if required {
                return Err(io::Error::other(
                    "Sandbox recovery journal exceeds its safe size limit",
                ));
            }
            bytes.truncate(start);
            if !self.saturated {
                self.saturated = true;
                tracing::warn!(
                    "Sandbox journal is full; revoking further descendants without recording them"
                );
            }
        }
        if bytes.is_empty() {
            return Ok(());
        }
        file.seek(SeekFrom::End(0))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        size += bytes.len() as u64;
        debug_assert!(size <= RECORD_BYTES);
        self.lines += added.len();
        self.known.extend(added);
        Ok(())
    }
}

/// Reopens a recorded object by full ID; a deleted object has no remaining ACL to revoke.
///
/// The handle names the object itself even if it has since become a link, so its own entry
/// can still be removed. Nothing here resolves a path.
pub(super) fn reopen(saved: &Entry) -> io::Result<Option<File>> {
    let root = saved
        .path
        .ancestors()
        .last()
        .ok_or(io::ErrorKind::InvalidInput)?;
    let volume = platform_fs::guarded_acl_path(root, true)?;
    if platform_fs::file_identity(&volume.file)?.0 != saved.identity.0 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let id = FILE_ID_DESCRIPTOR {
        dwSize: std::mem::size_of::<FILE_ID_DESCRIPTOR>() as u32,
        Type: ExtendedFileIdType,
        Anonymous: FILE_ID_DESCRIPTOR_0 {
            ExtendedFileId: FILE_ID_128 {
                Identifier: saved.identity.1.to_le_bytes(),
            },
        },
    };
    let open = |access: u32| unsafe {
        OpenFileById(
            volume.file.as_raw_handle(),
            &id,
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
        )
    };
    let access = READ_CONTROL | WRITE_DAC | FILE_READ_ATTRIBUTES;
    let mut handle = INVALID_HANDLE_VALUE;
    for attempt in 0..RETRIES {
        // Listing access lets a retained root enumerate its own children by handle.
        handle = open(access | if saved.inheritance_root { FILE_LIST_DIRECTORY } else { 0 });
        if handle != INVALID_HANDLE_VALUE || !transient(unsafe { GetLastError() }) {
            break;
        }
        pause(attempt);
    }
    if handle == INVALID_HANDLE_VALUE {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(2) {
            Ok(None)
        } else {
            Err(error)
        };
    }
    let file = unsafe { File::from_raw_handle(handle) };
    if platform_fs::file_identity(&file)? != saved.identity {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(Some(file))
}

/// Finds a recorded object by name when the filesystem cannot open it by ID.
///
/// Ancestors stay pinned and link-free; the leaf may be a link or multiply linked file because
/// only its own entry is removed. A different or missing object yields `None`.
pub(super) fn by_path(saved: &Entry) -> io::Result<Option<File>> {
    let (Some(parent), Some(name)) = (saved.path.parent(), saved.path.file_name()) else {
        return match platform_fs::guarded_acl_path(&saved.path, saved.directory) {
            Ok(pin) if platform_fs::file_identity(&pin.file)? == saved.identity => {
                Ok(Some(acl_handle(&pin, saved.directory)?))
            }
            Ok(_) => Ok(None),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        };
    };
    let pin = match platform_fs::guarded_acl_path(parent, true) {
        Ok(pin) => pin,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let name: Vec<u16> = name.encode_wide().collect();
    let Some(file) = open_child(&pin.file, &name, saved.directory)? else {
        return Ok(None);
    };
    if platform_fs::file_identity(&file)? != saved.identity {
        return Ok(None);
    }
    Ok(Some(file))
}

const RETRIES: u32 = 6;

/// Sharing and lock conflicts end when the other program closes its handle.
fn transient(error: u32) -> bool {
    matches!(error, 32 | 33)
}

/// Backs off for up to about 0.6 seconds across all attempts.
fn pause(attempt: u32) {
    std::thread::sleep(std::time::Duration::from_millis(20 << attempt));
}

/// Identifies a child this user cannot open for ACL access, through the one right Windows gives
/// whoever may list its folder (`FILE_READ_ATTRIBUTES`). The handle can change nothing; it only
/// proves that the name is a real object and which one. `None` when the name no longer exists.
///
/// Exactly that right is requested: no `SYNCHRONIZE` (and so no synchronous I/O), which only the
/// child's own list could grant. An Administrators-only folder refused the open that asked for it
/// (second test guest, 2026-10-06).
fn identify_child(parent: &File, name: &[u16]) -> io::Result<Option<(u32, (u64, u128))>> {
    let length = u16::try_from(name.len() * 2).map_err(|_| io::ErrorKind::InvalidInput)?;
    if name.is_empty() || name.contains(&(b'\\' as u16)) || name.contains(&(b'/' as u16)) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut text = name.to_vec();
    let mut name = UNICODE_STRING { Length: length, MaximumLength: length, Buffer: text.as_mut_ptr() };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &mut name,
        Attributes: 0,
        SecurityDescriptor: ptr::null(),
        SecurityQualityOfService: ptr::null_mut(),
    };
    let mut status: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
    let mut handle = ptr::null_mut();
    let result = unsafe {
        nt::NtCreateFile(
            &mut handle,
            FILE_READ_ATTRIBUTES,
            &attributes,
            &mut status,
            ptr::null(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            nt::FILE_OPEN,
            nt::FILE_OPEN_REPARSE_POINT,
            ptr::null(),
            0,
        )
    };
    if result < 0 {
        let error = unsafe { RtlNtStatusToDosError(result) };
        return if matches!(error, 2 | 3) { Ok(None) } else { Err(io::Error::from_raw_os_error(error as i32)) };
    }
    let file = unsafe { File::from_raw_handle(handle) };
    Ok(Some((self::attributes(&file)?, platform_fs::file_identity(&file)?)))
}

/// Identifies a named child of an open folder without ACL access (for tests of the journal).
#[cfg(test)]
pub(super) fn identify(parent: &File, name: &[u16]) -> io::Result<Option<(u64, u128)>> {
    Ok(identify_child(parent, name)?.map(|(_, identity)| identity))
}

/// Adds the object's path to an error, keeping its kind.
fn at(path: &Path, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}

/// Opens one named child of a verified directory handle, never following a link.
///
/// Returns `None` when the name no longer exists. The handle carries only ACL and attribute
/// access (plus listing for directories), which does not conflict with other programs' data
/// sharing modes.
fn open_child(parent: &File, name: &[u16], list: bool) -> io::Result<Option<File>> {
    let length = u16::try_from(name.len() * 2).map_err(|_| io::ErrorKind::InvalidInput)?;
    if name.is_empty() || name.contains(&(b'\\' as u16)) || name.contains(&(b'/' as u16)) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut text = name.to_vec();
    let mut name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: text.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &mut name,
        // Listed names are exact; a case-sensitive folder may hold two that differ only by case.
        Attributes: 0,
        SecurityDescriptor: ptr::null(),
        SecurityQualityOfService: ptr::null_mut(),
    };
    for attempt in 0..RETRIES {
        let mut status: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
        let mut handle = ptr::null_mut();
        let result = unsafe {
            nt::NtCreateFile(
                &mut handle,
                READ_CONTROL
                    | WRITE_DAC
                    | FILE_READ_ATTRIBUTES
                    | SYNCHRONIZE
                    | if list { FILE_LIST_DIRECTORY } else { 0 },
                &attributes,
                &mut status,
                ptr::null(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                nt::FILE_OPEN,
                nt::FILE_OPEN_REPARSE_POINT
                    | nt::FILE_SYNCHRONOUS_IO_NONALERT
                    | nt::FILE_OPEN_FOR_BACKUP_INTENT,
                ptr::null(),
                0,
            )
        };
        if result >= 0 {
            return Ok(Some(unsafe { File::from_raw_handle(handle) }));
        }
        // STATUS_DELETE_PENDING: the name is on its way out but has not gone yet.
        let pending = result as u32 == 0xC000_0056;
        let error = unsafe { RtlNtStatusToDosError(result) };
        if matches!(error, 2 | 3) {
            return Ok(None);
        }
        if !(pending || transient(error)) || attempt + 1 == RETRIES {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        pause(attempt);
    }
    unreachable!()
}

/// Reads native attributes from the open object, not from a name.
fn attributes(file: &File) -> io::Result<u32> {
    let mut info = FILE_ATTRIBUTE_TAG_INFO {
        FileAttributes: 0,
        ReparseTag: 0,
    };
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileAttributeTagInfo,
            (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(info.FileAttributes)
}

/// A directory whose own entries can be listed; links are never descended into.
fn plain_directory(attributes: u32) -> bool {
    attributes & FILE_ATTRIBUTE_DIRECTORY_BIT != 0 && attributes & FILE_ATTRIBUTE_REPARSE_BIT == 0
}

/// Lists a directory's own entries through its handle, with the attributes Windows reports.
fn children(directory: &File) -> io::Result<Vec<(Vec<u16>, u32)>> {
    let mut buffer = vec![0u64; 8192];
    let size = (buffer.len() * size_of::<u64>()) as u32;
    let mut class = FileIdBothDirectoryRestartInfo;
    let mut found = Vec::new();
    loop {
        if unsafe {
            GetFileInformationByHandleEx(
                directory.as_raw_handle(),
                class,
                buffer.as_mut_ptr().cast(),
                size,
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            // ERROR_NO_MORE_FILES ends a listing; an entirely empty volume root reports 2.
            return if matches!(error.raw_os_error(), Some(18 | 2)) {
                Ok(found)
            } else {
                Err(error)
            };
        }
        class = FileIdBothDirectoryInfo;
        let base = buffer.as_ptr().cast::<u8>();
        let mut offset = 0usize;
        loop {
            let (next, attributes, name) = unsafe {
                let entry = base.add(offset).cast::<FILE_ID_BOTH_DIR_INFO>();
                let name = std::slice::from_raw_parts(
                    ptr::addr_of!((*entry).FileName).cast::<u16>(),
                    (*entry).FileNameLength as usize / 2,
                );
                ((*entry).NextEntryOffset as usize, (*entry).FileAttributes, name)
            };
            if name != [b'.' as u16] && name != [b'.' as u16, b'.' as u16] {
                found.push((name.to_vec(), attributes));
            }
            if next == 0 {
                break;
            }
            offset += next;
        }
    }
}

/// Reports whether the stored DACL names this trustee, without changing anything. An entry kind
/// other than allow or deny is left to the removal path, which copies it unchanged.
fn names_trustee(file: &File, sid: PSID) -> io::Result<bool> {
    // A missing DACL holds no entry for anyone, so there is nothing of ours to remove.
    let Some((acl, _)) = stored_acl(file)? else {
        return Ok(false);
    };
    let trustee = unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), GetLengthSid(sid) as usize) };
    crate::windows_sandbox::acl::names_trustee(&acl, trustee)
}

/// Takes the cross-process ACL lock on first use and keeps it for a bounded run of changes.
#[derive(Default)]
pub(in crate::windows_sandbox::native) struct Batch(
    Option<crate::windows_security_lock::Guard>,
    usize,
);

/// Removes this run's entries from one open object; returns whether any were present.
pub(in crate::windows_sandbox::native) fn strip(file: &File, sid: PSID, batch: &mut Batch) -> io::Result<bool> {
    if !names_trustee(file, sid)? {
        return Ok(false);
    }
    // Other launches wait on this lock, so it is handed back between runs of changes.
    if batch.1 >= BATCH {
        *batch = Batch::default();
    }
    if batch.0.is_none() {
        batch.0 = Some(crate::windows_security_lock::acquire(false)?);
    }
    batch.1 += 1;
    change_acl_locked(file, sid, 0, REVOKE_ACCESS, NO_INHERITANCE)?;
    // The write reports success even when Windows kept an entry; removal is claimed only when seen.
    if names_trustee(file, sid)? {
        tracing::warn!("An inherited sandbox ACL entry survived its removal; removing it with a plain write");
        revoke_plainly(file, sid)?;
        if names_trustee(file, sid)? {
            return Err(io::Error::other("A sandbox ACL entry is still present after its removal"));
        }
    }
    Ok(true)
}

struct Parent {
    file: File,
    path: PathBuf,
}

struct Found {
    file: File,
    entry: Entry,
    descend: bool,
}

/// Removes the run's entries from a retained root and everything now beneath it.
///
/// The walk has no object, depth or time cap: a run may create more than a launch could have
/// stamped, and stopping early would strand the record. Every step is idempotent, so an
/// interrupted walk simply resumes from the root. A name that vanishes mid-walk may have been
/// renamed, so the walk repeats until one pass sees a stable tree.
///
/// Returns the descendants this user could not open for ACL access (access denied) but could
/// identify: what the run's entry on them is cannot be known from here. The caller decides; the
/// walk itself treats nothing as clean that it did not see.
pub(super) fn tree(
    root: File,
    path: PathBuf,
    journal: &mut File,
    ledger: &mut Ledger,
    sid: PSID,
) -> io::Result<Vec<Entry>> {
    let root = Rc::new(Parent { file: root, path });
    for _ in 0..3 {
        let (stable, unreadable) = walk(&root, journal, ledger, sid)?;
        if stable {
            return Ok(unreadable);
        }
    }
    Err(io::Error::other(
        "The folder kept changing during permission cleanup; it will be retried",
    ))
}

/// Performs one breadth-first pass; returns whether every listed name was still present, and the
/// descendants that could be identified but not opened for ACL access.
fn walk(root: &Rc<Parent>, journal: &mut File, ledger: &mut Ledger, sid: PSID) -> io::Result<(bool, Vec<Entry>)> {
    // The root loses its inheritable entry first, so nothing created later can inherit it.
    strip(&root.file, sid, &mut Batch::default()).map_err(|error| at(&root.path, error))?;
    let mut unreadable = Vec::new();
    let mut queue = VecDeque::new();
    if plain_directory(attributes(&root.file)?) {
        for (name, attributes) in children(&root.file)? {
            queue.push_back((root.clone(), name, attributes));
        }
    }
    let mut stable = true;
    let mut failure: Option<io::Error> = None;
    let mut keep = |error: io::Error| {
        tracing::warn!(%error, "Could not remove an expired sandbox ACL entry from a descendant");
        failure.get_or_insert(error);
    };
    while !queue.is_empty() {
        let mut found = Vec::with_capacity(BATCH.min(queue.len()));
        while found.len() < BATCH {
            let Some((parent, name, listed)) = queue.pop_front() else {
                break;
            };
            let path = parent.path.join(std::ffi::OsString::from_wide(&name));
            let file = match open_child(&parent.file, &name, plain_directory(listed)) {
                Ok(Some(file)) => file,
                Ok(None) => {
                    stable = false;
                    continue;
                }
                // Not openable for ACL access: identify it without access to its contents or list.
                Err(error) if error.raw_os_error() == Some(5) => {
                    match identify_child(&parent.file, &name) {
                        Ok(Some((attributes, identity))) => unreadable.push(Entry {
                            path,
                            identity,
                            directory: attributes & FILE_ATTRIBUTE_DIRECTORY_BIT != 0,
                            inheritance_root: false,
                            discovered: true,
                        }),
                        Ok(None) => stable = false,
                        Err(second) => keep(at(&path, second)),
                    }
                    continue;
                }
                Err(error) => {
                    keep(at(&path, error));
                    continue;
                }
            };
            let opened = match (attributes(&file), platform_fs::file_identity(&file)) {
                (Ok(attributes), Ok(identity)) => (attributes, identity),
                (Err(error), _) | (_, Err(error)) => {
                    keep(at(&path, error));
                    continue;
                }
            };
            // The name changed kind after it was listed; a later pass opens it correctly.
            if plain_directory(opened.0) != plain_directory(listed) {
                stable = false;
            }
            found.push(Found {
                entry: Entry {
                    path,
                    identity: opened.1,
                    directory: opened.0 & FILE_ATTRIBUTE_DIRECTORY_BIT != 0,
                    inheritance_root: false,
                    discovered: true,
                },
                descend: plain_directory(opened.0) && plain_directory(listed),
                file,
            });
        }
        let entries: Vec<Entry> = found.iter().map(|item| item.entry.clone()).collect();
        ledger.append(journal, &entries, false)?;
        let mut batch = Batch::default();
        for item in &found {
            if let Err(error) = strip(&item.file, sid, &mut batch) {
                keep(at(&item.entry.path, error));
            }
        }
        drop(batch);
        // Each directory lost its inheritable entry above, before its children are listed.
        for item in found {
            if !item.descend {
                continue;
            }
            match children(&item.file) {
                Ok(names) => {
                    let parent = Rc::new(Parent {
                        file: item.file,
                        path: item.entry.path,
                    });
                    for (name, attributes) in names {
                        queue.push_back((parent.clone(), name, attributes));
                    }
                }
                Err(error) => keep(error),
            }
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok((stable, unreadable)),
    }
}

/// Retries a removal while another program briefly holds the object open.
fn settle(mut remove: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    let mut attempt = 0;
    loop {
        match remove() {
            Err(error)
                if error.raw_os_error().is_some_and(|code| transient(code as u32))
                    && attempt + 1 < RETRIES =>
            {
                pause(attempt);
                attempt += 1;
            }
            result => return result,
        }
    }
}

/// Removes disposable contents while the protected root cannot be replaced or renamed.
///
/// Links are removed as links: nothing they point at is opened or deleted.
pub(in crate::windows_sandbox::native) fn remove_scratch(path: &Path) -> io::Result<()> {
    use std::os::windows::fs::FileTypeExt;
    let pin = match platform_fs::guarded_acl_path(path, true) {
        Ok(pin) => pin,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    // Revocation has restored the private root; never delete a repurposed shared folder.
    platform_fs::private_dir(path, true)?;
    for entry in fs::read_dir(&pin.path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = entry.path();
        settle(|| {
            if kind.is_dir() {
                fs::remove_dir_all(&target)
            } else if kind.is_symlink_dir() {
                fs::remove_dir(&target)
            } else {
                match fs::remove_file(&target) {
                    // Never for a link: changing attributes by name would follow it.
                    Err(error) if error.kind() == io::ErrorKind::PermissionDenied && kind.is_file() => {
                        // A read-only file is still ours to delete.
                        let mut permissions = fs::symlink_metadata(&target)?.permissions();
                        if !permissions.readonly() {
                            return Err(error);
                        }
                        #[allow(clippy::permissions_set_readonly_false)]
                        permissions.set_readonly(false);
                        fs::set_permissions(&target, permissions)?;
                        fs::remove_file(&target)
                    }
                    result => result,
                }
            }
        })?;
    }
    drop(pin);
    // Only empty-directory removal remains after dropping the namespace pin.
    settle(|| fs::remove_dir(path))
}
