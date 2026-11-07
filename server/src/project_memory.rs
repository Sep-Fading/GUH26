//! A project's shared memory: short notes that every conversation of the project can read and
//! write, kept by MiniClaw so they work the same for Codex, Claude and VM projects. A
//! conversation is told only the index, one line per note; a note's text is read on demand. The
//! user sees, edits and deletes every note in the project's settings.

// Shares the metadata store and its errors.
use crate::agents::{Issue, Store};
// Reads and writes the note table.
use rusqlite::{Connection, OptionalExtension, params};
// Shapes a note for the page.
use serde::{Deserialize, Serialize};
// Names the project a note belongs to.
use uuid::Uuid;

/// Most notes one project holds.
pub const NOTES: i64 = 64;
/// The longest note text, in bytes. A note is read whole, so this bounds one tool result.
pub const TEXT_BYTES: usize = 8 * 1024;
/// The most index text a conversation is told. Later notes are counted, not listed.
pub const INDEX_BYTES: usize = 4 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub name: String,
    pub description: String,
    pub text: String,
    #[serde(default)]
    pub updated_at: i64,
}

/// What the page and the tool send to save a note.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Draft { pub name: String, pub description: String, pub text: String }

#[derive(Debug, PartialEq, Eq)]
pub enum Refusal { Shape, Secret, Full, Missing, Storage }

impl From<Issue> for Refusal {
    fn from(_: Issue) -> Self { Self::Storage }
}

/// Creates the note table inside the existing private metadata database.
pub fn migrate(db: &Connection) -> Result<(), Issue> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS project_memory (
        project_id TEXT NOT NULL, key TEXT NOT NULL, name TEXT NOT NULL, description TEXT NOT NULL,
        text TEXT NOT NULL, updated_at INTEGER NOT NULL, thread_id TEXT,
        PRIMARY KEY (project_id, key)
    );").map_err(Issue::storage)
}

/// The key a name is found by: trimmed and in lower case, so "Art style" and "art style" are one note.
fn key(name: &str) -> String { name.trim().to_lowercase() }

/// Checks a draft's shape: a one-line name of at most 80 characters, a one-line description of at
/// most 200, and up to 8 KB of text.
fn shaped(draft: &Draft) -> bool {
    let line = |text: &str, most: usize| !text.trim().is_empty() && text.chars().count() <= most && !text.chars().any(char::is_control);
    line(&draft.name, 80) && line(&draft.description, 200) && !draft.text.trim().is_empty() && draft.text.len() <= TEXT_BYTES
        && !draft.text.contains('\0')
}

/// Reports whether text looks like it holds a secret: a private key, a token of a known shape, or
/// a password-like word followed by a long value. Memory is told to later conversations and shown
/// in settings, so a secret never belongs in it.
pub fn looks_secret(text: &str) -> bool {
    if text.contains("-----BEGIN") && text.contains("PRIVATE KEY") { return true; }
    let token = |word: &str| {
        let word = word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-');
        (["sk-", "ghp_", "gho_", "ghs_", "ghu_", "github_pat_", "xoxb-", "xoxp-", "glpat-"].iter().any(|prefix| word.starts_with(prefix)) && word.len() >= 20)
            || (word.len() == 20 && word.starts_with("AKIA") && word.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()))
    };
    if text.split_whitespace().any(token) { return true; }
    text.lines().any(|line| {
        let lower = line.to_lowercase();
        ["password", "passwd", "secret", "token", "api key", "api_key", "apikey"].iter().any(|word| {
            let Some(at) = lower.find(word) else { return false; };
            let rest = lower[at + word.len()..].trim_start();
            let Some(value) = rest.strip_prefix(':').or_else(|| rest.strip_prefix('=')) else { return false; };
            value.split_whitespace().next().is_some_and(|value| value.trim_matches(|c| c == '"' || c == '\'').len() >= 12)
        })
    })
}

/// Writes the index of a project's notes: one line each, by name, within `INDEX_BYTES`.
pub fn index(notes: &[Note]) -> String {
    let mut text = String::new();
    let mut left = 0;
    for note in notes {
        let line = format!("- {}: {}\n", note.name, note.description);
        if text.len() + line.len() > INDEX_BYTES { left += 1; } else { text.push_str(&line); }
    }
    if left > 0 { text.push_str(&format!("- and {left} more; their names are not listed.\n")); }
    text
}

fn read(row: &rusqlite::Row) -> rusqlite::Result<Note> {
    Ok(Note { name: row.get(0)?, description: row.get(1)?, text: row.get(2)?, updated_at: row.get(3)? })
}

