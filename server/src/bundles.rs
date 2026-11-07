//! Adds a setup bundle (agents, skills, skill groups, MCP servers, their permission rules and
//! capability defaults, and the main agent's defaults) to a profile database without replacing
//! anything. Setup calls it through `miniclaw-server --add-bundle`, which holds the data folder's
//! instance lock, so it never writes while MiniClaw runs.
//!
//! "Additive" means, per item:
//! - An agent, skill or skill group whose name (ignoring case and outer spaces) the user already
//!   has is left exactly as it is and reported as present; an MCP server is matched by its exact
//!   name, which is unique in the list. A deleted agent does not count. Anything missing is added.
//! - The bundle's own references (an agent's skills, a group's members, rules and defaults) point
//!   to whichever entry now holds that name: the one just added or the user's own.
//! - A permission rule or capability default is added only for an agent the bundle added in this
//!   run, and a rule of the main agent only for an MCP server the bundle added in this run. The
//!   user's own agents, servers and the main agent's existing rules are never changed.
//! - The main agent's model, effort and skills are set only when nothing was ever saved for it.
//! - Conversations, projects, approvals, settings and stored secrets are not touched.
//! Everything happens in one transaction, so a refusal (a limit, an invalid entry) adds nothing,
//! and running it again adds nothing new.

// Reuses the profile store, its definitions and its validation.
use crate::{agents::{self, Issue, Store}, mcp_servers, permissions::{Effect, Target}, skill_groups, skills};
// Writes inside one immediate transaction and reads names read-only.
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
// Reads the catalog setup sends and writes the report it prints.
use serde::{Deserialize, Serialize};
// Keeps definitions that are stored as they come.
use serde_json::Value;
// Maps bundle ids to stored ids.
use std::{collections::HashMap, path::Path};
// Identifies entries.
use uuid::Uuid;

/// What setup sends: the bundle's entries with fresh ids and this computer's paths filled in.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Catalog {
    pub agents: Vec<Entry<agents::Definition>>,
    pub skills: Vec<Entry<skills::Definition>>,
    pub groups: Vec<Entry<skill_groups::Definition>>,
    pub mcps: Vec<Entry<Value>>,
    #[serde(default)]
    pub permissions: Vec<Rule>,
    #[serde(default)]
    pub defaults: Vec<CapabilityDefault>,
    /// Sent only for a new data folder.
    #[serde(default)]
    pub main_agent: Option<MainAgent>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry<T> {
    pub id: Uuid,
    pub definition: T,
}

/// A permission rule of the main agent (nil scope) or of a bundle agent.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub scope: Uuid,
    pub target: Target,
    pub effect: Effect,
}

/// A capability preference of a bundle agent; `enabled` is 0 or 1 as the table stores it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityDefault {
    pub agent: Uuid,
    pub capability: String,
    pub enabled: u8,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MainAgent {
    pub model: Option<String>,
    pub effort: Option<String>,
    #[serde(default)]
    pub speed: Option<String>,
    #[serde(default)]
    pub skill_ids: Vec<Uuid>,
    #[serde(default)]
    pub skill_group_ids: Vec<Uuid>,
}

/// One entry's outcome: added now, or already present (the user's entry, kept).
#[derive(Debug, Serialize)]
pub struct Outcome {
    pub kind: &'static str,
    pub name: String,
    pub id: String,
    pub added: bool,
}

/// What was added and what was kept.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub entries: Vec<Outcome>,
    /// Permission rules and capability defaults added.
    pub rules: usize,
    pub defaults: usize,
    /// "set" (saved now), "kept" (the user's was left) or "none" (not part of this run).
    pub main_agent: &'static str,
}

/// The names a database holds, for `miniclaw bundle status`.
#[derive(Debug, Default, Serialize)]
pub struct Inventory {
    pub agents: Vec<String>,
    pub skills: Vec<String>,
    pub groups: Vec<String>,
    pub mcps: Vec<String>,
}

/// The most entries each list holds (the limits of the stores' own save functions).
const LIMITS: [(&str, i64); 4] = [("agents", 64), ("skills", 128), ("skill groups", 64), ("MCP servers", 32)];

/// Folds a name the way entries are matched.
fn fold(name: &str) -> String { name.trim().to_lowercase() }

