//! What a project's conversations can learn of each other: a short summary each one may save, a
//! digest of the latest ones, and a bounded look-up over the copies MiniClaw keeps of their
//! messages. Nothing here calls a provider or a model, and nothing leaves the project.

// Shares the metadata store and its errors.
use crate::agents::{Issue, Store};
// The backend, for the copies and the names.
use crate::chat::Backend;
// Reads and writes the summary table.
use rusqlite::{Connection, OptionalExtension, params};
// Names the project whose conversations are read.
use uuid::Uuid;

/// The longest summary, in characters.
pub const SUMMARY: usize = 600;
/// How many earlier conversations the digest names.
const DIGEST: usize = 8;
/// How many conversations a listing or a search covers.
const COVERED: usize = 50;
/// The size of one page of a conversation, in bytes.
const PAGE: usize = 8000;

/// One earlier conversation as the model is told of it.
pub struct Entry { pub id: String, pub name: String, pub date: String, pub summary: Option<String> }

/// Creates the summary table inside the existing private metadata database.
pub fn migrate(db: &Connection) -> Result<(), Issue> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS chat_summaries (thread_id TEXT PRIMARY KEY, text TEXT NOT NULL, updated_at INTEGER NOT NULL);")
        .map_err(Issue::storage)
}

impl Store {
    /// Saves a conversation's summary, replacing the earlier one.
    pub async fn save_summary(&self, thread: String, text: String) -> Result<(), Issue> {
        self.run(move |db| db.execute("INSERT INTO chat_summaries (thread_id, text, updated_at) VALUES (?1, ?2, ?3)
            ON CONFLICT(thread_id) DO UPDATE SET text = excluded.text, updated_at = excluded.updated_at",
            params![thread, text, chrono::Utc::now().timestamp_millis()]).map(|_| ()).map_err(Issue::storage)).await
    }

    /// Returns a conversation's summary, if one was saved.
    pub async fn summary(&self, thread: String) -> Result<Option<String>, Issue> {
        self.run(move |db| db.query_row("SELECT text FROM chat_summaries WHERE thread_id = ?1", [&thread], |row| row.get(0))
            .optional().map_err(Issue::storage)).await
    }

    /// Returns the project's latest conversations, newest first.
    pub async fn recent_project_threads(&self, project: Uuid, most: usize) -> Result<Vec<String>, Issue> {
        self.run(move |db| {
            let mut query = db.prepare("SELECT thread_id FROM project_chats WHERE project_id = ?1 ORDER BY rowid DESC LIMIT ?2").map_err(Issue::storage)?;
            query.query_map(params![project.to_string(), most as i64], |row| row.get::<_, String>(0)).map_err(Issue::storage)?
                .collect::<Result<_, _>>().map_err(Issue::storage)
        }).await
    }
}

/// Reads the kept copy of one conversation: its summary fields and its messages.
fn copy(backend: &Backend, thread: &str) -> Option<(serde_json::Value, Vec<serde_json::Value>)> {
    let (summary, messages) = crate::transcripts::load(backend.transcripts.get()?, thread)?;
    Some((summary, messages.as_array().cloned().unwrap_or_default()))
}

/// Returns the project's other conversations that have a kept copy, newest first. A conversation
/// with no finished response has no copy and is left out.
pub async fn entries(backend: &Backend, project: Uuid, asking: &str, most: usize) -> Vec<Entry> {
    let mut found = Vec::new();
    for thread in backend.agents.recent_project_threads(project, COVERED).await.unwrap_or_default() {
        if thread == asking || found.len() >= most { continue; }
        let Some((summary, _)) = copy(backend, &thread) else { continue; };
        // The user's own name wins over the one the copy was saved with.
        let name = backend.agents.chat_name(thread.clone()).await.ok().flatten()
            .or_else(|| summary["name"].as_str().map(str::to_owned)).unwrap_or_else(|| "Untitled".into());
        let date = summary["createdAt"].as_i64().and_then(chrono::DateTime::from_timestamp_millis)
            .map(|time| time.format("%Y-%m-%d").to_string()).unwrap_or_default();
        found.push(Entry { summary: backend.agents.summary(thread.clone()).await.ok().flatten(), id: thread, name, date });
    }
    found
}

/// Writes entries as lines: date, name, id, and the summary when there is one.
fn lines(entries: &[Entry]) -> String {
    entries.iter().map(|entry| format!("- {} {} (id {}){}\n", entry.date, entry.name, entry.id,
        entry.summary.as_ref().map(|text| format!(": {text}")).unwrap_or_else(|| ": no summary saved".into()))).collect()
}

