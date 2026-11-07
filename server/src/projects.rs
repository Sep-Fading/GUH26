// Reuses private storage and named-agent validation.
use crate::agents::{self, Issue, Store};
// Resolves active instruction copies without tools.
use crate::skills::{self, Skill};
// Stores project settings and membership atomically.
use rusqlite::{Connection, OptionalExtension, params};
// Serializes bounded project definitions and snapshots.
use serde::{Deserialize, Serialize};
// Adds project context to Codex requests.
use serde_json::{Value, json};
// Rejects duplicate identifiers in project settings.
use std::collections::HashSet;
// Stores the permanent project machine choice.
use crate::environment::ProjectChoice;
// Identifies projects independently of filesystem paths.
use uuid::Uuid;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Definition {
    pub name: String,
    #[serde(default)]
    pub agent_ids: Vec<Uuid>,
    #[serde(default)]
    pub skill_ids: Vec<Uuid>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skill_group_ids: Vec<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<ProjectChoice>,
    // The project's own folder. Its chats may read and write here without a rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// The user's standing instructions for every conversation of the project.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instructions: String,
    pub revision: Option<u64>,
}

#[derive(Clone, Serialize)]
pub struct Project {
    pub id: Uuid,
    #[serde(flatten)]
    pub definition: Definition,
}

#[derive(Clone)]
pub struct Context {
    pub project: Project,
    pub skills: Vec<Skill>,
}

impl Definition {
    /// Rejects unbounded names, duplicate selections, and oversized selection lists.
    fn validate(&self) -> Result<(), Issue> {
        if self.workspace.as_ref().is_some_and(|path| path.len() > 4096 || path == "/"
            || !crate::permissions::plain_absolute_path(path)) { return Err(Issue::Invalid); }
        if self.instructions.chars().count() > 16000 { return Err(Issue::Invalid); }
        if self.name.trim().is_empty() || self.name.len() > 80
            || self.name.chars().any(char::is_control)
            || self.environment.as_ref().is_some_and(|choice|
                choice.dedicated_id().is_some() && !choice.valid())
            || self.agent_ids.len() > 64 || self.skill_ids.len() > 8 || self.skill_group_ids.len() > 16
            || self.skill_group_ids.iter().collect::<HashSet<_>>().len() != self.skill_group_ids.len()
            || self.agent_ids.iter().collect::<HashSet<_>>().len() != self.agent_ids.len()
            || self.skill_ids.iter().collect::<HashSet<_>>().len() != self.skill_ids.len()
        { return Err(Issue::Invalid); }
        Ok(())
    }
}

impl Context {
    /// Adds project identity without changing the pinned named-agent identity.
    pub fn tag(&self, summary: &mut Value) {
        summary["projectId"] = json!(self.project.id);
        summary["projectName"] = json!(self.project.definition.name);
    }

    /// Applies default-agent skill snapshots; named-agent contexts contain no project skills.
    pub fn configure(&self, params: &mut Value) {
        if self.skills.is_empty() { return; }
        let text = self.skills.iter().map(|s| format!("## Skill: {}\n\n{}",
            s.definition.name, s.definition.instructions)).collect::<Vec<_>>().join("\n\n");
        params["developerInstructions"] = json!(text);
    }

    /// Pins project membership and default skills in the chat binding transaction.
    pub(crate) fn bind(&self, db: &Connection, thread: &str) -> Result<(), Issue> {
        let count: i64 = db.query_row("SELECT count(*) FROM project_chats", [], |r| r.get(0))
            .map_err(Issue::storage)?;
        if count >= 4096 { return Err(Issue::Limit); }
        db.execute("INSERT INTO project_chats (thread_id, project_id, skills) VALUES (?1, ?2, ?3)",
            params![thread, self.project.id.to_string(), serde_json::to_string(&self.skills)
                .map_err(Issue::storage)?]).map_err(Issue::storage)?;
        Ok(())
    }
}

impl Store {
    /// Returns project settings in creation order within the registry bound.
    pub async fn projects(&self) -> Result<Vec<Project>, Issue> {
        self.run(|db| {
            let mut query = db.prepare("SELECT id, definition FROM projects ORDER BY rowid LIMIT 65")
                .map_err(Issue::storage)?;
            let rows = query.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(Issue::storage)?;
            let mut projects = Vec::new();
            for row in rows {
                let (id, text) = row.map_err(Issue::storage)?;
                projects.push(decode(&id, &text)?);
            }
            if projects.len() > 64 { return Err(Issue::Limit); }
            Ok(projects)
        }).await
    }

