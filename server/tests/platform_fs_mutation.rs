// Exercises mutations against retained Windows directory handles.
#![cfg(windows)]
use miniclaw_server::platform_fs;
use std::{
    fs::{File, OpenOptions},
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
};
use windows_sys::Win32::{Storage::FileSystem::*, System::IO::DeviceIoControl};

/// Opens metadata mutation access without requesting rename or delete rights.
fn mutation_handle(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .access_mode(FILE_WRITE_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

/// Sets a mount point directly on an existing directory, avoiding path replacement.
fn set_junction(path: &Path, target: &Path) -> std::io::Result<()> {
    let file = mutation_handle(path)?;
    let substitute: Vec<u16> = format!(r"\??\{}", target.display())
        .encode_utf16()
        .collect();
    let display: Vec<u16> = target.to_str().unwrap().encode_utf16().collect();
    let names: Vec<u16> = substitute
        .iter()
        .copied()
        .chain([0])
        .chain(display.iter().copied())
        .chain([0])
        .collect();
    let mut buffer = Vec::new();
    buffer.extend(0xA0000003u32.to_le_bytes());
    buffer.extend(((names.len() * 2 + 8) as u16).to_le_bytes());
    buffer.extend(0u16.to_le_bytes());
    for field in [
        0,
        substitute.len() as u16 * 2,
        (substitute.len() as u16 + 1) * 2,
        display.len() as u16 * 2,
    ] {
        buffer.extend(field.to_le_bytes());
    }
    for unit in names {
        buffer.extend(unit.to_le_bytes());
    }
    let mut returned = 0;
    if unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            0x000900A4,
            buffer.as_ptr().cast(),
            buffer.len() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// An empty pinned directory cannot redirect an approved create into a sibling.
#[test]
fn pinned_directory_reparse_mutation() {
    let root = std::env::temp_dir().join(format!("mc-reparse-mutation-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let directory = root.join("approved");
    let outside = root.join("outside");
    std::fs::create_dir(&directory).unwrap();
    std::fs::create_dir(&outside).unwrap();
    let guard = platform_fs::guarded_path(&directory, false, true).unwrap();
    let mutation = set_junction(&directory, &outside);
    if mutation.is_ok() {
        let created = platform_fs::guarded_create(&guard, "new-file".as_ref());
        assert!(
            !outside.join("new-file").exists(),
            "Create escaped the pinned directory"
        );
        assert!(created.is_err(), "A mutated directory must be rejected");
    }
    drop(guard);
    if mutation.is_err() {
        set_junction(&directory, &outside)
            .expect("The same mutation must work after dropping the guard");
    }
    std::fs::remove_dir(&directory).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

/// Case-sensitive mutation must not bypass a previously approved case-insensitive key.
#[test]
fn pinned_directory_case_mutation() {
    let root = std::env::temp_dir().join(format!("mc-case-mutation-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let guard = platform_fs::guarded_path(&root, false, true).unwrap();
    let mutate = || -> std::io::Result<()> {
        let file = mutation_handle(&root)?;
        let info = FILE_CASE_SENSITIVE_INFO { Flags: 1 };
        if unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileCaseSensitiveInfo,
                (&info as *const FILE_CASE_SENSITIVE_INFO).cast(),
                std::mem::size_of_val(&info) as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    };
    let mutation = mutate();
    if mutation.is_ok() {
        assert!(platform_fs::guarded_create(&guard, "new-file".as_ref()).is_err());
        assert!(!root.join("new-file").exists());
    }
    drop(guard);
    if mutation.is_err() {
        mutate().expect("Case mutation must work without the pin");
    }
    std::fs::remove_dir(root).unwrap();
}
