// Shares private metadata storage and errors.
use super::{Issue, Store};
// Keeps compaction changes within one transaction.
use rusqlite::{Connection, params};
// Serializes previews without exposing policy snapshots.
use serde::Serialize;
// Computes stable confirmation tokens from observed state.
use sha2::{Digest, Sha256};
// Intersects bounded sets in deterministic order.
use std::collections::BTreeSet;
// Validates stored conversation and intent identifiers.
use uuid::Uuid;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub token: String,
    pub unknown_count: usize,
    pub known_count: usize,
    pub quarantined: bool,
    pub preserved_baseline_count: usize,
}

pub(super) struct Fence {
    revision: i64,
    baseline: Option<BTreeSet<String>>,
}

struct Intent {
    id: String,
    thread: Option<String>,
    baseline: BTreeSet<String>,
    source: String,
}

/// Rejects malformed identifiers rather than changing their admission meaning.
fn valid_id(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value)
}

/// Reads a bounded UUID set; retained fences must already be sorted and unique.
fn baseline(text: &str, strict: bool) -> Result<BTreeSet<String>, Issue> {
    if text.len() > 164000 { return Err(Issue::Storage); }
    let values: Vec<String> = serde_json::from_str(text).map_err(Issue::storage)?;
    if values.len() > 4096 || values.iter().any(|id| !valid_id(id)) { return Err(Issue::Storage); }
    let set: BTreeSet<_> = values.iter().cloned().collect();
    if strict && values != set.iter().cloned().collect::<Vec<_>>() { return Err(Issue::Storage); }
    Ok(set)
}

/// Requires the mandatory quarantine singleton and validates its active-state invariant.
pub(super) fn read(db: &Connection) -> Result<Fence, Issue> {
    let (revision, text): (i64, Option<String>) = db.query_row(
        "SELECT revision, baseline FROM creation_quarantine WHERE singleton = 1", [],
        |row| Ok((row.get(0)?, row.get(1)?))).map_err(Issue::storage)?;
    if revision < 0 || (revision == 0) != text.is_none() { return Err(Issue::Storage); }
    let baseline = text.as_deref().map(|text| baseline(text, true)).transpose()?;
    Ok(Fence { revision, baseline })
}

impl Fence {
    /// Preserves permanent denial for unadmitted IDs outside the compacted baseline.
    pub(super) fn check(&self, thread: &str) -> Result<(), Issue> {
        if self.baseline.as_ref().is_some_and(|baseline| !baseline.contains(thread)) {
            return Err(Issue::CreationUncertain);
        }
        Ok(())
    }
}

/// Reads bounded creation receipts without assigning unknown receipts to chats.
fn intents(db: &Connection) -> Result<Vec<Intent>, Issue> {
    let mut query = db.prepare("SELECT id, thread_id, baseline FROM chat_creations ORDER BY id LIMIT 65")
        .map_err(Issue::storage)?;
    let rows = query.query_map([], |row| Ok((row.get::<_, String>(0)?,
        row.get::<_, Option<String>>(1)?, row.get::<_, String>(2)?))).map_err(Issue::storage)?;
    let mut result = Vec::new();
    for row in rows {
        let (id, thread, text) = row.map_err(Issue::storage)?;
        if !valid_id(&id) || thread.as_ref().is_some_and(|id| !valid_id(id)) { return Err(Issue::Storage); }
        result.push(Intent { id, thread, baseline: baseline(&text, false)?, source: text });
    }
    if result.len() > 64 { return Err(Issue::Storage); }
    Ok(result)
}

