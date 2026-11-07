//! Setup writes private state without replacing existing contents.
use std::{io::Write, process::{Command, Stdio}};
#[test]
fn private_setup_file_is_bounded_private_and_never_overwrites() {
    let root = std::env::temp_dir().join(format!("mc-private-{}", uuid::Uuid::new_v4()));
    miniclaw_server::platform_fs::private_dir(&root, false).unwrap();
    let path = root.join("synthetic.json");
    let write = |bytes: &[u8]| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_miniclaw-server"))
            .args(["--write-private-file"]).arg(&path).stdin(Stdio::piped())
            .stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let _ = child.stdin.take().unwrap().write_all(bytes);
        child.wait().unwrap().success()
    };
    assert!(write(b"synthetic"));
    assert!(write(b"synthetic"));
    assert!(!write(b"changed"));
    assert!(!write(&vec![0; 65537]));
    let file = miniclaw_server::platform_fs::open_read_nofollow(&path).unwrap();
    assert!(miniclaw_server::platform_fs::private_regular_file(&file).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"synthetic");
    drop(file);
    #[cfg(windows)]
    {
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"synthetic").unwrap();
        assert!(!write(b"synthetic"), "inherited ACL must not be accepted as protected state");
    }
    std::fs::remove_dir_all(root).unwrap();
}
