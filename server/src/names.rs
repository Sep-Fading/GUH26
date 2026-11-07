//! The name a user gives a conversation. MiniClaw keeps it itself, so it works the same for
//! Codex, Claude and VM conversations, and is laid over the provider's own name wherever a
//! conversation is listed or opened.

// Reads and writes the name table.
use rusqlite::{Connection, OptionalExtension, params};
// Shares the metadata store and its errors.
use crate::agents::{Issue, Store};

/// The longest name, in characters.
pub const LIMIT: usize = 120;

/// Creates the name table inside the existing private metadata database.
pub fn migrate(db: &Connection) -> Result<(), Issue> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS chat_names (thread_id TEXT PRIMARY KEY, name TEXT NOT NULL);")
        .map_err(Issue::storage)
}

/// Trims a name and accepts it when it is one line of at most 120 characters. Empty means
/// "take the name back".
pub fn checked(name: &str) -> Option<String> {
    let name = name.trim();
    (name.chars().count() <= LIMIT && !name.chars().any(char::is_control)).then(|| name.to_owned())
}

impl Store {
    /// Returns the name the user gave a conversation, if any.
    pub async fn chat_name(&self, thread: String) -> Result<Option<String>, Issue> {
        self.run(move |db| db.query_row("SELECT name FROM chat_names WHERE thread_id = ?1", [&thread], |row| row.get(0))
            .optional().map_err(Issue::storage)).await
    }

    /// Saves a conversation's name; an empty name removes it.
    pub async fn name_chat(&self, thread: String, name: String) -> Result<(), Issue> {
        self.run(move |db| {
            if name.is_empty() {
                db.execute("DELETE FROM chat_names WHERE thread_id = ?1", [&thread]).map_err(Issue::storage)?;
            } else {
                db.execute("INSERT INTO chat_names (thread_id, name) VALUES (?1, ?2)
                    ON CONFLICT(thread_id) DO UPDATE SET name = excluded.name", params![thread, name]).map_err(Issue::storage)?;
            }
            Ok(())
        }).await
    }
}
