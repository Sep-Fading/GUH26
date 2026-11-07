// Exercises mandatory quarantine state and migration.
use miniclaw_server::agents::Store;
// Inspects only disposable metadata database files.
use rusqlite::{Connection, params};
// Builds bounded synthetic baseline representations for tests.
use serde_json::json;
// Owns cleanup of private temporary directories.
use std::{fs, path::PathBuf};
// Assigns synthetic creation identifiers without collisions.
use uuid::Uuid;

struct Directory(PathBuf);
impl Directory {
    /// Creates an independent directory and metadata path for one test.
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("mc-quarantine-{}", Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    /// Returns the private metadata filename inside this fixture.
    fn database(&self) -> PathBuf { self.0.join("agents.sqlite3") }
}
impl Drop for Directory {
    /// Removes only files owned by this disposable fixture.
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

/// The feature schema upgrade preserves unknown receipts and database identity.
#[test]
fn preserves_v1_journal() {
    let root = Directory::new();
    drop(Store::open(&root.database()).unwrap());
    let db = Connection::open(root.database()).unwrap();
    db.execute_batch("DROP TABLE permission_sessions;
        DROP TABLE permission_rules;
        DROP TABLE chat_environments;
        DROP TABLE creation_quarantine;
        DROP TABLE permission_app_requests; PRAGMA user_version=1;").unwrap();
    let id = Uuid::new_v4().to_string();
    db.execute("INSERT INTO chat_creations(id, baseline) VALUES(?1, '[]')", [&id]).unwrap();
    db.execute("INSERT INTO chat_admissions(thread_id, required) VALUES(?1, 0)", [&id]).unwrap();
    let identity: String = db.query_row("SELECT id FROM miniclaw_identity", [], |row| row.get(0)).unwrap();
    drop(Store::open(&root.database()).unwrap());
    assert_eq!(db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(), 5);
    assert_eq!(db.query_row("SELECT id FROM chat_creations", [], |row| row.get::<_, String>(0)).unwrap(), id);
    assert_eq!(db.query_row("SELECT id FROM miniclaw_identity", [], |row| row.get::<_, String>(0)).unwrap(), identity);
    assert_eq!(db.query_row("SELECT revision FROM creation_quarantine", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

/// A lost or malformed singleton cannot become a fresh inactive fence.
#[test]
fn rejects_lost_or_corrupt_fence() {
    let uuid = Uuid::new_v4().to_string();
    for sql in ["DELETE FROM creation_quarantine".to_owned(),
        "DROP TABLE creation_quarantine".to_owned(),
        "UPDATE creation_quarantine SET revision=1, baseline=NULL".to_owned(),
        "UPDATE creation_quarantine SET revision=0, baseline='[]'".to_owned(),
        "UPDATE creation_quarantine SET revision=1, baseline='[\"invalid\"]'".to_owned(),
        format!("UPDATE creation_quarantine SET revision=1, baseline='{}'", json!([uuid, uuid])),
    ] {
        let root = Directory::new();
        drop(Store::open(&root.database()).unwrap());
        let db = Connection::open(root.database()).unwrap();
        db.execute_batch(&sql).unwrap();
        assert!(Store::open(&root.database()).is_err(), "accepted corrupt fence: {sql}");
    }
}

/// Exhausted fence revisions leave all unknown receipts in place.
#[tokio::test]
async fn rejects_revision_overflow() {
    let root = Directory::new();
    let store = Store::open(&root.database()).unwrap();
    let db = Connection::open(root.database()).unwrap();
    let id = Uuid::new_v4().to_string();
    db.execute("INSERT INTO chat_creations(id, baseline) VALUES(?1, '[]')", [&id]).unwrap();
    db.execute("INSERT INTO chat_admissions(thread_id, required) VALUES(?1, 0)", [&id]).unwrap();
    db.execute("UPDATE creation_quarantine SET revision=?1, baseline='[]'", params![i64::MAX]).unwrap();
    let preview = store.creation_recovery().await.unwrap();
    assert!(store.compact_creations(preview.token).await.is_err());
    assert_eq!(db.query_row("SELECT count(*) FROM chat_creations", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
}
