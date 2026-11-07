#![cfg(windows)]
// Verifies Windows aliases cannot evade conversation blocks.
use miniclaw_server::{
    agents::Store,
    environment::Assignment,
    permissions::{self, Decision, Effect, Scope, Target},
    platform_fs,
};

/// A differently cased path still hits the exact block inside an allowed folder.
#[tokio::test]
async fn case_alias_keeps_exact_block() {
    let root = std::env::temp_dir().join(format!("mc-policy-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&root, false).unwrap();
    let folder = root.join("Project");
    platform_fs::private_dir(&folder, false).unwrap();
    drop(platform_fs::private_file(&folder.join("Secret.txt"), true).unwrap());
    let store = Store::open(&root.join("agents.sqlite3")).unwrap();
    let thread = uuid::Uuid::new_v4().to_string();
    store
        .assign_existing(thread.clone(), Assignment::Host {})
        .await
        .unwrap();
    let context = store.permission_context(thread).await.unwrap();
    let scope = Scope::Conversation(context.conversation());
    store
        .set_permission(
            scope,
            &Target::Folder {
                path: folder.to_str().unwrap().into(),
                action: "read".into(),
            },
            Some(Effect::Allow),
        )
        .await
        .unwrap();
    store
        .set_permission(
            scope,
            &Target::File {
                path: folder.join("Secret.txt").to_str().unwrap().into(),
                action: "read".into(),
            },
            Some(Effect::Block),
        )
        .await
        .unwrap();
    let alias = folder
        .join("secret.TXT")
        .to_str()
        .unwrap()
        .to_ascii_lowercase();
    assert_eq!(
        store
            .permission(
                &context,
                &Target::File {
                    path: alias,
                    action: "read".into()
                },
                1
            )
            .await
            .unwrap(),
        Decision::Block
    );
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

/// A name that does not exist yet keeps its own spelling beneath the pinned, normalized parent.
/// Requirement change 2026-10-04 (backend fixes after the independent review, see
/// docs/windows/review-processes.md "New file names"): any valid Unicode name may be created;
/// only the shape of a generated 8.3 alias ([MS-FSCC] 2.1.5.2.1), device names, streams and a
/// trailing dot or space are refused. New names are no longer case-folded into one key: blocked
/// names are compared with `windows_names_may_match`, which errs towards a match.
#[test]
fn new_names_are_conservative() {
    let root = std::env::temp_dir().join(format!("mc-new-policy-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&root, false).unwrap();
    let parent = platform_fs::permission_path(&root).unwrap();
    let key = |name: &str| platform_fs::permission_path(&root.join(name));
    // The existing parent is normalized even when the caller spells it in another case.
    let shouted = std::path::PathBuf::from(root.to_str().unwrap().to_uppercase()).join("New.txt");
    assert_eq!(platform_fs::permission_path(&shouted).unwrap(), format!("{parent}\\New.txt"));
    // A new leaf keeps the requested spelling; two spellings Windows could treat as one are
    // two keys here and are related by the conservative comparison used for blocked names.
    assert_eq!(key("New.txt").unwrap(), format!("{parent}\\New.txt"));
    assert_eq!(key("new.TXT").unwrap(), format!("{parent}\\new.TXT"));
    assert!(platform_fs::windows_names_may_match("New.txt", "new.TXT"));
    assert!(platform_fs::windows_names_may_match("Äpfel.txt", "äpfel.txt"));
    // Valid names in any language, including names that only look like an alias.
    for name in ["不存在.txt", "Ünïcödé file.md", "notes~draft.txt", "LONGERNAME~1.TXT", "My File~1.txt", "a~1.text"] {
        assert_eq!(key(name).unwrap(), format!("{parent}\\{name}"), "{name}");
    }
    // A new name below a new folder is still a plain suffix of the pinned parent.
    assert_eq!(platform_fs::permission_path(&root.join("新しい").join("ファイル.txt")).unwrap(),
        format!("{parent}\\新しい\\ファイル.txt"));
    // Generated 8.3 alias shapes, device names, streams and trailing dots or spaces are refused.
    for name in ["SECRET~1.TXT", "SECRE~12.txt", "ABCDEF~1", "雪~1.txt", "CON", "nul.txt", "COM1.log", "LPT9",
        "file.txt:hidden", "file.txt::$DATA", "trailing.", "trailing "] {
        assert!(key(name).is_err(), "{name}");
    }
    assert!(platform_fs::permission_path(&root.join("SECRET~1").join("inner.txt")).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

/// An NTFS short-name alias normalizes to the same long-name permission key.
#[test]
fn short_name_keeps_identity() {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let root = std::env::temp_dir().join(format!("mc-long-policy-name-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&root, false).unwrap();
    let path = root.join("a-long-file-name.txt");
    drop(platform_fs::private_file(&path, true).unwrap());
    let wide: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut buffer = vec![0u16; 32768];
    let length =
        unsafe { GetShortPathNameW(wide.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) };
    assert!(length > 0 && (length as usize) < buffer.len());
    let short = String::from_utf16(&buffer[..length as usize]).unwrap();
    assert_eq!(
        platform_fs::permission_path(&path).unwrap(),
        platform_fs::permission_path(std::path::Path::new(&short)).unwrap()
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// Case-sensitive directory trees are refused rather than merging sibling grants.
#[test]
fn case_sensitive_tree_is_refused() {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::*;
    let root = std::env::temp_dir().join(format!("mc-sensitive-policy-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&root, false).unwrap();
    let directory = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(&root)
        .unwrap();
    let info = FILE_CASE_SENSITIVE_INFO { Flags: 1 };
    let set = unsafe {
        SetFileInformationByHandle(
            directory.as_raw_handle(),
            FileCaseSensitiveInfo,
            (&info as *const FILE_CASE_SENSITIVE_INFO).cast(),
            std::mem::size_of_val(&info) as u32,
        )
    };
    assert_ne!(set, 0, "{}", std::io::Error::last_os_error());
    drop(directory);
    assert!(platform_fs::permission_path(&root.join("new.txt")).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

/// A project folder is accepted only in the plain drive form of its final path. Every other
/// spelling of the same folder (verbatim, case, slashes, 8.3 name, junction) stays refused,
/// because the sandbox and the file tool match the final path exactly.
#[test]
fn project_folder_must_be_its_plain_final_path() {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let root = std::env::temp_dir().join(format!("mc-project-folder-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let folder = root.join("Project Folder With A Long Name");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("file.txt"), "x").unwrap();
    let verbatim = std::fs::canonicalize(&folder).unwrap().to_str().unwrap().to_owned();
    let plain = verbatim.strip_prefix(r"\\?\").expect("canonicalize answers the verbatim form").to_owned();
    assert!(permissions::exact_folder(&plain), "drive form refused: {plain}");
    let wide: Vec<_> = std::ffi::OsStr::new(&plain).encode_wide().chain(Some(0)).collect();
    let mut buffer = vec![0u16; 32768];
    let length = unsafe { GetShortPathNameW(wide.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    assert!(length > 0 && length < buffer.len());
    let short = String::from_utf16(&buffer[..length]).unwrap();
    assert_ne!(short, plain, "this volume generates no 8.3 names, so the alias case cannot be checked");
    let junction = root.join("Alias");
    let status = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"]).arg(&junction).arg(&folder).status().unwrap();
    assert!(status.success());
    let junction = format!(r"{}\Alias", plain.rsplit_once('\\').unwrap().0);
    assert!(std::fs::metadata(&junction).unwrap().is_dir(), "the junction leads to the folder");
    for alias in [verbatim.clone(), plain.to_lowercase(), plain.replace('\\', "/"), short, junction.clone(),
        format!(r"{plain}\"), format!(r"{plain}\file.txt"), format!(r"{plain}\missing")] {
        assert!(!permissions::exact_folder(&alias), "alias accepted: {alias}");
    }
    std::fs::remove_dir(&junction).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
