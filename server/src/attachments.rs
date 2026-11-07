//! Files and pictures the user adds to a message. Each upload lives on this Mac in a folder of
//! its conversation, beside a small record of its name, its kind and the message that carried it.
//! A picture goes to the model as a picture, a small text file as text, and anything else is
//! named to the model and, where the conversation has a working folder, copied into it.

// Shapes records for the page and disk.
use serde::{Deserialize, Serialize};
// Holds records grouped by their turn.
use std::collections::HashMap;
// Locates the store and its files.
use std::path::{Path, PathBuf};
// Sets the store once; orders changes.
use std::sync::{Mutex, OnceLock};
// Names uploads and their messages.
use uuid::Uuid;

/// Most uploads one message carries.
pub const PER_MESSAGE: usize = 10;
/// The largest picture, which is also the most a provider is sent before it is scaled down.
pub const IMAGE_BYTES: u64 = 20 * 1024 * 1024;
/// The largest text file that is read into the turn; a larger one is kept as a file.
pub const TEXT_BYTES: u64 = 256 * 1024;
/// The largest upload of any kind.
const FILE_BYTES: u64 = 500 * 1024 * 1024;
/// A picture above either bound gets a smaller copy for the model: Anthropic's vision guide
/// gives 2576 px as the longest edge its newest models use, and 10 MB per picture as base64.
const MODEL_EDGE: u32 = 2576;
const MODEL_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind { Image, Text, File }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub id: Uuid,
    pub name: String,
    pub kind: Kind,
    pub media_type: String,
    pub size: u64,
    /// The message that carried this upload; unset until it is sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Uuid>,
    /// The turn that message started, which is how a reloaded conversation finds it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    /// Where a copy was placed for the agent, relative to the working folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved: Option<String>,
    /// The Codex image item that produced this picture, for stable history reloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_item: Option<String>,
    /// The upload's place among those of its message.
    #[serde(default)]
    pub position: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Issue { Name, Empty, TooLarge, Missing, Sent, Count, Storage }

/// The message a tool's picture belongs to: none of the user's. It is never shown under a user
/// message (it has no turn), never discarded as an unsent upload, and can be handed to an agent.
pub const TOOL: Uuid = Uuid::nil();

#[derive(Default)]
pub struct Store { pub dir: OnceLock<PathBuf>, gate: Mutex<()> }

/// Returns the largest upload; a test build may lower it to reach the limit quickly.
pub fn file_bytes() -> u64 {
    #[cfg(debug_assertions)]
    if let Some(bytes) = std::env::var("MINICLAW_TEST_UPLOAD_BYTES").ok().and_then(|text| text.parse().ok()) { return bytes; }
    FILE_BYTES
}

/// Accepts a plain file name: no folders, no leading dot, no control characters, at most 255 bytes.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 255 && !name.starts_with('.') && name.trim() == name
        && !name.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
}

/// Names a picture's type from its first bytes. Markup pictures are left out on purpose: they
/// can carry script, so they are treated as text.
pub fn picture(head: &[u8]) -> Option<&'static str> {
    match head {
        [0x89, b'P', b'N', b'G', ..] => Some("image/png"),
        [0xff, 0xd8, 0xff, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("image/webp"),
        _ => None,
    }
}

/// Decides what an uploaded file is from its content, never from its name.
fn classify(path: &Path, size: u64) -> std::io::Result<(Kind, String)> {
    use std::io::Read;
    let mut head = [0u8; 16];
    let read = std::fs::File::open(path)?.read(&mut head)?;
    if let Some(media) = picture(&head[..read]) { return Ok((Kind::Image, media.into())); }
    if size <= TEXT_BYTES {
        let bytes = std::fs::read(path)?;
        if !bytes.contains(&0) && std::str::from_utf8(&bytes).is_ok() { return Ok((Kind::Text, "text/plain".into())); }
    }
    Ok((Kind::File, "application/octet-stream".into()))
}

impl Record {
    /// What the page is told about an upload.
    pub fn view(&self) -> serde_json::Value {
        serde_json::json!({ "id": self.id, "name": self.name, "kind": self.kind, "mediaType": self.media_type, "size": self.size })
    }
}

