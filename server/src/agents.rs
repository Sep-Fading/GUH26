// Persists profiles and bindings with transactions.
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
// Validates definitions and serializes public profiles.
use serde::{Deserialize, Serialize};
// Adds agent identity to chat summaries.
use serde_json::{Value, json};
// Bounds collections and protects private storage.
use std::{
    collections::HashSet,
    path::Path,
    sync::{Arc, Mutex},
};
// Moves database operations off asynchronous workers.
use tokio::task::spawn_blocking;
// Generates identifiers without using user filenames.
use uuid::Uuid;
// Resolves reusable instructions without granting capabilities.
use crate::skills::{self, Skill};
// Pins each chat to one verified execution worker.
use crate::environment::Assignment;

mod identity;
mod creation;
pub mod quarantine;

#[derive(Clone, Copy, Debug)]
pub enum Issue {
    Invalid,
    Unsupported,
    Inactive,
    SkillInactive,
    ProjectAccess,
    Conflict,
    Missing,
    /// Another entry already has this name.
    NameTaken,
    /// The conversation's agent or project does not allow this capability.
    NotAllowed,
    Storage,
    Limit,
    CreationUncertain,
    CreationLimit,
    EnvironmentLocked,
    ConversationLocked,
    VmUnavailable,
}

impl Issue {
    /// Records a storage error category without logging SQL, paths or saved content.
    pub(crate) fn storage(error: impl std::error::Error + 'static) -> Self {
        let error = &error as &dyn std::error::Error;
        if let Some(error) = error.downcast_ref::<rusqlite::Error>() {
            let code = error.sqlite_error().map(|error| error.extended_code);
            eprintln!("storage_failure kind=sqlite code={}", code.unwrap_or(0));
        } else if error.is::<serde_json::Error>() {
            eprintln!("storage_failure kind=json");
        } else if let Some(error) = error.downcast_ref::<std::io::Error>() {
            eprintln!("storage_failure kind=io code={}", error.raw_os_error().unwrap_or(0));
        } else if error.is::<tokio::task::JoinError>() {
            eprintln!("storage_failure kind=task");
        } else {
            eprintln!("storage_failure kind=record");
        }
        Self::Storage
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    ComputerUse,
    DeepResearch,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Observe,
    Control,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Application {
    pub id: String,
    pub actions: Vec<Action>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileAccess {
    pub path: String,
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub write: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Access {
    pub applications: Vec<Application>,
    pub files: Vec<FileAccess>,
    pub network: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Definition {
    pub name: String,
    pub instructions: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// A speed tier of the model, such as Codex's Fast; none means standard.
    #[serde(default)]
    pub speed: Option<String>,
    pub revision: Option<u64>,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    #[serde(default)]
    pub access: Access,
    #[serde(default)]
    pub delegates_to: Vec<Uuid>,
    /// Bounds on what this agent may delegate; the user sets them per agent.
    #[serde(default)]
    pub delegation: Delegation,
    #[serde(default)]
    pub skill_ids: Vec<Uuid>,
    #[serde(default)]
    pub skill_group_ids: Vec<Uuid>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Delegation {
    /// How many delegations one of the user's messages may lead to.
    pub max_per_turn: u8,
    /// How long one delegated run may take.
    pub minutes: u16,
}

impl Default for Delegation {
    fn default() -> Self { Self { max_per_turn: 3, minutes: 10 } }
}

impl Delegation {
    /// The longest time limit, a day. There is no "unlimited": a run that hangs, or waits for an
    /// approval nobody gives, must still end and free its machine (user decision, 2026-10-11).
    pub const MOST_MINUTES: u16 = 1440;
    /// The longest a parent's turn waits for a run. The waiting tool call stays open that long, and
    /// MiniClaw's Codex and Claude bridges end any tool call after 65 minutes (`rpc.rs`, `claude.rs`).
    /// A run with a longer limit then continues in the background (lead decision, 2026-10-11).
    pub const MOST_WAIT_MINUTES: u16 = 60;

    /// Accepts 1 to 10 delegations per message and 1 to 1440 minutes per run.
    pub fn valid(&self) -> bool {
        (1..=10).contains(&self.max_per_turn) && (1..=Self::MOST_MINUTES).contains(&self.minutes)
    }

    /// Returns how long the parent's turn waits for a run it chose to wait for: the limit, at most an hour.
    pub fn wait_minutes(&self) -> u16 { self.minutes.min(Self::MOST_WAIT_MINUTES) }

    /// Tells whether a waited run still going at the hour continues in the background, because its
    /// own limit is longer. Any run gets its full limit either way.
    pub fn hands_over(&self) -> bool { self.minutes > Self::MOST_WAIT_MINUTES }
}

#[derive(Clone, Debug, Serialize)]
pub struct Agent {
    pub id: Uuid,
    #[serde(flatten)]
    pub definition: Definition,
    #[serde(skip)]
    pub skills: Vec<Skill>,
}

impl Definition {
    /// Validates bounded profile data; unsupported policies can only remain inactive.
    pub fn validate(&self) -> Result<(), Issue> {
        let access = &self.access;
        if self.name.trim().is_empty()
            || self.name.len() > 80
            || self.name.chars().any(char::is_control)
            || self.instructions.trim().is_empty()
            || self.instructions.len() > 16384
            || self.instructions.contains('\0')
            || self.skill_group_ids.len() > 16
            || self.skill_group_ids.iter().collect::<HashSet<_>>().len() != self.skill_group_ids.len()
            || self.skill_ids.len() > 8
            || self.skill_ids.iter().collect::<HashSet<_>>().len() != self.skill_ids.len()
            || self.capabilities.len() > 2
            || self.capabilities.iter().collect::<HashSet<_>>().len() != self.capabilities.len()
            || self.delegates_to.len() > 32
            || self.delegates_to.iter().collect::<HashSet<_>>().len() != self.delegates_to.len()
            || access.applications.len() > 32
            || access.files.len() > 32
            || access.network.len() > 32
        {
            return Err(Issue::Invalid);
        }
        for app in &access.applications {
            if app.id.is_empty()
                || app.id.len() > 255
                || !app.id.contains('.')
                || !app
                    .id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
                || app.actions.is_empty()
                || app.actions.len() > 2
                || app.actions.iter().collect::<HashSet<_>>().len() != app.actions.len()
            {
                return Err(Issue::Invalid);
            }
        }
        for file in &access.files {
            if !Path::new(&file.path).is_absolute()
                || file.path.len() > 4096
                || file.path.contains('\0')
                || (!file.read && !file.write)
            {
                return Err(Issue::Invalid);
            }
        }
        for host in &access.network {
            if host.is_empty()
                || host.len() > 253
                || !host
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b".-".contains(&c))
            {
                return Err(Issue::Invalid);
            }
        }
        if !self.delegation.valid() {
            return Err(Issue::Invalid);
        }
        if self.active && !self.supported() {
            return Err(Issue::Unsupported);
        }
        Ok(())
    }

    /// Returns whether this release can enforce every requested capability and grant.
    pub fn supported(&self) -> bool {
        self.capabilities.is_empty()
            && self.access.applications.is_empty()
            && self.access.files.is_empty()
            && self.access.network.is_empty()
    }
}

impl Agent {
    /// Applies the pinned instructions without enabling tools or changing shared config.
    pub fn configure(&self, params: &mut Value) {
        let mut instructions = self.definition.instructions.clone();
        for skill in &self.skills {
            instructions.push_str(&format!(
                "\n\n## Skill: {}\n\n{}",
                skill.definition.name, skill.definition.instructions
            ));
        }
        params["developerInstructions"] = json!(instructions);
    }

    /// Adds the pinned agent identity and revision to a public chat summary.
    pub fn tag(&self, summary: &mut Value) {
        summary["agentId"] = json!(self.id);
        summary["agentName"] = json!(self.definition.name);
        summary["agentRevision"] = json!(self.definition.revision);
    }
}

#[derive(Clone)]
/// The metadata connection, and on Windows the handles that pin the database's name.
pub struct Store(Arc<Mutex<Connection>>, #[allow(dead_code)] Option<Arc<crate::platform_fs::DatabaseGuard>>);

impl Store {
    /// Opens private metadata storage; database errors fail startup without replacement.
    pub fn open(path: &Path) -> Result<Self, Issue> {
        // Resolve macOS /var aliases while retaining NOFOLLOW on the database itself.
        let path = path
            .parent()
            .ok_or(Issue::Storage)?
            .canonicalize()
            .map_err(Issue::storage)?
            .join(path.file_name().ok_or(Issue::Storage)?);
        let mut marker_name = path.file_name().ok_or(Issue::Storage)?.to_os_string();
        marker_name.push(".identity");
        let marker_path = path.with_file_name(marker_name);
        let marker = identity::read(&marker_path)?;
        if marker.is_some() && !path.try_exists().map_err(Issue::storage)? {
            return Err(Issue::Storage);
        }
        let created = match crate::platform_fs::private_file(&path, true) {
            Ok(file) => { drop(file); true },
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
            Err(error) => return Err(Issue::storage(error)),
        };
        let guard = crate::platform_fs::database_guard(&path).map_err(Issue::storage)?;
        // Windows keeps the name pinned while the store lives. On Unix the check ends before
        // SQLite opens the file: closing any other descriptor on a database drops the
        // process's POSIX locks on it (sqlite.org/howtocorrupt.html, section 2.2).
        #[cfg(windows)]
        let guard = Some(Arc::new(guard));
        #[cfg(unix)]
        let guard = { drop(guard); None };
        let mut connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(Issue::storage)?;
        identity::verify(&connection, marker.as_deref(), created)?;
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
             PRAGMA synchronous = FULL;
             CREATE TABLE IF NOT EXISTS agents (
                 id TEXT PRIMARY KEY, definition TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS agent_chats (
                 thread_id TEXT PRIMARY KEY,
                 agent_id TEXT NOT NULL REFERENCES agents(id),
                 definition TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS skills (
                 id TEXT PRIMARY KEY, definition TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS agent_chat_skills (
                 thread_id TEXT PRIMARY KEY REFERENCES agent_chats(thread_id),
                 snapshots TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS removed (
                 kind TEXT NOT NULL, id TEXT NOT NULL, PRIMARY KEY(kind, id)
             );
             CREATE TABLE IF NOT EXISTS projects (
                 id TEXT PRIMARY KEY, definition TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS skill_groups (
                 id TEXT PRIMARY KEY, definition TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS chat_skill_selections (
                 thread_id TEXT PRIMARY KEY, selection TEXT NOT NULL, snapshots TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS project_chats (
                 thread_id TEXT PRIMARY KEY,
                 project_id TEXT NOT NULL REFERENCES projects(id),
                 skills TEXT NOT NULL
             );",
            )
            .map_err(Issue::storage)?;
        if marker.is_none() {
            identity::seal(&mut connection, &marker_path)?;
        }
        creation::migrate(&mut connection)?;
        crate::permissions::migrate(&mut connection)?;
        crate::notifications::migrate(&connection)?;
        crate::delegations::migrate(&connection)?;
        crate::schedules::migrate(&connection)?;
        crate::mcp_servers::migrate(&connection)?;
        crate::queue::migrate(&connection)?;
        crate::main_agent::migrate(&connection)?;
        crate::capability_defaults::migrate(&connection)?;
        crate::names::migrate(&connection)?;
        crate::speeds::migrate(&connection)?;
        crate::project_memory::migrate(&connection)?;
        crate::issues::migrate(&connection)?;
        crate::project_history::migrate(&connection)?;
        crate::analytics::migrate(&connection)?;
        Ok(Self(Arc::new(Mutex::new(connection)), guard))
    }

    /// Runs a bounded metadata operation without blocking Tokio's event reader.
    pub(crate) async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, Issue> + Send + 'static,
    ) -> Result<T, Issue> {
        let store = self.0.clone();
        spawn_blocking(move || {
            let mut connection = store.lock().map_err(|_| Issue::Storage)?;
            operation(&mut connection)
        })
        .await
        .map_err(Issue::storage)?
    }

    /// Returns the current profiles in creation order, with no Codex or filesystem data.
    pub async fn list(&self) -> Result<Vec<Agent>, Issue> {
        self.run(|db| {
            let mut query = db
                .prepare("SELECT id, definition FROM agents WHERE id NOT IN
                    (SELECT id FROM removed WHERE kind = 'agent') ORDER BY rowid LIMIT 65")
                .map_err(Issue::storage)?;
            let rows = query
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(Issue::storage)?;
            let mut agents = Vec::new();
            for row in rows {
                let (id, text) = row.map_err(Issue::storage)?;
                agents.push(decode(&id, &text)?);
            }
            if agents.len() > 64 {
                return Err(Issue::Limit);
            }
            Ok(agents)
        })
        .await
    }

    /// Returns one defined agent, including inactive profiles needed for management.
    pub async fn get(&self, id: Uuid) -> Result<Agent, Issue> {
        self.run(move |db| read(db, id)).await
    }

    /// Loads active skill revisions for a new chat while admission holds the shared gate.
    pub async fn prepare(&self, id: Uuid) -> Result<Agent, Issue> {
        self.run(move |db| {
            let mut agent = read(db, id)?;
            agent.skills = skills::resolve(db, &agent.definition.skill_ids, true)?;
            Ok(agent)
        })
        .await
    }

    /// Creates a profile or replaces its current revision atomically.
    pub async fn save(&self, id: Option<Uuid>, mut definition: Definition) -> Result<Agent, Issue> {
        definition.validate()?;
        self.run(move |db| {
            let tx = db.transaction().map_err(Issue::storage)?;
            skills::resolve(&tx, &definition.skill_ids, definition.active)?;
            crate::skill_groups::check_refs(&tx, &definition.skill_group_ids)?;
            // A recipient must be a saved agent and never the agent itself.
            for recipient in &definition.delegates_to {
                if Some(*recipient) == id { return Err(Issue::Invalid); }
                read(&tx, *recipient)?;
            }
            let id = if let Some(id) = id {
                let old = read(&tx, id)?;
                if definition.revision != old.definition.revision {
                    return Err(Issue::Conflict);
                }
                definition.revision = Some(
                    definition
                        .revision
                        .and_then(|v| v.checked_add(1))
                        .ok_or(Issue::Invalid)?,
                );
                id
            } else {
                if definition.revision.is_some() {
                    return Err(Issue::Invalid);
                }
                let count: i64 = tx
                    .query_row("SELECT count(*) FROM agents WHERE id NOT IN
                        (SELECT id FROM removed WHERE kind = 'agent')", [], |r| r.get(0))
                    .map_err(Issue::storage)?;
                if count >= 64 {
                    return Err(Issue::Limit);
                }
                definition.revision = Some(1);
                Uuid::new_v4()
            };
            let text = serde_json::to_string(&definition).map_err(Issue::storage)?;
            tx.execute(
                "INSERT INTO agents (id, definition) VALUES (?1, ?2)
                ON CONFLICT(id) DO UPDATE SET definition = excluded.definition",
                params![id.to_string(), text],
            )
            .map_err(Issue::storage)?;
            tx.commit().map_err(Issue::storage)?;
            Ok(Agent {
                id,
                definition,
                skills: Vec::new(),
            })
        })
        .await
    }

    /// Saves provisional bindings and a creation journal before contacting Codex.
    pub async fn reserve(&self, thread: String, environment: Assignment, agent: Option<Agent>,
        project: Option<crate::projects::Context>,
        selection: Option<crate::skill_groups::Selection>, tools: Vec<crate::permissions::Target>,
        baseline: Vec<String>) -> Result<(), Issue> {
        self.run(move |db| {
            let tx = db.transaction().map_err(Issue::storage)?;
            let count: i64 = tx.query_row("SELECT count(*) FROM chat_creations", [], |r| r.get(0))
                .map_err(Issue::storage)?;
            quarantine::read(&tx)?;
            if count >= 64 { return Err(Issue::CreationLimit); }
            if baseline.len() > 4096 { return Err(Issue::Limit); }
            tx.execute("INSERT INTO chat_creations (id, baseline) VALUES (?1, ?2)",
                params![thread, serde_json::to_string(&baseline).map_err(Issue::storage)?])
                .map_err(Issue::storage)?;
            let required = i64::from(agent.is_some()) | (i64::from(project.is_some()) << 1)
                | (i64::from(selection.is_some()) << 2) | 8;
            tx.execute("INSERT INTO chat_admissions (thread_id, required) VALUES (?1, ?2)",
                params![thread, required]).map_err(Issue::storage)?;
            tx.execute("INSERT INTO chat_environments (thread_id, assignment) VALUES (?1, ?2)",
                params![thread, serde_json::to_string(&environment).map_err(Issue::storage)?])
                .map_err(Issue::storage)?;
            for target in tools {
                tx.execute("INSERT INTO permission_rules (scope_kind, scope_id, target, effect)
                    VALUES ('conversation', ?1, ?2, 'allow')", params![thread, target.key()?])
                    .map_err(Issue::storage)?;
            }
            if let Some(project) = project { project.bind(&tx, &thread)?; }
            if let Some(mut selection) = selection {
                crate::skill_groups::write_selection(&tx, &thread, &mut selection)?;
            }
            if let Some(agent) = agent {
            let current = read(&tx, agent.id)?;
            if !current.definition.active {
                return Err(Issue::Inactive);
            }
            if current.definition.revision != agent.definition.revision {
                return Err(Issue::Conflict);
            }
            let current_skills = skills::resolve(&tx, &agent.definition.skill_ids, true)?;
            if current_skills
                .iter()
                .map(|s| (s.id, s.definition.revision))
                .collect::<Vec<_>>()
                != agent
                    .skills
                    .iter()
                    .map(|s| (s.id, s.definition.revision))
                    .collect::<Vec<_>>()
            {
                return Err(Issue::Conflict);
            }
            let count: i64 = tx
                .query_row("SELECT count(*) FROM agent_chats", [], |r| r.get(0))
                .map_err(Issue::storage)?;
            if count >= 4096 {
                return Err(Issue::Limit);
            }
            tx.execute(
                "INSERT INTO agent_chats (thread_id, agent_id, definition)
                VALUES (?1, ?2, ?3)",
                params![
                    thread,
                    agent.id.to_string(),
                    serde_json::to_string(&agent.definition).map_err(Issue::storage)?
                ],
            )
            .map_err(Issue::storage)?;
            tx.execute(
                "INSERT INTO agent_chat_skills (thread_id, snapshots) VALUES (?1, ?2)",
                params![
                    thread,
                    serde_json::to_string(&agent.skills).map_err(Issue::storage)?
                ],
            )
            .map_err(Issue::storage)?;
            }
            tx.commit().map_err(Issue::storage)
        })
        .await
    }

    /// Returns a chat's pinned profile; optionally requires its agent to remain active.
    pub async fn binding(
        &self,
        thread: String,
        require_active: bool,
    ) -> Result<Option<Agent>, Issue> {
        self.check_creation(thread.clone()).await?;
        self.run(move |db| {
            let row: Option<(String, String)> = db
                .query_row(
                    "SELECT agent_id, definition FROM agent_chats WHERE thread_id = ?1",
                    [&thread],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(Issue::storage)?;
            let Some((id, text)) = row else {
                return Ok(None);
            };
            let mut agent = decode(&id, &text)?;
            let snapshots: Option<String> = db
                .query_row(
                    "SELECT snapshots FROM agent_chat_skills WHERE thread_id = ?1",
                    [&thread],
                    |r| r.get(0),
                )
                .optional()
                .map_err(Issue::storage)?;
            agent.skills = serde_json::from_str(snapshots.as_deref().unwrap_or("[]"))
                .map_err(Issue::storage)?;
            if agent.skills.iter().map(|s| s.id).collect::<Vec<_>>() != agent.definition.skill_ids {
                return Err(Issue::Storage);
            }
            for skill in &agent.skills {
                skill.definition.validate().map_err(|_| Issue::Storage)?;
            }
            if require_active {
                skills::resolve(db, &agent.definition.skill_ids, true)
                    .map_err(|e| if matches!(e, Issue::Missing) { Issue::SkillInactive } else { e })?;
                if removed(db, "agent", &id)? || !read(db, agent.id)?.definition.active {
                    return Err(Issue::Inactive);
                }
            }
            if !agent.definition.supported() {
                return Err(Issue::Unsupported);
            }
            Ok(Some(agent))
        })
        .await
    }
}

/// Reads a current profile by identifier; missing definitions never fall back to defaults.
pub(crate) fn read(db: &Connection, id: Uuid) -> Result<Agent, Issue> {
    if removed(db, "agent", &id.to_string())? { return Err(Issue::Missing); }
    let text: String = db
        .query_row(
            "SELECT definition FROM agents WHERE id = ?1",
            [id.to_string()],
            |r| r.get(0),
        )
        .optional()
        .map_err(Issue::storage)?
        .ok_or(Issue::Missing)?;
    decode(&id.to_string(), &text)
}

/// Checks persistent removal markers without reading private conversation contents.
pub(crate) fn removed(db: &Connection, kind: &str, id: &str) -> Result<bool, Issue> {
    db.query_row("SELECT EXISTS(SELECT 1 FROM removed WHERE kind = ?1 AND id = ?2)",
        params![kind, id], |row| row.get(0)).map_err(Issue::storage)
}

/// Validates stored definitions before exposing or using their permissions.
fn decode(id: &str, text: &str) -> Result<Agent, Issue> {
    let definition: Definition = serde_json::from_str(text).map_err(Issue::storage)?;
    definition.validate().map_err(|_| Issue::Storage)?;
    if definition.revision.is_none_or(|r| r == 0) {
        return Err(Issue::Storage);
    }
    Ok(Agent {
        id: Uuid::parse_str(id).map_err(Issue::storage)?,
        definition,
        skills: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads a stored agent definition with the given delegation field, or none.
    fn stored(delegation: Option<Value>) -> Result<Agent, Issue> {
        let mut definition = json!({ "name": "Lead", "instructions": "Lead.", "model": null, "effort": null, "revision": 1 });
        if let Some(delegation) = delegation { definition["delegation"] = delegation; }
        decode("019a0000-0000-7000-8000-000000000001", &definition.to_string())
    }

    // Requirement 2026-10-11 (user decision): a time limit of up to a day, for every agent.
    #[test]
    fn delegation_bounds() {
        for (most, minutes, valid) in [(1, 1, true), (10, 1440, true), (3, 61, true), (3, 600, true),
            (0, 10, false), (11, 10, false), (3, 0, false), (3, 1441, false)] {
            let read = stored(Some(json!({ "maxPerTurn": most, "minutes": minutes })));
            assert_eq!(read.is_ok(), valid, "{most} per message, {minutes} min");
        }
    }

    #[test]
    fn older_records_keep_default_limits() {
        assert_eq!(stored(None).unwrap().definition.delegation, Delegation { max_per_turn: 3, minutes: 10 });
    }

    // A run the parent waits for holds its turn open; MiniClaw's provider bridges end that call after 65 minutes.
    // Requirement change 2026-10-11 (lead decision): at the hour a longer run is handed to the background, not stopped.
    #[test]
    fn a_waited_run_is_waited_for_at_most_an_hour_then_handed_over() {
        let long = Delegation { max_per_turn: 3, minutes: 240 };
        assert_eq!((long.wait_minutes(), long.hands_over()), (60, true));
        let hour = Delegation { max_per_turn: 3, minutes: 60 };
        assert_eq!((hour.wait_minutes(), hour.hands_over()), (60, false));
        let short = Delegation { max_per_turn: 3, minutes: 5 };
        assert_eq!((short.wait_minutes(), short.hands_over()), (5, false));
    }
}
