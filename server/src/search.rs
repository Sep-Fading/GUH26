//! A disposable full-history index. SQLite keeps text off the page and can spill its cache
//! to disk; provider history remains authoritative.

// Serializes index operations across blocking workers.
use std::sync::{Arc, Mutex};
// Stores text and retrieves indexed candidates.
use rusqlite::{Connection, params};
// Carries public summaries and message results.
use serde_json::{Value, json};

#[derive(Default)]
pub struct Search {
    pub status: Arc<tokio::sync::Mutex<Status>>,
}

#[derive(Default)]
pub struct Status {
    pub generation: Option<String>,
    pub index: Option<Index>,
    pub indexing: bool,
    pub indexed: usize,
    pub incomplete: bool,
    pub live: std::collections::HashMap<String, String>,
}

#[derive(Clone)]
pub struct Index(Arc<Mutex<Connection>>);

impl Index {
    /// Opens a private temporary index, removed when its last connection closes.
    pub fn new() -> rusqlite::Result<Self> {
        let db = Connection::open("")?;
        db.execute_batch("PRAGMA cache_size=-8192;
            CREATE TABLE vocabulary(word TEXT PRIMARY KEY) WITHOUT ROWID;
            CREATE TABLE variants(key TEXT,word TEXT,PRIMARY KEY(key,word)) WITHOUT ROWID;
            CREATE TABLE messages(id INTEGER PRIMARY KEY, thread TEXT, message TEXT, summary TEXT, text TEXT,
                UNIQUE(thread,message));
            CREATE VIRTUAL TABLE words USING fts5(text, content='messages', content_rowid='id', tokenize='trigram');
            CREATE TRIGGER inserted AFTER INSERT ON messages BEGIN
                INSERT INTO words(rowid,text) VALUES(new.id,new.text); END;
            CREATE TRIGGER deleted AFTER DELETE ON messages BEGIN
                INSERT INTO words(words,rowid,text) VALUES('delete',old.id,old.text); END;")?;
        Ok(Self(Arc::new(Mutex::new(db))))
    }

    /// Replaces messages by stable identity without discarding earlier indexed pages.
    pub fn put(&self, thread: &Value, messages: &[Value]) -> rusqlite::Result<()> {
        let mut db = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let tx = db.transaction()?;
        let summary = thread.to_string();
        for message in messages {
            let (Some(id), Some(text)) = (message["id"].as_str(), message["text"].as_str()) else { continue; };
            if !matches!(message["role"].as_str(), Some("user" | "assistant")) { continue; }
            for word in text.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
                if !(4..=64).contains(&word.chars().count()) { continue; }
                if tx.execute("INSERT OR IGNORE INTO vocabulary VALUES(?1)", [word])? == 0 { continue; }
                for key in deletes(word) {
                    tx.execute("INSERT OR IGNORE INTO variants VALUES(?1,?2)", params![key,word])?;
                }
            }
            tx.execute("DELETE FROM messages WHERE thread=?1 AND message=?2", params![thread["id"].as_str(), id])?;
            tx.execute("INSERT INTO messages(thread,message,summary,text) VALUES(?1,?2,?3,?4)",
                params![thread["id"].as_str(), id, summary, text])?;
        }
        tx.commit()
    }

