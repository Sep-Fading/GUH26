//! The VM side of file transfer. The worker offers two folders and nothing else: the
//! workspace, which can be read, and the Transfers folder, which can be read and written.
//! Paths are relative, never pass through a link, and a file appears under its name only
//! once its last piece has arrived.

// Shapes listings for the backend.
use serde_json::{Value, json};
// Opens files without following links and reads at an offset.
use std::{fs, io::{Read, Seek, SeekFrom, Write}, path::{Component, Path, PathBuf}};

/// Most entries one listing returns.
const ENTRIES: usize = 500;
/// Most bytes one read or write carries; a JSON line holds it comfortably as base64.
pub const PIECE: usize = 512 * 1024;
/// Free space that a transfer must leave on the volume.
const RESERVE: u64 = 1024 * 1024 * 1024;
const PARTIAL: &str = ".miniclaw-partial";

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Refusal {
    /// The folder is not offered, or the path leaves it, names a link or is malformed.
    Path,
    /// The destination exists and the user did not choose Replace.
    Exists,
    /// A piece arrived at the wrong offset.
    Sequence,
    /// The volume lacks room for the transfer.
    Space,
    /// The file system refused the operation.
    Failed,
}

impl Refusal {
    /// Names the refusal for the backend without a path or system message.
    pub fn code(self) -> &'static str {
        match self { Self::Path => "path", Self::Exists => "exists", Self::Sequence => "sequence",
            Self::Space => "space", Self::Failed => "failed" }
    }
}

#[derive(Debug)]
pub struct Piece { pub bytes: Vec<u8>, pub size: u64, pub modified: i64, pub end: bool }

pub struct Guest { folders: Vec<(String, PathBuf)> }

impl Guest {
    /// Offers the named folders. The first use of a folder creates it.
    pub fn new(folders: Vec<(String, PathBuf)>) -> Self { Self { folders } }

    /// The folders every VM offers.
    pub fn standard() -> Self {
        Self::new(vec![("workspace".into(), "/Users/Shared/MiniClaw/Workspace".into()),
            ("transfers".into(), "/Users/Shared/MiniClaw/Transfers".into())])
    }

    /// Joins a relative path to an offered folder, refusing anything but plain names and any
    /// existing component that is a link. `create` makes missing folders on the way.
    fn resolve(&self, folder: &str, relative: &str, create: bool) -> Result<PathBuf, Refusal> {
        let root = &self.folders.iter().find(|(name, _)| name == folder).ok_or(Refusal::Path)?.1;
        if relative.len() > 4096 || relative.contains('\0') || relative.starts_with('/') {
            return Err(Refusal::Path);
        }
        fs::create_dir_all(root).map_err(|_| Refusal::Failed)?;
        let mut path = root.clone();
        let names: Vec<_> = Path::new(relative).components().collect();
        if relative.split('/').any(|name| name == "." ) { return Err(Refusal::Path); }
        for (index, component) in names.iter().enumerate() {
            let Component::Normal(name) = component else { return Err(Refusal::Path) };
            path.push(name);
            match fs::symlink_metadata(&path) {
                Ok(found) if crate::platform_fs::is_link(&found) => return Err(Refusal::Path),
                Ok(found) if index + 1 < names.len() && !found.is_dir() => return Err(Refusal::Path),
                Ok(_) => {}
                Err(_) if create && index + 1 < names.len() =>
                    fs::create_dir(&path).map_err(|_| Refusal::Failed)?,
                Err(_) => {}
            }
        }
        Ok(path)
    }

    /// Lists files and folders by name. Links and partial copies are left out.
    pub fn list(&self, folder: &str, relative: &str) -> Result<Value, Refusal> {
        let path = self.resolve(folder, relative, false)?;
        let mut entries = Vec::new();
        for entry in fs::read_dir(&path).map_err(|_| Refusal::Path)?.flatten() {
            let (Ok(found), Some(name)) = (entry.metadata(), entry.file_name().to_str().map(str::to_owned))
                else { continue };
            if entry.file_type().is_ok_and(|kind| kind.is_symlink()) || name.ends_with(PARTIAL) { continue; }
            if !found.is_dir() && !found.is_file() { continue; }
            entries.push((name, found.is_dir(), if found.is_file() { found.len() } else { 0 }));
        }
        entries.sort();
        Ok(Value::Array(entries.into_iter().take(ENTRIES).map(|(name, folder, size)|
            json!({ "name": name, "kind": if folder { "folder" } else { "file" }, "size": size })).collect()))
    }

    /// Reports whether a file or folder already has this name.
    pub fn exists(&self, folder: &str, relative: &str) -> Result<bool, Refusal> {
        Ok(fs::symlink_metadata(self.resolve(folder, relative, false)?).is_ok())
    }

