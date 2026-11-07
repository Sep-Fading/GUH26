// Exercises Windows grants without native filesystem access.
use miniclaw_server::{seatbelt::Grant, windows_sandbox::Policy};

/// Builds one already-normalized policy entry.
fn grant(path: &str, write: bool, folder: bool, blocked: bool) -> Grant {
    Grant {
        path: path.into(),
        write,
        folder,
        blocked,
    }
}

#[test]
/// A child deny wins over every containing allow, including write grants.
fn nested_deny_wins() {
    let policy = Policy::from_normalized(vec![
        grant(r"C:\WORK", true, true, false),
        grant(r"C:\WORK\PRIVATE", false, true, true),
        grant(r"C:\WORK\PRIVATE\NOTE.TXT", true, false, false),
    ]);
    assert!(policy.allows(r"C:\WORK\PUBLIC.TXT", true));
    assert!(!policy.allows(r"C:\WORK\PRIVATE\NOTE.TXT", false));
    assert!(!policy.allows(r"C:\WORK\PRIVATE", false));
    assert!(policy.allows(r"C:\WORK\PRIVATE-NOTES.TXT", false));
    assert!(!policy.allows(r"C:\WORKOTHER\NOTE.TXT", false));
}

#[test]
/// Exact-file grants never cover descendants and reads never imply writes.
fn exact_and_read_scopes() {
    let policy = Policy::from_normalized(vec![grant(r"C:\WORK\NOTE", false, false, false)]);
    assert!(policy.allows(r"C:\WORK\NOTE", false));
    assert!(!policy.allows(r"C:\WORK\NOTE", true));
    assert!(!policy.allows(r"C:\WORK\NOTE\CHILD", false));
    assert!(!policy.allows(r"C:\WORK", false));
}

