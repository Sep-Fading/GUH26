//! A project's issues: the bugs and features its agents and the user track. Each issue has at
//! most one owning agent, which alone may finish or release it; a release carries a handover.
//! The project's review level decides whether a finished issue closes or waits for the user.
//! Stages, in board order: backlog, dev, qa, agent_review, review (the user's), closed.

// Shares the metadata store and its errors.
use crate::agents::{Issue as StoreIssue, Store};
// Reads and writes the issue tables.
use rusqlite::{Connection, OptionalExtension, Transaction, params};
// Shapes issues for the page and the tool.
use serde::Serialize;
// Names the project an issue belongs to.
use uuid::Uuid;
// Tells open pages that a project's issues changed.
use std::sync::OnceLock;
use tokio::sync::watch;

/// The latest change: a counter and the project it was in. Pages subscribe to reload at once.
static CHANGES: OnceLock<watch::Sender<(u64, Uuid)>> = OnceLock::new();

/// Returns the change signal, made on first use.
fn changes() -> &'static watch::Sender<(u64, Uuid)> { CHANGES.get_or_init(|| watch::channel((0, Uuid::nil())).0) }

/// Subscribes to issue changes in every project; each value names the project that changed.
pub fn subscribe() -> watch::Receiver<(u64, Uuid)> { changes().subscribe() }

/// Announces that one project's issues changed.
fn changed(project: Uuid) { changes().send_modify(|(count, which)| { *count += 1; *which = project; }); }

/// The stages an owner moves an issue through by itself; review and closed come from issue_done and the user.
pub const WORK_STAGES: [&str; 3] = ["dev", "qa", "agent_review"];

/// Reports whether an issue in this status is still being worked on (not waiting for the user, not closed).
pub fn active(status: &str) -> bool { matches!(status, "backlog" | "dev" | "qa" | "agent_review") }

/// Most issues one project holds.
pub const MOST: i64 = 500;
/// The longest issue text or note, in bytes.
pub const TEXT_BYTES: usize = 8 * 1024;
/// How the user is named in an issue's history.
pub const USER: &str = "You";

/// Who acts on an issue: an agent's id (`main` for the main agent) and the name shown for it.
#[derive(Clone, Debug)]
pub struct Actor { pub id: String, pub name: String }

/// One entry in an issue's history.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event { pub at: i64, pub author: String, pub kind: String, pub text: String }

/// One issue, with its history when read on its own.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub number: i64,
    pub kind: String,
    pub title: String,
    pub text: String,
    pub severity: String,
    pub tags: Vec<String>,
    pub status: String,
    pub owner: Option<String>,
    pub owner_id: Option<String>,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<Event>>,
}

/// What an agent or the user sends to file an issue.
#[derive(Clone, Debug)]
pub struct Draft { pub kind: String, pub title: String, pub text: String, pub severity: String, pub tags: Vec<String> }

/// Why a change was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal { Shape, Missing, Full, Owned(String), NotOwner, Unowned, Closed, Lower, Storage }

impl From<StoreIssue> for Refusal {
    fn from(_: StoreIssue) -> Self { Self::Storage }
}

/// Ranks a severity; `None` for a word that is not one.
pub fn rank(severity: &str) -> Option<u8> {
    match severity { "low" => Some(0), "medium" => Some(1), "high" => Some(2), _ => None }
}

/// Ranks a review level: the highest severity that closes without review, or -1 for none.
fn level_rank(level: &str) -> Option<i8> {
    match level { "none" => Some(-1), "low" => Some(0), "medium" => Some(1), "high" => Some(2), _ => None }
}

/// Reports whether a review level is one the project setting accepts.
pub fn valid_level(level: &str) -> bool { level_rank(level).is_some() }

/// Reports whether an issue of this severity closes without the user under this level.
pub fn closes_alone(severity: &str, level: &str) -> bool {
    matches!((rank(severity), level_rank(level)), (Some(severity), Some(level)) if i8::try_from(severity).unwrap_or(i8::MAX) <= level)
}

