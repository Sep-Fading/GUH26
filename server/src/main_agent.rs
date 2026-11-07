//! The main agent: the configuration of conversations that have no agent. Its capabilities are
//! permission rules under the agent scope with the nil id, limited to switches and MCP servers.
//! Its default model and effort, its skills, its skill groups and its delegation limits are kept
//! here. A named agent never inherits any of it.

// Reuses the private metadata database and an agent's delegation limits.
use crate::agents::{Delegation, Issue, Store};
// Reads the one row that may not exist yet.
use rusqlite::{Connection, OptionalExtension, params};
// Reads the page's request and writes its answer.
use serde::{Deserialize, Serialize};
// The id that stands for the main agent in permission rules.
use uuid::Uuid;

/// The main agent's id in permission rules. No saved agent can have it.
pub const ID: Uuid = Uuid::nil();

/// The most skills and groups the main agent holds: the limits of an agent and of a conversation.
const MOST_SKILLS: usize = 8;
const MOST_GROUPS: usize = 16;

/// What the page sends. A list left out keeps what is saved, so saving the model alone does
/// not empty the skills.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Change {
    pub model: Option<String>,
    pub effort: Option<String>,
    #[serde(default)] pub speed: Option<String>,
    #[serde(default)] pub skill_ids: Option<Vec<Uuid>>,
    #[serde(default)] pub skill_group_ids: Option<Vec<Uuid>>,
    #[serde(default)] pub delegation: Option<Delegation>,
}

/// The main agent's saved configuration besides its capabilities: what a new conversation
/// without an agent starts with, and the skills and groups every such conversation carries.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Defaults {
    pub model: Option<String>,
    pub effort: Option<String>,
    /// A speed tier of the model; none means standard.
    pub speed: Option<String>,
    pub skill_ids: Vec<Uuid>,
    pub skill_group_ids: Vec<Uuid>,
    /// How many delegations one message may lead to and how long a run may take. Nothing saved
    /// means the defaults every install had before they were editable: 3 and 10 minutes.
    pub delegation: Delegation,
}

impl Change {
    /// Accepts bounded names and lists without repeats; an effort or a speed belongs to a model, so it needs one.
    pub fn valid(&self) -> bool {
        let name = |text: &String, most: usize| !text.is_empty() && text.len() <= most && !text.chars().any(char::is_control);
        let distinct = |ids: &Vec<Uuid>, most: usize| ids.len() <= most
            && ids.iter().collect::<std::collections::HashSet<_>>().len() == ids.len();
        self.model.as_ref().is_none_or(|model| name(model, 128))
            && self.effort.as_ref().is_none_or(|effort| name(effort, 32) && self.model.is_some())
            && self.speed.as_ref().is_none_or(|speed| name(speed, 32) && self.model.is_some())
            && self.skill_ids.as_ref().is_none_or(|ids| distinct(ids, MOST_SKILLS))
            && self.skill_group_ids.as_ref().is_none_or(|ids| distinct(ids, MOST_GROUPS))
            && self.delegation.as_ref().is_none_or(Delegation::valid)
    }
}

/// Creates the one-row table inside the existing private metadata database.
pub fn migrate(db: &Connection) -> Result<(), Issue> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS main_agent (
        id INTEGER PRIMARY KEY CHECK (id = 1), model TEXT, effort TEXT, speed TEXT
    );").map_err(Issue::storage)?;
    // The lists, the speed and the delegation limits arrived after the first release of this table.
    for (column, shape) in [("skill_ids", "TEXT NOT NULL DEFAULT '[]'"), ("skill_group_ids", "TEXT NOT NULL DEFAULT '[]'"), ("speed", "TEXT"),
        ("delegation", "TEXT")] {
        let known: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('main_agent') WHERE name = ?1)",
            [column], |row| row.get(0)).map_err(Issue::storage)?;
        if !known {
            db.execute_batch(&format!("ALTER TABLE main_agent ADD COLUMN {column} {shape};")).map_err(Issue::storage)?;
        }
    }
    Ok(())
}