/// Derives a preview token from the exact saved receipts, baselines and fence revision.
fn preview(fence: &Fence, intents: &[Intent]) -> Preview {
    let mut hash = Sha256::new();
    hash.update(fence.revision.to_be_bytes());
    if let Some(baseline) = &fence.baseline {
        for id in baseline { hash.update(id.as_bytes()); }
    }
    for intent in intents {
        hash.update(intent.id.as_bytes());
        hash.update([u8::from(intent.thread.is_some())]);
        if let Some(thread) = &intent.thread { hash.update(thread.as_bytes()); }
        hash.update((intent.source.len() as u64).to_be_bytes());
        hash.update(intent.source.as_bytes());
    }
    Preview {
        token: hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect(),
        unknown_count: intents.iter().filter(|intent| intent.thread.is_none()).count(),
        known_count: intents.iter().filter(|intent| intent.thread.is_some()).count(),
        quarantined: fence.baseline.is_some(),
        preserved_baseline_count: fence.baseline.as_ref().map_or(0, BTreeSet::len),
    }
}

impl Store {
    /// Returns an observed recovery preview without modifying any receipt or binding.
    pub async fn creation_recovery(&self) -> Result<Preview, Issue> {
        self.run(|db| {
            let tx = db.transaction().map_err(Issue::storage)?;
            let result = preview(&read(&tx)?, &intents(&tx)?);
            tx.commit().map_err(Issue::storage)?;
            Ok(result)
        }).await
    }

    /// Atomically replaces confirmed unknown journals with an equivalent permanent fence.
    pub async fn compact_creations(&self, token: String) -> Result<usize, Issue> {
        self.run(move |db| {
            let tx = db.transaction().map_err(Issue::storage)?;
            let fence = read(&tx)?;
            let intents = intents(&tx)?;
            let observed = preview(&fence, &intents);
            if observed.token != token { return Err(Issue::Conflict); }
            if observed.unknown_count == 0 { return Err(Issue::Invalid); }
            let mut preserved = fence.baseline;
            for intent in intents.iter().filter(|intent| intent.thread.is_none()) {
                let required: i64 = tx.query_row("SELECT required FROM chat_admissions WHERE thread_id = ?1",
                    [&intent.id], |row| row.get(0)).map_err(Issue::storage)?;
                for (bit, table) in [(1, "agent_chats"), (2, "project_chats"), (4, "chat_skill_selections")] {
                    let exists: bool = tx.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE thread_id = ?1)"),
                        [&intent.id], |row| row.get(0)).map_err(Issue::storage)?;
                    if required & bit != 0 && !exists { return Err(Issue::Storage); }
                }
                preserved = Some(match preserved {
                    None => intent.baseline.clone(),
                    Some(existing) => existing.intersection(&intent.baseline).cloned().collect(),
                });
            }
            let revision = fence.revision.checked_add(1).ok_or(Issue::Limit)?;
            let preserved = preserved.ok_or(Issue::Storage)?;
            let baseline = serde_json::to_string(&preserved).map_err(Issue::storage)?;
            let changed = tx.execute("UPDATE creation_quarantine SET revision = ?1, baseline = ?2 WHERE singleton = 1",
                params![revision, baseline]).map_err(Issue::storage)?;
            let stored = read(&tx)?;
            if changed != 1 || stored.revision != revision || stored.baseline.as_ref() != Some(&preserved) {
                return Err(Issue::Storage);
            }
            for intent in intents.iter().filter(|intent| intent.thread.is_none()) {
                for table in ["agent_chat_skills", "agent_chats", "project_chats", "chat_skill_selections", "chat_admissions"] {
                    tx.execute(&format!("DELETE FROM {table} WHERE thread_id = ?1"), [&intent.id]).map_err(Issue::storage)?;
                }
                tx.execute("DELETE FROM permission_rules WHERE scope_kind = 'conversation' AND scope_id = ?1",
                    [&intent.id]).map_err(Issue::storage)?;
                let removed = tx.execute("DELETE FROM chat_creations WHERE id = ?1 AND thread_id IS NULL", [&intent.id])
                    .map_err(Issue::storage)?;
                if removed != 1 { return Err(Issue::Storage); }
            }
            tx.commit().map_err(Issue::storage)?;
            Ok(observed.unknown_count)
        }).await
    }
}