impl Store {
    /// Names a conversation's folder, refusing anything that is not a canonical UUID.
    fn thread(&self, thread: &str) -> Result<PathBuf, Issue> {
        let dir = self.dir.get().ok_or(Issue::Storage)?;
        (Uuid::parse_str(thread).ok().map(|id| id.to_string()).as_deref() == Some(thread))
            .then(|| dir.join(thread)).ok_or(Issue::Missing)
    }

    /// Returns where an upload still being received is written, inside its conversation's folder.
    pub fn incoming(&self, thread: &str) -> Result<PathBuf, Issue> {
        let folder = self.thread(thread)?;
        crate::platform_fs::private_dir(&folder, true).map_err(|_| Issue::Storage)?;
        Ok(folder.join(format!(".incoming-{}", Uuid::new_v4())))
    }

    /// Returns the bytes of one upload.
    pub fn content(&self, thread: &str, id: Uuid) -> Result<PathBuf, Issue> {
        Ok(self.thread(thread)?.join(id.to_string()).join("content"))
    }

    /// Reads one upload's record.
    pub fn get(&self, thread: &str, id: Uuid) -> Result<Record, Issue> {
        let text = std::fs::read(self.thread(thread)?.join(id.to_string()).join("record.json")).map_err(|_| Issue::Missing)?;
        serde_json::from_slice(&text).map_err(|_| Issue::Storage)
    }

    /// Writes one upload's record through a rename, for its owner only.
    pub fn put(&self, thread: &str, record: &Record) -> Result<(), Issue> {
        let folder = self.thread(thread)?.join(record.id.to_string());
        let staged = folder.join(".record");
        let mut file = crate::platform_fs::private_file(&staged, false).map_err(|_| Issue::Storage)?;
        std::io::Write::write_all(&mut file, &serde_json::to_vec(record).map_err(|_| Issue::Storage)?).map_err(|_| Issue::Storage)?;
        file.sync_all().map_err(|_| Issue::Storage)?;
        drop(file);
        std::fs::rename(staged, folder.join("record.json")).map_err(|_| Issue::Storage)
    }

    /// Takes a received file into the store and returns its record. The received file is
    /// removed when it is refused.
    pub fn accept(&self, thread: &str, name: &str, received: &Path, size: u64) -> Result<Record, Issue> {
        let done = (|| {
            if !valid_name(name) { return Err(Issue::Name); }
            if size == 0 { return Err(Issue::Empty); }
            let (kind, media_type) = classify(received, size).map_err(|_| Issue::Storage)?;
            if size > file_bytes() || (kind == Kind::Image && size > IMAGE_BYTES) { return Err(Issue::TooLarge); }
            let record = Record { id: Uuid::new_v4(), name: name.to_owned(), kind, media_type, size, message: None, turn: None, saved: None, source_item: None, position: 0 };
            let folder = self.thread(thread)?.join(record.id.to_string());
            crate::platform_fs::private_dir(&folder, false).map_err(|_| Issue::Storage)?;
            std::fs::rename(received, folder.join("content")).map_err(|_| Issue::Storage)?;
            self.put(thread, &record)?;
            Ok(record)
        })();
        if done.is_err() { let _ = std::fs::remove_file(received); }
        done
    }

    /// Keeps a picture a tool returned in the conversation's store, as the tool's, not the user's.
    pub fn keep(&self, thread: &str, name: &str, bytes: &[u8]) -> Result<Record, Issue> {
        let received = self.incoming(thread)?;
        let mut file = crate::platform_fs::private_file(&received, true).map_err(|_| Issue::Storage)?;
        if std::io::Write::write_all(&mut file, bytes).is_err() { let _ = std::fs::remove_file(&received); return Err(Issue::Storage); }
        let mut record = self.accept(thread, name, &received, bytes.len() as u64)?;
        record.message = Some(TOOL);
        self.put(thread, &record)?;
        Ok(record)
    }