/// Reads (folded name, id) of one table's live entries.
fn names(db: &Connection, sql: &str) -> Result<HashMap<String, String>, Issue> {
    let mut query = db.prepare(sql).map_err(Issue::storage)?;
    let rows = query.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))).map_err(Issue::storage)?;
    let mut found = HashMap::new();
    for row in rows {
        let (id, text) = row.map_err(Issue::storage)?;
        // A definition column holds JSON; the MCP list's name column holds the name itself.
        let name = serde_json::from_str::<Value>(&text).ok()
            .and_then(|value| value.get("name").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or(text);
        found.entry(fold(&name)).or_insert(id);
    }
    Ok(found)
}

const AGENTS: &str = "SELECT id, definition FROM agents WHERE id NOT IN (SELECT id FROM removed WHERE kind = 'agent') ORDER BY rowid";
const SKILLS: &str = "SELECT id, definition FROM skills ORDER BY rowid";
const GROUPS: &str = "SELECT id, definition FROM skill_groups ORDER BY rowid";
const SERVERS: &str = "SELECT id, name FROM mcp_servers ORDER BY rowid";

/// Where each bundle id points after the merge, and whether this run added it.
#[derive(Default)]
struct Map(HashMap<Uuid, (String, bool)>);

impl Map {
    fn get(&self, id: &Uuid, what: &str) -> Result<&(String, bool), String> {
        self.0.get(id).ok_or_else(|| format!("The bundle refers to a {what} it does not contain ({id})."))
    }
    fn uuid(&self, id: &Uuid, what: &str) -> Result<Uuid, String> {
        Uuid::parse_str(&self.get(id, what)?.0).map_err(|_| format!("The existing {what} {id} has an unexpected id."))
    }
}

/// Decides, for every entry of one kind, whether it is added or the user's entry is kept.
fn plan<T>(entries: &[Entry<T>], name: impl Fn(&T) -> String, existing: &HashMap<String, String>, kind: &'static str,
    map: &mut Map, outcomes: &mut Vec<Outcome>) -> Result<usize, String> {
    let mut seen = std::collections::HashSet::new();
    let mut added = 0;
    for entry in entries {
        let name = name(&entry.definition);
        if !seen.insert(fold(&name)) { return Err(format!("The bundle names two {kind}s \"{name}\".")); }
        let (id, new) = match existing.get(&fold(&name)) {
            Some(id) => (id.clone(), false),
            None => { added += 1; (entry.id.to_string(), true) }
        };
        if map.0.insert(entry.id, (id.clone(), new)).is_some() { return Err(format!("The bundle uses the id {} twice.", entry.id)); }
        outcomes.push(Outcome { kind, name, id, added: new });
    }
    Ok(added)
}

/// Rewrites an MCP target's server id; other targets are unchanged.
fn retarget(target: &Target, servers: &Map) -> Result<(Target, bool), String> {
    let server = |id: &String| -> Result<(String, bool), String> {
        let bundle = Uuid::parse_str(id).map_err(|_| format!("The bundle's rule names an invalid server id {id}."))?;
        Ok(servers.get(&bundle, "MCP server")?.clone())
    };
    Ok(match target {
        Target::McpServer { server: id } => { let (id, added) = server(id)?; (Target::McpServer { server: id }, added) }
        Target::McpTool { server: id, tool } => { let (id, added) = server(id)?; (Target::McpTool { server: id, tool: tool.clone() }, added) }
        other => (other.clone(), false),
    })
}