#[test]
/// A drive-root grant still covers its direct children.
fn drive_root_scope() {
    let policy = Policy::from_normalized(vec![grant(r"C:\", false, true, false)]);
    assert!(policy.allows(r"C:\NOTE.TXT", false));
    assert!(!policy.allows(r"D:\NOTE.TXT", false));
}

/// The identity-bound host policy, on real files: what a path names decides, not how it is spelled.
#[cfg(windows)]
mod native {
    use miniclaw_server::{executor, scoped::Operation, seatbelt::Grant};
    use serde_json::Value;
    use std::{fs, os::windows::ffi::{OsStrExt, OsStringExt}, path::{Path, PathBuf}};

    struct Fixture(PathBuf);
    impl Fixture {
        /// A workspace whose own name needs Unicode, with a blocked folder that has a long name.
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("mc policy \u{96ea} {}", uuid::Uuid::new_v4()));
            fs::create_dir_all(root.join("blocked long folder name")).unwrap();
            fs::write(root.join("blocked long folder name/secret.txt"), "SECRET").unwrap();
            fs::write(root.join("allowed long file name.txt"), "allowed").unwrap();
            Self(root.canonicalize().unwrap())
        }
        fn path(&self, name: &str) -> String { self.0.join(name).to_string_lossy().into_owned() }
        fn grants(&self) -> Vec<Grant> {
            vec![Grant { path: self.path(""), write: true, folder: true, blocked: false },
                Grant { path: self.path("blocked long folder name"), write: false, folder: true, blocked: true }]
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
    }

    async fn write(path: String, grants: &[Grant]) -> Value {
        executor::execute_host(Operation::WriteFile { path, text: "written".into() }, grants).await
    }
    async fn read(path: String, grants: &[Grant]) -> Value {
        executor::execute_host(Operation::ReadFile { path }, grants).await
    }

    /// The 8.3 alias of an existing path, or `None` where the volume generates none.
    fn short(path: &Path) -> Option<PathBuf> {
        use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut buffer = vec![0u16; 32768];
        let length = unsafe { GetShortPathNameW(wide.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
        assert!(length > 0 && length < buffer.len());
        let alias = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length]));
        (alias != path).then_some(alias)
    }

    /// A new file may be named in any language, and with a tilde that is no alias.
    #[tokio::test]
    async fn unicode_names_can_be_created() {
        let fixture = Fixture::new();
        for name in ["r\u{e9}sum\u{e9}.txt", "\u{65e5}\u{672c}\u{8a9e} \u{30ce}\u{30fc}\u{30c8}.md", "\u{1f600} notes.txt", "notes~draft.txt", "~$report.docx"] {
            let created = write(fixture.path(name), &fixture.grants()).await;
            assert_eq!(created["status"], "ok", "{name}: {created}");
            assert_eq!(fs::read_to_string(fixture.0.join(name)).unwrap(), "written", "{name}");
            // The same file is then an existing one: read, and replaced, under the same name.
            assert_eq!(read(fixture.path(name), &fixture.grants()).await["text"], "written", "{name}");
            assert_eq!(write(fixture.path(name), &fixture.grants()).await["status"], "ok", "{name}");
        }
        let names: Vec<String> = fs::read_dir(&fixture.0).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(names.contains(&"r\u{e9}sum\u{e9}.txt".to_owned()), "{names:?}");
    }

    /// Alias-shaped, device, stream and trailing dot or space names never create anything.
    #[tokio::test]
    async fn alias_and_reserved_new_names_are_refused() {
        let fixture = Fixture::new();
        let before = fs::read_dir(&fixture.0).unwrap().count();
        for name in ["RESUME~1.TXT", "a~1", "NEWFIL~2.md", "NUL.txt", "COM1", "tail.", "tail ", "file.txt:stream", "file.txt::$DATA"] {
            let refused = write(fixture.path(name), &fixture.grants()).await;
            assert_eq!(refused["status"], "denied", "{name}: {refused}");
        }
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), before, "nothing was created");
        assert_eq!(fs::read_to_string(fixture.0.join("allowed long file name.txt")).unwrap(), "allowed");
    }

    /// A path that reaches a file through its 8.3 alias is judged by the file it reaches.
    #[tokio::test]
    async fn an_existing_alias_is_judged_by_identity() {
        let fixture = Fixture::new();
        let grants = fixture.grants();
        let Some(blocked) = short(&fixture.0.join("blocked long folder name")) else {
            println!("8.3 names are not generated on this volume; the alias cases cannot be exercised here");
            return;
        };
        assert!(blocked.to_string_lossy().contains('~'), "{}", blocked.display());
        let secret = blocked.join("secret.txt").to_string_lossy().into_owned();
        assert_eq!(read(secret.clone(), &grants).await["status"], "denied");
        assert_eq!(write(secret, &grants).await["status"], "denied");
        assert_eq!(write(blocked.join("new.txt").to_string_lossy().into_owned(), &grants).await["status"], "denied");
        assert_eq!(fs::read_to_string(fixture.0.join("blocked long folder name/secret.txt")).unwrap(), "SECRET");
        assert!(!fixture.0.join("blocked long folder name/new.txt").exists());
        // The allowed file is the same file by its alias, and is written in place.
        let allowed = short(&fixture.0.join("allowed long file name.txt")).expect("the file has an alias too");
        assert_eq!(read(allowed.to_string_lossy().into_owned(), &grants).await["text"], "allowed");
        assert_eq!(write(allowed.to_string_lossy().into_owned(), &grants).await["status"], "ok");
        assert_eq!(fs::read_to_string(fixture.0.join("allowed long file name.txt")).unwrap(), "written");
        // A grant that is itself spelled through the alias binds the same folder.
        let by_alias = vec![grants[0].clone(), Grant { path: blocked.to_string_lossy().into_owned(), write: false, folder: true, blocked: true }];
        assert_eq!(read(fixture.path("blocked long folder name/secret.txt"), &by_alias).await["status"], "denied");
    }

    /// Case and spelling do not move a file out of, or into, a grant.
    #[tokio::test]
    async fn grants_follow_the_object_not_the_spelling() {
        let fixture = Fixture::new();
        let grants = fixture.grants();
        let upper = fixture.path("BLOCKED LONG FOLDER NAME/SECRET.TXT");
        assert_eq!(read(upper, &grants).await["status"], "denied");
        assert_eq!(read(fixture.path("ALLOWED LONG FILE NAME.TXT"), &grants).await["text"], "allowed");
        // An exact-file grant covers that file only, and reading does not allow writing.
        let only = vec![Grant { path: fixture.path("allowed long file name.txt"), write: false, folder: false, blocked: false }];
        assert_eq!(read(fixture.path("Allowed Long File Name.txt"), &only).await["text"], "allowed");
        assert_eq!(write(fixture.path("allowed long file name.txt"), &only).await["status"], "denied");
        assert_eq!(write(fixture.path("sibling.txt"), &only).await["status"], "denied");
        assert!(!fixture.0.join("sibling.txt").exists());
        // The same grants no longer cover a different object that takes the old name.
        fs::rename(fixture.0.join("allowed long file name.txt"), fixture.0.join("moved.txt")).unwrap();
        assert_eq!(read(fixture.path("moved.txt"), &only).await["status"], "denied");
    }

    /// A blocked name that does not exist yet cannot be created in another case, in any language.
    #[tokio::test]
    async fn a_blocked_absent_name_is_not_created_in_another_case() {
        let fixture = Fixture::new();
        let mut grants = fixture.grants();
        grants.push(Grant { path: fixture.path("priv\u{e9}.txt"), write: false, folder: false, blocked: true });
        grants.push(Grant { path: fixture.path("Secret.env"), write: false, folder: false, blocked: true });
        for name in ["PRIV\u{c9}.TXT", "priv\u{e9}.txt", "SECRET.ENV", "secret.env"] {
            assert_eq!(write(fixture.path(name), &grants).await["status"], "denied", "{name}");
            assert!(!fixture.0.join(name).exists(), "{name}");
        }
        // A different name beside it is still an ordinary new file.
        assert_eq!(write(fixture.path("prive.txt"), &grants).await["status"], "ok");
    }
}
