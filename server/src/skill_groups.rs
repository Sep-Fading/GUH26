// Shares private storage and validation failures.
use crate::agents::{Issue, Store};
// Resolves owned instructions without granting tools.
use crate::skills::{self, Skill};
// Persists revisions and selections within transactions.
use rusqlite::{Connection, OptionalExtension, params};
// Serializes group settings and conversation selections.
use serde::{Deserialize, Serialize};
// Deduplicates instruction contributions across selected sources.
use std::collections::HashSet;
// Identifies groups independently from their names.
use uuid::Uuid;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Member {
    pub skill_id: Uuid,
    pub enabled: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub name: String,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub members: Vec<Member>,
    pub revision: Option<u64>,
}

#[derive(Serialize)]
pub struct Group {
    pub id: Uuid,
    #[serde(flatten)]
    pub definition: Definition,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selection {
    #[serde(default)]
    pub skill_ids: Vec<Uuid>,
    #[serde(default)]
    pub skill_group_ids: Vec<Uuid>,
    pub revision: u64,
}

impl Definition {
    /// Rejects oversized groups, duplicate members, and invalid names.
    pub fn validate(&self) -> Result<(), Issue> {
        if self.name.trim().is_empty()
            || self.name.len() > 80
            || self.name.chars().any(char::is_control)
            || self.members.len() > 128
            || self
                .members
                .iter()
                .map(|m| m.skill_id)
                .collect::<HashSet<_>>()
                .len()
                != self.members.len()
        {
            return Err(Issue::Invalid);
        }
        Ok(())
    }
}

impl Selection {
    /// Bounds independent selections and rejects duplicate references.
    pub fn validate(&self) -> Result<(), Issue> {
        if self.skill_ids.len() > 8
            || self.skill_group_ids.len() > 16
            || self.skill_ids.iter().collect::<HashSet<_>>().len() != self.skill_ids.len()
            || self.skill_group_ids.iter().collect::<HashSet<_>>().len()
                != self.skill_group_ids.len()
        {
            return Err(Issue::Invalid);
        }
        Ok(())
    }
}

impl Store {
    /// Checks a new conversation's selection before creating its Codex task.
    pub async fn check_selection(&self, value: Selection) -> Result<(), Issue> {
        value.validate()?;
        if value.revision != 0 {
            return Err(Issue::Invalid);
        }
        self.run(move |db| {
            check_refs(db, &value.skill_group_ids)?;
            skills::resolve(db, &value.skill_ids, true)?;
            Ok(())
        })
        .await
    }

    /// Lists saved groups in creation order, including inactive groups.
    pub async fn groups(&self) -> Result<Vec<Group>, Issue> {
        self.run(|db| {
            let mut query = db
                .prepare("SELECT id FROM skill_groups ORDER BY rowid LIMIT 65")
                .map_err(Issue::storage)?;
            let ids = query
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(Issue::storage)?;
            let mut result = Vec::new();
            for id in ids {
                let id = Uuid::parse_str(&id.map_err(Issue::storage)?)
                    .map_err(Issue::storage)?;
                result.push(read(db, id)?.ok_or(Issue::Storage)?);
            }
            if result.len() > 64 {
                return Err(Issue::Limit);
            }
            Ok(result)
        })
        .await
    }

    /// Saves one group revision without changing any skill or other group.
    pub async fn save_group(
        &self,
        id: Option<Uuid>,
        mut definition: Definition,
    ) -> Result<Group, Issue> {
        definition.validate()?;
        self.run(move |db| {
            let tx = db.transaction().map_err(Issue::storage)?;
            let members = definition
                .members
                .iter()
                .map(|m| m.skill_id)
                .collect::<Vec<_>>();
            skills::resolve(&tx, &members, false)?;
            let id = if let Some(id) = id {
                let old = read(&tx, id)?.ok_or(Issue::Missing)?;
                if old.definition.revision != definition.revision {
                    return Err(Issue::Conflict);
                }
                definition.revision = Some(
                    definition
                        .revision
                        .and_then(|r| r.checked_add(1))
                        .ok_or(Issue::Invalid)?,
                );
                id
            } else {
                if definition.revision.is_some() {
                    return Err(Issue::Invalid);
                }
                let count: i64 = tx
                    .query_row("SELECT count(*) FROM skill_groups", [], |r| r.get(0))
                    .map_err(Issue::storage)?;
                if count >= 64 {
                    return Err(Issue::Limit);
                }
                definition.revision = Some(1);
                Uuid::new_v4()
            };
            tx.execute(
                "INSERT INTO skill_groups (id, definition) VALUES (?1, ?2)
                ON CONFLICT(id) DO UPDATE SET definition = excluded.definition",
                params![
                    id.to_string(),
                    serde_json::to_string(&definition).map_err(Issue::storage)?
                ],
            )
            .map_err(Issue::storage)?;
            tx.commit().map_err(Issue::storage)?;
            Ok(Group { id, definition })
        })
        .await
    }