/// Merges the catalog into the open database as described in the module documentation.
pub fn add(db: &mut Connection, catalog: Catalog) -> Result<Result<Report, String>, Issue> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate).map_err(Issue::storage)?;
    let mut map = Map::default();
    let mut outcomes = Vec::new();
    let refuse = |message: String| Ok(Err(message));
    // Every entry must be one the store itself would save.
    for entry in &catalog.agents {
        if entry.definition.validate().is_err() { return refuse(format!("The bundle's agent \"{}\" is not valid.", entry.definition.name)); }
    }
    for entry in &catalog.skills {
        if entry.definition.validate().is_err() { return refuse(format!("The bundle's skill \"{}\" is not valid.", entry.definition.name)); }
    }
    for entry in &catalog.groups {
        if entry.definition.validate().is_err() { return refuse(format!("The bundle's skill group \"{}\" is not valid.", entry.definition.name)); }
    }
    let mut servers = Vec::new();
    for entry in &catalog.mcps {
        match serde_json::from_value::<mcp_servers::Server>(entry.definition.clone()) {
            Ok(server) if mcp_servers::valid_name(&server.name) => servers.push(server.name),
            _ => return refuse("The bundle has an MCP server that is not valid.".into()),
        }
    }
    let counts = [
        plan(&catalog.agents, |d| d.name.clone(), &names(&tx, AGENTS)?, "agent", &mut map, &mut outcomes),
        plan(&catalog.skills, |d| d.name.clone(), &names(&tx, SKILLS)?, "skill", &mut map, &mut outcomes),
        plan(&catalog.groups, |d| d.name.clone(), &names(&tx, GROUPS)?, "skill group", &mut map, &mut outcomes),
        plan(&catalog.mcps, |d| d.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
            // MCP names are unique and exact; fold() keeps them as they are (they are lowercase).
            &names(&tx, SERVERS)?, "MCP server", &mut map, &mut outcomes),
    ];
    let mut counted = Vec::new();
    for count in counts { match count { Ok(count) => counted.push(count), Err(message) => return refuse(message) } }
    for ((label, limit), (adding, sql)) in LIMITS.iter().zip(counted.iter().zip([AGENTS, SKILLS, GROUPS, SERVERS])) {
        let present = names(&tx, sql)?.len() as i64;
        if *adding as i64 + present > *limit {
            return refuse(format!("The bundle would add {adding} {label}, but this installation already has {present} of at most {limit}. \
                Remove some in MiniClaw first; nothing was added."));
        }
    }
    let result = (|| -> Result<Result<Report, String>, Issue> {
        let insert = |table: &str, id: &str, definition: String| tx.execute(
            &format!("INSERT INTO {table} (id, definition) VALUES (?1, ?2)"), params![id, definition]).map(|_| ()).map_err(Issue::storage);
        for entry in catalog.skills {
            if !map.0[&entry.id].1 { continue; }
            let mut definition = entry.definition;
            definition.revision = Some(1);
            insert("skills", &entry.id.to_string(), serde_json::to_string(&definition).map_err(Issue::storage)?)?;
        }
        for entry in catalog.groups {
            if !map.0[&entry.id].1 { continue; }
            let mut definition = entry.definition;
            for member in &mut definition.members {
                member.skill_id = match map.uuid(&member.skill_id, "skill") { Ok(id) => id, Err(message) => return Ok(Err(message)) };
            }
            definition.revision = Some(1);
            insert("skill_groups", &entry.id.to_string(), serde_json::to_string(&definition).map_err(Issue::storage)?)?;
        }
        for (entry, name) in catalog.mcps.into_iter().zip(servers) {
            if !map.0[&entry.id].1 { continue; }
            // Stored as the store's own save stores it: without id and revision, which have columns.
            let mut definition = entry.definition;
            if let Some(fields) = definition.as_object_mut() { fields.remove("id"); fields.remove("revision"); }
            tx.execute("INSERT INTO mcp_servers (id, name, definition, revision) VALUES (?1, ?2, ?3, 1)",
                params![entry.id.to_string(), name, definition.to_string()]).map_err(Issue::storage)?;
        }
        for entry in catalog.agents {
            if !map.0[&entry.id].1 { continue; }
            let mut definition = entry.definition;
            let remap = |ids: &mut Vec<Uuid>, what: &str| -> Result<(), String> {
                for id in ids.iter_mut() { *id = map.uuid(id, what)?; }
                Ok(())
            };
            if let Err(message) = remap(&mut definition.skill_ids, "skill").and(remap(&mut definition.skill_group_ids, "skill group"))
                .and(remap(&mut definition.delegates_to, "agent")) { return Ok(Err(message)); }
            definition.revision = Some(1);
            insert("agents", &entry.id.to_string(), serde_json::to_string(&definition).map_err(Issue::storage)?)?;
        }
        let mut rules = 0;
        for rule in catalog.permissions {
            let (scope, scope_added) = if rule.scope.is_nil() { (Uuid::nil(), false) } else {
                match map.uuid(&rule.scope, "agent") { Ok(id) => (id, map.0[&rule.scope].1), Err(message) => return Ok(Err(message)) }
            };
            let (target, server_added) = match retarget(&rule.target, &map) { Ok(found) => found, Err(message) => return Ok(Err(message)) };
            // A bundle agent added now gets its rules; the main agent only gets servers added now.
            if !(scope_added || (rule.scope.is_nil() && server_added)) { continue; }
            let Ok(key) = target.key() else { return Ok(Err("The bundle has a permission rule that is not valid.".into())) };
            let effect = if rule.effect == Effect::Allow { "allow" } else { "block" };
            rules += tx.execute("INSERT OR IGNORE INTO permission_rules VALUES ('agent', ?1, ?2, ?3)",
                params![scope.to_string(), key, effect]).map_err(Issue::storage)?;
        }
        let mut defaults = 0;
        for default in catalog.defaults {
            let (agent, added) = match map.get(&default.agent, "agent") { Ok(found) => found.clone(), Err(message) => return Ok(Err(message)) };
            if !added { continue; }
            if default.enabled > 1 { return Ok(Err("The bundle has a capability default that is not valid.".into())); }
            let capability = match default.capability.strip_prefix("mcp:") {
                Some(server) => match Uuid::parse_str(server).map_err(|_| String::new()).and_then(|id| map.get(&id, "MCP server").cloned()) {
                    Ok((id, _)) => format!("mcp:{id}"),
                    Err(_) => return Ok(Err("The bundle has a capability default for a server it does not contain.".into())),
                },
                None => default.capability,
            };
            defaults += tx.execute("INSERT OR IGNORE INTO capability_defaults (agent, capability, enabled) VALUES (?1, ?2, ?3)",
                params![agent, capability, default.enabled]).map_err(Issue::storage)?;
        }
        let main_agent = match catalog.main_agent {
            None => "none",
            Some(main) => {
                let saved: Option<i64> = tx.query_row("SELECT id FROM main_agent WHERE id = 1", [], |row| row.get(0))
                    .optional().map_err(Issue::storage)?;
                if saved.is_some() { "kept" } else {
                    let mut skill_ids = Vec::new();
                    let mut group_ids = Vec::new();
                    for id in &main.skill_ids { match map.uuid(id, "skill") { Ok(id) => skill_ids.push(id), Err(message) => return Ok(Err(message)) } }
                    for id in &main.skill_group_ids { match map.uuid(id, "skill group") { Ok(id) => group_ids.push(id), Err(message) => return Ok(Err(message)) } }
                    let change = crate::main_agent::Change { model: main.model, effort: main.effort, speed: main.speed,
                        skill_ids: Some(skill_ids), skill_group_ids: Some(group_ids), delegation: None };
                    if !change.valid() { return Ok(Err("The bundle's main agent settings are not valid.".into())); }
                    tx.execute("INSERT INTO main_agent (id, model, effort, speed, skill_ids, skill_group_ids) VALUES (1, ?1, ?2, ?3, ?4, ?5)",
                        params![change.model, change.effort, change.speed,
                            serde_json::to_string(&change.skill_ids.unwrap_or_default()).map_err(Issue::storage)?,
                            serde_json::to_string(&change.skill_group_ids.unwrap_or_default()).map_err(Issue::storage)?]).map_err(Issue::storage)?;
                    "set"
                }
            }
        };
        Ok(Ok(Report { entries: outcomes, rules, defaults, main_agent }))
    })()?;
    // A refusal found while writing leaves the transaction uncommitted, so nothing stays.
    if result.is_ok() { tx.commit().map_err(Issue::storage)?; }
    Ok(result)
}