    /// Keeps one Codex result once, keyed by its item ID; only bounded PNG data is accepted.
    pub fn generated(&self, thread: &str, item: &str, result: &str) -> Result<Record, Issue> {
        if item.is_empty() || item.len() > 128 || item.chars().any(char::is_control) { return Err(Issue::Name); }
        let _gate = self.gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(record) = self.all(thread).into_iter().find(|record| record.source_item.as_deref() == Some(item)) {
            return Ok(record);
        }
        let encoded = result.strip_prefix("data:image/png;base64,").unwrap_or(result);
        if encoded.len() > IMAGE_BYTES.div_ceil(3) as usize * 4 { return Err(Issue::TooLarge); }
        let bytes = crate::frames::decode(encoded).ok_or(Issue::Name)?;
        if bytes.len() as u64 > IMAGE_BYTES || picture(&bytes) != Some("image/png") { return Err(Issue::Name); }
        let mut record = self.keep(thread, "generated-image.png", &bytes)?;
        record.source_item = Some(item.to_owned());
        self.put(thread, &record)?;
        Ok(record)
    }

    /// Copies one of a conversation's pictures or files into another conversation as a new,
    /// unsent upload there, which that conversation's next message can carry.
    pub fn copy_to(&self, from: &str, id: Uuid, to: &str) -> Result<Record, Issue> {
        let record = self.get(from, id)?;
        let received = self.incoming(to)?;
        if std::fs::copy(self.content(from, id)?, &received).is_err() { let _ = std::fs::remove_file(&received); return Err(Issue::Storage); }
        self.accept(to, &record.name, &received, record.size)
    }

    /// Reads every record of a conversation.
    fn all(&self, thread: &str) -> Vec<Record> {
        let Ok(folder) = self.thread(thread) else { return Vec::new(); };
        std::fs::read_dir(folder).into_iter().flatten().flatten()
            .filter_map(|entry| Uuid::parse_str(entry.file_name().to_str()?).ok())
            .filter_map(|id| self.get(thread, id).ok()).collect()
    }

    /// Checks the uploads named by a message without changing them: each exists, is named once,
    /// and is unsent or already belongs to this same message.
    pub fn check(&self, thread: &str, ids: &[Uuid], message: Uuid) -> Result<Vec<Record>, Issue> {
        if ids.len() > PER_MESSAGE || ids.iter().enumerate().any(|(index, id)| ids[..index].contains(id)) { return Err(Issue::Count); }
        ids.iter().map(|id| match self.get(thread, *id)? {
            record if record.message.is_none_or(|owner| owner == message) => Ok(record),
            _ => Err(Issue::Sent),
        }).collect()
    }

    /// Ties uploads to the message that carries them, with where each was copied for the agent.
    pub fn claim(&self, thread: &str, records: &mut [Record], message: Uuid) -> Result<(), Issue> {
        let _gate = self.gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        for (position, record) in records.iter_mut().enumerate() {
            record.position = position as u32;
            record.message = Some(message);
            self.put(thread, record)?;
        }
        Ok(())
    }

    /// Notes the turn a message started, so its uploads are found again after a reload.
    pub fn started(&self, thread: &str, message: Uuid, turn: &str) {
        let _gate = self.gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        for mut record in self.all(thread).into_iter().filter(|record| record.message == Some(message)) {
            record.turn = Some(turn.to_owned());
            let _ = self.put(thread, &record);
        }
    }

    /// Groups a conversation's sent uploads by turn, in the order they were named.
    pub fn by_turn(&self, thread: &str) -> HashMap<String, Vec<serde_json::Value>> {
        let mut found: HashMap<String, Vec<Record>> = HashMap::new();
        for record in self.all(thread) {
            if let Some(turn) = record.turn.clone() { found.entry(turn).or_default().push(record); }
        }
        found.into_iter().map(|(turn, mut records)| {
            records.sort_by_key(|record| record.position);
            (turn, records.iter().map(Record::view).collect())
        }).collect()
    }

