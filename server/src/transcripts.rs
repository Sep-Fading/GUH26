//! Keeps a private copy of each VM chat's messages on the Mac, so a chat can be read
//! while its VM is off. The VM's Codex stays the source of truth; this is a display copy.

// Builds and checks the stored document.
use serde_json::{Value, json};
// Writes private files through a rename.
use std::{fs, io::Write, path::{Path, PathBuf}};
// Orders saves and deletes and remembers deleted chats.
use std::{collections::HashSet, sync::{Mutex, OnceLock}};

/// The newest messages kept for one chat.
const KEPT: usize = 400;

/// Chats deleted in this run. Saves run on background threads, so one queued before a
/// delete may only start after it; chat ids are never reused, so the set only grows.
fn removed() -> &'static Mutex<HashSet<String>> {
    static REMOVED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    REMOVED.get_or_init(Default::default)
}

/// Names the copy for one chat, refusing anything that is not a canonical UUID.
fn file(dir: &Path, id: &str) -> Option<PathBuf> {
    (uuid::Uuid::parse_str(id).ok()?.to_string() == id).then(|| dir.join(format!("{id}.json")))
}

/// Stores the chat's summary and its newest messages in a file only this user can read.
pub fn save(dir: &Path, id: &str, thread: &Value, messages: &Value) -> std::io::Result<()> {
    let path = file(dir, id).ok_or(std::io::ErrorKind::InvalidInput)?;
    // The lock is held until the rename, so saves and deletes of copies never interleave.
    let removed = removed().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if removed.contains(id) { return Ok(()); }
    crate::platform_fs::private_dir(dir, true)?;
    let all = messages.as_array().map(Vec::as_slice).unwrap_or_default();
    let kept = &all[all.len().saturating_sub(KEPT)..];
    // Another MiniClaw process may save too, so the temporary name is unique per save.
    let staged = dir.join(format!(".{id}.{}", uuid::Uuid::new_v4()));
    let mut output = crate::platform_fs::private_file(&staged, true)?;
    output.write_all(json!({ "thread": thread, "messages": kept }).to_string().as_bytes())?;
    output.sync_all()?;
    drop(output);
    fs::rename(staged, path)
}

/// Reads a stored copy, returning its summary and messages.
pub fn load(dir: &Path, id: &str) -> Option<(Value, Value)> {
    let text = fs::read_to_string(file(dir, id)?).ok()?;
    let mut value: Value = serde_json::from_str(&text).ok()?;
    if !value["thread"].is_object() || !value["messages"].is_array() { return None; }
    Some((value["thread"].take(), value["messages"].take()))
}

/// Deletes the copy of a removed chat.
pub fn remove(dir: &Path, id: &str) {
    let Some(path) = file(dir, id) else { return };
    let mut removed = removed().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    removed.insert(id.to_owned());
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    const ID: &str = "019a7777-7777-7777-8777-777777777777";

    /// Creates an empty private folder that the test removes again.
    struct Folder(PathBuf);
    impl Folder {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("mc-transcripts-{}", uuid::Uuid::new_v4()));
            // The store is a private folder; Windows refuses to adopt one that inherited a shared
            // access list, so there it is created the way the backend creates its data folder.
            #[cfg(windows)]
            crate::platform_fs::private_dir(&path, true).unwrap();
            #[cfg(not(windows))]
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path { &self.0 }
    }
    impl Drop for Folder {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
    }

    #[test]
    fn a_copy_is_private_and_reads_back() {
        let dir = Folder::new();
        let store = dir.path().join("vm-transcripts");
        save(&store, ID, &json!({"id": ID, "name": "Task"}), &json!([{"text": "hello"}])).unwrap();
        #[cfg(unix)]
        let mode = fs::metadata(store.join(format!("{ID}.json"))).unwrap().permissions().mode();
        #[cfg(unix)]
        assert_eq!(mode & 0o777, 0o600);
        let (thread, messages) = load(&store, ID).unwrap();
        assert_eq!(thread["name"], "Task");
        assert_eq!(messages[0]["text"], "hello");
        let gone = "019a7777-7777-7777-8777-77777777bbbb";
        save(&store, gone, &json!({"id": gone}), &json!([])).unwrap();
        remove(&store, gone);
        assert!(load(&store, gone).is_none());
    }

    #[test]
    fn only_the_newest_messages_are_kept() {
        let dir = Folder::new();
        let messages: Vec<Value> = (0..KEPT + 5).map(|n| json!({"n": n})).collect();
        save(dir.path(), ID, &json!({}), &json!(messages)).unwrap();
        let (_, kept) = load(dir.path(), ID).unwrap();
        assert_eq!(kept.as_array().unwrap().len(), KEPT);
        assert_eq!(kept[0]["n"], 5);
    }

    #[test]
    fn saves_of_one_chat_at_the_same_time_leave_one_whole_copy() {
        let dir = Folder::new();
        let path = dir.path().to_path_buf();
        let writers: Vec<_> = (0..8).map(|n| {
            let path = path.clone();
            std::thread::spawn(move || {
                let messages: Vec<Value> = (0..200 + n * 40).map(|i| json!({"text": format!("{n}-{i}")})).collect();
                for _ in 0..20 { save(&path, ID, &json!({"id": ID}), &json!(messages)).unwrap(); }
            })
        }).collect();
        for writer in writers { writer.join().unwrap(); }
        assert!(load(dir.path(), ID).is_some(), "the stored copy must stay valid JSON");
        let leftovers = fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(leftovers, 1, "no temporary files remain");
    }

    #[test]
    fn a_save_that_lands_after_the_delete_does_not_bring_the_copy_back() {
        let dir = Folder::new();
        let late = "019a7777-7777-7777-8777-77777777aaaa";
        save(dir.path(), late, &json!({"id": late}), &json!([])).unwrap();
        remove(dir.path(), late);
        // A save queued before the delete may only run now.
        let _ = save(dir.path(), late, &json!({"id": late}), &json!([{"text": "late"}]));
        assert!(load(dir.path(), late).is_none());
    }

    #[test]
    fn a_name_that_is_not_a_chat_id_is_refused() {
        let dir = Folder::new();
        assert!(save(dir.path(), "../escape", &json!({}), &json!([])).is_err());
        assert!(load(dir.path(), "../escape").is_none());
    }
}