impl Store {
    /// Adds a bundle in one transaction; the error is a sentence for the user.
    pub async fn add_bundle(&self, catalog: Catalog) -> Result<Report, String> {
        match self.run(move |db| add(db, catalog)).await {
            Ok(result) => result,
            Err(_) => Err("The profile database could not be changed; nothing was added.".into()),
        }
    }
}

/// Lists the names a profile database holds without changing it or waiting for MiniClaw:
/// the database is opened read-only. A missing database holds nothing.
pub fn inventory(path: &Path) -> Result<Inventory, Issue> {
    if !path.try_exists().map_err(Issue::storage)? { return Ok(Inventory::default()); }
    // NOFOLLOW applies to the database itself; aliases of its folder (macOS /var) are resolved first, as Store::open does.
    let path = path.parent().ok_or(Issue::Storage)?.canonicalize().map_err(Issue::storage)?
        .join(path.file_name().ok_or(Issue::Storage)?);
    let db = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW).map_err(Issue::storage)?;
    let read = |sql: &str| -> Result<Vec<String>, Issue> {
        let mut query = db.prepare(sql).map_err(Issue::storage)?;
        let rows = query.query_map([], |row| row.get::<_, String>(1)).map_err(Issue::storage)?;
        let mut found = Vec::new();
        for text in rows {
            let text = text.map_err(Issue::storage)?;
            found.push(serde_json::from_str::<Value>(&text).ok()
                .and_then(|value| value.get("name").and_then(Value::as_str).map(str::to_owned)).unwrap_or(text));
        }
        Ok(found)
    };
    Ok(Inventory { agents: read(AGENTS)?, skills: read(SKILLS)?, groups: read(GROUPS)?, mcps: read(SERVERS)? })
}