/// Reads the saved row; nothing saved means no model, empty lists and the default limits.
pub fn read(db: &Connection) -> Result<Defaults, Issue> {
    let row: Option<(Option<String>, Option<String>, Option<String>, String, String, Option<String>)> = db.query_row(
        "SELECT model, effort, speed, skill_ids, skill_group_ids, delegation FROM main_agent WHERE id = 1", [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))).optional().map_err(Issue::storage)?;
    let Some((model, effort, speed, skills, groups, delegation)) = row else { return Ok(Defaults::default()) };
    let delegation = match delegation {
        Some(text) => serde_json::from_str::<Delegation>(&text).map_err(Issue::storage)?,
        None => Delegation::default(),
    };
    // Limits that are out of bounds are never used, as with a named agent's stored definition.
    if !delegation.valid() { return Err(Issue::Storage); }
    Ok(Defaults { model, effort, speed, skill_ids: serde_json::from_str(&skills).map_err(Issue::storage)?,
        skill_group_ids: serde_json::from_str(&groups).map_err(Issue::storage)?, delegation })
}

impl Store {
    /// Returns the main agent's defaults, skills and groups.
    pub async fn main_agent(&self) -> Result<Defaults, Issue> {
        self.run(|db| read(db)).await
    }

    /// Replaces the model and effort, and each list and the limits when sent. A skill or group that
    /// does not exist is refused; one deleted later simply stops contributing.
    pub async fn save_main_agent(&self, change: Change) -> Result<Defaults, Issue> {
        if !change.valid() { return Err(Issue::Invalid); }
        self.run(move |db| {
            let before = read(db)?;
            let skills = change.skill_ids.unwrap_or(before.skill_ids);
            let groups = change.skill_group_ids.unwrap_or(before.skill_group_ids);
            crate::skills::resolve(db, &skills, false)?;
            crate::skill_groups::check_refs(db, &groups)?;
            // Limits left out stay as saved; limits never saved stay unstored, so they remain the defaults.
            let limits = change.delegation.map(|limits| serde_json::to_string(&limits)).transpose().map_err(Issue::storage)?;
            let saved = Defaults { model: change.model, effort: change.effort, speed: change.speed, skill_ids: skills, skill_group_ids: groups,
                delegation: change.delegation.unwrap_or(before.delegation) };
            db.execute("INSERT INTO main_agent (id, model, effort, speed, skill_ids, skill_group_ids, delegation) VALUES (1, ?1, ?2, ?5, ?3, ?4, ?6)
                ON CONFLICT(id) DO UPDATE SET model = excluded.model, effort = excluded.effort, speed = excluded.speed,
                skill_ids = excluded.skill_ids, skill_group_ids = excluded.skill_group_ids,
                delegation = COALESCE(excluded.delegation, main_agent.delegation)",
                params![saved.model, saved.effort, serde_json::to_string(&saved.skill_ids).map_err(Issue::storage)?,
                    serde_json::to_string(&saved.skill_group_ids).map_err(Issue::storage)?, saved.speed, limits]).map_err(Issue::storage)?;
            Ok(saved)
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::Delegation;

    // Requirement 2026-10-11 (user decision): the main agent's delegation limits are editable.
    #[test]
    fn an_older_table_gains_default_delegation_limits() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE main_agent (id INTEGER PRIMARY KEY CHECK (id = 1), model TEXT, effort TEXT, speed TEXT,
            skill_ids TEXT NOT NULL DEFAULT '[]', skill_group_ids TEXT NOT NULL DEFAULT '[]');
            INSERT INTO main_agent (id, model) VALUES (1, 'fixture-model');").unwrap();
        migrate(&db).unwrap();
        let read = read(&db).unwrap();
        assert_eq!((read.model.as_deref(), read.delegation), (Some("fixture-model"), Delegation { max_per_turn: 3, minutes: 10 }));
        let fresh = Connection::open_in_memory().unwrap();
        migrate(&fresh).unwrap();
        assert_eq!(super::read(&fresh).unwrap().delegation, Delegation::default());
    }

    #[test]
    fn a_change_checks_its_delegation_limits() {
        let change = |max_per_turn, minutes| Change { delegation: Some(Delegation { max_per_turn, minutes }), ..Change::default() };
        assert!(change(1, 1).valid() && change(10, 1440).valid() && Change::default().valid());
        assert!(!change(0, 10).valid() && !change(11, 10).valid() && !change(3, 0).valid() && !change(3, 1441).valid());
    }
}
