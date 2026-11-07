// Checks that a setup bundle is added to a profile database without replacing anything.
use miniclaw_server::{agents::Store, bundles::{self, Catalog}};
// Builds catalogs and reads stored rows.
use serde_json::{Value, json};
// Creates disposable private folders.
use std::{fs, path::PathBuf};
// Separates each test's folder.
use uuid::Uuid;

struct Directory(PathBuf);

impl Directory {
    /// Creates an isolated private directory for one test's database.
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("mc-bundle-{}", Uuid::new_v4()));
        miniclaw_server::platform_fs::private_dir(&path, false).unwrap();
        Self(path)
    }
    fn database(&self) -> PathBuf { self.0.join("agents.sqlite3") }
}

impl Drop for Directory {
    /// Removes only this test's directory.
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

const AGENT: &str = "0b52add5-b4b8-4e30-b1a7-60ddb5aac3bc";
const SKILL: &str = "8b598979-c84a-4a3b-bf6e-0d800b79ad9f";
const SHARED_SKILL: &str = "4ff7e98a-5742-4a09-864f-1071e89290e3";
const GROUP: &str = "7f9c9ec6-986a-4af8-bd81-537363af3f7e";
const SERVER: &str = "2b3aa07f-8e62-4bb8-a7e9-7d031e74fe39";
const SHARED_SERVER: &str = "dcd07b35-90f3-4243-a161-d3bbb53da7f9";
const MAIN: &str = "00000000-0000-0000-0000-000000000000";

/// A small bundle with the shapes of the real one: an agent using a skill, a group, two
/// servers, rules for the main agent and the bundle's agent, capability defaults, main agent.
fn catalog(fresh_ids: bool, main_agent: bool) -> Catalog {
    let id = |bundle: &str| if fresh_ids { Uuid::new_v4().to_string() } else { bundle.to_owned() };
    let (agent, skill, shared, group, server, studio) = (id(AGENT), id(SKILL), id(SHARED_SKILL), id(GROUP), id(SERVER), id(SHARED_SERVER));
    let mut value = json!({
        "agents": [{ "id": agent, "definition": { "name": "Blender Agent", "instructions": "Model things.", "model": "gpt-6-astra",
            "effort": "high", "speed": null, "revision": 1, "active": true, "capabilities": [], "access": { "applications": [], "files": [], "network": [] },
            "delegatesTo": [], "delegation": { "maxPerTurn": 3, "minutes": 10 }, "skillIds": [skill], "skillGroupIds": [] } }],
        "skills": [
            { "id": skill, "definition": { "name": "roblox-lowpoly-assets", "description": "Low poly.", "instructions": "Keep it low.", "active": true, "slash": false, "revision": 1 } },
            { "id": shared, "definition": { "name": "unslop", "description": "Bundle copy.", "instructions": "Bundle text.", "active": true, "slash": false, "revision": 1 } }],
        "groups": [{ "id": group, "definition": { "name": "Universal", "active": true, "members": [{ "skillId": shared, "enabled": true }], "revision": 1 } }],
        "mcps": [
            { "id": server, "definition": { "name": "blender", "kind": "stdio", "command": "/opt/bundle/python/bin/blender-mcp", "args": ["--transport", "stdio"],
                "folder": "/opt/bundle", "env": [], "headers": [], "hasToken": false, "needs": [], "ports": [9876], "revision": 1 } },
            { "id": studio, "definition": { "name": "roblox_studio", "kind": "stdio", "command": "/Applications/Studio", "args": [],
                "env": [], "headers": [], "hasToken": false, "needs": [], "revision": 1 } }],
        "permissions": [
            { "scope": MAIN, "target": { "target_type": "mcp_server", "server": server }, "effect": "allow" },
            { "scope": MAIN, "target": { "target_type": "mcp_server", "server": studio }, "effect": "allow" },
            { "scope": agent, "target": { "target_type": "feature", "name": "network" }, "effect": "allow" },
            { "scope": agent, "target": { "target_type": "mcp_server", "server": studio }, "effect": "allow" }],
        "defaults": [
            { "agent": agent, "capability": "network", "enabled": 0 },
            { "agent": agent, "capability": format!("mcp:{studio}"), "enabled": 1 }],
        "mainAgent": { "model": "claude-opus-5-5[1m]", "effort": "high", "speed": null, "skillIds": [], "skillGroupIds": [group] },
    });
    if !main_agent { value.as_object_mut().unwrap().remove("mainAgent"); }
    serde_json::from_value(value).unwrap()
}

/// Reads one table's rows as (id, definition) pairs.
fn rows(path: &PathBuf, sql: &str) -> Vec<(String, Value)> {
    let db = rusqlite::Connection::open(path).unwrap();
    let mut query = db.prepare(sql).unwrap();
    query.query_map([], |row| Ok((row.get::<_, String>(0)?, serde_json::from_str(&row.get::<_, String>(1)?).unwrap())))
        .unwrap().collect::<Result<_, _>>().unwrap()
}

fn count(path: &PathBuf, sql: &str) -> i64 {
    rusqlite::Connection::open(path).unwrap().query_row(sql, [], |row| row.get(0)).unwrap()
}

#[tokio::test]
/// A new database receives everything, the main agent included; a second run adds nothing.
async fn adds_everything_once_and_is_idempotent() {
    let root = Directory::new();
    let store = Store::open(&root.database()).unwrap();
    let report = store.add_bundle(catalog(true, true)).await.unwrap();
    assert!(report.entries.iter().all(|entry| entry.added), "{:?}", report.entries);
    assert_eq!(report.entries.len(), 6);
    assert_eq!(report.main_agent, "set");
    assert_eq!(report.rules, 4);
    assert_eq!(report.defaults, 2);
    let agents = rows(&root.database(), "SELECT id, definition FROM agents");
    let skills = rows(&root.database(), "SELECT id, definition FROM skills");
    assert_eq!(agents[0].1["skillIds"][0], skills.iter().find(|(_, s)| s["name"] == "roblox-lowpoly-assets").unwrap().0.as_str());
    let main = store.main_agent().await.unwrap();
    assert_eq!(main.model.as_deref(), Some("claude-opus-5-5[1m]"));
    let groups = rows(&root.database(), "SELECT id, definition FROM skill_groups");
    assert_eq!(main.skill_group_ids[0].to_string(), groups[0].0);
    // The listing the page reads parses every added server.
    assert_eq!(store.mcp_servers().await.unwrap().len(), 2);

    let again = store.add_bundle(catalog(true, true)).await.unwrap();
    assert!(again.entries.iter().all(|entry| !entry.added), "{:?}", again.entries);
    assert_eq!((again.rules, again.defaults, again.main_agent), (0, 0, "kept"));
    for (table, expected) in [("agents", 1), ("skills", 2), ("skill_groups", 1), ("mcp_servers", 2), ("permission_rules", 4), ("capability_defaults", 2)] {
        assert_eq!(count(&root.database(), &format!("SELECT count(*) FROM {table}")), expected, "{table}");
    }
}

#[tokio::test]
/// An entry the user already has under the bundle's name is left exactly as it is, the bundle's
/// references point to it, and nothing is granted to the user's own entries.
async fn keeps_the_users_entries_and_points_references_to_them() {
    let root = Directory::new();
    let store = Store::open(&root.database()).unwrap();
    let user_skill = store.save_skill(None, serde_json::from_value(json!({ "name": "Unslop", "instructions": "Mine.", "active": true, "revision": null })).unwrap()).await.unwrap();
    let mine = std::env::temp_dir().join("my-studio").to_string_lossy().into_owned();
    let draft = serde_json::from_value(json!({ "name": "roblox_studio", "kind": "stdio", "command": mine })).unwrap();
    let (user_server, _) = store.save_mcp_server(None, draft, Vec::new()).await.unwrap();
    store.save_main_agent(serde_json::from_value(json!({ "model": "my-model", "effort": "low" })).unwrap()).await.unwrap();
    let before_rules = count(&root.database(), "SELECT count(*) FROM permission_rules");

    let report = store.add_bundle(catalog(true, true)).await.unwrap();
    let status = |name: &str| report.entries.iter().find(|entry| entry.name == name).unwrap();
    assert!(!status("unslop").added);
    assert_eq!(status("unslop").id, user_skill.id.to_string());
    assert!(!status("roblox_studio").added);
    assert_eq!(status("roblox_studio").id, user_server.id);
    assert!(status("blender").added && status("Blender Agent").added && status("Universal").added);
    assert_eq!(report.main_agent, "kept");

    // The user's skill, server and main agent are unchanged.
    let skills = rows(&root.database(), "SELECT id, definition FROM skills");
    let kept = skills.iter().find(|(id, _)| *id == user_skill.id.to_string()).unwrap();
    assert_eq!(kept.1["instructions"], "Mine.");
    assert_eq!(skills.len(), 2);
    let servers = store.mcp_servers().await.unwrap();
    assert_eq!(servers.iter().find(|server| server.id == user_server.id).unwrap().command.as_deref(), Some(mine.as_str()));
    assert_eq!(store.main_agent().await.unwrap().model.as_deref(), Some("my-model"));
    // The new group holds the user's skill of that name.
    let groups = rows(&root.database(), "SELECT id, definition FROM skill_groups");
    assert_eq!(groups[0].1["members"][0]["skillId"], user_skill.id.to_string().as_str());
    // The main agent is allowed only the server the bundle added; the bundle's own agent gets its rules,
    // which now name the user's roblox_studio.
    let rules = rows(&root.database(), "SELECT scope_id, target FROM permission_rules");
    let main: Vec<_> = rules.iter().filter(|(scope, _)| scope == MAIN).collect();
    assert_eq!(main.len() as i64 - before_rules, 1);
    assert_eq!(main.last().unwrap().1["server"], servers.iter().find(|server| server.name == "blender").unwrap().id.as_str());
    let agent = status("Blender Agent").id.clone();
    assert!(rules.iter().any(|(scope, target)| *scope == agent && target["server"] == user_server.id.as_str()));
    let defaults = count(&root.database(), &format!("SELECT count(*) FROM capability_defaults WHERE agent = '{agent}' AND capability = 'mcp:{}'", user_server.id));
    assert_eq!(defaults, 1);
}

#[tokio::test]
/// An agent the user deleted does not count as present; a bundle that would pass a limit adds nothing.
async fn refuses_past_a_limit_without_adding_anything() {
    let root = Directory::new();
    let store = Store::open(&root.database()).unwrap();
    for index in 0..31 {
        let command = std::env::temp_dir().join("tool").to_string_lossy().into_owned();
        let draft = serde_json::from_value(json!({ "name": format!("server-{index}"), "kind": "stdio", "command": command })).unwrap();
        store.save_mcp_server(None, draft, Vec::new()).await.unwrap();
    }
    let error = store.add_bundle(catalog(true, true)).await.unwrap_err();
    assert!(error.contains("MCP servers"), "{error}");
    assert_eq!(count(&root.database(), "SELECT count(*) FROM skills"), 0);
    assert_eq!(count(&root.database(), "SELECT count(*) FROM agents"), 0);
    assert_eq!(count(&root.database(), "SELECT count(*) FROM main_agent"), 0);
}

#[tokio::test]
/// A reference outside the bundle is refused before anything is written.
async fn refuses_references_outside_the_bundle() {
    let root = Directory::new();
    let store = Store::open(&root.database()).unwrap();
    let mut catalog = catalog(false, false);
    catalog.defaults.clear();
    catalog.permissions.clear();
    catalog.groups[0].definition.members[0].skill_id = Uuid::new_v4();
    assert!(store.add_bundle(catalog).await.is_err());
    assert_eq!(count(&root.database(), "SELECT count(*) FROM skills"), 0);
}

#[tokio::test]
/// The read-only listing names what the database holds and works without a database.
async fn inventory_lists_names() {
    let root = Directory::new();
    assert!(bundles::inventory(&root.database()).unwrap().agents.is_empty());
    let store = Store::open(&root.database()).unwrap();
    store.add_bundle(catalog(true, false)).await.unwrap();
    let found = bundles::inventory(&root.database()).unwrap();
    assert_eq!(found.agents, ["Blender Agent"]);
    assert_eq!(found.mcps, ["blender", "roblox_studio"]);
    assert_eq!(found.skills.len(), 2);
    assert_eq!(found.groups, ["Universal"]);
}
