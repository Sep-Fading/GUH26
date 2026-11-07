//! Records of delegated runs. A run is one agent doing one task for another agent's
//! conversation. The record binds the run to its parent turn and its tool call, so a repeated
//! call never starts a second run, and a restart finds every unfinished run to close.

// Reuses the private metadata database.
use crate::agents::{Issue, Store};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub parent: String,
    pub turn_id: String,
    pub call_id: String,
    /// The run's own thread, once it exists.
    pub thread: Option<String>,
    pub agent_id: String,
    pub agent: String,
    pub task: String,
    /// `wait`, `background`, or empty while the user has not answered. A waited run handed to the
    /// background at the hour becomes `background`.
    pub mode: String,
    /// `asking`, `running`, `done`, `failed`, `declined`, `cancelled`.
    pub status: String,
    pub result: Option<String>,
    pub started: i64,
    pub ended: Option<i64>,
    /// A detached run is an ordinary conversation of its own.
    pub detached: bool,
    /// The pictures and files that went with the task, as the page shows them: id, name, kind.
    pub attachments: Vec<serde_json::Value>,
    /// The delegating model's one-line description of the task, for the run's card. Empty for a
    /// run recorded before it was asked for.
    pub summary: String,
    /// Set when the user chose to wait and the run, still going at the hour, continued in the
    /// background (lead decision, 2026-10-11). Its card says so.
    pub handed_over: bool,
}

impl Run {
    /// Keeps delegated-tool restrictions until an opened run finishes its original task.
    pub fn is_delegated(&self) -> bool {
        !self.detached || matches!(self.status.as_str(), "asking" | "running")
    }
}

/// Creates the run table inside the existing private metadata database.
pub fn migrate(db: &Connection) -> Result<(), Issue> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS delegated_runs (
        id TEXT PRIMARY KEY, parent TEXT NOT NULL, turn_id TEXT NOT NULL, call_id TEXT NOT NULL,
        thread TEXT, agent_id TEXT NOT NULL, agent TEXT NOT NULL, task TEXT NOT NULL,
        mode TEXT NOT NULL DEFAULT '', status TEXT NOT NULL, result TEXT,
        started INTEGER NOT NULL, ended INTEGER, detached INTEGER NOT NULL DEFAULT 0,
        delivered INTEGER NOT NULL DEFAULT 0,
        UNIQUE(parent, call_id)
    );").map_err(Issue::storage)?;
    // What went with the task, its summary and the hand-over arrived after the first release of this table.
    for (column, shape) in [("attachments", "TEXT NOT NULL DEFAULT '[]'"), ("summary", "TEXT NOT NULL DEFAULT ''"),
        ("handed_over", "INTEGER NOT NULL DEFAULT 0")] {
        let known = db.prepare("SELECT 1 FROM pragma_table_info('delegated_runs') WHERE name = ?1")
            .and_then(|mut query| query.exists([column])).map_err(Issue::storage)?;
        if !known {
            db.execute_batch(&format!("ALTER TABLE delegated_runs ADD COLUMN {column} {shape};")).map_err(Issue::storage)?;
        }
    }
    Ok(())
}

const COLUMNS: &str = "id, parent, turn_id, call_id, thread, agent_id, agent, task, mode, status, result, started, ended, detached, attachments, summary, handed_over";

fn row(row: &rusqlite::Row) -> rusqlite::Result<Run> {
    Ok(Run { id: row.get(0)?, parent: row.get(1)?, turn_id: row.get(2)?, call_id: row.get(3)?,
        thread: row.get(4)?, agent_id: row.get(5)?, agent: row.get(6)?, task: row.get(7)?,
        mode: row.get(8)?, status: row.get(9)?, result: row.get(10)?, started: row.get(11)?,
        ended: row.get(12)?, detached: row.get(13)?,
        attachments: serde_json::from_str(&row.get::<_, String>(14)?).unwrap_or_default(), summary: row.get(15)?,
        handed_over: row.get(16)? })
}

