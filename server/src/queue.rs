//! Messages waiting for their turn. While a response runs, a conversation's further messages
//! are kept here, on the Mac, and leave one per turn in the order they were queued. A queue
//! that is paused sends nothing until the user resumes it: it pauses when a turn fails, is
//! stopped or has an unknown outcome, and every queue comes back paused after a restart.

// Reuses the private metadata database.
use crate::agents::{Issue, Store};
// Runs the queue's statements and optional lookups.
use rusqlite::{Connection, OptionalExtension, params};
// Shapes queued messages for the page.
use serde::Serialize;
// Names queued messages and keeps sends idempotent.
use uuid::Uuid;

/// The most messages one conversation's queue holds.
pub const LIMIT: i64 = 20;

/// One queued message. The model and effort are those chosen when it was queued.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub text: String,
    pub client_message_id: Uuid,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// The speed tier chosen with it; none means standard.
    pub speed: Option<String>,
    pub created_at: i64,
    /// The uploads that leave with the message, as the page shows them.
    pub attachments: Vec<serde_json::Value>,
}

/// A conversation's queue as the page sees it.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct View {
    /// Grows with every read, across restarts too. It is taken before the rows are read, so of
    /// two views the one with the larger version is never the older state; the page keeps the
    /// newest and ignores an answer that arrives after a later event.
    pub version: u64,
    pub items: Vec<Item>,
    pub paused: bool,
    /// Why it is paused: `failed`, `stopped`, `unknown`, `restart`, `refused` or `user`.
    pub reason: Option<String>,
}

/// Creates the queue tables inside the existing private metadata database.
pub fn migrate(db: &Connection) -> Result<(), Issue> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS message_queue (
        id TEXT PRIMARY KEY, thread TEXT NOT NULL, position INTEGER NOT NULL, text TEXT NOT NULL,
        client_message_id TEXT NOT NULL, model TEXT, effort TEXT, created_at INTEGER NOT NULL,
        UNIQUE(thread, client_message_id)
    );
    CREATE INDEX IF NOT EXISTS message_queue_order ON message_queue (thread, position);
    CREATE TABLE IF NOT EXISTS message_queue_state (
        thread TEXT PRIMARY KEY, paused INTEGER NOT NULL, reason TEXT
    );").map_err(Issue::storage)?;
    for (column, shape) in [("attachments", "TEXT NOT NULL DEFAULT '[]'"), ("speed", "TEXT")] {
        let known = db.prepare("SELECT 1 FROM pragma_table_info('message_queue') WHERE name = ?1")
            .and_then(|mut query| query.exists([column])).map_err(Issue::storage)?;
        if !known {
            db.execute_batch(&format!("ALTER TABLE message_queue ADD COLUMN {column} {shape};")).map_err(Issue::storage)?;
        }
    }
    Ok(())
}

fn item(row: &rusqlite::Row) -> rusqlite::Result<Item> {
    let client: String = row.get(2)?;
    Ok(Item { id: row.get(0)?, text: row.get(1)?, client_message_id: Uuid::parse_str(&client).unwrap_or_default(),
        model: row.get(3)?, effort: row.get(4)?, created_at: row.get(5)?,
        attachments: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(), speed: row.get(7)? })
}

const COLUMNS: &str = "id, text, client_message_id, model, effort, created_at, attachments, speed";

/// Returns the next view version: the start time in microseconds, then one more per read.
fn next_version() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static VERSION: AtomicU64 = AtomicU64::new(0);
    let _ = VERSION.compare_exchange(0, chrono::Utc::now().timestamp_micros().max(1) as u64, Ordering::AcqRel, Ordering::Acquire);
    VERSION.fetch_add(1, Ordering::AcqRel)
}

/// Reads one conversation's queue in sending order.
fn view(db: &Connection, thread: &str) -> Result<View, Issue> {
    let version = next_version();
    let mut query = db.prepare(&format!("SELECT {COLUMNS} FROM message_queue WHERE thread = ?1 ORDER BY position LIMIT 64"))
        .map_err(Issue::storage)?;
    let items = query.query_map([thread], item).map_err(Issue::storage)?
        .collect::<Result<Vec<_>, _>>().map_err(Issue::storage)?;
    let state: Option<(bool, Option<String>)> = db.query_row("SELECT paused, reason FROM message_queue_state WHERE thread = ?1",
        [thread], |row| Ok((row.get(0)?, row.get(1)?))).optional().map_err(Issue::storage)?;
    let (paused, reason) = state.unwrap_or((false, None));
    Ok(View { version, items, paused, reason: reason.filter(|_| paused) })
}

impl Store {
    /// Returns one conversation's queue.
    pub async fn queue(&self, thread: String) -> Result<View, Issue> {
        self.run(move |db| view(db, &thread)).await
    }

