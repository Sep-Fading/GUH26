// Uses the existing storage failure category.
use super::Issue;
// Rejects symlink and special-file marker reads.
use crate::platform_fs;
// Reads identity and validates retained tables.
use rusqlite::Connection;
// Reads bounded markers and persists private files.
use std::{
    io::{Read, Write},
    path::Path,
};
// Assigns an identity to each database.
use uuid::Uuid;

/// Reads a private identity marker; absent markers are valid only before upgrade.
pub(super) fn read(path: &Path) -> Result<Option<String>, Issue> {
    let file = match platform_fs::open_read_nofollow(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Issue::storage(error)),
    };
    let metadata = file.metadata().map_err(Issue::storage)?;
    if !platform_fs::private_regular_file(&file).map_err(Issue::storage)?
        || metadata.len() != 36
    {
        return Err(Issue::Storage);
    }
    let mut value = String::new();
    file.take(37)
        .read_to_string(&mut value)
        .map_err(Issue::storage)?;
    Uuid::parse_str(&value).map_err(Issue::storage)?;
    Ok(Some(value))
}

/// Checks identity and existing policy tables before any schema creation.
pub(super) fn verify(db: &Connection, marker: Option<&str>, created: bool) -> Result<(), Issue> {
    let sealed = db
        .table_exists(Some("main"), "miniclaw_identity")
        .map_err(Issue::storage)?;
    if sealed {
        let id: String = db
            .query_row(
                "SELECT id FROM miniclaw_identity WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(Issue::storage)?;
        if marker != Some(id.as_str()) {
            return Err(Issue::Storage);
        }
        for name in [
            "agents",
            "agent_chats",
            "skills",
            "agent_chat_skills",
            "removed",
            "projects",
            "skill_groups",
            "chat_skill_selections",
            "project_chats",
        ] {
            if !db
                .table_exists(Some("main"), name)
                .map_err(Issue::storage)?
            {
                return Err(Issue::Storage);
            }
        }
    } else {
        if marker.is_some() {
            return Err(Issue::Storage);
        }
        if !created {
            for name in ["agents", "agent_chats"] {
                if !db
                    .table_exists(Some("main"), name)
                    .map_err(Issue::storage)?
                {
                    return Err(Issue::Storage);
                }
            }
        }
    }
    Ok(())
}

/// Seals a new or legacy database; interrupted writes require explicit recovery.
pub(super) fn seal(db: &mut Connection, path: &Path) -> Result<(), Issue> {
    let id = Uuid::new_v4().to_string();
    let transaction = db.transaction().map_err(Issue::storage)?;
    transaction
        .execute_batch(
            "CREATE TABLE miniclaw_identity (
        singleton INTEGER PRIMARY KEY CHECK(singleton = 1), id TEXT NOT NULL
    )",
        )
        .map_err(Issue::storage)?;
    transaction
        .execute("INSERT INTO miniclaw_identity VALUES (1, ?1)", [&id])
        .map_err(Issue::storage)?;
    transaction.commit().map_err(Issue::storage)?;
    let mut file = platform_fs::private_file(path, true)
        .map_err(Issue::storage)?;
    file.write_all(id.as_bytes()).map_err(Issue::storage)?;
    file.sync_all().map_err(Issue::storage)?;
    platform_fs::sync_parent(path).map_err(Issue::storage)?;
    Ok(())
}