impl Store {
    /// Returns a project's notes by name. The order never depends on when a note changed, so
    /// changing a note's text leaves the index as it was and nobody has to be told it again.
    pub async fn notes(&self, project: Uuid) -> Result<Vec<Note>, Issue> {
        self.run(move |db| {
            let mut query = db.prepare("SELECT name, description, text, updated_at FROM project_memory WHERE project_id = ?1
                ORDER BY key LIMIT 64").map_err(Issue::storage)?;
            query.query_map([project.to_string()], read).map_err(Issue::storage)?.collect::<Result<_, _>>().map_err(Issue::storage)
        }).await
    }

    /// Returns one note by its name, in any letter case.
    pub async fn note(&self, project: Uuid, name: String) -> Result<Option<Note>, Issue> {
        self.run(move |db| db.query_row("SELECT name, description, text, updated_at FROM project_memory WHERE project_id = ?1 AND key = ?2",
            params![project.to_string(), key(&name)], read).optional().map_err(Issue::storage)).await
    }

    /// Saves a note. With `replaces` it changes that note, which must exist, and may rename it;
    /// otherwise it creates the note or replaces the one of the same name.
    pub async fn save_note(&self, project: Uuid, draft: Draft, replaces: Option<String>, thread: Option<String>) -> Result<Note, Refusal> {
        if !shaped(&draft) { return Err(Refusal::Shape); }
        if looks_secret(&draft.text) || looks_secret(&draft.description) || looks_secret(&draft.name) { return Err(Refusal::Secret); }
        let note = Note { name: draft.name.trim().to_owned(), description: draft.description.trim().to_owned(), text: draft.text,
            updated_at: chrono::Utc::now().timestamp_millis() };
        let saved = note.clone();
        self.run(move |db| {
            let tx = db.unchecked_transaction().map_err(Issue::storage)?;
            let project = project.to_string();
            if let Some(old) = &replaces {
                if tx.execute("DELETE FROM project_memory WHERE project_id = ?1 AND key = ?2", params![project, key(old)]).map_err(Issue::storage)? == 0 {
                    return Ok(Err(Refusal::Missing));
                }
            }
            let known: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM project_memory WHERE project_id = ?1 AND key = ?2)",
                params![project, key(&note.name)], |row| row.get(0)).map_err(Issue::storage)?;
            let count: i64 = tx.query_row("SELECT count(*) FROM project_memory WHERE project_id = ?1", [&project], |row| row.get(0)).map_err(Issue::storage)?;
            if !known && count >= NOTES { return Ok(Err(Refusal::Full)); }
            tx.execute("INSERT INTO project_memory (project_id, key, name, description, text, updated_at, thread_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(project_id, key) DO UPDATE SET description = excluded.description, text = excluded.text,
                updated_at = excluded.updated_at, thread_id = excluded.thread_id",
                params![project, key(&note.name), note.name, note.description, note.text, note.updated_at, thread]).map_err(Issue::storage)?;
            tx.commit().map_err(Issue::storage)?;
            Ok(Ok(()))
        }).await??;
        Ok(saved)
    }

    /// Removes one note; reports whether it existed.
    pub async fn remove_note(&self, project: Uuid, name: String) -> Result<bool, Issue> {
        self.run(move |db| db.execute("DELETE FROM project_memory WHERE project_id = ?1 AND key = ?2", params![project.to_string(), key(&name)])
            .map(|removed| removed > 0).map_err(Issue::storage)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_recognised_and_ordinary_text_is_not() {
        for secret in ["sk-proj-abcdefghijklmnopqrstuvwxyz0123456789", "use ghp_abcdefghijklmnopqrstuvwxyz0123456789 here", "AKIAIOSFODNN7EXAMPLE",
            "-----BEGIN OPENSSH PRIVATE KEY-----\nabc", "password: hunter2hunter2hunter2", "API_KEY=\"abcdef1234567890\""] {
            assert!(looks_secret(secret), "{secret}");
        }
        for plain in ["The token budget is 4 KB.", "Ask the user for the password: never store it.", "sk-short", "Skies are AKIA blue.",
            "Secrets: keep them in the Keychain."] {
            assert!(!looks_secret(plain), "{plain}");
        }
    }

    #[test]
    fn the_index_stays_within_its_size() {
        let notes: Vec<Note> = (0..64).map(|n| Note { name: format!("Note {n}"), description: "d".repeat(200), text: "t".into(), updated_at: 0 }).collect();
        let text = index(&notes);
        assert!(text.len() <= INDEX_BYTES + 80);
        assert!(text.contains("more; their names are not listed"));
        assert_eq!(index(&[]), "");
    }
}
