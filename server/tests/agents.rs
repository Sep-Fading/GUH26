// Exercises profile storage and permission validation.
use miniclaw_server::agents::Store;
// Constructs definitions without using production defaults.
use serde_json::json;
// Creates disposable files and checks metadata.
use std::{
    fs,
    path::PathBuf,
};
#[cfg(unix)]
use std::os::unix::fs::{PermissionsExt, symlink};
// Separates each test's private temporary directory.
use uuid::Uuid;

struct Directory(PathBuf);

impl Directory {
    /// Creates an isolated directory for this test's metadata files.
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("mc-agent-{}", Uuid::new_v4()));
        miniclaw_server::platform_fs::private_dir(&path, false).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    /// Removes only the temporary directory owned by this test.
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
/// Rejects a database symlink without modifying its destination.
#[cfg(unix)]
fn rejects_symlinks() {
    let root = Directory::new();
    let target = root.0.join("unrelated");
    fs::write(&target, "unchanged").unwrap();
    let path = root.0.join("agents.sqlite3");
    symlink(&target, &path).unwrap();
    assert!(Store::open(&path).is_err());
    assert_eq!(fs::read_to_string(target).unwrap(), "unchanged");
}

#[test]
/// Preserves a corrupt database instead of replacing it with empty policy state.
fn preserves_corruption() {
    let root = Directory::new();
    let path = root.0.join("agents.sqlite3");
    fs::write(&path, "not a database").unwrap();
    assert!(Store::open(&path).is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "not a database");
}

#[test]
/// Refuses existing metadata files readable by other users.
#[cfg(unix)]
fn rejects_public_storage() {
    let root = Directory::new();
    let path = root.0.join("agents.sqlite3");
    drop(Store::open(&path).unwrap());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(Store::open(&path).is_err());
}

/// Losing an initialized policy database must not create unrestricted empty state.
#[test]
fn rejects_missing_database() {
    let root = Directory::new();
    let path = root.0.join("agents.sqlite3");
    drop(Store::open(&path).unwrap());
    fs::remove_file(&path).unwrap();
    assert!(Store::open(&path).is_err());
    assert!(!path.exists());
}

/// An empty replacement or another installation's database cannot replace policy.
#[test]
fn rejects_replaced_database() {
    let root = Directory::new();
    let path = root.0.join("agents.sqlite3");
    let other = root.0.join("other.sqlite3");
    drop(Store::open(&path).unwrap());
    drop(Store::open(&other).unwrap());
    fs::copy(&other, &path).unwrap();
    assert!(Store::open(&path).is_err());
    fs::write(&path, b"").unwrap();
    assert!(Store::open(&path).is_err());
}

/// A retained database opens again without losing its installation identity.
#[test]
fn reopens_database() {
    let root = Directory::new();
    let path = root.0.join("agents.sqlite3");
    drop(Store::open(&path).unwrap());
    assert!(Store::open(&path).is_ok());
}

/// Removing a binding table must fail startup instead of recreating empty policy.
#[test]
fn rejects_missing_bindings() {
    let root = Directory::new();
    let path = root.0.join("agents.sqlite3");
    drop(Store::open(&path).unwrap());
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("DROP TABLE agent_chats").unwrap();
    drop(db);
    assert!(Store::open(&path).is_err());
}

/// A missing creation journal cannot be silently initialized after its first migration.
#[test]
fn rejects_missing_journal() {
    for table in ["chat_creations", "chat_admissions"] {
        let root = Directory::new();
        let path = root.0.join("agents.sqlite3");
        drop(Store::open(&path).unwrap());
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(&format!("DROP TABLE {table}")).unwrap();
        drop(db);
        assert!(Store::open(&path).is_err());
    }
}