    /// Saves a project revision after validating references, without changing global activation.
    pub async fn save_project(&self, id: Option<Uuid>, mut definition: Definition)
        -> Result<Project, Issue>
    {
        definition.validate()?;
        if definition.environment.is_none() { return Err(Issue::Invalid); }
        if definition.environment.as_ref().is_some_and(|choice|
            choice.needs_dedicated_vm() && choice.dedicated_id().is_none()) {
            return Err(Issue::VmUnavailable);
        }
        self.run(move |db| {
            let tx = db.transaction().map_err(Issue::storage)?;
            for id in &definition.agent_ids { agents::read(&tx, *id)?; }
            skills::resolve(&tx, &definition.skill_ids, false)?;
            crate::skill_groups::check_refs(&tx, &definition.skill_group_ids)?;
            let id = match id {
                Some(id) => {
                    let old = read(&tx, id)?;
                    if old.definition.revision != definition.revision { return Err(Issue::Conflict); }
                    if old.definition.environment.is_some()
                        && old.definition.environment != definition.environment
                    { return Err(Issue::EnvironmentLocked); }
                    definition.revision = Some(definition.revision.and_then(|r| r.checked_add(1))
                        .ok_or(Issue::Invalid)?);
                    id
                }
                None => {
                    if definition.revision.is_some() { return Err(Issue::Invalid); }
                    let count: i64 = tx.query_row("SELECT count(*) FROM projects", [], |r| r.get(0))
                        .map_err(Issue::storage)?;
                    if count >= 64 { return Err(Issue::Limit); }
                    definition.revision = Some(1);
                    Uuid::new_v4()
                }
            };
            if let Some(vm_id) = definition.environment.as_ref()
                .and_then(ProjectChoice::dedicated_id) {
                let mut query = tx.prepare("SELECT id, definition FROM projects")
                    .map_err(Issue::storage)?;
                let rows = query.query_map([], |row| Ok((row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?))).map_err(Issue::storage)?;
                for row in rows {
                    let (other_id, text) = row.map_err(Issue::storage)?;
                    if other_id != id.to_string() && decode(&other_id, &text)?.definition
                        .environment.as_ref().and_then(ProjectChoice::dedicated_id) == Some(vm_id) {
                        return Err(Issue::Conflict);
                    }
                }
            }
            tx.execute("INSERT INTO projects (id, definition) VALUES (?1, ?2)
                ON CONFLICT(id) DO UPDATE SET definition = excluded.definition",
                params![id.to_string(), serde_json::to_string(&definition)
                    .map_err(Issue::storage)?]).map_err(Issue::storage)?;
            tx.commit().map_err(Issue::storage)?;
            Ok(Project { id, definition })
        }).await
    }

    /// Checks project membership and resolves skills only for the default MiniClaw agent.
    pub async fn prepare_project(&self, id: Uuid, agent: Option<Uuid>) -> Result<Context, Issue> {
        self.run(move |db| {
            let project = read(db, id)?;
            check_agent(&project, agent)?;
            let skills = if agent.is_none() {
                skills::resolve(db, &project.definition.skill_ids, true)?
            } else { Vec::new() };
            Ok(Context { project, skills })
        }).await
    }

    /// Restores project membership and pinned skills, optionally checking current admission.
    pub async fn project_context(&self, thread: String, agent: Option<Uuid>, active: bool)
        -> Result<Option<Context>, Issue>
    {
        self.run(move |db| {
            let row: Option<(String, String)> = db.query_row(
                "SELECT project_id, skills FROM project_chats WHERE thread_id = ?1",
                [&thread], |r| Ok((r.get(0)?, r.get(1)?))).optional().map_err(Issue::storage)?;
            let Some((id, text)) = row else { return Ok(None); };
            let project = read(db, Uuid::parse_str(&id).map_err(Issue::storage)?)?;
            let skills: Vec<Skill> = serde_json::from_str(&text).map_err(Issue::storage)?;
            if skills.len() > 8 || (agent.is_some() && !skills.is_empty()) { return Err(Issue::Storage); }
            for skill in &skills { skill.definition.validate().map_err(|_| Issue::Storage)?; }
            if active {
                check_agent(&project, agent)?;
                skills::resolve(db, &skills.iter().map(|s| s.id).collect::<Vec<_>>(), true)
                    .map_err(|e| if matches!(e, Issue::Missing) { Issue::SkillInactive } else { e })?;
            }
            Ok(Some(Context { project, skills }))
        }).await
    }

    /// Rejects revoked profile or project context before another turn is admitted.
    pub async fn admit(&self, thread: String) -> Result<(), Issue> {
        let agent = self.binding(thread.clone(), true).await?;
        self.project_context(thread, agent.map(|a| a.id), true).await?;
        Ok(())
    }

    /// Removes a profile from management while retaining pinned conversation history.
    pub async fn remove_agent(&self, id: Uuid, revision: u64) -> Result<(), Issue> {
        self.run(move |db| {
            let agent = agents::read(db, id)?;
            if agent.definition.revision != Some(revision) { return Err(Issue::Conflict); }
            db.execute("INSERT INTO removed (kind, id) VALUES ('agent', ?1)", [id.to_string()])
                .map_err(Issue::storage)?;
            Ok(())
        }).await
    }

    /// Deletes an owned skill revision without touching installed source files or snapshots.
    pub async fn remove_skill(&self, id: Uuid, revision: u64) -> Result<(), Issue> {
        self.run(move |db| {
            let skill = skills::resolve(db, &[id], false)?.remove(0);
            if skill.definition.revision != Some(revision) { return Err(Issue::Conflict); }
            db.execute("DELETE FROM skills WHERE id = ?1", [id.to_string()])
                .map_err(Issue::storage)?;
            Ok(())
        }).await
    }