/// Writes the digest a new conversation is told once: the latest conversations of its project.
pub async fn digest(backend: &Backend, project: Uuid, asking: &str) -> Option<String> {
    let entries = entries(backend, project, asking, DIGEST).await;
    (!entries.is_empty()).then(|| format!("## Earlier conversations in this project\nThe latest, newest first. Look one up with miniclaw_project (conversations_search, conversation_read) when the task builds on it; do not read them otherwise.\n{}", lines(&entries)))
}

/// Lists the project's other conversations, optionally only those whose name or summary has the words.
pub async fn list(backend: &Backend, project: Uuid, asking: &str, query: Option<&str>) -> String {
    let wanted = query.map(str::to_lowercase);
    let entries: Vec<Entry> = entries(backend, project, asking, COVERED).await.into_iter().filter(|entry| wanted.as_ref().is_none_or(|wanted|
        entry.name.to_lowercase().contains(wanted) || entry.summary.as_ref().is_some_and(|text| text.to_lowercase().contains(wanted)))).collect();
    if entries.is_empty() { "No other conversation of this project matches.".into() } else { lines(&entries) }
}

/// Finds messages of the project's other conversations that hold every word of the query, in any
/// order and letter case, and returns up to ten short passages. A model rarely guesses an exact
/// phrase, and a missed search costs a whole extra call.
pub async fn search(backend: &Backend, project: Uuid, asking: &str, query: &str) -> String {
    let wanted: Vec<String> = query.to_lowercase().split_whitespace().map(str::to_owned).collect();
    let mut out = String::new();
    let mut hits = 0;
    for entry in entries(backend, project, asking, COVERED).await {
        let Some((_, messages)) = copy(backend, &entry.id) else { continue; };
        for message in &messages {
            let text = message["text"].as_str().unwrap_or_default();
            let lower = text.to_lowercase();
            if !wanted.iter().all(|word| lower.contains(word)) { continue; }
            // The passage is taken around the rarest-looking word: the longest one.
            let Some(at) = wanted.iter().max_by_key(|word| word.len()).and_then(|word| lower.find(word)) else { continue; };
            // Lowercasing can change byte lengths, so the passage is cut on the original's own boundaries.
            let mut start = at.min(text.len()).saturating_sub(120);
            while !text.is_char_boundary(start) { start -= 1; }
            let mut end = (start + 300).min(text.len());
            while !text.is_char_boundary(end) { end -= 1; }
            out.push_str(&format!("- {} {} ({}), {}: …{}…\n", entry.id, entry.name, entry.date,
                if message["role"] == "user" { "User" } else { "Assistant" }, text[start..end].replace('\n', " ")));
            hits += 1;
            if hits >= 10 { return out; }
        }
    }
    if out.is_empty() { "Nothing in this project's other conversations has those words.".into() } else { out }
}

/// Returns one page of a conversation of the same project, with its name, or why it cannot be read.
pub async fn read(backend: &Backend, project: Uuid, asking: &str, id: &str, page: usize) -> Result<(String, String), &'static str> {
    let known = backend.agents.recent_project_threads(project, 4096).await.unwrap_or_default();
    if id == asking || !known.iter().any(|thread| thread == id) { return Err("No other conversation of this project has that id. conversations_list gives the ids."); }
    let (summary, messages) = copy(backend, id).ok_or("MiniClaw has no copy of that conversation yet; it has no finished response.")?;
    let name = backend.agents.chat_name(id.to_owned()).await.ok().flatten()
        .or_else(|| summary["name"].as_str().map(str::to_owned)).unwrap_or_else(|| "Untitled".into());
    let whole: String = messages.iter().map(|message| format!("{}: {}\n\n", if message["role"] == "user" { "User" } else { "Assistant" },
        message["text"].as_str().unwrap_or_default())).collect();
    let mut pages = Vec::new();
    let mut rest = whole.as_str();
    while !rest.is_empty() {
        let mut end = PAGE.min(rest.len());
        while !rest.is_char_boundary(end) { end -= 1; }
        pages.push(&rest[..end]);
        rest = &rest[end..];
    }
    let text = pages.get(page.saturating_sub(1)).filter(|_| page >= 1).ok_or("That conversation has no such page.")?;
    Ok((name.clone(), format!("Conversation \"{name}\", page {page} of {}. Attached files and tool steps are not part of the copy.\n\n{text}", pages.len())))
}