/// A missing or untrusted companion marker cannot silently reseal the database.
#[test]
#[cfg(unix)]
fn rejects_invalid_markers() {
    let root = Directory::new();
    let path = root.0.join("agents.sqlite3");
    let marker = root.0.join("agents.sqlite3.identity");
    drop(Store::open(&path).unwrap());
    let saved = fs::read(&marker).unwrap();
    fs::remove_file(&marker).unwrap();
    assert!(Store::open(&path).is_err());
    assert!(!marker.exists());
    fs::write(&marker, &saved).unwrap();
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(Store::open(&path).is_err());
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(&marker, root.0.join("linked-marker")).unwrap();
    assert!(Store::open(&path).is_err());
    fs::remove_file(&marker).unwrap();
    symlink(root.0.join("linked-marker"), &marker).unwrap();
    assert!(Store::open(&path).is_err());
    assert_eq!(fs::read(root.0.join("linked-marker")).unwrap(), saved);
}

/// A private pre-identity database retains its profiles through the upgrade.
#[tokio::test]
#[cfg(unix)]
async fn upgrades_legacy_database() {
    let root = Directory::new();
    let path = root.0.join("agents.sqlite3");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TABLE agents (id TEXT PRIMARY KEY, definition TEXT NOT NULL);
        CREATE TABLE agent_chats (thread_id TEXT PRIMARY KEY, agent_id TEXT NOT NULL,
            definition TEXT NOT NULL)",
    )
    .unwrap();
    drop(db);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let store = Store::open(&path).unwrap();
    let agent = store
        .save(
            None,
            serde_json::from_value(json!({
                "name": "Legacy", "instructions": "Retain these instructions."
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    drop(store);
    assert_eq!(
        Store::open(&path)
            .unwrap()
            .get(agent.id)
            .await
            .unwrap()
            .definition
            .name,
        "Legacy"
    );
}

#[tokio::test]
/// Stores SQL-looking instructions literally and leaves unrelated profiles intact.
async fn stores_literal_text() {
    let root = Directory::new();
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let text = "'; DROP TABLE agents; --";
    let agent = store
        .save(
            None,
            serde_json::from_value(json!({
                "name": "Editor", "instructions": text,
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        store.get(agent.id).await.unwrap().definition.instructions,
        text
    );
    assert_eq!(store.list().await.unwrap().len(), 1);
}

/// Windows identity markers cannot be removed or replaced with hard-link aliases.
#[cfg(windows)]
#[test]
fn windows_markers_fail_closed() {
    let root = Directory::new();
    let path = root.0.join("agents.sqlite3");
    let marker = root.0.join("agents.sqlite3.identity");
    drop(Store::open(&path).unwrap());
    fs::hard_link(&marker, root.0.join("alias")).unwrap();
    assert!(Store::open(&path).is_err());
    fs::remove_file(root.0.join("alias")).unwrap();
    assert!(Store::open(&path).is_ok());
    fs::remove_file(&marker).unwrap();
    assert!(Store::open(&path).is_err());
    assert!(!marker.exists());
}

/// Counts this process's open descriptors on one file, by its device and inode.
#[cfg(unix)]
fn descriptors_on(path: &std::path::Path) -> usize {
    use std::os::unix::fs::MetadataExt;
    let wanted = fs::metadata(path).unwrap();
    (0..4096).filter(|descriptor| {
        let mut found: libc::stat = unsafe { std::mem::zeroed() };
        let open = unsafe { libc::fstat(*descriptor, &mut found) } == 0;
        open && found.st_dev as u64 == wanted.dev() && found.st_ino as u64 == wanted.ino()
    }).count()
}

#[test]
/// SQLite's own descriptor is the only one on the database: closing any other descriptor on
/// the file would drop the process's POSIX locks (SQLite, "How To Corrupt", section 2.2).
#[cfg(unix)]
fn store_holds_no_second_descriptor() {
    let root = Directory::new();
    let path = root.0.join("agents.sqlite3");
    let store = Store::open(&path).unwrap();
    assert_eq!(descriptors_on(&path), 1);
    drop(store);
    assert_eq!(descriptors_on(&path), 0);
    // An existing database is checked and reopened the same way.
    let store = Store::open(&path).unwrap();
    assert_eq!(descriptors_on(&path), 1);
    drop(store);
}