    /// Removes a group contribution while retaining its member skills and other selections.
    pub async fn remove_group(&self, id: Uuid, revision: u64) -> Result<(), Issue> {
        self.run(move |db| {
            let old = read(db, id)?.ok_or(Issue::Missing)?;
            if old.definition.revision != Some(revision) {
                return Err(Issue::Conflict);
            }
            db.execute("DELETE FROM skill_groups WHERE id = ?1", [id.to_string()])
                .map_err(Issue::storage)?;
            Ok(())
        })
        .await
    }

    /// Returns explicit conversation selections; an untouched chat starts at revision zero.
    pub async fn selection(&self, thread: String) -> Result<Selection, Issue> {
        self.run(move |db| selection(db, &thread)).await
    }

    /// Saves independent references and pins individually selected instruction copies.
    pub async fn save_selection(
        &self,
        thread: String,
        mut value: Selection,
    ) -> Result<Selection, Issue> {
        value.validate()?;
        self.run(move |db| {
            let tx = db.transaction().map_err(Issue::storage)?;
            write_selection(&tx, &thread, &mut value)?;
            tx.commit().map_err(Issue::storage)?;
            Ok(value)
        })
        .await
    }

    /// Resolves the next turn's contributions, excluding skills already pinned by its agent or project.
    pub async fn turn_skills(&self, thread: String) -> Result<Option<String>, Issue> {
        let agent = self.binding(thread.clone(), false).await?;
        let project = self
            .project_context(thread.clone(), agent.as_ref().map(|a| a.id), false)
            .await?;
        let mut pinned = agent
            .as_ref()
            .map(|a| a.skills.iter().map(|s| s.id).collect::<HashSet<_>>())
            .unwrap_or_default();
        let project_groups = if agent.is_none() {
            if let Some(project) = project {
                pinned.extend(project.skills.iter().map(|s| s.id));
                project.project.definition.skill_group_ids
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        let no_agent = agent.is_none();
        let agent_groups = agent.map(|a| a.definition.skill_group_ids).unwrap_or_default();
        self.run(move |db| {
            let selected = selection(db, &thread)?;
            // A conversation with no agent belongs to the main agent and carries its skills and
            // groups. They are read now, not pinned, so a change reaches the next message.
            let main = if no_agent { crate::main_agent::read(db)? } else { Default::default() };
            let tracked = selected.revision > 0 || !project_groups.is_empty() || !agent_groups.is_empty()
                || !main.skill_ids.is_empty() || !main.skill_group_ids.is_empty();
            if !tracked { return Ok(None); }
            skills::resolve(db, &selected.skill_ids, true)
                .map_err(|e| if matches!(e, Issue::Missing) { Issue::SkillInactive } else { e })?;
            let mut effective = copies(db, &thread, &selected)?;
            // An always-on skill that is inactive or deleted is left out; it never blocks a message.
            for id in &main.skill_ids {
                match skills::resolve(db, &[*id], true) {
                    Ok(found) => effective.extend(found),
                    Err(Issue::Missing | Issue::SkillInactive) => {},
                    Err(error) => return Err(error),
                }
            }
            for id in main.skill_group_ids.iter().chain(&agent_groups).chain(&project_groups).chain(&selected.skill_group_ids) {
                let Some(group) = read(db, *id)? else { continue; };
                if !group.definition.active { continue; }
                for member in group.definition.members.iter().filter(|m| m.enabled) {
                    // A deleted member stops contributing; group activation ignores individual activation.
                    match skills::resolve(db, &[member.skill_id], false) {
                        Ok(copies) => effective.extend(copies),
                        Err(Issue::Missing) => {},
                        Err(error) => return Err(error),
                    }
                }
            }
            effective.retain(|skill| pinned.insert(skill.id));
            if effective.len() > 32 { return Err(Issue::Limit); }
            let mut text = String::from("Current MiniClaw conversation skills follow. These replace earlier \
                conversation skill selections; pinned agent and project instructions remain in effect.\n");
            for skill in effective {
                skill.definition.validate().map_err(|_| Issue::Storage)?;
                text.push_str(&format!("\n## Skill: {}\n\n{}\n", skill.definition.name,
                    skill.definition.instructions));
            }
            if text.len() > 131072 { return Err(Issue::Limit); }
            Ok(Some(text))
        }).await
    }
}

/// Pins explicit skills and references in the caller's binding or settings transaction.
pub(crate) fn write_selection(
    db: &Connection,
    thread: &str,
    value: &mut Selection,
) -> Result<(), Issue> {
    value.validate()?;
    let old = selection(db, thread)?;
    if old.revision != value.revision {
        return Err(Issue::Conflict);
    }
    check_refs(db, &value.skill_group_ids)?;
    let pinned = copies(db, thread, &old)?;
    let copies = skills::resolve(db, &value.skill_ids, true)?
        .into_iter()
        .map(|skill| {
            pinned
                .iter()
                .find(|old| old.id == skill.id)
                .cloned()
                .unwrap_or(skill)
        })
        .collect::<Vec<_>>();
    let count: i64 = db
        .query_row("SELECT count(*) FROM chat_skill_selections", [], |r| {
            r.get(0)
        })
        .map_err(Issue::storage)?;
    if old.revision == 0 && count >= 4096 {
        return Err(Issue::Limit);
    }
    value.revision = value.revision.checked_add(1).ok_or(Issue::Invalid)?;
    db.execute(
        "INSERT INTO chat_skill_selections (thread_id, selection, snapshots)
        VALUES (?1, ?2, ?3) ON CONFLICT(thread_id) DO UPDATE SET
        selection = excluded.selection, snapshots = excluded.snapshots",
        params![
            thread,
            serde_json::to_string(value).map_err(Issue::storage)?,
            serde_json::to_string(&copies).map_err(Issue::storage)?
        ],
    )
    .map_err(Issue::storage)?;
    Ok(())
}

/// Restores individually selected copies without changing their pinned revisions.
fn copies(db: &Connection, thread: &str, selected: &Selection) -> Result<Vec<Skill>, Issue> {
    let text: Option<String> = db
        .query_row(
            "SELECT snapshots FROM chat_skill_selections WHERE thread_id = ?1",
            [thread],
            |r| r.get(0),
        )
        .optional()
        .map_err(Issue::storage)?;
    let copies: Vec<Skill> =
        serde_json::from_str(text.as_deref().unwrap_or("[]")).map_err(Issue::storage)?;
    if copies.iter().map(|s| s.id).collect::<Vec<_>>() != selected.skill_ids {
        return Err(Issue::Storage);
    }
    for skill in &copies {
        skill.definition.validate().map_err(|_| Issue::Storage)?;
    }
    Ok(copies)
}

/// Rejects missing group references when new settings are saved.
pub fn check_refs(db: &Connection, ids: &[Uuid]) -> Result<(), Issue> {
    if ids.len() > 16 || ids.iter().collect::<HashSet<_>>().len() != ids.len() {
        return Err(Issue::Invalid);
    }
    for id in ids {
        read(db, *id)?.ok_or(Issue::Missing)?;
    }
    Ok(())
}

/// Reads and validates one group; deletion is distinct from storage failure.
fn read(db: &Connection, id: Uuid) -> Result<Option<Group>, Issue> {
    let text: Option<String> = db
        .query_row(
            "SELECT definition FROM skill_groups WHERE id = ?1",
            [id.to_string()],
            |r| r.get(0),
        )
        .optional()
        .map_err(Issue::storage)?;
    let Some(text) = text else {
        return Ok(None);
    };
    let definition: Definition = serde_json::from_str(&text).map_err(Issue::storage)?;
    definition.validate().map_err(|_| Issue::Storage)?;
    if definition.revision.is_none_or(|r| r == 0) {
        return Err(Issue::Storage);
    }
    Ok(Some(Group { id, definition }))
}

/// Reads validated selection metadata without flattening its activation sources.
fn selection(db: &Connection, thread: &str) -> Result<Selection, Issue> {
    let text: Option<String> = db
        .query_row(
            "SELECT selection FROM chat_skill_selections WHERE thread_id = ?1",
            [thread],
            |r| r.get(0),
        )
        .optional()
        .map_err(Issue::storage)?;
    let Some(text) = text else {
        return Ok(Selection::default());
    };
    let value: Selection = serde_json::from_str(&text).map_err(Issue::storage)?;
    value.validate().map_err(|_| Issue::Storage)?;
    if value.revision == 0 {
        return Err(Issue::Storage);
    }
    Ok(value)
}