    /// Removes an upload that no message carries yet.
    pub fn discard(&self, thread: &str, id: Uuid) -> Result<(), Issue> {
        let _gate = self.gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.get(thread, id)?.message.is_some() { return Err(Issue::Sent); }
        std::fs::remove_dir_all(self.thread(thread)?.join(id.to_string())).map_err(|_| Issue::Storage)
    }

    /// Moves a deleted conversation's uploads to the Trash, where the user can still find them.
    pub fn trash(&self, thread: &str, trash: &Path) {
        let Ok(folder) = self.thread(thread) else { return; };
        if !folder.is_dir() { return; }
        #[cfg(windows)]
        if !windows_trash(trash) { tracing::warn!("Recovery storage could not be secured."); return; }
        #[cfg(unix)]
        let _ = std::fs::create_dir_all(trash);
        let target = trash.join(format!("MiniClaw uploads {} {}", &thread[..8], uuid::Uuid::new_v4()));
        if std::fs::rename(&folder, &target).is_err() { tracing::warn!("A conversation's uploads could not be moved to the Trash."); }
    }
}

/// Makes the Windows recovery folder ready. A missing one is created private. An existing one
/// must be a real folder, not a link; it need not be private itself, because the uploads folder
/// moved into it keeps its own protected owner-only access list (a rename on one volume carries
/// the security descriptor; checked natively 2026-10-04). Requiring a private folder refused
/// every folder the user or a test made, which left a deleted conversation's uploads behind.
#[cfg(windows)]
fn windows_trash(trash: &Path) -> bool {
    match std::fs::symlink_metadata(trash) {
        Ok(metadata) => metadata.is_dir() && !crate::platform_fs::is_link(&metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => crate::platform_fs::private_dir(trash, true).is_ok(),
        Err(_) => false,
    }
}

/// Copies an upload into `<root>/uploads` under its own name, or the next free one, and returns
/// the path relative to the root. The copy never replaces a file and never follows a link.
pub fn place(root: &Path, record: &Record, content: &Path) -> Option<String> {
    let folder = root.join("uploads");
    std::fs::create_dir_all(&folder).ok()?;
    if crate::platform_fs::is_link(&std::fs::symlink_metadata(&folder).ok()?) { return None; }
    let (stem, extension) = match record.name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (record.name.as_str(), String::new()),
    };
    for count in 1..1000 {
        let name = if count == 1 { record.name.clone() } else { format!("{stem} {count}{extension}") };
        let mut target = match crate::platform_fs::create_new(&folder.join(&name), false) {
            Ok(target) => target,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        };
        std::io::copy(&mut std::fs::File::open(content).ok()?, &mut target).ok()?;
        return Some(format!("uploads/{name}"));
    }
    None
}

/// Returns the picture a provider is given: the upload itself, or a smaller copy made with
/// macOS's own `sips` when the upload is larger than a model uses. The copy is made once.
#[cfg(unix)]
pub async fn for_model(content: &Path, record: &Record) -> PathBuf {
    let smaller = content.with_file_name("model.jpg");
    if smaller.is_file() { return smaller; }
    let edge = async {
        let output = tokio::process::Command::new("/usr/bin/sips").args(["-g", "pixelWidth", "-g", "pixelHeight"]).arg(content)
            .output().await.ok()?;
        String::from_utf8_lossy(&output.stdout).lines().filter_map(|line| line.rsplit(':').next()?.trim().parse::<u32>().ok()).max()
    };
    if record.size <= MODEL_BYTES && edge.await.is_none_or(|edge| edge <= MODEL_EDGE) { return content.to_owned(); }
    let made = tokio::process::Command::new("/usr/bin/sips").args(["-s", "format", "jpeg", "-Z", &MODEL_EDGE.to_string()])
        .arg(content).arg("--out").arg(&smaller).output().await;
    if made.is_ok_and(|output| output.status.success()) && smaller.is_file() { smaller } else { content.to_owned() }
}

/// Keeps the bounded original on Windows; no macOS image helper is launched.
#[cfg(windows)]
pub async fn for_model(content: &Path, _record: &Record) -> PathBuf {
    content.to_owned()
}

