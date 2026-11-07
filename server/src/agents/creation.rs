// Reuses serialized storage and classified failures.
use super::{Issue, Store};
// Commits creation receipts and bindings together.
use rusqlite::{Connection, OptionalExtension, params};
// Validates upstream IDs before recording ownership.
use uuid::Uuid;

/// Upgrades legacy storage once; missing journal tables after upgrade stop startup.
pub(super) fn migrate(db: &mut Connection) -> Result<(), Issue> {
    let version: i64 = db
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(Issue::storage)?;
    if (1..=5).contains(&version) {
        for table in ["chat_creations", "chat_admissions"] {
            if !db
                .table_exists(Some("main"), table)
                .map_err(Issue::storage)?
            {
                return Err(Issue::Storage);
            }
        }
        if version >= 3 {
            super::quarantine::read(db)?;
            if !db.table_exists(Some("main"), "chat_environments")
                .map_err(Issue::storage)? { return Err(Issue::Storage); }
            return Ok(());
        }
        if version == 1 { add_quarantine(db)?; }
        return add_environment(db);
    }
    if version != 0 {
        return Err(Issue::Storage);
    }
    let tx = db.transaction().map_err(Issue::storage)?;
    tx.execute_batch(
        "CREATE TABLE chat_creations (
        id TEXT PRIMARY KEY, thread_id TEXT UNIQUE, baseline TEXT NOT NULL
    );
    CREATE TABLE chat_admissions (
        thread_id TEXT PRIMARY KEY, required INTEGER NOT NULL CHECK(required BETWEEN 0 AND 7)
    );
    INSERT INTO chat_admissions
    SELECT thread_id,
        EXISTS(SELECT 1 FROM agent_chats a WHERE a.thread_id = ids.thread_id)
        + 2 * EXISTS(SELECT 1 FROM project_chats p WHERE p.thread_id = ids.thread_id)
        + 4 * EXISTS(SELECT 1 FROM chat_skill_selections s WHERE s.thread_id = ids.thread_id)
    FROM (SELECT thread_id FROM agent_chats UNION SELECT thread_id FROM project_chats
        UNION SELECT thread_id FROM chat_skill_selections) ids;
    PRAGMA user_version = 1;",
    )
    .map_err(Issue::storage)?;
    tx.commit().map_err(Issue::storage)?;
    add_quarantine(db)?;
    add_environment(db)
}

/// Adds the mandatory inactive fence without changing any existing admission or journal.
fn add_quarantine(db: &mut Connection) -> Result<(), Issue> {
    let tx = db.transaction().map_err(Issue::storage)?;
    tx.execute_batch("CREATE TABLE creation_quarantine (
        singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
        revision INTEGER NOT NULL CHECK(revision >= 0), baseline TEXT
    );
    INSERT INTO creation_quarantine (singleton, revision, baseline) VALUES (1, 0, NULL);
    PRAGMA user_version = 2;").map_err(Issue::storage)?;
    tx.commit().map_err(Issue::storage)
}

/// Adds a pinned machine binding and reserves an admission bit for new chats.
fn add_environment(db: &mut Connection) -> Result<(), Issue> {
    let tx = db.transaction().map_err(Issue::storage)?;
    tx.execute_batch("CREATE TABLE chat_environments (
        thread_id TEXT PRIMARY KEY, assignment TEXT NOT NULL
    );
    CREATE TABLE chat_admissions_v3 (
        thread_id TEXT PRIMARY KEY, required INTEGER NOT NULL CHECK(required BETWEEN 0 AND 15)
    );
    INSERT INTO chat_admissions_v3 SELECT thread_id, required FROM chat_admissions;
    DROP TABLE chat_admissions;
    ALTER TABLE chat_admissions_v3 RENAME TO chat_admissions;
    PRAGMA user_version = 3;") .map_err(Issue::storage)?;
    tx.commit().map_err(Issue::storage)
}

impl Store {
    /// Records Codex's returned ID separately so a failed binding move can be retried.
    pub async fn created(&self, intent: String, thread: String) -> Result<(), Issue> {
        Uuid::parse_str(&thread).map_err(|_| Issue::CreationUncertain)?;
        self.run(move |db| {
            let changed = db
                .execute(
                    "UPDATE chat_creations SET thread_id = ?2 WHERE id = ?1 AND thread_id IS NULL",
                    params![intent, thread],
                )
                .map_err(Issue::storage)?;
            if changed != 1 {
                return Err(Issue::Storage);
            }
            Ok(())
        })
        .await
    }

