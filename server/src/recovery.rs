// Names recovery records and reports storage failures.
use std::{io, path::Path};
// Persists recovery markers without blocking request handling.
use tokio::fs;
// Rejects filenames that are not task identifiers.
use uuid::Uuid;

/// Records the task ID so a crash between archive and unarchive can be recovered.
/// Stores no messages or credentials.
pub async fn mark(directory: &Path, id: &str) -> io::Result<()> {
    Uuid::parse_str(id).map_err(io::Error::other)?;
    crate::platform_fs::private_dir(directory, true)?;
    let file = crate::platform_fs::private_lock_file(&directory.join(id))?;
    file.sync_all()?;
    crate::platform_fs::sync_parent(&directory.join(id))
}

/// Removes a recovery record after restoration, accepting an already absent record.
pub async fn clear(directory: &Path, id: &str) -> io::Result<()> {
    match fs::remove_file(directory.join(id)).await {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(()) => crate::platform_fs::sync_parent(&directory.join(id)),
    }
}

/// Returns at most 32 recorded task IDs, ignoring unrelated files.
pub async fn pending(directory: &Path) -> io::Result<Vec<String>> {
    let mut entries = match fs::read_dir(directory).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut ids = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let id = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().await?.is_file() && Uuid::parse_str(&id).is_ok() {
            if ids.len() >= 32 {
                return Err(io::Error::other("Too many pending computer recoveries"));
            }
            ids.push(id);
        }
    }
    Ok(ids)
}