/// Writes what the model is told about this message's uploads: the text of each text file, where
/// each copy was placed, and which files it cannot open.
pub fn note(store: &Store, thread: &str, records: &[Record], pictures_reach: bool) -> Option<String> {
    let mut parts = Vec::new();
    for record in records {
        // The id is how the model names the file when it hands a task to another agent.
        let place = format!(" Its attachment id is {}.{}", record.id,
            record.saved.as_ref().map(|path| format!(" A copy is in the working folder at {path}.")).unwrap_or_default());
        match record.kind {
            Kind::Image if !pictures_reach => parts.push(format!(
                "Picture \"{}\" was attached, but MiniClaw cannot show pictures to a model in a virtual machine yet. Tell the user you cannot see it.", record.name)),
            Kind::Image => parts.push(format!("Picture \"{}\" is attached to this message.{place}", record.name)),
            Kind::Text => {
                let text = store.content(thread, record.id).ok().and_then(|path| std::fs::read_to_string(path).ok()).unwrap_or_default();
                parts.push(format!("File \"{}\" ({} bytes).{place} Its content:\n<attachment name=\"{}\">\n{text}\n</attachment>", record.name, record.size, record.name));
            }
            Kind::File if record.saved.is_none() => parts.push(format!(
                "File \"{}\" ({} bytes) was attached. MiniClaw keeps it, but this conversation has no working folder, so you cannot open it. Say so if the user expects you to read it.",
                record.name, record.size)),
            Kind::File => parts.push(format!("File \"{}\" ({} bytes) was attached. It is not text, so open it with a tool.{place}", record.name, record.size)),
        }
    }
    (!parts.is_empty()).then(|| format!("The user attached files to this message.\n\n{}", parts.join("\n\n")))
}

#[cfg(all(test, windows))]
mod windows_trash_tests {
    use super::*;

    /// Runs `trash` for one conversation whose uploads folder holds one file.
    fn trashed(trash: &Path) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("mc-attach-store-{}", Uuid::new_v4()));
        let store = Store::default();
        let _ = store.dir.set(base.join("attachments"));
        let thread = Uuid::new_v4().to_string();
        let folder = store.thread(&thread).unwrap();
        crate::platform_fs::private_dir(&folder, true).unwrap();
        std::fs::write(folder.join("upload.png"), b"picture").unwrap();
        store.trash(&thread, trash);
        (base, folder)
    }

    /// An existing folder the user made (inherited, not private) still receives the uploads: the
    /// moved folder keeps its own protected owner-only access list (native check, 2026-10-04).
    #[test]
    fn uploads_reach_an_existing_trash_folder() {
        let trash = std::env::temp_dir().join(format!("mc-attach-trash-{}", Uuid::new_v4()));
        std::fs::create_dir(&trash).unwrap();
        let (base, folder) = trashed(&trash);
        assert!(!folder.exists(), "the uploads left the conversation");
        let moved: Vec<_> = std::fs::read_dir(&trash).unwrap().flatten().collect();
        assert_eq!(moved.len(), 1);
        assert_eq!(std::fs::read(moved[0].path().join("upload.png")).unwrap(), b"picture");
        let _ = std::fs::remove_dir_all(base);
        let _ = std::fs::remove_dir_all(trash);
    }

    /// A missing trash folder is created private; a file in its place receives nothing.
    #[test]
    fn a_missing_trash_is_created_and_a_file_is_refused() {
        let root = std::env::temp_dir().join(format!("mc-attach-trash-root-{}", Uuid::new_v4()));
        crate::platform_fs::private_dir(&root, false).unwrap();
        let (base, folder) = trashed(&root.join("Recovered files"));
        assert!(!folder.exists());
        assert_eq!(std::fs::read_dir(root.join("Recovered files")).unwrap().count(), 1);
        let file = root.join("not a folder");
        std::fs::write(&file, b"x").unwrap();
        let (other, kept) = trashed(&file);
        assert!(kept.join("upload.png").exists(), "nothing moves into a file");
        for path in [base, other, root] { let _ = std::fs::remove_dir_all(path); }
    }
}