impl Store {
    /// Records a new request, or returns `None` when this tool call was already recorded.
    pub async fn begin_run(&self, run: Run) -> Result<Option<Run>, Issue> {
        self.run(move |db| {
            let added = db.execute(&format!("INSERT INTO delegated_runs ({COLUMNS})
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                ON CONFLICT(parent, call_id) DO NOTHING"),
                params![run.id, run.parent, run.turn_id, run.call_id, run.thread, run.agent_id, run.agent,
                    run.task, run.mode, run.status, run.result, run.started, run.ended, run.detached,
                    serde_json::Value::from(run.attachments.clone()).to_string(), run.summary, run.handed_over])
                .map_err(Issue::storage)?;
            Ok((added == 1).then_some(run))
        }).await
    }

    /// Counts the requests one parent turn has made, whatever became of them.
    pub async fn runs_in_turn(&self, parent: String, turn: String) -> Result<u32, Issue> {
        self.run(move |db| db.query_row("SELECT count(*) FROM delegated_runs WHERE parent = ?1 AND turn_id = ?2",
            params![parent, turn], |row| row.get(0)).map_err(Issue::storage)).await
    }

    /// Changes a run's state. A finished run is never changed again, so a late or duplicate
    /// event cannot rewrite its outcome.
    pub async fn update_run(&self, id: String, mode: Option<String>, thread: Option<String>, status: String,
        result: Option<String>) -> Result<Option<Run>, Issue> {
        self.run(move |db| {
            let ended = (!matches!(status.as_str(), "asking" | "running")).then(|| chrono::Utc::now().timestamp_millis());
            db.execute("UPDATE delegated_runs SET mode = COALESCE(?2, mode), thread = COALESCE(?3, thread),
                status = ?4, result = COALESCE(?5, result), ended = COALESCE(?6, ended)
                WHERE id = ?1 AND status IN ('asking', 'running')",
                params![id, mode, thread, status, result, ended]).map_err(Issue::storage)?;
            db.query_row(&format!("SELECT {COLUMNS} FROM delegated_runs WHERE id = ?1"), [&id], row)
                .optional().map_err(Issue::storage)
        }).await
    }

    /// Hands a waited run that is still going to the background: from now on it is a background run,
    /// whose result goes back to the parent by itself. `None` when it is not a running waited run.
    pub async fn hand_over_run(&self, id: String) -> Result<Option<Run>, Issue> {
        self.run(move |db| {
            let changed = db.execute("UPDATE delegated_runs SET mode = 'background', handed_over = 1
                WHERE id = ?1 AND status = 'running' AND mode = 'wait'", [&id]).map_err(Issue::storage)?;
            if changed == 0 { return Ok(None); }
            db.query_row(&format!("SELECT {COLUMNS} FROM delegated_runs WHERE id = ?1"), [&id], row)
                .optional().map_err(Issue::storage)
        }).await
    }

    /// Lists a conversation's runs, oldest first.
    pub async fn runs(&self, parent: String) -> Result<Vec<Run>, Issue> {
        self.run(move |db| {
            let mut query = db.prepare(&format!("SELECT {COLUMNS} FROM delegated_runs WHERE parent = ?1
                ORDER BY started, rowid LIMIT 500")).map_err(Issue::storage)?;
            let rows = query.query_map([parent], row).map_err(Issue::storage)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(Issue::storage)
        }).await
    }

    /// Lists active delegations for pending-request discovery without loading transcripts.
    pub async fn running_runs(&self) -> Result<Vec<Run>, Issue> {
        self.run(|db| {
            let mut query = db.prepare(&format!("SELECT {COLUMNS} FROM delegated_runs
                WHERE status = 'running' ORDER BY started, rowid")).map_err(Issue::storage)?;
            let rows = query.query_map([], row).map_err(Issue::storage)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(Issue::storage)
        }).await
    }

    /// Reads recent runs of one parent, or one exact owned run, for a bounded status response.
    pub async fn recent_runs(&self, parent: String, id: Option<String>) -> Result<Vec<Run>, Issue> {
        self.run(move |db| {
            let mut query = db.prepare(&format!("SELECT {COLUMNS} FROM delegated_runs WHERE parent = ?1
                AND (?2 IS NULL OR id = ?2) ORDER BY started DESC, rowid DESC LIMIT 10"))
                .map_err(Issue::storage)?;
            let rows = query.query_map(params![parent, id], row).map_err(Issue::storage)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(Issue::storage)
        }).await
    }

    /// Finds the run whose own thread this is.
    pub async fn run_of_thread(&self, thread: String) -> Result<Option<Run>, Issue> {
        self.run(move |db| db.query_row(&format!("SELECT {COLUMNS} FROM delegated_runs WHERE thread = ?1"),
            [thread], row).optional().map_err(Issue::storage)).await
    }

    /// Lists the threads that belong to runs and are not conversations of their own.
    pub async fn hidden_run_threads(&self) -> Result<std::collections::HashSet<String>, Issue> {
        self.run(|db| {
            let mut query = db.prepare("SELECT thread FROM delegated_runs WHERE thread IS NOT NULL AND detached = 0")
                .map_err(Issue::storage)?;
            let rows = query.query_map([], |row| row.get::<_, String>(0)).map_err(Issue::storage)?;
            rows.collect::<Result<_, _>>().map_err(Issue::storage)
        }).await
    }

    /// Lists the background results the parent agent has not been told yet, without taking them,
    /// so MiniClaw can say whose they are when it starts the parent's turn that delivers them.
    pub async fn pending_results(&self, parent: String) -> Result<Vec<Run>, Issue> {
        self.run(move |db| {
            let mut query = db.prepare(&format!("SELECT {COLUMNS} FROM delegated_runs WHERE parent = ?1
                AND mode = 'background' AND delivered = 0 AND status IN ('done', 'failed') ORDER BY ended LIMIT 16"))
                .map_err(Issue::storage)?;
            let rows = query.query_map([&parent], row).map_err(Issue::storage)?;
            rows.collect::<Result<_, _>>().map_err(Issue::storage)
        }).await
    }

    /// Takes the background results the parent agent has not been told yet. Each is handed
    /// out once, so a result is never repeated to the agent.
    pub async fn take_results(&self, parent: String) -> Result<Vec<Run>, Issue> {
        self.run(move |db| {
            let tx = db.transaction().map_err(Issue::storage)?;
            let runs = {
                let mut query = tx.prepare(&format!("SELECT {COLUMNS} FROM delegated_runs WHERE parent = ?1
                    AND mode = 'background' AND delivered = 0 AND status IN ('done', 'failed') ORDER BY ended LIMIT 16"))
                    .map_err(Issue::storage)?;
                query.query_map([&parent], row).map_err(Issue::storage)?
                    .collect::<Result<Vec<_>, _>>().map_err(Issue::storage)?
            };
            for run in &runs {
                tx.execute("UPDATE delegated_runs SET delivered = 1 WHERE id = ?1", [&run.id]).map_err(Issue::storage)?;
            }
            tx.commit().map_err(Issue::storage)?;
            Ok(runs)
        }).await
    }

    /// Makes an existing run visible as its own conversation without restarting its work.
    pub async fn detach_run(&self, id: String) -> Result<Option<Run>, Issue> {
        self.run(move |db| {
            db.execute("UPDATE delegated_runs SET detached = 1 WHERE id = ?1 AND thread IS NOT NULL
                AND status != 'asking'", [&id]).map_err(Issue::storage)?;
            db.query_row(&format!("SELECT {COLUMNS} FROM delegated_runs WHERE id = ?1"), [&id], row)
                .optional().map_err(Issue::storage)
        }).await
    }

    /// Closes runs a stopped backend left open. They are never run again.
    pub async fn close_interrupted_runs(&self) -> Result<usize, Issue> {
        self.run(|db| db.execute("UPDATE delegated_runs SET status = 'failed',
            result = 'MiniClaw stopped before this run finished. It was not run again.', ended = ?1
            WHERE status IN ('asking', 'running')", [chrono::Utc::now().timestamp_millis()])
            .map_err(Issue::storage)).await
    }

    /// Removes the records of a deleted conversation.
    pub async fn remove_runs(&self, parent: String) -> Result<Vec<String>, Issue> {
        self.run(move |db| {
            let mut query = db.prepare("SELECT thread FROM delegated_runs WHERE parent = ?1 AND thread IS NOT NULL
                AND detached = 0").map_err(Issue::storage)?;
            let threads = query.query_map([&parent], |row| row.get::<_, String>(0)).map_err(Issue::storage)?
                .collect::<Result<Vec<_>, _>>().map_err(Issue::storage)?;
            db.execute("DELETE FROM delegated_runs WHERE parent = ?1", [&parent]).map_err(Issue::storage)?;
            Ok(threads)
        }).await
    }
}
