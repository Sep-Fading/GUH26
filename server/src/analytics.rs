//! What MiniClaw records about its own use: one row per finished turn with the tokens the
//! provider reported, one row per tool call, and one row per skill that was active. It holds
//! counts and names, never a message, a file's content or a tool's output. The Analytics tab
//! reads the aggregates; nothing here is ever sent to a model.

// Shares the metadata store and its errors.
use crate::agents::{Issue, Store};
// Reads and writes the three tables.
use rusqlite::{Connection, params};
// Shapes a recorded turn and the answer the page reads.
use serde::Serialize;
use serde_json::{Value, json};

/// How long rows are kept. A year of counts is small and makes a year's comparison possible.
const KEPT_DAYS: i64 = 365;
/// The longest range one request may ask for.
pub const MOST_DAYS: i64 = 365;
/// How many rows one grouped list returns.
const ROWS: usize = 24;

/// One finished turn, with what the provider charged for it.
#[derive(Clone, Debug, Default)]
pub struct Turn {
    pub thread: String,
    pub turn: String,
    pub provider: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub agent: Option<String>,
    pub project: Option<String>,
    pub status: String,
    pub seconds: f64,
    pub input: i64,
    pub cached: i64,
    pub output: i64,
    pub reasoning: i64,
    pub total: i64,
}

/// One tool call, as the Activity log describes it.
#[derive(Clone, Debug)]
pub struct Step { pub thread: String, pub turn: String, pub kind: String, pub name: String, pub status: String, pub ms: i64 }

/// One skill that took part in a turn, and where it came from.
#[derive(Clone, Debug)]
pub struct SkillUse { pub thread: String, pub turn: String, pub name: String, pub source: String }

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Day { date: String, turns: i64, tokens: i64, input_tokens: i64, output_tokens: i64, cached_tokens: i64, steps: i64 }

/// Creates the three tables inside the existing private metadata database.
pub fn migrate(db: &Connection) -> Result<(), Issue> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS usage_turns (
        thread_id TEXT NOT NULL, turn_id TEXT NOT NULL, at INTEGER NOT NULL, provider TEXT NOT NULL,
        model TEXT, effort TEXT, agent TEXT, project TEXT, status TEXT NOT NULL, seconds REAL NOT NULL DEFAULT 0,
        input_tokens INTEGER NOT NULL DEFAULT 0, cached_tokens INTEGER NOT NULL DEFAULT 0,
        output_tokens INTEGER NOT NULL DEFAULT 0, reasoning_tokens INTEGER NOT NULL DEFAULT 0,
        total_tokens INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (thread_id, turn_id)
    );
    CREATE INDEX IF NOT EXISTS usage_turns_at ON usage_turns (at);
    CREATE TABLE IF NOT EXISTS usage_steps (
        id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, turn_id TEXT NOT NULL, at INTEGER NOT NULL,
        kind TEXT NOT NULL, name TEXT NOT NULL, status TEXT NOT NULL, ms INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX IF NOT EXISTS usage_steps_at ON usage_steps (at);
    CREATE INDEX IF NOT EXISTS usage_steps_turn ON usage_steps (thread_id, turn_id);
    CREATE TABLE IF NOT EXISTS usage_skills (
        thread_id TEXT NOT NULL, turn_id TEXT NOT NULL, at INTEGER NOT NULL, name TEXT NOT NULL,
        source TEXT NOT NULL, PRIMARY KEY (thread_id, turn_id, name, source)
    );
    CREATE INDEX IF NOT EXISTS usage_skills_at ON usage_skills (at);").map_err(Issue::storage)
}