/// Capitalises a severity or level for people and models.
pub fn word(value: &str) -> String {
    let mut chars = value.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// Says for a model what this project closes without review.
pub fn level_sentence(level: &str) -> &'static str {
    match level {
        "none" => "every issue you finish waits for the user",
        "medium" => "you may close Low and Medium issues yourself; High wait for the user",
        "high" => "you may close every issue yourself",
        _ => "you may close Low issues yourself; Medium and High wait for the user",
    }
}

/// Checks a one-line text of at most `most` characters.
fn line(text: &str, most: usize) -> bool {
    !text.trim().is_empty() && text.chars().count() <= most && !text.chars().any(char::is_control)
}

/// Checks a body text: present, bounded and free of NUL.
pub fn body(text: &str) -> bool { !text.trim().is_empty() && text.len() <= TEXT_BYTES && !text.contains('\0') }

/// Checks and tidies a draft, or refuses its shape.
fn tidy(draft: Draft) -> Result<Draft, Refusal> {
    let tags: Vec<String> = draft.tags.iter().map(|tag| tag.trim().to_lowercase()).filter(|tag| !tag.is_empty()).collect();
    if !matches!(draft.kind.as_str(), "bug" | "feature") || !line(&draft.title, 120) || !body(&draft.text) || rank(&draft.severity).is_none()
        || tags.len() > 8 || tags.iter().any(|tag| !line(tag, 24) || tag.contains(',')) {
        return Err(Refusal::Shape);
    }
    Ok(Draft { title: draft.title.trim().to_owned(), tags, ..draft })
}

/// Creates the issue tables inside the existing private metadata database.
pub fn migrate(db: &Connection) -> Result<(), StoreIssue> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS issues (
        project_id TEXT NOT NULL, number INTEGER NOT NULL, kind TEXT NOT NULL, title TEXT NOT NULL, text TEXT NOT NULL,
        severity TEXT NOT NULL, tags TEXT NOT NULL, status TEXT NOT NULL, owner_id TEXT, owner_name TEXT,
        created_by TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
        PRIMARY KEY (project_id, number)
    );
    CREATE TABLE IF NOT EXISTS issue_events (
        project_id TEXT NOT NULL, number INTEGER NOT NULL, at INTEGER NOT NULL, author TEXT NOT NULL,
        kind TEXT NOT NULL, text TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS issue_events_by_issue ON issue_events (project_id, number);
    CREATE TABLE IF NOT EXISTS issue_settings (project_id TEXT PRIMARY KEY, close_without_review TEXT NOT NULL);
    UPDATE issues SET status = CASE WHEN owner_id IS NULL THEN 'backlog' ELSE 'dev' END WHERE status = 'open';").map_err(StoreIssue::storage)
}

const COLUMNS: &str = "number, kind, title, text, severity, tags, status, owner_name, owner_id, created_by, created_at, updated_at";

/// Reads one issue row, without its history.
fn read(row: &rusqlite::Row) -> rusqlite::Result<Record> {
    let tags: String = row.get(5)?;
    Ok(Record { number: row.get(0)?, kind: row.get(1)?, title: row.get(2)?, text: row.get(3)?, severity: row.get(4)?,
        tags: tags.split(',').filter(|tag| !tag.is_empty()).map(str::to_owned).collect(), status: row.get(6)?,
        owner: row.get(7)?, owner_id: row.get(8)?, created_by: row.get(9)?, created_at: row.get(10)?, updated_at: row.get(11)?, events: None })
}

/// Reads one issue inside a transaction.
fn find(tx: &Transaction, project: &str, number: i64) -> Result<Option<Record>, Refusal> {
    tx.query_row(&format!("SELECT {COLUMNS} FROM issues WHERE project_id = ?1 AND number = ?2"), params![project, number], read)
        .optional().map_err(|_| Refusal::Storage)
}