    /// Lists the conversations that belong to a project.
    pub async fn project_threads(&self, id: Uuid) -> Result<Vec<String>, Issue> {
        self.run(move |db| {
            let mut query = db.prepare("SELECT thread_id FROM project_chats WHERE project_id = ?1 LIMIT 4096")
                .map_err(Issue::storage)?;
            let rows = query.query_map([id.to_string()], |row| row.get::<_, String>(0)).map_err(Issue::storage)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(Issue::storage)
        }).await
    }

    /// Deletes an observed project revision with its rules. Its conversations are kept: they
    /// leave the project and stay on the machine they were started on.
    pub async fn remove_project(&self, id: Uuid, revision: u64) -> Result<Project, Issue> {
        self.run(move |db| {
            let project = read(db, id)?;
            if project.definition.revision != Some(revision) { return Err(Issue::Conflict); }
            let tx = db.transaction().map_err(Issue::storage)?;
            let key = id.to_string();
            // A conversation's admission record names the bindings it must have. The project
            // bit goes with the binding, or the conversation would read as damaged.
            tx.execute("UPDATE chat_admissions SET required = required & ~2 WHERE thread_id IN
                (SELECT thread_id FROM project_chats WHERE project_id = ?1)", [&key])
                .map_err(Issue::storage)?;
            tx.execute("DELETE FROM project_chats WHERE project_id = ?1", [&key])
                .map_err(Issue::storage)?;
            for table in ["permission_rules", "permission_app_requests"] {
                tx.execute(&format!("DELETE FROM {table} WHERE scope_kind = 'project'
                    AND scope_id = ?1"), [&key]).map_err(Issue::storage)?;
            }
            // The project's shared notes go with it.
            tx.execute("DELETE FROM project_memory WHERE project_id = ?1", [&key]).map_err(Issue::storage)?;
            tx.execute("DELETE FROM projects WHERE id = ?1", [&key]).map_err(Issue::storage)?;
            tx.commit().map_err(Issue::storage)?;
            Ok(project)
        }).await
    }

    /// Records acknowledged chat deletion and removes its project and instruction bindings.
    pub async fn remove_chat(&self, id: String) -> Result<(), Issue> {
        self.run(move |db| {
            let tx = db.transaction().map_err(Issue::storage)?;
            tx.execute("INSERT OR IGNORE INTO removed (kind, id) VALUES ('chat', ?1)", [&id])
                .map_err(Issue::storage)?;
            for table in ["project_chats", "agent_chat_skills", "agent_chats", "chat_skill_selections", "chat_environments", "chat_names", "chat_summaries"] {
                tx.execute(&format!("DELETE FROM {table} WHERE thread_id = ?1"), [&id])
                    .map_err(Issue::storage)?;
            }
            // Messages still queued for a deleted conversation go with it.
            for table in ["message_queue", "message_queue_state"] {
                tx.execute(&format!("DELETE FROM {table} WHERE thread = ?1"), [&id]).map_err(Issue::storage)?;
            }
            tx.execute("DELETE FROM permission_rules WHERE scope_kind = 'conversation'
                AND scope_id = ?1", [&id]).map_err(Issue::storage)?;
            tx.execute("DELETE FROM permission_sessions WHERE conversation = ?1", [&id])
                .map_err(Issue::storage)?;
            tx.execute("DELETE FROM permission_app_requests
                WHERE scope_kind = 'conversation' AND scope_id = ?1", [&id])
                .map_err(Issue::storage)?;
            tx.commit().map_err(Issue::storage)
        }).await
    }

    /// Returns whether an acknowledged deletion excludes this chat from MiniClaw.
    pub async fn chat_removed(&self, id: String) -> Result<bool, Issue> {
        self.run(move |db| agents::removed(db, "chat", &id)).await
    }
}

/// Rejects a named agent unless the project explicitly includes its identity.
fn check_agent(project: &Project, agent: Option<Uuid>) -> Result<(), Issue> {
    if agent.is_some_and(|id| !project.definition.agent_ids.contains(&id)) {
        return Err(Issue::ProjectAccess);
    }
    Ok(())
}

/// Reads project metadata without treating its name as a filesystem path.
fn read(db: &Connection, id: Uuid) -> Result<Project, Issue> {
    let text: String = db.query_row("SELECT definition FROM projects WHERE id = ?1",
        [id.to_string()], |r| r.get(0)).optional().map_err(Issue::storage)?
        .ok_or(Issue::Missing)?;
    decode(&id.to_string(), &text)
}

/// Validates persisted project settings before exposing them or authorizing work.
fn decode(id: &str, text: &str) -> Result<Project, Issue> {
    let definition: Definition = serde_json::from_str(text).map_err(Issue::storage)?;
    definition.validate().map_err(|_| Issue::Storage)?;
    if definition.environment.as_ref().is_some_and(|choice| !choice.valid()) {
        return Err(Issue::Storage);
    }
    if definition.revision.is_none_or(|r| r == 0) { return Err(Issue::Storage); }
    Ok(Project { id: Uuid::parse_str(id).map_err(Issue::storage)?, definition })
}