/// The day a moment belongs to, in the user's own time zone, as `YYYY-MM-DD`.
fn day(at: i64) -> String {
    chrono::DateTime::from_timestamp_millis(at).map(|time| time.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// The moment this many days ago started, so a range always begins at a day's boundary.
fn since(days: i64) -> i64 {
    use chrono::{Duration, Local};
    (Local::now().date_naive() - Duration::days(days - 1)).and_hms_opt(0, 0, 0)
        .and_then(|naive| naive.and_local_timezone(Local).earliest())
        .map(|time| time.timestamp_millis()).unwrap_or(0)
}

/// Sums one column of a grouped query into rows of `{ name, ... }`.
fn grouped(db: &Connection, sql: &str, from: i64, shape: impl Fn(&rusqlite::Row) -> rusqlite::Result<Value>) -> Result<Vec<Value>, Issue> {
    let mut query = db.prepare(sql).map_err(Issue::storage)?;
    let rows = query.query_map(params![from, ROWS as i64], |row| shape(row)).map_err(Issue::storage)?;
    rows.collect::<Result<_, _>>().map_err(Issue::storage)
}

impl Store {
    /// Records one finished turn and drops anything older than a year.
    pub async fn record_turn(&self, turn: Turn) -> Result<(), Issue> {
        self.run(move |db| {
            let at = chrono::Utc::now().timestamp_millis();
            db.execute("INSERT INTO usage_turns (thread_id, turn_id, at, provider, model, effort, agent, project, status, seconds,
                input_tokens, cached_tokens, output_tokens, reasoning_tokens, total_tokens)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                ON CONFLICT(thread_id, turn_id) DO UPDATE SET status = excluded.status, seconds = excluded.seconds,
                input_tokens = excluded.input_tokens, cached_tokens = excluded.cached_tokens, output_tokens = excluded.output_tokens,
                reasoning_tokens = excluded.reasoning_tokens, total_tokens = excluded.total_tokens",
                params![turn.thread, turn.turn, at, turn.provider, turn.model, turn.effort, turn.agent, turn.project, turn.status,
                    turn.seconds, turn.input, turn.cached, turn.output, turn.reasoning, turn.total]).map_err(Issue::storage)?;
            let old = at - KEPT_DAYS * 86_400_000;
            for table in ["usage_turns", "usage_steps", "usage_skills"] {
                db.execute(&format!("DELETE FROM {table} WHERE at < ?1"), [old]).map_err(Issue::storage)?;
            }
            Ok(())
        }).await
    }

    /// Records one tool call. The same id twice is the same call, finished.
    pub async fn record_step(&self, id: String, step: Step) -> Result<(), Issue> {
        self.run(move |db| db.execute("INSERT INTO usage_steps (id, thread_id, turn_id, at, kind, name, status, ms)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) ON CONFLICT(id) DO UPDATE SET status = excluded.status, ms = excluded.ms",
            params![id, step.thread, step.turn, chrono::Utc::now().timestamp_millis(), step.kind, step.name, step.status, step.ms])
            .map(|_| ()).map_err(Issue::storage)).await
    }

    /// Records the skills that took part in one turn.
    pub async fn record_skills(&self, uses: Vec<SkillUse>) -> Result<(), Issue> {
        if uses.is_empty() { return Ok(()); }
        self.run(move |db| {
            let at = chrono::Utc::now().timestamp_millis();
            for use_ in uses {
                db.execute("INSERT OR IGNORE INTO usage_skills (thread_id, turn_id, at, name, source) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![use_.thread, use_.turn, at, use_.name, use_.source]).map_err(Issue::storage)?;
            }
            Ok(())
        }).await
    }

    /// Returns everything the Analytics tab draws for a range of days.
    pub async fn analytics(&self, days: i64) -> Result<Value, Issue> {
        self.run(move |db| {
            let from = since(days);
            // Tokens are attributed to whatever took part in a turn, so one turn's tokens can be
            // counted under an agent, a skill and an MCP server at once. That is participation,
            // not causation, and the page says so.
            let totals: (i64, i64, i64, i64, i64, i64, f64) = db.query_row(
                "SELECT count(*), coalesce(sum(total_tokens), 0), coalesce(sum(input_tokens), 0), coalesce(sum(output_tokens), 0),
                    coalesce(sum(cached_tokens), 0), coalesce(sum(reasoning_tokens), 0), coalesce(sum(seconds), 0.0)
                 FROM usage_turns WHERE at >= ?1", [from], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)))
                .map_err(Issue::storage)?;
            let conversations: i64 = db.query_row("SELECT count(DISTINCT thread_id) FROM usage_turns WHERE at >= ?1", [from], |row| row.get(0))
                .map_err(Issue::storage)?;
            let steps: i64 = db.query_row("SELECT count(*) FROM usage_steps WHERE at >= ?1", [from], |row| row.get(0)).map_err(Issue::storage)?;

            // One row per day, including the days nothing happened, so a chart's axis is whole.
            let mut turns_by_day = std::collections::HashMap::new();
            let mut query = db.prepare("SELECT at, total_tokens, input_tokens, output_tokens, cached_tokens FROM usage_turns WHERE at >= ?1")
                .map_err(Issue::storage)?;
            for row in query.query_map([from], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?, row.get::<_, i64>(4)?))).map_err(Issue::storage)? {
                let (at, total, input, output, cached) = row.map_err(Issue::storage)?;
                let entry = turns_by_day.entry(day(at)).or_insert((0, 0, 0, 0, 0));
                *entry = (entry.0 + 1, entry.1 + total, entry.2 + input, entry.3 + output, entry.4 + cached);
            }
            let mut steps_by_day = std::collections::HashMap::new();
            let mut query = db.prepare("SELECT at FROM usage_steps WHERE at >= ?1").map_err(Issue::storage)?;
            for row in query.query_map([from], |row| row.get::<_, i64>(0)).map_err(Issue::storage)? {
                *steps_by_day.entry(day(row.map_err(Issue::storage)?)).or_insert(0) += 1;
            }
            let today = chrono::Local::now().date_naive();
            let daily: Vec<Day> = (0..days).rev().map(|back| {
                let date = (today - chrono::Duration::days(back)).format("%Y-%m-%d").to_string();
                let (turns, tokens, input, output, cached) = turns_by_day.get(&date).copied().unwrap_or_default();
                Day { steps: steps_by_day.get(&date).copied().unwrap_or(0), date, turns, tokens,
                    input_tokens: input, output_tokens: output, cached_tokens: cached }
            }).collect();

            let named = |sql: &str| grouped(db, sql, from, |row| Ok(json!({ "name": row.get::<_, String>(0)?,
                "turns": row.get::<_, i64>(1)?, "tokens": row.get::<_, i64>(2)? })));
            let agents = named("SELECT coalesce(agent, 'No agent'), count(*), coalesce(sum(total_tokens), 0) FROM usage_turns
                WHERE at >= ?1 GROUP BY 1 ORDER BY 3 DESC, 2 DESC LIMIT ?2")?;
            let projects = named("SELECT project, count(*), coalesce(sum(total_tokens), 0) FROM usage_turns
                WHERE at >= ?1 AND project IS NOT NULL GROUP BY 1 ORDER BY 3 DESC, 2 DESC LIMIT ?2")?;
            let providers = named("SELECT provider, count(*), coalesce(sum(total_tokens), 0) FROM usage_turns
                WHERE at >= ?1 GROUP BY 1 ORDER BY 3 DESC LIMIT ?2")?;
            let models = grouped(db, "SELECT coalesce(model, 'Default'), provider, count(*), coalesce(sum(total_tokens), 0) FROM usage_turns
                WHERE at >= ?1 GROUP BY 1, 2 ORDER BY 4 DESC, 3 DESC LIMIT ?2", from,
                |row| Ok(json!({ "name": row.get::<_, String>(0)?, "provider": row.get::<_, String>(1)?,
                    "turns": row.get::<_, i64>(2)?, "tokens": row.get::<_, i64>(3)? })))?;
            let efforts = named("SELECT coalesce(effort, 'default'), count(*), coalesce(sum(total_tokens), 0) FROM usage_turns
                WHERE at >= ?1 GROUP BY 1 ORDER BY 2 DESC LIMIT ?2")?;

            // A step's status: `ok`, `failed` or `refused`, each worth seeing on its own.
            let steps_by_kind = grouped(db, "SELECT kind, count(*), sum(status = 'ok'), sum(status = 'failed'), sum(status = 'refused'),
                cast(coalesce(avg(ms), 0) as integer) FROM usage_steps WHERE at >= ?1 GROUP BY 1 ORDER BY 2 DESC LIMIT ?2", from,
                |row| Ok(json!({ "kind": row.get::<_, String>(0)?, "calls": row.get::<_, i64>(1)?, "ok": row.get::<_, i64>(2)?,
                    "failed": row.get::<_, i64>(3)?, "refused": row.get::<_, i64>(4)?, "averageMs": row.get::<_, i64>(5)? })))?;

            // An MCP step is named `server: tool`, so the server is the part before the colon.
            // A server's tokens are the tokens of the distinct turns it took part in, so two calls
            // in one turn do not count that turn twice.
            let mcp_servers = grouped(db, "WITH mcp AS (
                    SELECT thread_id, turn_id, substr(name, 1, instr(name, ':') - 1) AS server, status FROM usage_steps
                    WHERE at >= ?1 AND kind = 'mcp' AND instr(name, ':') > 0),
                    per_turn AS (SELECT DISTINCT server, thread_id, turn_id FROM mcp)
                SELECT m.server, count(*), sum(m.status <> 'ok'),
                    (SELECT coalesce(sum(t.total_tokens), 0) FROM per_turn p LEFT JOIN usage_turns t
                        ON t.thread_id = p.thread_id AND t.turn_id = p.turn_id WHERE p.server = m.server),
                    (SELECT count(*) FROM per_turn p WHERE p.server = m.server)
                FROM mcp m GROUP BY m.server ORDER BY 2 DESC LIMIT ?2", from,
                |row| Ok(json!({ "name": row.get::<_, String>(0)?, "calls": row.get::<_, i64>(1)?, "failures": row.get::<_, i64>(2)?,
                    "tokens": row.get::<_, i64>(3)?, "turns": row.get::<_, i64>(4)? })))?;
            let mcp_tools = grouped(db, "SELECT name, count(*), sum(status <> 'ok') FROM usage_steps
                WHERE at >= ?1 AND kind = 'mcp' AND instr(name, ':') > 0 GROUP BY 1 ORDER BY 2 DESC LIMIT ?2", from,
                |row| Ok(json!({ "name": row.get::<_, String>(0)?, "calls": row.get::<_, i64>(1)?, "failures": row.get::<_, i64>(2)? })))?;

            let skills = grouped(db, "SELECT skills.name, skills.source, count(*), coalesce(sum(turns.total_tokens), 0)
                FROM usage_skills skills LEFT JOIN usage_turns turns ON turns.thread_id = skills.thread_id AND turns.turn_id = skills.turn_id
                WHERE skills.at >= ?1 GROUP BY 1, 2 ORDER BY 3 DESC, 1 LIMIT ?2", from,
                |row| Ok(json!({ "name": row.get::<_, String>(0)?, "source": row.get::<_, String>(1)?,
                    "invocations": row.get::<_, i64>(2)?, "tokens": row.get::<_, i64>(3)? })))?;

            Ok(json!({
                "days": days, "from": from,
                "totals": { "turns": totals.0, "tokens": totals.1, "inputTokens": totals.2, "outputTokens": totals.3,
                    "cachedTokens": totals.4, "reasoningTokens": totals.5, "seconds": totals.6, "steps": steps,
                    "conversations": conversations },
                "daily": daily, "agents": agents, "projects": projects, "providers": providers, "models": models,
                "efforts": efforts, "steps": steps_by_kind, "mcpServers": mcp_servers, "mcpTools": mcp_tools, "skills": skills,
            }))
        }).await
    }
}

/// How many recent events each agent's tile lists.
const RECENT: usize = 3;

/// One agent's figures while they are being gathered. The main agent is the empty name.
#[derive(Default)]
struct AgentFigures {
    id: Option<String>,
    turns: i64, tokens: i64, input: i64, output: i64, seconds: f64, average_seconds: f64,
    completed: i64, interrupted: i64, failed_turns: i64, last_turn: Option<(i64, String)>,
    runs: i64, done: i64, failed_runs: i64, cancelled: i64, declined: i64, running: i64,
    average_run_ms: Option<f64>, running_since: Option<i64>, last_run: Option<(i64, String)>,
    recent: Vec<(i64, Value)>,
}

/// Reads each agent's figures since `from`: its responses (turns) and the tasks delegated to it,
/// whether it is running now and since when, and what it did last. `now` is the reading's time.
fn agent_report(db: &Connection, from: i64, now: i64) -> Result<Vec<Value>, Issue> {
    use std::collections::BTreeMap;
    let mut agents: BTreeMap<String, AgentFigures> = BTreeMap::new();

    // Responses, with the status the provider ended them with: `completed`, `interrupted` or `failed`.
    let mut query = db.prepare("SELECT coalesce(agent, ''), count(*), coalesce(sum(total_tokens), 0),
            coalesce(sum(input_tokens), 0), coalesce(sum(output_tokens), 0), coalesce(sum(seconds), 0.0),
            coalesce(avg(CASE WHEN status = 'completed' THEN seconds END), 0.0),
            sum(status = 'completed'), sum(status = 'interrupted'), sum(status = 'failed')
        FROM usage_turns WHERE at >= ?1 GROUP BY 1").map_err(Issue::storage)?;
    let rows = query.query_map([from], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?,
        row.get::<_, i64>(3)?, row.get::<_, i64>(4)?, row.get::<_, f64>(5)?, row.get::<_, f64>(6)?,
        row.get::<_, i64>(7)?, row.get::<_, i64>(8)?, row.get::<_, i64>(9)?))).map_err(Issue::storage)?;
    for row in rows {
        let (name, turns, tokens, input, output, seconds, average, completed, interrupted, failed) = row.map_err(Issue::storage)?;
        let agent = agents.entry(name).or_default();
        (agent.turns, agent.tokens, agent.input, agent.output, agent.seconds, agent.average_seconds) = (turns, tokens, input, output, seconds, average);
        (agent.completed, agent.interrupted, agent.failed_turns) = (completed, interrupted, failed);
    }

    // Delegated tasks. A run still going is counted however long ago it started.
    let mut query = db.prepare("SELECT agent, max(agent_id), count(*), sum(status = 'done'), sum(status = 'failed'),
            sum(status = 'cancelled'), sum(status = 'declined'), sum(status IN ('asking', 'running')),
            avg(CASE WHEN status = 'done' AND ended IS NOT NULL THEN ended - started END),
            min(CASE WHEN status IN ('asking', 'running') THEN started END)
        FROM delegated_runs WHERE started >= ?1 OR status IN ('asking', 'running') GROUP BY 1").map_err(Issue::storage)?;
    let rows = query.query_map([from], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, i64>(2)?,
        row.get::<_, i64>(3)?, row.get::<_, i64>(4)?, row.get::<_, i64>(5)?, row.get::<_, i64>(6)?, row.get::<_, i64>(7)?,
        row.get::<_, Option<f64>>(8)?, row.get::<_, Option<i64>>(9)?))).map_err(Issue::storage)?;
    for row in rows {
        let (name, id, runs, done, failed, cancelled, declined, running, average, since) = row.map_err(Issue::storage)?;
        let agent = agents.entry(name).or_default();
        agent.id = id;
        (agent.runs, agent.done, agent.failed_runs, agent.cancelled, agent.declined, agent.running) = (runs, done, failed, cancelled, declined, running);
        (agent.average_run_ms, agent.running_since) = (average, since);
    }

    // The latest responses and tasks of each agent, for its activity list and its health.
    let mut query = db.prepare("SELECT name, at, status, seconds, total_tokens FROM (
            SELECT coalesce(agent, '') AS name, at, status, seconds, total_tokens,
                row_number() OVER (PARTITION BY coalesce(agent, '') ORDER BY at DESC) AS n
            FROM usage_turns WHERE at >= ?1) WHERE n <= ?2").map_err(Issue::storage)?;
    let rows = query.query_map(params![from, RECENT as i64], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?,
        row.get::<_, String>(2)?, row.get::<_, f64>(3)?, row.get::<_, i64>(4)?))).map_err(Issue::storage)?;
    for row in rows {
        let (name, at, status, seconds, tokens) = row.map_err(Issue::storage)?;
        let agent = agents.entry(name).or_default();
        if agent.last_turn.as_ref().is_none_or(|(last, _)| at > *last) { agent.last_turn = Some((at, status.clone())); }
        agent.recent.push((at, json!({ "kind": "response", "at": at, "status": status, "seconds": seconds, "tokens": tokens })));
    }
    // A run's line is its one-line summary, which its card already shows; an older run without one
    // shows the start of its task.
    let mut query = db.prepare("SELECT name, started, ended, status, label FROM (
            SELECT agent AS name, started, ended, status,
                CASE WHEN summary <> '' THEN summary ELSE substr(task, 1, 120) END AS label,
                row_number() OVER (PARTITION BY agent ORDER BY started DESC) AS n
            FROM delegated_runs WHERE started >= ?1 OR status IN ('asking', 'running')) WHERE n <= ?2").map_err(Issue::storage)?;
    let rows = query.query_map(params![from, RECENT as i64], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?,
        row.get::<_, Option<i64>>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?))).map_err(Issue::storage)?;
    for row in rows {
        let (name, started, ended, status, label) = row.map_err(Issue::storage)?;
        let agent = agents.entry(name).or_default();
        let at = ended.unwrap_or(started);
        if agent.last_run.as_ref().is_none_or(|(last, _)| at > *last) { agent.last_run = Some((at, status.clone())); }
        agent.recent.push((at, json!({ "kind": "task", "at": at, "started": started, "ended": ended, "status": status, "summary": label })));
    }

    // Delegations in progress, both ways round: the agent whose conversation asked (its name pinned in
    // `agent_chats`; the main agent's conversations have no row) and the agent doing the work.
    let mut delegating: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut working_for: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut query = db.prepare("SELECT coalesce(json_extract(chats.definition, '$.name'), ''), runs.agent, runs.status,
            runs.started, runs.mode, CASE WHEN runs.summary <> '' THEN runs.summary ELSE substr(runs.task, 1, 120) END
        FROM delegated_runs runs LEFT JOIN agent_chats chats ON chats.thread_id = runs.parent
        WHERE runs.status IN ('asking', 'running') ORDER BY runs.started").map_err(Issue::storage)?;
    let rows = query.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?,
        row.get::<_, i64>(3)?, row.get::<_, String>(4)?, row.get::<_, String>(5)?))).map_err(Issue::storage)?;
    for row in rows {
        let (from, to, status, started, mode, summary) = row.map_err(Issue::storage)?;
        agents.entry(from.clone()).or_default();
        agents.entry(to.clone()).or_default();
        delegating.entry(from.clone()).or_default().push(json!({ "agent": to, "status": status, "started": started,
            "mode": mode, "summary": summary }));
        let names = working_for.entry(to).or_default();
        if !names.contains(&from) { names.push(from); }
    }

    let all_turns: i64 = agents.values().map(|agent| agent.turns).sum();
    let all_runs: i64 = agents.values().map(|agent| agent.runs).sum();
    Ok(agents.into_iter().map(|(name, mut agent)| {
        // Running beats everything; otherwise the agent is as healthy as the last thing it did.
        let last = match (&agent.last_turn, &agent.last_run) {
            (Some(turn), Some(run)) => Some(if run.0 >= turn.0 { run.1.as_str() } else { turn.1.as_str() }),
            (Some(turn), None) => Some(turn.1.as_str()),
            (None, Some(run)) => Some(run.1.as_str()),
            (None, None) => None,
        };
        let state = if agent.running > 0 { "running" } else { match last {
            Some("failed") => "failed",
            Some("interrupted" | "cancelled" | "declined") => "stopped",
            Some(_) => "healthy",
            None => "idle",
        } };
        let delegating_to = delegating.remove(&name).unwrap_or_default();
        let works_for = working_for.remove(&name).unwrap_or_default();
        agent.recent.sort_by(|a, b| b.0.cmp(&a.0));
        agent.recent.truncate(RECENT);
        let last_active = agent.recent.first().map(|(at, _)| *at);
        let share = |part: i64, whole: i64| if whole > 0 { part as f64 / whole as f64 } else { 0.0 };
        json!({
            "name": name, "main": name.is_empty(), "id": agent.id, "state": state, "lastActive": last_active,
            "runningSince": agent.running_since,
            "runningMs": agent.running_since.map(|since| (now - since).max(0)),
            "responses": { "count": agent.turns, "completed": agent.completed, "interrupted": agent.interrupted,
                "failed": agent.failed_turns, "averageSeconds": agent.average_seconds, "busySeconds": agent.seconds,
                "share": share(agent.turns, all_turns) },
            "tokens": { "total": agent.tokens, "input": agent.input, "output": agent.output,
                "perResponse": if agent.turns > 0 { agent.tokens / agent.turns } else { 0 } },
            "tasks": { "count": agent.runs, "done": agent.done, "failed": agent.failed_runs, "cancelled": agent.cancelled,
                "declined": agent.declined, "running": agent.running,
                "averageSeconds": agent.average_run_ms.map(|ms| ms / 1000.0), "share": share(agent.runs, all_runs) },
            "recent": agent.recent.into_iter().map(|(_, event)| event).collect::<Vec<_>>(),
            "delegatingTo": delegating_to, "workingFor": works_for,
        })
    }).collect())
}

