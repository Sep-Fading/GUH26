//! Saved prompts that run by themselves: one agent, optionally in one project, hourly, daily
//! or weekly. A schedule is saved switched off. Each run is an ordinary conversation, so its
//! whole transcript and activity can be read afterwards.

// Reuses the private metadata database.
use crate::agents::{Issue, Store};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Schedule {
    #[serde(default, skip_deserializing)]
    pub id: String,
    pub name: String,
    pub agent_id: Uuid,
    #[serde(default)]
    pub project_id: Option<Uuid>,
    pub prompt: String,
    /// `hourly`, `daily` or `weekly`.
    pub every: String,
    #[serde(default)]
    pub enabled: bool,
    /// How long an approval may wait for an answer before the run is terminated.
    #[serde(default = "five")]
    pub approval_minutes: u8,
    #[serde(default, skip_deserializing)] pub last_run: Option<i64>,
    #[serde(default, skip_deserializing)] pub last_status: Option<String>,
    #[serde(default, skip_deserializing)] pub last_result: Option<String>,
    #[serde(default, skip_deserializing)] pub last_thread: Option<String>,
}

fn five() -> u8 { 5 }

impl Schedule {
    /// Accepts bounded text, a known period and an approval wait of 1 to 60 minutes.
    pub fn valid(&self) -> bool {
        let name = self.name.trim();
        !name.is_empty() && name.len() <= 80 && !self.prompt.trim().is_empty() && self.prompt.len() <= 8000
            && matches!(self.every.as_str(), "hourly" | "daily" | "weekly") && (1..=60).contains(&self.approval_minutes)
    }

    /// The seconds between two runs.
    pub fn period(&self) -> i64 {
        match self.every.as_str() { "hourly" => 3600, "weekly" => 604_800, _ => 86_400 }
    }
}

/// Creates the schedule table inside the existing private metadata database.
pub fn migrate(db: &Connection) -> Result<(), Issue> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS schedules (
        id TEXT PRIMARY KEY, definition TEXT NOT NULL, last_run INTEGER, last_status TEXT,
        last_result TEXT, last_thread TEXT
    );").map_err(Issue::storage)
}

fn read(row: &rusqlite::Row) -> rusqlite::Result<Option<Schedule>> {
    let mut schedule: Schedule = match serde_json::from_str(&row.get::<_, String>(1)?) { Ok(value) => value, Err(_) => return Ok(None) };
    schedule.id = row.get(0)?;
    (schedule.last_run, schedule.last_status, schedule.last_result, schedule.last_thread) =
        (row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?);
    Ok(Some(schedule))
}

impl Store {
    /// Lists every saved schedule, oldest first.
    pub async fn schedules(&self) -> Result<Vec<Schedule>, Issue> {
        self.run(|db| {
            let mut query = db.prepare("SELECT id, definition, last_run, last_status, last_result, last_thread
                FROM schedules ORDER BY rowid LIMIT 64").map_err(Issue::storage)?;
            let rows = query.query_map([], read).map_err(Issue::storage)?;
            Ok(rows.collect::<Result<Vec<_>, _>>().map_err(Issue::storage)?.into_iter().flatten().collect())
        }).await
    }

    /// Creates a schedule, or replaces the definition of an existing one. At most 32 exist.
    pub async fn save_schedule(&self, id: Option<String>, schedule: Schedule) -> Result<Schedule, Issue> {
        if !schedule.valid() { return Err(Issue::Invalid); }
        self.run(move |db| {
            // Only what the user defines is stored here; the state of the last run has its own columns.
            let mut definition = serde_json::to_value(&schedule).map_err(Issue::storage)?;
            if let Some(fields) = definition.as_object_mut() {
                for state in ["id", "lastRun", "lastStatus", "lastResult", "lastThread"] { fields.remove(state); }
            }
            let text = definition.to_string();
            let id = match id {
                Some(id) => {
                    if db.execute("UPDATE schedules SET definition = ?2 WHERE id = ?1", params![id, text])
                        .map_err(Issue::storage)? == 0 { return Err(Issue::Missing); }
                    id
                }
                None => {
                    let count: i64 = db.query_row("SELECT count(*) FROM schedules", [], |row| row.get(0)).map_err(Issue::storage)?;
                    if count >= 32 { return Err(Issue::Limit); }
                    let id = Uuid::new_v4().to_string();
                    db.execute("INSERT INTO schedules (id, definition) VALUES (?1, ?2)", params![id, text]).map_err(Issue::storage)?;
                    id
                }
            };
            db.query_row("SELECT id, definition, last_run, last_status, last_result, last_thread FROM schedules WHERE id = ?1",
                [&id], read).optional().map_err(Issue::storage)?.flatten().ok_or(Issue::Missing)
        }).await
    }

    /// Deletes a schedule. Conversations of its earlier runs stay.
    pub async fn remove_schedule(&self, id: String) -> Result<(), Issue> {
        self.run(move |db| {
            if db.execute("DELETE FROM schedules WHERE id = ?1", [&id]).map_err(Issue::storage)? == 0 { return Err(Issue::Missing); }
            Ok(())
        }).await
    }

    /// Records where a run stands. `started` also stamps the time the period counts from.
    pub async fn mark_schedule(&self, id: String, status: String, result: Option<String>, thread: Option<String>,
        started: Option<i64>) -> Result<(), Issue> {
        self.run(move |db| {
            db.execute("UPDATE schedules SET last_status = ?2, last_result = ?3, last_thread = COALESCE(?4, last_thread),
                last_run = COALESCE(?5, last_run) WHERE id = ?1", params![id, status, result, thread, started])
                .map_err(Issue::storage)?;
            Ok(())
        }).await
    }

    /// Closes runs a stopped backend left open. They are not run again until they are next due.
    pub async fn close_interrupted_schedules(&self) -> Result<usize, Issue> {
        self.run(|db| db.execute("UPDATE schedules SET last_status = 'failed',
            last_result = 'MiniClaw stopped before this run finished.' WHERE last_status = 'running'", [])
            .map_err(Issue::storage)).await
    }
}
