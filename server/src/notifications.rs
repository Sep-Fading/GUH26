// Reuses protected metadata storage and errors.
use crate::agents::{Issue, Store};
// Applies transactional persistence and parameterized queries.
use rusqlite::{Connection, params};
// Serializes safe notice fields for browsers.
use serde::Serialize;
// Reads account identity without storing content.
use serde_json::Value;
// Derives opaque account keys without credentials.
use sha2::{Digest, Sha256};
// Creates notice identities independent of event content.
use uuid::Uuid;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    pub id: String,
    pub kind: String,
    pub message: &'static str,
    pub thread_id: Option<String>,
    pub created_at: i64,
    pub read: bool,
}

/// Returns an account key only for a known signed-in identity.
pub fn owner(account: &Value) -> Option<String> {
    let email = account["email"].as_str()?.trim();
    if account["status"] != "signed_in" || email.is_empty() || email.len() > 320 {
        return None;
    }
    Some(Sha256::digest(email.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Creates the notice table inside the existing private metadata database.
pub fn migrate(db: &Connection) -> Result<(), Issue> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS notifications (
        id TEXT PRIMARY KEY, owner TEXT NOT NULL, event TEXT NOT NULL,
        kind TEXT NOT NULL, thread_id TEXT, created_at INTEGER NOT NULL,
        read INTEGER NOT NULL DEFAULT 0, UNIQUE(owner, event)
    );").map_err(Issue::storage)
}

impl Store {
    /// Records one safe response failure; duplicate events preserve its read state.
    pub async fn response_failure(&self, owner: String, thread: String, turn: String)
        -> Result<(), Issue>
    {
        if Uuid::parse_str(&thread).is_err() || turn.is_empty() || turn.len() > 128 {
            return Err(Issue::Invalid);
        }
        self.record_notice(owner, format!("response:{thread}:{turn}"),
            "response_failed", Some(thread)).await
    }

    /// Records one runtime disconnection per backend session without upstream details.
    pub async fn runtime_failure(&self, owner: String, session: String) -> Result<(), Issue> {
        self.record_notice(owner, format!("runtime:{session}"), "runtime_disconnected", None).await
    }

    /// Records that a VM was shut down for being idle; one notice per shutdown.
    pub async fn vm_idle_stopped(&self, owner: String, vm_id: String) -> Result<(), Issue> {
        self.record_notice(owner, format!("vm-idle:{vm_id}:{}", Uuid::new_v4()),
            "vm_idle_stopped", None).await
    }

    /// Records that a backup failed after its retries; one notice per failure.
    pub async fn backup_failed(&self, owner: String, vm_id: String) -> Result<(), Issue> {
        self.record_notice(owner, format!("backup:{vm_id}:{}", Uuid::new_v4()), "backup_failed", None).await
    }

    /// Records a scheduled run that failed or was terminated; one notice per run.
    pub async fn scheduled_run_ended(&self, owner: String, schedule: String, status: &str) -> Result<(), Issue> {
        self.record_notice(owner, format!("schedule:{schedule}:{}", Uuid::new_v4()),
            if status == "terminated" { "scheduled_run_terminated" } else { "scheduled_run_failed" }, None).await
    }

    /// Records a proposed change that nobody reviewed in time.
    pub async fn review_expired(&self, owner: String, thread: String) -> Result<(), Issue> {
        self.record_notice(owner, format!("review:{thread}:{}", Uuid::new_v4()), "review_expired", Some(thread)).await
    }

    /// Records a VM that failed to start or stopped by itself; one notice per failure.
    pub async fn vm_failed(&self, owner: String, vm_id: String, kind: &'static str) -> Result<(), Issue> {
        self.record_notice(owner, format!("vm:{vm_id}:{}", Uuid::new_v4()), kind, None).await
    }

    /// Commits a failure and retains the newest thousand notices across accounts.
    async fn record_notice(&self, owner: String, event: String, kind: &'static str,
        thread: Option<String>) -> Result<(), Issue>
    {
        self.run(move |db| {
            let tx = db.transaction().map_err(Issue::storage)?;
            tx.execute("INSERT INTO notifications (id, owner, event, kind, thread_id, created_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(owner, event) DO NOTHING",
                params![Uuid::new_v4().to_string(), owner, event, kind, thread,
                    chrono::Utc::now().timestamp()]).map_err(Issue::storage)?;
            tx.execute("DELETE FROM notifications WHERE rowid NOT IN
                (SELECT rowid FROM notifications ORDER BY rowid DESC LIMIT 1000)", [])
                .map_err(Issue::storage)?;
            tx.commit().map_err(Issue::storage)
        }).await
    }

    /// Lists the current account's retained notices with fixed public messages.
    pub async fn notices(&self, owner: String) -> Result<Vec<Notice>, Issue> {
        self.run(move |db| {
            let mut query = db.prepare("SELECT id, kind, thread_id, created_at, read
                FROM notifications WHERE owner = ?1 ORDER BY rowid DESC LIMIT 1000")
                .map_err(Issue::storage)?;
            let rows = query.query_map([owner], |row| {
                let kind: String = row.get(1)?;
                let message = match kind.as_str() {
                    "response_failed" => "Codex could not complete this response. Open the conversation to review it.",
                    "runtime_disconnected" => "The Codex connection was lost. Check connection status before continuing.",
                    "scheduled_run_terminated" => "A scheduled run was terminated: it needed an approval and nobody answered in time. See Schedules for the request.",
                    "scheduled_run_failed" => "A scheduled run failed. See Schedules for the reason.",
                    "vm_start_failed" => "A VM could not start. Settings, Virtualisation shows its state and the reason.",
                    "vm_stopped_unexpectedly" => "A VM stopped by itself. Conversations in it cannot continue until it is started again.",
                    "review_expired" => "A proposed change waited 30 minutes for review and was rejected. Open the conversation to continue.",
                    "backup_failed" => "A VM backup failed after three retries. See the VM's backup settings for the reason.",
                    "vm_idle_stopped" => "A VM was shut down because it was idle. Change the idle time in its settings.",
                    _ => "An application operation failed. Check connection status.",
                };
                Ok(Notice { id: row.get(0)?, kind, message, thread_id: row.get(2)?,
                    created_at: row.get(3)?, read: row.get(4)? })
            }).map_err(Issue::storage)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(Issue::storage)
        }).await
    }

    /// Marks every unread notice of this account read, and says how many that was.
    pub async fn read_all_notices(&self, owner: String) -> Result<usize, Issue> {
        self.run(move |db| db.execute("UPDATE notifications SET read = 1 WHERE owner = ?1 AND read = 0", [owner])
            .map_err(Issue::storage)).await
    }

    /// Deletes this account's read notices and returns the number removed.
    pub async fn clear_read_notices(&self, owner: String) -> Result<usize, Issue> {
        self.run(move |db| {
            db.execute("DELETE FROM notifications WHERE owner = ?1 AND read = 1", [owner])
                .map_err(Issue::storage)
        }).await
    }

    /// Marks an owned notice read without changing any other account's records.
    pub async fn read_notice(&self, owner: String, id: Uuid) -> Result<(), Issue> {
        self.run(move |db| {
            let count = db.execute("UPDATE notifications SET read = 1 WHERE owner = ?1 AND id = ?2",
                params![owner, id.to_string()]).map_err(Issue::storage)?;
            if count == 0 { return Err(Issue::Missing); }
            Ok(())
        }).await
    }
}
