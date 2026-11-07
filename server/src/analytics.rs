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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_starts_at_a_days_boundary_and_covers_it() {
        let one = since(1);
        assert_eq!(day(one), chrono::Local::now().format("%Y-%m-%d").to_string());
        assert!(since(30) < one, "a longer range starts earlier");
        assert_eq!((one - since(30)) / 86_400_000, 29, "29 whole days between the two starts");
    }
}
