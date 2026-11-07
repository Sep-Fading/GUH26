// Checks private storage and hostile Windows paths.
use miniclaw_server::platform_fs;

/// Rejects drive-relative, device, UNC and ambiguous Win32 paths.
#[test]
fn windows_paths_are_unambiguous() {
    for path in [r"C:\Users\Ada\project", r"d:/work/file.txt"] {
        assert!(platform_fs::plain_windows_path(path), "{path}");
    }
    for path in [
        r"C:foo",
        r"\foo",
        r"\\server\share\file",
        r"\\?\C:\work",
        r"\\.\pipe\x",
        r"C:\a\..\b",
        r"C:\a\.\b",
        r"C:\a\file:stream",
        r"C:\a\CON.txt",
        r"C:\a\CON .txt",
        r"C:\a\tail.",
        r"C:\a\tail ",
        r"C:\a\\b",
    ] {
        assert!(!platform_fs::plain_windows_path(path), "{path}");
    }
}

/// A new name may use any language; only the shape of a generated 8.3 alias is refused.
#[test]
fn new_names_allow_unicode_but_not_short_aliases() {
    for name in ["notes.txt", "r\u{e9}sum\u{e9}.txt", "\u{65e5}\u{672c}\u{8a9e} \u{30ce}\u{30fc}\u{30c8}.md", "na\u{ef}ve caf\u{e9}",
        "\u{1f600}.txt", "notes~draft.txt", "~$report.docx", "~", "a~b.txt", "backup~1.tar.gz", "longfilename~1.txt",
        "report~1.html", "my~1 file.txt", "file~.txt", "~1x.txt"] {
        assert!(platform_fs::new_windows_leaf(name), "{name}");
    }
    // Up to eight characters ending in `~` and digits, with at most a three-character
    // extension, is what Windows generates as the short alias of another file.
    for name in ["RESUME~1.TXT", "resume~1.txt", "PROGRA~1", "a~1", "~1", "A~12.md", "R\u{c9}SUM~1.TXT", "ABCDEF~9", "ABCD~123.C",
        "", "a\\b", "a/b"] {
        assert!(!platform_fs::new_windows_leaf(name), "{name}");
    }
}

/// A blocked name stays blocked in every spelling Windows may fold into it.
#[test]
fn windows_names_match_conservatively() {
    let same = platform_fs::windows_names_may_match;
    assert!(same(r"C:\Work\Secret.TXT", r"c:\work\secret.txt"));
    assert!(same("priv\u{e9}.txt", "PRIV\u{c9}.TXT"));
    assert!(same("\u{43f}\u{440}\u{438}\u{432}\u{435}\u{442}", "\u{41f}\u{420}\u{418}\u{412}\u{415}\u{422}"));
    // Letters whose other case is an ASCII letter: dotless i, long s, the Kelvin sign.
    assert!(same("f\u{131}le", "FILE"));
    assert!(same("me\u{17f}s", "MESS"));
    assert!(same("\u{212a}elvin", "kelvin"));
    // Unknown foldings between two non-ASCII letters are assumed, never ruled out.
    assert!(same("priv\u{e9}", "priv\u{e8}"));
    // What no folding can join stays different.
    assert!(!same("priv\u{e9}.txt", "prive.txt"));
    assert!(!same("secret.txt", "secret.txt.bak"));
    assert!(!same("secret", "secrets"));
    assert!(!same("a\u{1f600}", "a\u{e9}"));
    assert!(!same("notes", "n0tes"));
}