    /// Removes provisional records only when no Codex request has been sent.
    pub async fn cancel_creation(&self, intent: String) -> Result<(), Issue> {
        self.run(move |db| {
            let tx = db.transaction().map_err(Issue::storage)?;
            for table in [
                "agent_chat_skills",
                "agent_chats",
                "project_chats",
                "chat_environments",
                "chat_skill_selections",
                "chat_admissions",
            ] {
                tx.execute(
                    &format!("DELETE FROM {table} WHERE thread_id = ?1"),
                    [&intent],
                )
                .map_err(Issue::storage)?;
            }
            tx.execute(
                "DELETE FROM chat_creations WHERE id = ?1 AND thread_id IS NULL",
                [&intent],
            )
            .map_err(Issue::storage)?;
            tx.execute("DELETE FROM permission_rules WHERE scope_kind = 'conversation' AND scope_id = ?1",
                [&intent]).map_err(Issue::storage)?;
            tx.commit().map_err(Issue::storage)
        })
        .await
    }

    /// Finishes known receipts; denies lost bindings and history with uncertain ownership.
    pub async fn check_creation(&self, thread: String) -> Result<(), Issue> {
        self.run(move |db| {
            let tx = db.transaction().map_err(Issue::storage)?;
            let quarantine = super::quarantine::read(&tx)?;
            let intent: Option<String> = tx
                .query_row(
                    "SELECT id FROM chat_creations WHERE thread_id = ?1",
                    [&thread],
                    |r| r.get(0),
                )
                .optional()
                .map_err(Issue::storage)?;
            if let Some(intent) = intent {
                // A receipt cannot replace policy lost before its final commit.
                let _: i64 = tx
                    .query_row(
                        "SELECT required FROM chat_admissions WHERE thread_id = ?1",
                        [&intent],
                        |r| r.get(0),
                    )
                    .map_err(Issue::storage)?;
                tx.execute_batch("PRAGMA defer_foreign_keys = ON;")
                    .map_err(Issue::storage)?;
                for table in [
                    "agent_chats",
                    "agent_chat_skills",
                    "project_chats",
                    "chat_environments",
                    "chat_skill_selections",
                    "chat_admissions",
                ] {
                    tx.execute(
                        &format!("UPDATE {table} SET thread_id = ?2 WHERE thread_id = ?1"),
                        params![intent, thread],
                    )
                    .map_err(Issue::storage)?;
                }
                tx.execute("UPDATE permission_rules SET scope_id = ?2
                    WHERE scope_kind = 'conversation' AND scope_id = ?1", params![intent, thread])
                    .map_err(Issue::storage)?;
                tx.execute("DELETE FROM chat_creations WHERE id = ?1", [&intent])
                    .map_err(Issue::storage)?;
            }
            let required: Option<i64> = tx
                .query_row(
                    "SELECT required FROM chat_admissions WHERE thread_id = ?1",
                    [&thread],
                    |r| r.get(0),
                )
                .optional()
                .map_err(Issue::storage)?;
            if let Some(required) = required {
                for (bit, table) in [
                    (1, "agent_chats"),
                    (2, "project_chats"),
                    (4, "chat_skill_selections"),
                    (8, "chat_environments"),
                ] {
                    let exists: bool = tx
                        .query_row(
                            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE thread_id = ?1)"),
                            [&thread],
                            |r| r.get(0),
                        )
                        .map_err(Issue::storage)?;
                    if required & bit != 0 && !exists {
                        return Err(Issue::Storage);
                    }
                }
            } else {
                quarantine.check(&thread)?;
                let mut query = tx
                    .prepare("SELECT baseline FROM chat_creations WHERE thread_id IS NULL")
                    .map_err(Issue::storage)?;
                let rows = query
                    .query_map([], |r| r.get::<_, String>(0))
                    .map_err(Issue::storage)?;
                for row in rows {
                    let baseline: Vec<String> = serde_json::from_str(&row.map_err(Issue::storage)?)
                        .map_err(Issue::storage)?;
                    if !baseline.contains(&thread) {
                        return Err(Issue::CreationUncertain);
                    }
                }
            }
            tx.commit().map_err(Issue::storage)
        })
        .await
    }
}