    /// Finds literal matches first, then checks a bounded set of indexed typo candidates.
    pub fn query(&self, query: &str) -> rusqlite::Result<(Vec<Value>, bool)> {
        let terms: Vec<String> = query.to_lowercase().split_whitespace().map(str::to_owned).collect();
        let long: Vec<_> = terms.iter().filter(|word| word.chars().count() >= 3).collect();
        if long.is_empty() { return Ok((vec![], false)); }
        let literal = long.iter().map(|word| quote(word)).collect::<Vec<_>>().join(" AND ");
        let db = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let mut corrected = Vec::new();
        let mut limited = false;
        for word in &long {
            let mut choices = std::collections::BTreeSet::from([word.to_string()]);
            if (4..=64).contains(&word.chars().count()) {
                for key in deletes(word) {
                    let mut statement = db.prepare("SELECT word FROM variants WHERE key=?1 LIMIT 65")?;
                    let rows = statement.query_map([key], |row| row.get::<_, String>(0))?;
                    for (n, row) in rows.enumerate() {
                        if n == 64 { limited = true; break; }
                        let candidate = row?;
                        if close(word, &candidate) { choices.insert(candidate); }
                    }
                }
            }
            corrected.push(format!("({})", choices.iter().map(|word| quote(word)).collect::<Vec<_>>().join(" OR ")));
        }
        let fuzzy = corrected.join(" AND ");
        let mut found = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (expression, typos) in [(literal, false), (fuzzy, true)] {
            if found.len() >= 50 { limited = true; break; }
            let mut statement = db.prepare("SELECT m.thread,m.message,m.summary,m.text FROM words
                JOIN messages m ON m.id=words.rowid WHERE words MATCH ?1 ORDER BY rank LIMIT 1001")?;
            let rows = statement.query_map([expression], |row| Ok((row.get::<_, String>(0)?,
                row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?)))?;
            for (n, row) in rows.enumerate() {
                if n == 1000 { limited = true; break; }
                let (thread, id, summary, text) = row?;
                if seen.contains(&(thread.clone(), id.clone())) { continue; }
                let lower = text.to_lowercase();
                if !terms.iter().all(|term| lower.contains(term) || (typos && term.chars().count() >= 4
                    && lower.split(|c: char| !c.is_alphanumeric()).any(|word| close(term, word)))) { continue; }
                seen.insert((thread, id.clone()));
                let position = lower.split_inclusive(|c: char| !c.is_alphanumeric())
                    .scan(0, |offset, word| { let start = *offset; *offset += word.chars().count(); Some((start, word)) })
                    .find(|(_, word)| terms.iter().any(|term| word.contains(term)
                        || (typos && close(term, word.trim_matches(|c: char| !c.is_alphanumeric())))))
                    .map(|(start, _)| start).unwrap_or(0);
                let start = position.saturating_sub(70);
                let snippet: String = text.chars().skip(start).take(240).collect();
                found.push(json!({ "thread": serde_json::from_str::<Value>(&summary).unwrap_or_default(),
                    "messageId": id, "snippet": snippet, "approximate": typos }));
                if found.len() == 50 { limited = true; break; }
            }
        }
        Ok((found, limited))
    }
}

/// Returns a word and its one-letter deletions for indexed spelling lookup.
fn deletes(word: &str) -> Vec<String> {
    let chars: Vec<_> = word.chars().collect();
    let mut keys = vec![word.to_owned()];
    keys.extend((0..chars.len()).map(|skip| chars.iter().enumerate()
        .filter_map(|(i, c)| (i != skip).then_some(c)).collect()));
    keys
}

/// Quotes a user term so FTS operators are always treated as text.
fn quote(value: &str) -> String { format!("\"{}\"", value.replace('"', "\"\"")) }

/// Accepts one insertion, deletion, replacement or adjacent transposition in a word.
fn close(left: &str, right: &str) -> bool {
    let a: Vec<_> = left.chars().collect();
    let b: Vec<_> = right.chars().collect();
    if a.len().abs_diff(b.len()) > 1 { return false; }
    let first = a.iter().zip(&b).position(|(a, b)| a != b).unwrap_or(a.len().min(b.len()));
    if first == a.len().min(b.len()) { return true; }
    if a.len() == b.len() {
        a[first + 1..] == b[first + 1..] || (first + 1 < a.len()
            && a[first] == b[first + 1] && a[first + 1] == b[first]
            && a[first + 2..] == b[first + 2..])
    } else if a.len() > b.len() { a[first + 1..] == b[first..] }
    else { a[first..] == b[first + 1..] }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercises literal ranking, typo correction, Unicode and replacement of indexed text.
    #[test]
    fn matching_and_updates() {
        let index = Index::new().unwrap();
        let thread = json!({"id":"chat", "name":"Example"});
        index.put(&thread, &[
            json!({"id":"typo", "role":"assistant", "text":"authetication flow"}),
            json!({"id":"exact", "role":"assistant", "text":"authentication flow café 日本語 world"}),
        ]).unwrap();
        assert_eq!(index.query("authentication").unwrap().0[0]["messageId"], "exact");
        assert_eq!(index.query("authentiaction").unwrap().0[0]["messageId"], "exact");
        assert_eq!(index.query("wrold").unwrap().0[0]["messageId"], "exact");
        assert_eq!(index.query("日本語").unwrap().0.len(), 1);
        index.put(&thread, &[json!({"id":"exact", "role":"assistant", "text":"Replaced"})]).unwrap();
        assert!(index.query("日本語").unwrap().0.is_empty());
    }

    /// Measures warmed indexed queries over a synthetic 50,000-message archive.
    #[test]
    fn large_archive() {
        let index = Index::new().unwrap();
        let messages: Vec<_> = (0..50_000).map(|n| json!({"id":n.to_string(), "role":"assistant",
            "text":format!("Discussion {n}: implementation details, testing a local server, reviewing changes and deployment. {}",
                if n == 31415 { "Authentication flow and credentials." } else { "Ordinary project notes." })})).collect();
        index.put(&json!({"id":"chat"}), &messages).unwrap();
        index.query("authentication").unwrap();
        let start = std::time::Instant::now();
        for _ in 0..20 { assert_eq!(index.query("authetication").unwrap().0[0]["messageId"], "31415"); }
        eprintln!("50,000 messages, typo search mean: {:?}", start.elapsed() / 20);
    }
}
