// Shares private storage and validation failures.
use crate::agents::{Issue, Store};
// Persists skill revisions within existing transactions.
use rusqlite::{Connection, OptionalExtension, params};
// Opens anchored paths without following symlinks.
#[cfg(unix)]
use rustix::fs::{Mode, OFlags, openat};
// Serializes bounded instruction documents and snapshots.
use serde::{Deserialize, Serialize};
// Reads bounded local files without executing them.
use std::{fs::File, io::Read, path::PathBuf};
// Moves directory scans off asynchronous workers.
use tokio::task::spawn_blocking;
// Uses opaque identifiers for owned copies.
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub instructions: String,
    #[serde(default)]
    pub active: bool,
    /// Callable from the composer as `/its-name`, for one turn, without attaching it.
    #[serde(default)]
    pub slash: bool,
    pub revision: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Skill {
    pub id: Uuid,
    #[serde(flatten)]
    pub definition: Definition,
}

impl Definition {
    /// Rejects empty, oversized, or control-bearing metadata before storage.
    pub fn validate(&self) -> Result<(), Issue> {
        if self.name.trim().is_empty()
            || self.name.len() > 80
            || self.name.chars().any(char::is_control)
            || self.description.len() > 500
            || self.description.contains('\0')
            || self.instructions.trim().is_empty()
            || self.instructions.len() > 16384
            || self.instructions.contains('\0')
        {
            return Err(Issue::Invalid);
        }
        Ok(())
    }
}

impl Store {
    /// Returns owned skill copies; installed Codex skills are not included automatically.
    pub async fn skills(&self) -> Result<Vec<Skill>, Issue> {
        self.run(|db| {
            let mut query = db
                .prepare("SELECT id, definition FROM skills ORDER BY rowid LIMIT 129")
                .map_err(Issue::storage)?;
            let rows = query
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(Issue::storage)?;
            let mut skills = Vec::new();
            for row in rows {
                let (id, text) = row.map_err(Issue::storage)?;
                skills.push(decode(&id, &text)?);
            }
            if skills.len() > 128 {
                return Err(Issue::Limit);
            }
            Ok(skills)
        })
        .await
    }

    /// Creates or updates an owned copy after checking its revision atomically.
    pub async fn save_skill(
        &self,
        id: Option<Uuid>,
        mut definition: Definition,
    ) -> Result<Skill, Issue> {
        definition.validate()?;
        self.run(move |db| {
            let tx = db.transaction().map_err(Issue::storage)?;
            let id = if let Some(id) = id {
                let old = read(&tx, id)?;
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
                    .query_row("SELECT count(*) FROM skills", [], |r| r.get(0))
                    .map_err(Issue::storage)?;
                if count >= 128 {
                    return Err(Issue::Limit);
                }
                definition.revision = Some(1);
                Uuid::new_v4()
            };
            tx.execute(
                "INSERT INTO skills (id, definition) VALUES (?1, ?2)
                ON CONFLICT(id) DO UPDATE SET definition = excluded.definition",
                params![
                    id.to_string(),
                    serde_json::to_string(&definition).map_err(Issue::storage)?
                ],
            )
            .map_err(Issue::storage)?;
            tx.commit().map_err(Issue::storage)?;
            Ok(Skill { id, definition })
        })
        .await
    }
}

/// Resolves selected copies in profile order; inactive skills cannot authorize new work.
pub fn resolve(db: &Connection, ids: &[Uuid], active: bool) -> Result<Vec<Skill>, Issue> {
    ids.iter()
        .map(|id| {
            let skill = read(db, *id)?;
            if active && !skill.definition.active {
                return Err(Issue::SkillInactive);
            }
            Ok(skill)
        })
        .collect()
}

/// Reads a stored copy by identifier without falling back to filesystem content.
fn read(db: &Connection, id: Uuid) -> Result<Skill, Issue> {
    let text: String = db
        .query_row(
            "SELECT definition FROM skills WHERE id = ?1",
            [id.to_string()],
            |r| r.get(0),
        )
        .optional()
        .map_err(Issue::storage)?
        .ok_or(Issue::Missing)?;
    decode(&id.to_string(), &text)
}

/// Checks stored skill data before it enters a response or agent context.
fn decode(id: &str, text: &str) -> Result<Skill, Issue> {
    let definition: Definition = serde_json::from_str(text).map_err(Issue::storage)?;
    definition.validate().map_err(|_| Issue::Storage)?;
    if definition.revision.is_none_or(|r| r == 0) {
        return Err(Issue::Storage);
    }
    Ok(Skill {
        id: Uuid::parse_str(id).map_err(Issue::storage)?,
        definition,
    })
}

/// Where installed skills are imported from. Both keep one folder per skill with a `SKILL.md`,
/// so a skill from either serves conversations of either provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Provider { #[default] Codex, Claude }

