//! Agent preferences copied into new conversations, separate from permission allowances.

// Reuses the agent's private metadata database.
use crate::agents::{Issue, Store};
// Creates the table and binds saved values.
use rusqlite::{Connection, params};
// Keeps capability preferences in deterministic order.
use std::collections::BTreeMap;
// Identifies named agents and the main agent.
use uuid::Uuid;

/// Creates storage for explicit on and off defaults; it grants no access itself.
pub fn migrate(db: &Connection) -> Result<(), Issue> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS capability_defaults (
        agent TEXT NOT NULL, capability TEXT NOT NULL, enabled INTEGER NOT NULL,
        PRIMARY KEY (agent, capability)
    );").map_err(Issue::storage)
}

impl Store {
    /// Reads this agent's preferences, without inheriting the main agent's values.
    pub async fn capability_defaults(&self, agent: Option<Uuid>) -> Result<BTreeMap<String, bool>, Issue> {
        let id = agent.unwrap_or(crate::main_agent::ID).to_string();
        self.run(move |db| {
            let mut query = db.prepare("SELECT capability, enabled FROM capability_defaults WHERE agent = ?1")
                .map_err(Issue::storage)?;
            query.query_map([id], |row| Ok((row.get(0)?, row.get(1)?))).map_err(Issue::storage)?
                .collect::<Result<_, _>>().map_err(Issue::storage)
        }).await
    }

    /// Saves one validated preference without touching existing conversations or sessions.
    pub async fn save_capability_default(&self, agent: Uuid, key: String, on: bool) -> Result<(), Issue> {
        self.run(move |db| {
            db.execute("INSERT INTO capability_defaults (agent, capability, enabled) VALUES (?1, ?2, ?3)
                ON CONFLICT(agent, capability) DO UPDATE SET enabled = excluded.enabled",
                params![agent.to_string(), key, on]).map_err(Issue::storage)?;
            Ok(())
        }).await
    }
}
