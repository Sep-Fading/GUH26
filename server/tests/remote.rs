#![cfg(unix)]

// Checks private worker settings before connection.
use miniclaw_server::remote::Settings;
// Creates disposable files with explicit permissions.
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};
// Names temporary files without shared fixtures.
use uuid::Uuid;

/// Rejects linked, public, malformed or overlong worker settings without dialing.
#[test]
fn rejects_unsafe_settings() {
    let dir = std::env::temp_dir().join(format!("mc-remote-{}", Uuid::new_v4()));
    fs::create_dir(&dir).unwrap();
    let path = dir.join("worker.json");
    fs::write(
        &path,
        format!(
            r#"{{"key":"{}","cwd":"/Users/Shared/MiniClaw/Workspace"}}"#,
            "ab".repeat(32)
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let settings = Settings::read(&path).unwrap();
    assert_eq!(settings.key, [0xab; 32]);
    assert_eq!(settings.cwd, "/Users/Shared/MiniClaw/Workspace");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(Settings::read(&path).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&path, dir.join("linked")).unwrap();
    assert!(Settings::read(&dir.join("linked")).is_err());
    for content in [
        "{}".into(),
        "x".repeat(4097),
        format!(r#"{{"key":"{}","cwd":"relative"}}"#, "ab".repeat(32)),
    ] {
        fs::write(&path, content).unwrap();
        assert!(Settings::read(&path).is_err());
    }
    fs::remove_dir_all(dir).unwrap();
}