/// How many blocks the timeline splits a range into: an hour each for a day, a day each for a month.
fn blocks_for(days: i64) -> i64 {
    match days { 1 => 24, 2..=7 => 42, 8..=30 => 30, 31..=90 => 45, _ => 52 }
}

/// One block of one agent's timeline: working time that finished, was stopped, or failed, in ms, and how
/// many failures (a failed response, or a delegated task that failed or was cancelled) ended in it.
#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Block { ok: i64, stopped: i64, failed: i64, failures: i64 }

/// Each agent's working time across the range in equal blocks, so stops and failures show where they
/// happened, and the part of its working time that did not finish (its downtime).
///
/// Time comes from responses: a response that ran from `at - seconds` to `at` adds its time to every block
/// it overlaps. A delegated task's own work is its agent's responses, so its time is not counted twice; a
/// failed or cancelled task only marks the block it ended in.
fn timeline(db: &Connection, from: i64, now: i64, days: i64) -> Result<Value, Issue> {
    use std::collections::BTreeMap;
    let count = blocks_for(days);
    let width = ((now - from) / count).max(1);
    let mut agents: BTreeMap<String, Vec<Block>> = BTreeMap::new();
    let index = |at: i64| ((at - from) / width).clamp(0, count - 1) as usize;

    let mut query = db.prepare("SELECT coalesce(agent, ''), at, seconds, status FROM usage_turns WHERE at >= ?1")
        .map_err(Issue::storage)?;
    let rows = query.query_map([from], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?,
        row.get::<_, f64>(2)?, row.get::<_, String>(3)?))).map_err(Issue::storage)?;
    for row in rows {
        let (name, end, seconds, status) = row.map_err(Issue::storage)?;
        let blocks = agents.entry(name).or_insert_with(|| vec![Block::default(); count as usize]);
        let start = (end - (seconds * 1000.0) as i64).max(from);
        for slot in index(start)..=index(end) {
            let (block_start, block_end) = (from + slot as i64 * width, from + (slot as i64 + 1) * width);
            let part = (end.min(block_end) - start.max(block_start)).max(0);
            let block = &mut blocks[slot];
            match status.as_str() {
                "completed" => block.ok += part,
                "failed" => block.failed += part,
                _ => block.stopped += part,
            }
        }
        if status == "failed" { blocks[index(end)].failures += 1; }
    }
    let mut query = db.prepare("SELECT agent, ended FROM delegated_runs
        WHERE status IN ('failed', 'cancelled') AND ended IS NOT NULL AND ended >= ?1").map_err(Issue::storage)?;
    let rows = query.query_map([from], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))).map_err(Issue::storage)?;
    for row in rows {
        let (name, ended) = row.map_err(Issue::storage)?;
        agents.entry(name).or_insert_with(|| vec![Block::default(); count as usize])[index(ended)].failures += 1;
    }

    let rows: Vec<Value> = agents.into_iter().map(|(name, blocks)| {
        let (ok, stopped, failed) = blocks.iter().fold((0, 0, 0), |sum, block| (sum.0 + block.ok, sum.1 + block.stopped, sum.2 + block.failed));
        let total = ok + stopped + failed;
        json!({ "name": name, "okMs": ok, "stoppedMs": stopped, "failedMs": failed,
            "downtime": if total > 0 { (stopped + failed) as f64 / total as f64 } else { 0.0 },
            "failures": blocks.iter().map(|block| block.failures).sum::<i64>(), "blocks": blocks })
    }).collect();
    Ok(json!({ "from": from, "to": now, "width": width, "count": count, "agents": rows }))
}