/// New private markers retain their permissions and reject replacement.
#[test]
fn private_file_round_trip() {
    let root = std::env::temp_dir().join(format!("mc-private-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&root, false).unwrap();
    let path = root.join("identity");
    let file = platform_fs::private_file(&path, true).unwrap();
    assert!(platform_fs::private_regular_file(&file).unwrap());
    assert!(platform_fs::private_file(&path, true).is_err());
    drop(file);
    assert!(platform_fs::open_read_nofollow(&path).is_ok());
    std::fs::remove_dir_all(root).unwrap();
}

/// A no-follow read refuses links instead of reading their destination.
#[cfg(unix)]
#[test]
fn refuses_link_reads() {
    let root = std::env::temp_dir().join(format!("mc-links-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("real"), "private").unwrap();
    std::os::unix::fs::symlink(root.join("real"), root.join("link")).unwrap();
    assert!(platform_fs::open_read_nofollow(&root.join("link")).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

/// Windows canonical paths identify the same object as a plain drive path.
#[cfg(windows)]
#[test]
fn canonical_path_round_trip() {
    let root = std::env::temp_dir().join(format!("mc-canonical-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&root, false).unwrap();
    let path = root.join("file");
    let file = platform_fs::private_file(&path, true).unwrap();
    let identity = platform_fs::file_identity(&file).unwrap();
    drop(file);
    let canonical = std::fs::canonicalize(&path).unwrap();
    let reopened = platform_fs::open_read_nofollow(&canonical).unwrap();
    assert_eq!(platform_fs::file_identity(&reopened).unwrap(), identity);
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

/// Windows rejects hard-link aliases of files used for protected storage.
#[cfg(windows)]
#[test]
fn hardlinks_are_refused() {
    let root = std::env::temp_dir().join(format!("mc-hardlink-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&root, false).unwrap();
    drop(platform_fs::private_file(&root.join("file"), true).unwrap());
    std::fs::hard_link(root.join("file"), root.join("alias")).unwrap();
    assert!(platform_fs::open_read_nofollow(&root.join("alias")).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

/// A directory junction cannot redirect a read into another tree.
#[cfg(windows)]
#[test]
fn junction_ancestors_are_refused() {
    let root = std::env::temp_dir().join(format!("mc-junction-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&root, false).unwrap();
    platform_fs::private_dir(&root.join("real"), false).unwrap();
    drop(platform_fs::private_file(&root.join("real/file"), true).unwrap());
    let status = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(root.join("alias"))
        .arg(root.join("real"))
        .status()
        .unwrap();
    assert!(status.success());
    assert!(platform_fs::open_read_nofollow(&root.join("alias/file")).is_err());
    std::fs::remove_dir(root.join("alias")).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

/// A Windows SQLite guard allows data access but prevents path replacement.
#[cfg(windows)]
#[test]
fn database_guard_pins_name() {
    let root = std::env::temp_dir().join(format!("mc-db-guard-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&root, false).unwrap();
    let path = root.join("database");
    drop(platform_fs::private_file(&path, true).unwrap());
    let guard = platform_fs::database_guard(&path).unwrap();
    std::fs::write(&path, "data").unwrap();
    assert!(std::fs::rename(&path, root.join("moved")).is_err());
    assert!(std::fs::remove_file(&path).is_err());
    drop(guard);
    std::fs::rename(path, root.join("moved")).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

/// SQLite can commit and reopen its database while its filename remains pinned.
#[cfg(windows)]
#[test]
fn database_guard_allows_sqlite() {
    let root = std::env::temp_dir().join(format!("mc-sqlite-guard-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&root, false).unwrap();
    let path = root.join("database");
    drop(platform_fs::private_file(&path, true).unwrap());
    let guard = platform_fs::database_guard(&path).unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE sample(value TEXT); INSERT INTO sample VALUES ('committed');").unwrap();
    drop(db);
    let db = rusqlite::Connection::open(&path).unwrap();
    let value: String = db
        .query_row("SELECT value FROM sample", [], |row| row.get(0))
        .unwrap();
    assert_eq!(value, "committed");
    drop(db);
    assert_eq!(
        std::fs::rename(&path, root.join("moved"))
            .unwrap_err()
            .raw_os_error(),
        Some(32)
    );
    drop(guard);
    std::fs::remove_dir_all(root).unwrap();
}

/// Both directory and ACL guards retain names while ACL guards permit data writes.
#[cfg(windows)]
#[test]
fn namespace_guards_pin_leaves() {
    let root = std::env::temp_dir().join(format!("mc-name-guard-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&root, false).unwrap();
    let directory = root.join("directory");
    platform_fs::private_dir(&directory, false).unwrap();
    let guard = platform_fs::guarded_path(&directory, false, true).unwrap();
    assert_eq!(
        std::fs::rename(&directory, root.join("moved"))
            .unwrap_err()
            .raw_os_error(),
        Some(32)
    );
    drop(guard);
    for is_directory in [false, true] {
        let path = if is_directory {
            directory.clone()
        } else {
            root.join("file")
        };
        if !is_directory {
            drop(platform_fs::private_file(&path, true).unwrap());
        }
        let guard = platform_fs::guarded_acl_path(&path, is_directory).unwrap();
        if !is_directory {
            std::fs::write(&path, "still writable").unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "still writable");
        }
        assert_eq!(
            std::fs::rename(&path, root.join("moved"))
                .unwrap_err()
                .raw_os_error(),
            Some(32)
        );
        drop(guard);
        std::fs::rename(&path, root.join("moved")).unwrap();
        std::fs::rename(root.join("moved"), &path).unwrap();
    }
    std::fs::remove_dir_all(root).unwrap();
}