/// Returns the provider's configured home, without exposing its path to the browser. Only its
/// `skills/<name>/SKILL.md` files are ever opened; sign-ins beside them are never read.
fn home(provider: Provider) -> Result<PathBuf, Issue> {
    let (variable, folder) = match provider {
        Provider::Codex => ("CODEX_HOME", ".codex"),
        Provider::Claude => ("CLAUDE_CONFIG_DIR", ".claude"),
    };
    std::env::var_os(variable)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME")
            .or_else(|| cfg!(windows).then(|| std::env::var_os("USERPROFILE")).flatten())
            .map(|p| PathBuf::from(p).join(folder)))
        .ok_or(Issue::Storage)
}

/// Accepts one visible folder name; paths and hidden system directories are excluded.
fn source_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 80
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// Opens each path segment relative to its parent, refusing symlinks and special files.
fn source_file(provider: Provider, name: &str) -> Result<File, Issue> {
    source_file_from(home(provider)?, name)
}

/// Reads one bounded source through the platform's no-follow file primitives.
fn source_file_from(provider_home: PathBuf, name: &str) -> Result<File, Issue> {
    if !source_name(name) {
        return Err(Issue::Invalid);
    }
    #[cfg(unix)]
    let file = {
        let home = File::open(provider_home).map_err(|_| Issue::Missing)?;
        let directory = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let root = openat(&home, "skills", directory, Mode::empty()).map_err(|_| Issue::Missing)?;
        let folder = openat(&root, name, directory, Mode::empty()).map_err(|_| Issue::Missing)?;
        let fd = openat(
            &folder,
            "SKILL.md",
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| Issue::Missing)?;
        File::from(fd)
    };
    #[cfg(windows)]
    let file = crate::platform_fs::open_read_nofollow(&provider_home.join("skills").join(name).join("SKILL.md"))
        .map_err(|_| Issue::Missing)?;
    let metadata = file.metadata().map_err(Issue::storage)?;
    if !metadata.is_file() || metadata.len() > 16384 {
        return Err(Issue::Invalid);
    }
    Ok(file)
}

#[derive(Serialize)]
pub struct Source {
    id: String,
    name: String,
}

/// Lists bounded import candidates; no instructions are read or copied by listing.
pub async fn sources(provider: Provider) -> Result<Vec<Source>, Issue> {
    spawn_blocking(move || {
        let entries = match std::fs::read_dir(home(provider)?.join("skills")) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(Issue::Storage),
        };
        let mut sources = Vec::new();
        for (index, entry) in entries.enumerate() {
            if index >= 1024 {
                return Err(Issue::Limit);
            }
            let name = entry
                .map_err(Issue::storage)?
                .file_name()
                .to_string_lossy()
                .into_owned();
            if source_name(&name) && source_file(provider, &name).is_ok() {
                sources.push(Source {
                    id: name.clone(),
                    name,
                });
            }
        }
        sources.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(sources)
    })
    .await
    .map_err(Issue::storage)?
}

/// Copies bounded UTF-8 Markdown verbatim; frontmatter stays text and files never execute.
pub async fn import(provider: Provider, source: String) -> Result<Definition, Issue> {
    spawn_blocking(move || {
        let mut instructions = String::new();
        source_file(provider, &source)?
            .take(16385)
            .read_to_string(&mut instructions)
            .map_err(|_| Issue::Invalid)?;
        let definition = Definition {
            name: source,
            description: String::new(),
            instructions,
            active: false,
            slash: false,
            revision: None,
        };
        definition.validate()?;
        Ok(definition)
    })
    .await
    .map_err(Issue::storage)?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Imports bounded text from one provider folder without activating the copy.
    #[test]
    fn source_reads_one_skill() {
        let root = std::env::temp_dir().join(format!("mc skill-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join("skills/example")).unwrap();
        std::fs::write(root.join("skills/example/SKILL.md"), "# Example\nInstructions").unwrap();
        let mut text = String::new();
        source_file_from(root.clone(), "example").unwrap().read_to_string(&mut text).unwrap();
        assert_eq!(text, "# Example\nInstructions");
        assert!(source_file_from(root.clone(), "../example").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A linked skill cannot import data through an alternate filesystem name.
    #[cfg(any(unix, windows))]
    #[test]
    fn source_refuses_link() {
        let root = std::env::temp_dir().join(format!("mc-skill-link-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join("skills/example")).unwrap();
        std::fs::write(root.join("private"), "private fixture").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("private"), root.join("skills/example/SKILL.md")).unwrap();
        #[cfg(windows)]
        std::fs::hard_link(root.join("private"), root.join("skills/example/SKILL.md")).unwrap();
        assert!(source_file_from(root.clone(), "example").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