impl Store {
    /// Returns each agent's figures for the Agents page: averages, tokens, health and recent activity.
    pub async fn agent_analytics(&self, days: i64) -> Result<Value, Issue> {
        self.run(move |db| {
            let (from, now) = (since(days), chrono::Utc::now().timestamp_millis());
            Ok(json!({ "days": days, "from": from, "now": now, "agents": agent_report(db, from, now)?,
                "timeline": timeline(db, from, now, days)? }))
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        crate::delegations::migrate(&db).unwrap();
        // The part of the agent store the report reads: which agent a conversation belongs to.
        db.execute_batch("CREATE TABLE agent_chats (thread_id TEXT PRIMARY KEY, agent_id TEXT NOT NULL, definition TEXT NOT NULL);")
            .unwrap();
        db
    }

    #[test]
    fn the_timeline_spreads_time_over_blocks_and_measures_downtime() {
        let db = database();
        // A day split into 24 one-hour blocks, starting at 0.
        let hour = 3_600_000;
        // 30 minutes that finished, ending half way through the second hour...
        turn(&db, "1", hour + hour / 2, None, "completed", 1_800.0, 10);
        // ...and 2 hours that failed, ending at the start of the sixth hour: it covers the fourth and fifth.
        turn(&db, "2", 5 * hour, None, "failed", 7_200.0, 10);
        // 30 minutes stopped, inside the eleventh hour.
        turn(&db, "3", 10 * hour + hour / 2, None, "interrupted", 1_800.0, 10);
        run(&db, "r1", "Tester", "cancelled", 0, Some(2 * hour + 1));
        let line = timeline(&db, 0, 24 * hour, 1).unwrap();
        assert_eq!(line["count"], 24);
        let main = line["agents"].as_array().unwrap().iter().find(|row| row["name"] == "").unwrap();
        assert_eq!(main["blocks"][1]["ok"], hour / 2);
        assert_eq!(main["blocks"][3]["failed"], hour);
        assert_eq!(main["blocks"][4]["failed"], hour);
        assert_eq!(main["blocks"][4]["failures"], 0, "a failure is marked where it ended");
        assert_eq!(main["blocks"][5]["failures"], 1);
        assert_eq!(main["blocks"][10]["stopped"], hour / 2);
        // 2.5 hours of 3 did not finish.
        assert!((main["downtime"].as_f64().unwrap() - 2.5 / 3.0).abs() < 1e-9);
        let tester = line["agents"].as_array().unwrap().iter().find(|row| row["name"] == "Tester").unwrap();
        assert_eq!(tester["blocks"][2]["failures"], 1);
        assert_eq!(tester["downtime"], 0.0);
    }

    #[test]
    fn a_delegation_in_progress_tags_both_agents() {
        let db = database();
        // The Writer's conversation `w` hands work to the Builder; the main agent's `p` hands work to the Tester.
        db.execute("INSERT INTO agent_chats VALUES ('w', 'a-2', '{\"name\":\"Writer\"}')", []).unwrap();
        turn(&db, "1", 1_000, Some("Writer"), "interrupted", 2.0, 10);
        db.execute("INSERT INTO delegated_runs (id, parent, turn_id, call_id, agent_id, agent, task, status, started, summary)
            VALUES ('r1', 'w', 'u', 'c1', 'a-1', 'Builder', 'Build it', 'running', 5000, 'Build the page')", []).unwrap();
        run(&db, "r2", "Tester", "running", 6_000, None);
        run(&db, "r3", "Tester", "done", 1_000, Some(2_000));
        let agents = agent_report(&db, 0, 10_000).unwrap();
        let find = |name: &str| agents.iter().find(|agent| agent["name"] == name).unwrap().clone();

        let writer = find("Writer");
        // Stopped, and delegating as well: the two tags are independent.
        assert_eq!(writer["state"], "stopped");
        assert_eq!(writer["delegatingTo"][0]["agent"], "Builder");
        assert_eq!(writer["delegatingTo"][0]["summary"], "Build the page");
        assert_eq!(find("Builder")["workingFor"], json!(["Writer"]));
        assert_eq!(find("")["delegatingTo"][0]["agent"], "Tester");
        assert_eq!(find("Tester")["workingFor"], json!([""]));
        // A finished run is no longer a delegation in progress.
        assert_eq!(find("Tester")["delegatingTo"], json!([]));
    }

    fn turn(db: &Connection, id: &str, at: i64, agent: Option<&str>, status: &str, seconds: f64, tokens: i64) {
        db.execute("INSERT INTO usage_turns (thread_id, turn_id, at, provider, agent, status, seconds, total_tokens)
            VALUES ('t', ?1, ?2, 'codex', ?3, ?4, ?5, ?6)", params![id, at, agent, status, seconds, tokens]).unwrap();
    }

    fn run(db: &Connection, id: &str, agent: &str, status: &str, started: i64, ended: Option<i64>) {
        db.execute("INSERT INTO delegated_runs (id, parent, turn_id, call_id, agent_id, agent, task, status, started, ended, summary)
            VALUES (?1, 'p', 'u', ?1, 'a-1', ?2, 'Write the tests', ?3, ?4, ?5, 'Tests')",
            params![id, agent, status, started, ended]).unwrap();
    }

    #[test]
    fn each_agent_gets_its_averages_share_and_health() {
        let db = database();
        turn(&db, "1", 1_000, None, "completed", 4.0, 100);
        turn(&db, "2", 2_000, None, "completed", 6.0, 300);
        turn(&db, "3", 3_000, Some("Tester"), "completed", 10.0, 600);
        turn(&db, "4", 4_000, Some("Tester"), "interrupted", 1.0, 0);
        run(&db, "r1", "Tester", "done", 1_000, Some(61_000));
        let agents = agent_report(&db, 0, 100_000).unwrap();
        let main = agents.iter().find(|agent| agent["main"] == true).unwrap();
        let tester = agents.iter().find(|agent| agent["name"] == "Tester").unwrap();

        assert_eq!(main["responses"]["averageSeconds"], 5.0);
        assert_eq!(main["tokens"]["total"], 400);
        assert_eq!(main["responses"]["share"], 0.5);
        assert_eq!(main["state"], "healthy");
        // The interrupted response is not part of the average, and the finished task ended last.
        assert_eq!(tester["responses"]["averageSeconds"], 10.0);
        assert_eq!(tester["tasks"]["averageSeconds"], 60.0);
        assert_eq!(tester["tasks"]["share"], 1.0);
        assert_eq!(tester["state"], "healthy");
        assert_eq!(tester["recent"][0]["kind"], "task");
    }

    #[test]
    fn a_running_task_wins_and_a_stop_shows() {
        let db = database();
        turn(&db, "1", 5_000, Some("Writer"), "interrupted", 2.0, 10);
        turn(&db, "2", 5_000, Some("Builder"), "completed", 2.0, 10);
        run(&db, "r1", "Builder", "running", 8_000, None);
        let agents = agent_report(&db, 0, 20_000).unwrap();
        let writer = agents.iter().find(|agent| agent["name"] == "Writer").unwrap();
        let builder = agents.iter().find(|agent| agent["name"] == "Builder").unwrap();
        assert_eq!(writer["state"], "stopped");
        assert_eq!(builder["state"], "running");
        assert_eq!(builder["runningMs"], 12_000);
    }

    #[test]
    fn a_range_starts_at_a_days_boundary_and_covers_it() {
        let one = since(1);
        assert_eq!(day(one), chrono::Local::now().format("%Y-%m-%d").to_string());
        assert!(since(30) < one, "a longer range starts earlier");
        assert_eq!((one - since(30)) / 86_400_000, 29, "29 whole days between the two starts");
    }
}
