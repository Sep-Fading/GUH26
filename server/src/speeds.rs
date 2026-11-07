//! The speed a conversation runs at: a Codex service tier such as Fast. Codex takes it on
//! every turn and does not keep it on the thread, so MiniClaw keeps it here, beside the
//! conversation's name, and lays it over the summary wherever the conversation is opened.

// Reads and writes the speed table.
use rusqlite::{Connection, OptionalExtension, params};
// Shares the metadata store and its errors.
use crate::agents::{Issue, Store};

/// Creates the speed table inside the existing private metadata database.
pub fn migrate(db: &Connection) -> Result<(), Issue> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS chat_speeds (thread_id TEXT PRIMARY KEY, speed TEXT NOT NULL);")
        .map_err(Issue::storage)
}

impl Store {
    /// Returns the speed a conversation last ran at, if it was not standard.
    pub async fn chat_speed(&self, thread: String) -> Result<Option<String>, Issue> {
        self.run(move |db| db.query_row("SELECT speed FROM chat_speeds WHERE thread_id = ?1", [&thread], |row| row.get(0))
            .optional().map_err(Issue::storage)).await
    }

    /// Records a conversation's speed; none means standard and removes the row.
    pub async fn set_chat_speed(&self, thread: String, speed: Option<String>) -> Result<(), Issue> {
        self.run(move |db| {
            match speed {
                None => { db.execute("DELETE FROM chat_speeds WHERE thread_id = ?1", [&thread]).map_err(Issue::storage)?; }
                Some(speed) => { db.execute("INSERT INTO chat_speeds (thread_id, speed) VALUES (?1, ?2)
                    ON CONFLICT(thread_id) DO UPDATE SET speed = excluded.speed", params![thread, speed]).map_err(Issue::storage)?; }
            }
            Ok(())
        }).await
    }
}