/// Adds one history entry and marks the issue changed.
fn note(tx: &Transaction, project: &str, number: i64, author: &str, kind: &str, text: &str) -> Result<(), Refusal> {
    let now = chrono::Utc::now().timestamp_millis();
    tx.execute("INSERT INTO issue_events (project_id, number, at, author, kind, text) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![project, number, now, author, kind, text]).map_err(|_| Refusal::Storage)?;
    tx.execute("UPDATE issues SET updated_at = ?3 WHERE project_id = ?1 AND number = ?2", params![project, number, now])
        .map_err(|_| Refusal::Storage)?;
    Ok(())
}

impl Store {
    /// Lists a project's issues, newest change first, optionally filtered by words and status.
    pub async fn issues(&self, project: Uuid, query: Option<String>, status: Option<String>) -> Result<Vec<Record>, StoreIssue> {
        self.run(move |db| {
            let mut statement = db.prepare(&format!("SELECT {COLUMNS} FROM issues WHERE project_id = ?1 ORDER BY updated_at DESC, number DESC"))
                .map_err(StoreIssue::storage)?;
            let all: Vec<Record> = statement.query_map([project.to_string()], read).map_err(StoreIssue::storage)?
                .collect::<Result<_, _>>().map_err(StoreIssue::storage)?;
            let words: Vec<String> = query.unwrap_or_default().to_lowercase().split_whitespace().map(str::to_owned).collect();
            Ok(all.into_iter().filter(|issue| status.as_deref().is_none_or(|status| issue.status == status))
                .filter(|issue| {
                    let haystack = format!("#{} {} {} {}", issue.number, issue.title, issue.text, issue.tags.join(" ")).to_lowercase();
                    words.iter().all(|word| haystack.contains(word))
                }).collect())
        }).await
    }

    /// Returns one issue with its history.
    pub async fn issue(&self, project: Uuid, number: i64) -> Result<Option<Record>, StoreIssue> {
        self.run(move |db| {
            let project = project.to_string();
            let Some(mut issue) = db.query_row(&format!("SELECT {COLUMNS} FROM issues WHERE project_id = ?1 AND number = ?2"),
                params![project, number], read).optional().map_err(StoreIssue::storage)? else { return Ok(None) };
            let mut statement = db.prepare("SELECT at, author, kind, text FROM issue_events WHERE project_id = ?1 AND number = ?2 ORDER BY rowid")
                .map_err(StoreIssue::storage)?;
            issue.events = Some(statement.query_map(params![project, number], |row| Ok(Event { at: row.get(0)?, author: row.get(1)?,
                kind: row.get(2)?, text: row.get(3)? })).map_err(StoreIssue::storage)?.collect::<Result<_, _>>().map_err(StoreIssue::storage)?);
            Ok(Some(issue))
        }).await
    }

    /// Files a new open, unowned issue and returns it.
    pub async fn create_issue(&self, project: Uuid, draft: Draft, author: String) -> Result<Record, Refusal> {
        let draft = tidy(draft)?;
        self.run(move |db| {
            let tx = db.transaction().map_err(StoreIssue::storage)?;
            let project = project.to_string();
            let (count, last): (i64, i64) = tx.query_row("SELECT count(*), coalesce(max(number), 0) FROM issues WHERE project_id = ?1",
                [&project], |row| Ok((row.get(0)?, row.get(1)?))).map_err(StoreIssue::storage)?;
            if count >= MOST { return Ok(Err(Refusal::Full)); }
            let now = chrono::Utc::now().timestamp_millis();
            let number = last + 1;
            tx.execute("INSERT INTO issues (project_id, number, kind, title, text, severity, tags, status, created_by, created_at, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'backlog', ?8, ?9, ?9)",
                params![project, number, draft.kind, draft.title, draft.text, draft.severity, draft.tags.join(","), author, now])
                .map_err(StoreIssue::storage)?;
            let filed = format!("Filed this {}.", draft.kind);
            if let Err(refusal) = note(&tx, &project, number, &author, "filed", &filed) { return Ok(Err(refusal)); }
            let created = find(&tx, &project, number);
            tx.commit().map_err(StoreIssue::storage)?;
            Ok(created.and_then(|found| found.ok_or(Refusal::Storage)))
        }).await?.inspect(|_| changed(project))
    }

    /// Runs one change to an existing issue in a transaction, handing it the issue as it is now.
    async fn change<T: Send + 'static>(&self, project: Uuid, number: i64,
        change: impl FnOnce(&Transaction, &str, Record) -> Result<T, Refusal> + Send + 'static) -> Result<T, Refusal> {
        self.run(move |db| {
            let tx = db.transaction().map_err(StoreIssue::storage)?;
            let project = project.to_string();
            let outcome = match find(&tx, &project, number) {
                Ok(Some(issue)) => change(&tx, &project, issue),
                Ok(None) => Err(Refusal::Missing),
                Err(refusal) => Err(refusal),
            };
            if outcome.is_ok() { tx.commit().map_err(StoreIssue::storage)?; }
            Ok(outcome)
        }).await?.inspect(|_| changed(project))
    }

    /// Makes an agent the owner of an unowned issue; refuses with the current owner's name.
    pub async fn claim_issue(&self, project: Uuid, number: i64, actor: Actor) -> Result<(), Refusal> {
        self.change(project, number, move |tx, project, issue| {
            if issue.status == "closed" { return Err(Refusal::Closed); }
            match issue.owner_id.as_deref() {
                Some(id) if id == actor.id => return Ok(()),
                Some(_) => return Err(Refusal::Owned(issue.owner.unwrap_or_default())),
                None => {}
            }
            let changed = tx.execute("UPDATE issues SET owner_id = ?3, owner_name = ?4, status = CASE status WHEN 'backlog' THEN 'dev' ELSE status END
                WHERE project_id = ?1 AND number = ?2 AND owner_id IS NULL",
                params![project, number, actor.id, actor.name]).map_err(|_| Refusal::Storage)?;
            if changed == 0 { return Err(Refusal::Storage); }
            note(tx, project, number, &actor.name, "claim", "")
        }).await
    }

    /// Lets the owning agent give an issue up, with a handover note for the next owner.
    pub async fn release_issue(&self, project: Uuid, number: i64, actor: Actor, handover: String) -> Result<(), Refusal> {
        if !body(&handover) { return Err(Refusal::Shape); }
        self.change(project, number, move |tx, project, issue| {
            if issue.owner_id.as_deref() != Some(actor.id.as_str()) { return Err(Refusal::NotOwner); }
            tx.execute("UPDATE issues SET owner_id = NULL, owner_name = NULL, status = CASE status WHEN 'dev' THEN 'backlog' ELSE status END
                WHERE project_id = ?1 AND number = ?2", params![project, number]).map_err(|_| Refusal::Storage)?;
            note(tx, project, number, &actor.name, "handover", handover.trim())
        }).await
    }

    /// Records the owner's word that an issue is done. It closes when the project's level allows
    /// that severity, and otherwise waits for the user's review. Returns the new status.
    pub async fn finish_issue(&self, project: Uuid, number: i64, actor: Actor, summary: String) -> Result<String, Refusal> {
        if !body(&summary) { return Err(Refusal::Shape); }
        let level = self.review_level(project).await?;
        self.change(project, number, move |tx, project, issue| {
            if !active(&issue.status) { return Err(Refusal::Closed); }
            if issue.owner_id.as_deref() != Some(actor.id.as_str()) {
                return Err(if issue.owner_id.is_none() { Refusal::Unowned } else { Refusal::NotOwner });
            }
            let status = if closes_alone(&issue.severity, &level) { "closed" } else { "review" };
            tx.execute("UPDATE issues SET status = ?3 WHERE project_id = ?1 AND number = ?2", params![project, number, status])
                .map_err(|_| Refusal::Storage)?;
            note(tx, project, number, &actor.name, if status == "closed" { "closed" } else { "done" }, summary.trim())?;
            Ok(status.to_owned())
        }).await
    }

    /// Puts an issue its owner gives up on in front of the user, whatever the review level: a swarm
    /// does this with an issue that kept coming back or kept failing.
    pub async fn await_review(&self, project: Uuid, number: i64, actor: Actor, why: String) -> Result<(), Refusal> {
        if !body(&why) { return Err(Refusal::Shape); }
        self.change(project, number, move |tx, project, issue| {
            if !active(&issue.status) { return Err(Refusal::Closed); }
            if issue.owner_id.as_deref() != Some(actor.id.as_str()) { return Err(Refusal::NotOwner); }
            tx.execute("UPDATE issues SET status = 'review' WHERE project_id = ?1 AND number = ?2", params![project, number])
                .map_err(|_| Refusal::Storage)?;
            note(tx, project, number, &actor.name, "done", why.trim())
        }).await
    }

    /// Changes severity. An agent may only raise it; the user (`None`) may set any.
    pub async fn set_issue_severity(&self, project: Uuid, number: i64, severity: String, actor: Option<Actor>) -> Result<(), Refusal> {
        if rank(&severity).is_none() { return Err(Refusal::Shape); }
        self.change(project, number, move |tx, project, issue| {
            if actor.is_some() && rank(&severity) < rank(&issue.severity) { return Err(Refusal::Lower); }
            if issue.severity == severity { return Ok(()); }
            tx.execute("UPDATE issues SET severity = ?3 WHERE project_id = ?1 AND number = ?2", params![project, number, severity])
                .map_err(|_| Refusal::Storage)?;
            let author = actor.map(|actor| actor.name).unwrap_or_else(|| USER.into());
            note(tx, project, number, &author, "severity", &format!("{} to {}", word(&issue.severity), word(&severity)))
        }).await
    }

    /// Adds a comment from an agent or the user.
    pub async fn comment_issue(&self, project: Uuid, number: i64, author: String, text: String) -> Result<(), Refusal> {
        if !body(&text) { return Err(Refusal::Shape); }
        self.change(project, number, move |tx, project, _| note(tx, project, number, &author, "comment", text.trim())).await
    }

    /// The user gives an issue to an agent, or takes it away with `None`.
    pub async fn assign_issue(&self, project: Uuid, number: i64, owner: Option<Actor>) -> Result<(), Refusal> {
        self.assign_issue_as(project, number, owner, USER.into()).await
    }

    /// Gives an issue to an agent, or takes it away, in the name of `author`: the user, or a swarm's Lead.
    pub async fn assign_issue_as(&self, project: Uuid, number: i64, owner: Option<Actor>, author: String) -> Result<(), Refusal> {
        self.change(project, number, move |tx, project, _| {
            tx.execute("UPDATE issues SET owner_id = ?3, owner_name = ?4 WHERE project_id = ?1 AND number = ?2",
                params![project, number, owner.as_ref().map(|owner| owner.id.clone()), owner.as_ref().map(|owner| owner.name.clone())])
                .map_err(|_| Refusal::Storage)?;
            let text = owner.map(|owner| format!("Gave it to {}.", owner.name)).unwrap_or_else(|| "Took ownership away.".into());
            note(tx, project, number, &author, "owner", &text)
        }).await
    }

    /// The user closes an issue (approving a review) or reopens it.
    pub async fn set_issue_status(&self, project: Uuid, number: i64, close: bool) -> Result<(), Refusal> {
        self.change(project, number, move |tx, project, issue| {
            let status = if close { "closed" } else if issue.owner_id.is_some() { "dev" } else { "backlog" };
            tx.execute("UPDATE issues SET status = ?3 WHERE project_id = ?1 AND number = ?2", params![project, number, status])
                .map_err(|_| Refusal::Storage)?;
            note(tx, project, number, USER, if close { "closed" } else { "reopened" }, "")
        }).await
    }

    /// Deletes every issue of a project, their histories and its review level, when a swarm is deleted.
    pub async fn delete_project_issues(&self, project: Uuid) -> Result<(), StoreIssue> {
        self.run(move |db| {
            let id = project.to_string();
            for table in ["issue_events", "issues", "issue_settings"] {
                db.execute(&format!("DELETE FROM {table} WHERE project_id = ?1"), [&id]).map_err(StoreIssue::storage)?;
            }
            Ok(())
        }).await.inspect(|_| changed(project))
    }

    /// Returns the highest severity this project closes without review: none, low (default), medium or high.
    pub async fn review_level(&self, project: Uuid) -> Result<String, StoreIssue> {
        self.run(move |db| db.query_row("SELECT close_without_review FROM issue_settings WHERE project_id = ?1", [project.to_string()],
            |row| row.get::<_, String>(0)).optional().map(|level| level.unwrap_or_else(|| "low".into())).map_err(StoreIssue::storage)).await
    }

    /// Saves the project's review level.
    pub async fn set_review_level(&self, project: Uuid, level: String) -> Result<(), StoreIssue> {
        self.run(move |db| db.execute("INSERT INTO issue_settings (project_id, close_without_review) VALUES (?1, ?2)
            ON CONFLICT(project_id) DO UPDATE SET close_without_review = excluded.close_without_review", params![project.to_string(), level])
            .map(|_| ()).map_err(StoreIssue::storage)).await.inspect(|_| changed(project))
    }

    /// Moves an issue its owner is working on to another working stage: dev, qa or agent_review.
    pub async fn set_issue_stage(&self, project: Uuid, number: i64, actor: Actor, stage: String) -> Result<(), Refusal> {
        if !WORK_STAGES.contains(&stage.as_str()) { return Err(Refusal::Shape); }
        self.change(project, number, move |tx, project, issue| {
            if !active(&issue.status) { return Err(Refusal::Closed); }
            if issue.owner_id.as_deref() != Some(actor.id.as_str()) {
                return Err(if issue.owner_id.is_none() { Refusal::Unowned } else { Refusal::NotOwner });
            }
            if issue.status == stage { return Ok(()); }
            tx.execute("UPDATE issues SET status = ?3 WHERE project_id = ?1 AND number = ?2", params![project, number, stage])
                .map_err(|_| Refusal::Storage)?;
            note(tx, project, number, &actor.name, "stage", stage_word(&stage))
        }).await
    }
}

/// Names a stage the way the board does.
pub fn stage_word(stage: &str) -> &'static str {
    match stage {
        "backlog" => "Backlog", "dev" => "In Dev", "qa" => "QA", "agent_review" => "Review",
        "review" => "Human Review", "closed" => "Closed", _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_levels_close_up_to_their_severity() {
        assert!(closes_alone("low", "low") && !closes_alone("medium", "low") && !closes_alone("high", "low"));
        assert!(!closes_alone("low", "none"));
        assert!(closes_alone("medium", "medium") && !closes_alone("high", "medium"));
        assert!(closes_alone("high", "high"));
        assert!(!closes_alone("urgent", "high") && !closes_alone("low", "most"));
    }

    #[test]
    fn drafts_are_checked() {
        let draft = |title: &str, severity: &str, tags: Vec<&str>| Draft { kind: "bug".into(), title: title.into(), text: "Steps.".into(),
            severity: severity.into(), tags: tags.into_iter().map(str::to_owned).collect() };
        assert!(tidy(draft("Queue paused", "low", vec!["Queue", " "])).is_ok_and(|draft| draft.tags == ["queue"]));
        assert_eq!(tidy(draft("", "low", vec![])).err(), Some(Refusal::Shape));
        assert_eq!(tidy(draft("Two\nlines", "low", vec![])).err(), Some(Refusal::Shape));
        assert_eq!(tidy(draft("Ok", "urgent", vec![])).err(), Some(Refusal::Shape));
        assert_eq!(tidy(draft("Ok", "low", vec!["a,b"])).err(), Some(Refusal::Shape));
    }
}