    /// Adds a message at the end. The same id with the same words is queued once; the same id
    /// with other words is a conflict.
    pub async fn enqueue(&self, thread: String, text: String, client: Uuid, model: Option<String>,
        effort: Option<String>, speed: Option<String>, attachments: Vec<serde_json::Value>) -> Result<Item, Issue> {
        self.run(move |db| {
            let tx = db.unchecked_transaction().map_err(Issue::storage)?;
            let known = tx.query_row(&format!("SELECT {COLUMNS} FROM message_queue WHERE thread = ?1 AND client_message_id = ?2"),
                params![thread, client.to_string()], item).optional().map_err(Issue::storage)?;
            if let Some(known) = known {
                return if known.text == text && known.model == model && known.effort == effort && known.speed == speed && known.attachments == attachments { Ok(known) } else { Err(Issue::Conflict) };
            }
            let (count, last): (i64, Option<i64>) = tx.query_row("SELECT count(*), max(position) FROM message_queue WHERE thread = ?1",
                [&thread], |row| Ok((row.get(0)?, row.get(1)?))).map_err(Issue::storage)?;
            if count >= LIMIT { return Err(Issue::Limit); }
            let queued = Item { id: Uuid::new_v4().to_string(), text, client_message_id: client, model, effort, speed,
                created_at: chrono::Utc::now().timestamp_millis(), attachments };
            tx.execute("INSERT INTO message_queue (id, thread, position, text, client_message_id, model, effort, created_at, attachments, speed)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)", params![queued.id, thread, last.unwrap_or(0) + 1, queued.text,
                client.to_string(), queued.model, queued.effort, queued.created_at, serde_json::Value::from(queued.attachments.clone()).to_string(), queued.speed]).map_err(Issue::storage)?;
            tx.commit().map_err(Issue::storage)?;
            Ok(queued)
        }).await
    }

    /// Replaces the words of one queued message.
    pub async fn edit_queued(&self, thread: String, id: String, text: String) -> Result<Item, Issue> {
        self.run(move |db| {
            if db.execute("UPDATE message_queue SET text = ?3 WHERE thread = ?1 AND id = ?2", params![thread, id, text])
                .map_err(Issue::storage)? == 0 { return Err(Issue::Missing); }
            db.query_row(&format!("SELECT {COLUMNS} FROM message_queue WHERE id = ?1"), [&id], item).map_err(Issue::storage)
        }).await
    }

    /// Removes one queued message, or all of a conversation's when no id is given. An emptied
    /// queue is no longer paused: there is nothing left to hold back.
    pub async fn remove_queued(&self, thread: String, id: Option<String>) -> Result<(), Issue> {
        self.run(move |db| {
            let removed = match &id {
                Some(id) => db.execute("DELETE FROM message_queue WHERE thread = ?1 AND id = ?2", params![thread, id]),
                None => db.execute("DELETE FROM message_queue WHERE thread = ?1", [&thread]),
            }.map_err(Issue::storage)?;
            if id.is_some() && removed == 0 { return Err(Issue::Missing); }
            db.execute("DELETE FROM message_queue_state WHERE thread = ?1
                AND NOT EXISTS (SELECT 1 FROM message_queue WHERE thread = ?1)", [&thread]).map_err(Issue::storage)?;
            Ok(())
        }).await
    }

    /// Moves one queued message to the front, so it is the next to leave.
    pub async fn queue_first(&self, thread: String, id: String) -> Result<(), Issue> {
        self.run(move |db| {
            if db.execute("UPDATE message_queue SET position = (SELECT min(position) - 1 FROM message_queue WHERE thread = ?1)
                WHERE thread = ?1 AND id = ?2", params![thread, id]).map_err(Issue::storage)? == 0 { return Err(Issue::Missing); }
            Ok(())
        }).await
    }

    /// Pauses a queue that holds something, or resumes it. An empty queue is never paused.
    pub async fn pause_queue(&self, thread: String, reason: Option<&'static str>) -> Result<(), Issue> {
        self.run(move |db| {
            match reason {
                Some(reason) => db.execute("INSERT INTO message_queue_state (thread, paused, reason)
                    SELECT ?1, 1, ?2 WHERE EXISTS (SELECT 1 FROM message_queue WHERE thread = ?1)
                    ON CONFLICT(thread) DO UPDATE SET paused = 1, reason = excluded.reason", params![thread, reason]),
                None => db.execute("DELETE FROM message_queue_state WHERE thread = ?1", [&thread]),
            }.map_err(Issue::storage)?;
            Ok(())
        }).await
    }

    /// Pauses every queue that holds something, when the backend starts: no turn of this run
    /// ended, so nothing may leave a queue by itself.
    pub async fn pause_queues_after_restart(&self) -> Result<usize, Issue> {
        self.run(|db| db.execute("INSERT INTO message_queue_state (thread, paused, reason)
            SELECT DISTINCT thread, 1, 'restart' FROM message_queue WHERE true
            ON CONFLICT(thread) DO UPDATE SET paused = 1, reason = 'restart'", []).map_err(Issue::storage)).await
    }
}