    /// Reads one piece of a regular file with its size and change time, so the backend can
    /// tell that a file changed while it was copied.
    pub fn read(&self, folder: &str, relative: &str, offset: u64, length: usize) -> Result<Piece, Refusal> {
        let path = self.resolve(folder, relative, false)?;
        let mut file = crate::platform_fs::open_read_nofollow(&path)
            .map_err(|_| Refusal::Path)?;
        let found = file.metadata().map_err(|_| Refusal::Failed)?;
        if !found.is_file() { return Err(Refusal::Path); }
        file.seek(SeekFrom::Start(offset)).map_err(|_| Refusal::Failed)?;
        let mut bytes = vec![0; length.min(PIECE)];
        let mut filled = 0;
        while filled < bytes.len() {
            match file.read(&mut bytes[filled..]).map_err(|_| Refusal::Failed)? { 0 => break, count => filled += count }
        }
        bytes.truncate(filled);
        Ok(Piece { end: offset + filled as u64 >= found.len(), bytes, size: found.len(), modified: (crate::platform_fs::modified_ns(&found).map_err(|_| Refusal::Failed)? / 1_000_000_000) as i64 })
    }

    /// Checks that the volume can take `bytes` and still keep its reserve.
    pub fn room(&self, folder: &str, bytes: u64) -> Result<(), Refusal> {
        let path = self.resolve(folder, "", false)?;
        let free = crate::platform_fs::free_space(&path).map_err(|_| Refusal::Failed)?;
        if bytes.saturating_add(RESERVE) > free { Err(Refusal::Space) } else { Ok(()) }
    }

    /// Appends one piece to the partial copy. The last piece gives the file its name; an
    /// existing file is kept unless `replace` is set. Only the Transfers folder is written.
    pub fn write(&self, folder: &str, relative: &str, offset: u64, bytes: &[u8], last: bool, replace: bool)
        -> Result<(), Refusal> {
        if folder != "transfers" || relative.is_empty() || bytes.len() > PIECE { return Err(Refusal::Path); }
        let target = self.resolve(folder, relative, true)?;
        if !replace && fs::symlink_metadata(&target).is_ok() { return Err(Refusal::Exists); }
        if fs::symlink_metadata(&target).is_ok_and(|found| found.is_dir()) { return Err(Refusal::Exists); }
        let partial = partial(&target);
        let mut file = if offset == 0 {
            let _ = fs::remove_file(&partial);
            crate::platform_fs::create_new(&partial, false)
        } else { crate::platform_fs::open_existing(&partial, true, false) }.map_err(|_| Refusal::Sequence)?;
        file.seek(SeekFrom::Start(offset)).map_err(|_| Refusal::Sequence)?;
        if file.metadata().map_err(|_| Refusal::Failed)?.len() != offset { return Err(Refusal::Sequence); }
        file.write_all(bytes).map_err(|_| Refusal::Failed)?;
        if last {
            file.sync_all().map_err(|_| Refusal::Failed)?;
            drop(file);
            fs::rename(&partial, &target).map_err(|_| Refusal::Failed)?;
        }
        Ok(())
    }

    /// Removes the partial copy of a cancelled or failed file.
    pub fn discard(&self, folder: &str, relative: &str) -> Result<(), Refusal> {
        if folder != "transfers" || relative.is_empty() { return Err(Refusal::Path); }
        let _ = fs::remove_file(partial(&self.resolve(folder, relative, false)?));
        Ok(())
    }
}

/// Names the partial copy beside its target.
fn partial(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_owned();
    name.push(PARTIAL);
    target.with_file_name(name)
}

/// Answers one `miniclaw/transfer/*` request from the backend.
pub fn handle(guest: &Guest, method: &str, params: &Value) -> Value {
    let folder = params["folder"].as_str().unwrap_or_default();
    let path = params["path"].as_str().unwrap_or_default();
    let result = match method {
        "list" => guest.list(folder, path).map(|entries| json!({ "entries": entries })),
        "exists" => guest.exists(folder, path).map(|exists| json!({ "exists": exists })),
        "room" => guest.room(folder, params["bytes"].as_u64().unwrap_or(u64::MAX)).map(|()| json!({})),
        "read" => guest.read(folder, path, params["offset"].as_u64().unwrap_or(0), PIECE).map(|piece|
            json!({ "data": crate::frames::encode(&piece.bytes), "size": piece.size,
                "modified": piece.modified, "end": piece.end })),
        "write" => crate::frames::decode(params["data"].as_str().unwrap_or_default()).ok_or(Refusal::Failed)
            .and_then(|bytes| guest.write(folder, path, params["offset"].as_u64().unwrap_or(u64::MAX),
                &bytes, params["last"] == true, params["replace"] == true)).map(|()| json!({})),
        "discard" => guest.discard(folder, path).map(|()| json!({})),
        _ => Err(Refusal::Path),
    };
    result.unwrap_or_else(|refusal| json!({ "refused": refusal.code() }))
}
