//! Transfers between this Mac and a VM's two offered folders. A transfer copies whole files in
//! pieces; a file gets its name only when its last piece arrived, so a cancelled, failed or
//! interrupted copy is never mistaken for a finished one. Links are never copied.

// Talks to the VM worker, or to a local stand-in under test.
use crate::{rpc::Client, transfer::{Guest, PIECE}};
// Builds requests and the view the page polls.
use serde_json::{Value, json};
// Walks and writes Mac files without following links.
use std::{collections::HashMap, fs, io::{Read, Write}, path::{Path, PathBuf}, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}};
// Wakes a job that waits for the user's answer.
use tokio::sync::Notify;
// Names jobs and selections.
use uuid::Uuid;

/// Most files one transfer may hold, and how deep a folder may nest.
const FILES: usize = 10_000;
const DEPTH: usize = 32;
const PARTIAL: &str = ".miniclaw-partial";

#[derive(Clone)]
pub enum Remote { Worker(Client), Local(Arc<Guest>) }

impl Remote {
    /// Sends one transfer request. A refusal or a lost worker is an error with a short code.
    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let reply = match self {
            Self::Worker(client) => client.call(&format!("miniclaw/transfer/{method}"), params).await
                .map_err(|_| "disconnected".to_owned())?,
            Self::Local(guest) => crate::transfer::handle(guest, method, &params),
        };
        match reply["refused"].as_str() { Some(code) => Err(code.to_owned()), None => Ok(reply) }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Choice { Replace, KeepBoth, Skip }

#[derive(Clone, Debug)]
pub struct Source { pub folder: String, pub path: String }

#[derive(Default)]
struct Progress {
    state: &'static str, total_files: u64, done_files: u64, total_bytes: u64, done_bytes: u64,
    skipped: u64, current: String, conflict: Option<String>, error: Option<&'static str>,
    answer: Option<Choice>, sticky: Option<Choice>,
}

struct Job { progress: Mutex<Progress>, cancelled: AtomicBool, wake: Notify }

#[derive(Default)]
pub struct Transfers { jobs: Mutex<HashMap<Uuid, Arc<Job>>> }

/// One file to copy: where it is, and its path relative to the destination.
struct Item { mac: PathBuf, remote: Source, relative: String, size: u64 }

impl Transfers {
    /// Registers a job and runs it in the background. Finished jobs beyond 32 are dropped.
    fn start<F, Fut>(&self, run: F) -> Uuid
    where F: FnOnce(Arc<Job>) -> Fut + Send + 'static, Fut: Future<Output = Result<(), &'static str>> + Send {
        let job = Arc::new(Job { progress: Mutex::new(Progress { state: "running", ..Progress::default() }),
            cancelled: AtomicBool::new(false), wake: Notify::new() });
        let id = Uuid::new_v4();
        let mut jobs = self.jobs.lock().unwrap_or_else(|error| error.into_inner());
        if jobs.len() >= 32 { jobs.retain(|_, job| matches!(job.state(), "running" | "conflict")); }
        jobs.insert(id, job.clone());
        tokio::spawn(async move {
            let result = run(job.clone()).await;
            let mut progress = job.progress.lock().unwrap_or_else(|error| error.into_inner());
            progress.conflict = None;
            (progress.state, progress.error) = match result {
                Ok(()) => ("done", None),
                Err("cancelled") => ("cancelled", None),
                Err(reason) => ("failed", Some(reason)),
            };
        });
        id
    }

    /// Copies chosen Mac files and folders into the VM's Transfers folder under `day`.
    pub fn to_vm(&self, remote: Remote, sources: Vec<PathBuf>, day: String) -> Uuid {
        self.start(move |job| async move {
            let (items, skipped) = tokio::task::spawn_blocking(move || plan_mac(&sources, &day)).await
                .map_err(|_| "The files could not be read.")??;
            job.begin(&items, skipped);
            remote.call("room", json!({ "folder": "transfers", "bytes": items.iter().map(|item| item.size).sum::<u64>() }))
                .await.map_err(|code| explain(&code))?;
            for item in items { job.copy(&remote, item, true).await?; }
            Ok(())
        })
    }

    /// Copies chosen VM files and folders into a Mac folder the user picked.
    pub fn to_mac(&self, remote: Remote, sources: Vec<Source>, destination: PathBuf) -> Uuid {
        self.start(move |job| async move {
            let mut items = Vec::new();
            for source in &sources {
                let name = source.path.rsplit('/').next().unwrap_or_default().to_owned();
                if name.is_empty() { return Err("Choose a file or folder inside the offered folders."); }
                plan_vm(&remote, source, &name, &destination, 0, &mut items).await?;
            }
            job.begin(&items, 0);
            for item in items { job.copy(&remote, item, false).await?; }
            Ok(())
        })
    }

    /// Copies VM paths so that each lands directly in `destination`: a folder's contents, not the
    /// folder. An empty path is the whole offered folder. Backups use this.
    pub fn to_mac_into(&self, remote: Remote, sources: Vec<Source>, destination: PathBuf) -> Uuid {
        self.start(move |job| async move {
            let mut items = Vec::new();
            for source in &sources {
                let listing = remote.call("list", json!({ "folder": source.folder, "path": source.path })).await
                    .map_err(|code| explain(&code))?;
                let under = if source.path.is_empty() { destination.clone() } else { destination.join(&source.path) };
                for entry in listing["entries"].as_array().into_iter().flatten() {
                    let Some(name) = entry["name"].as_str().filter(|name| !name.contains('/')) else { continue };
                    let child = Source { folder: source.folder.clone(),
                        path: if source.path.is_empty() { name.to_owned() } else { format!("{}/{name}", source.path) } };
                    plan_vm(&remote, &child, name, &under, 0, &mut items).await?;
                }
            }
            job.begin(&items, 0);
            for item in items { job.copy(&remote, item, false).await?; }
            Ok(())
        })
    }

    /// Returns what the page shows for one job.
    pub fn view(&self, id: Uuid) -> Option<Value> {
        let job = self.jobs.lock().unwrap_or_else(|error| error.into_inner()).get(&id)?.clone();
        let progress = job.progress.lock().unwrap_or_else(|error| error.into_inner());
        Some(json!({ "state": progress.state, "totalFiles": progress.total_files, "doneFiles": progress.done_files,
            "totalBytes": progress.total_bytes, "doneBytes": progress.done_bytes, "skipped": progress.skipped,
            "current": progress.current, "conflict": progress.conflict, "error": progress.error }))
    }

    /// Answers the open name conflict; `all` answers the rest of this transfer the same way.
    pub fn resolve(&self, id: Uuid, choice: Choice, all: bool) -> bool {
        let Some(job) = self.jobs.lock().unwrap_or_else(|error| error.into_inner()).get(&id).cloned() else { return false };
        let mut progress = job.progress.lock().unwrap_or_else(|error| error.into_inner());
        if progress.state != "conflict" { return false; }
        progress.answer = Some(choice);
        if all { progress.sticky = Some(choice); }
        progress.state = "running";
        progress.conflict = None;
        drop(progress);
        job.wake.notify_one();
        true
    }

    /// Asks a job to stop. It stops between pieces and removes its partial file.
    pub fn cancel(&self, id: Uuid) -> bool {
        let Some(job) = self.jobs.lock().unwrap_or_else(|error| error.into_inner()).get(&id).cloned() else { return false };
        job.cancelled.store(true, Ordering::Release);
        job.wake.notify_one();
        true
    }
}

impl Job {
    fn state(&self) -> &'static str { self.progress.lock().unwrap_or_else(|error| error.into_inner()).state }

    /// Records the size of the work before the first byte moves.
    fn begin(&self, items: &[Item], skipped: u64) {
        let mut progress = self.progress.lock().unwrap_or_else(|error| error.into_inner());
        progress.total_files = items.len() as u64;
        progress.total_bytes = items.iter().map(|item| item.size).sum();
        progress.skipped = skipped;
    }

    /// Waits for the user's answer to one name conflict, unless "apply to all" already gave it.
    async fn ask(&self, name: &str) -> Result<Choice, &'static str> {
        {
            let mut progress = self.progress.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(choice) = progress.sticky { return Ok(choice); }
            progress.state = "conflict";
            progress.conflict = Some(name.to_owned());
            progress.answer = None;
        }
        loop {
            self.wake.notified().await;
            if self.cancelled.load(Ordering::Acquire) { return Err("cancelled"); }
            if let Some(choice) = self.progress.lock().unwrap_or_else(|error| error.into_inner()).answer.take() {
                return Ok(choice);
            }
        }
    }

    /// Copies one file in pieces, in either direction, after settling a name conflict.
    async fn copy(&self, remote: &Remote, mut item: Item, to_vm: bool) -> Result<(), &'static str> {
        if self.cancelled.load(Ordering::Acquire) { return Err("cancelled"); }
        self.progress.lock().unwrap_or_else(|error| error.into_inner()).current = item.relative.clone();
        let taken = |item: &Item| { let (remote, item_remote, mac) = (remote.clone(), item.remote.clone(), item.mac.clone());
            async move { if to_vm {
                remote.call("exists", json!({ "folder": item_remote.folder, "path": item_remote.path })).await
                    .map(|reply| reply["exists"] == true).map_err(|code| explain(&code))
            } else { Ok(fs::symlink_metadata(&mac).is_ok()) } } };
        let mut replace = false;
        if taken(&item).await? {
            match self.ask(&item.relative).await? {
                Choice::Replace => replace = true,
                Choice::Skip => {
                    let mut progress = self.progress.lock().unwrap_or_else(|error| error.into_inner());
                    progress.skipped += 1; progress.total_files -= 1; progress.total_bytes -= item.size;
                    return Ok(());
                }
                Choice::KeepBoth => {
                    let mut number = 2;
                    loop {
                        let candidate = numbered(&item, number, to_vm);
                        if !taken(&candidate).await? { item = candidate; break; }
                        number += 1;
                        if number > 1000 { return Err("Too many copies of the same name exist."); }
                    }
                }
            }
        }
        let result = if to_vm { self.send(remote, &item, replace).await } else { self.receive(remote, &item, replace).await };
        if result.is_err() {
            if to_vm { let _ = remote.call("discard", json!({ "folder": "transfers", "path": item.remote.path })).await; }
            else { let _ = fs::remove_file(partial(&item.mac)); }
        } else { self.progress.lock().unwrap_or_else(|error| error.into_inner()).done_files += 1; }
        result
    }

    /// Sends one Mac file. The file must look the same after the copy as before it.
    async fn send(&self, remote: &Remote, item: &Item, replace: bool) -> Result<(), &'static str> {
        let mut file = crate::platform_fs::open_read_nofollow(&item.mac)
            .map_err(|_| "A file could not be opened.")?;
        let before = file.metadata().map_err(|_| "A file could not be read.")?;
        let (mut offset, mut buffer) = (0u64, vec![0u8; PIECE]);
        loop {
            if self.cancelled.load(Ordering::Acquire) { return Err("cancelled"); }
            let mut filled = 0;
            while filled < buffer.len() {
                match file.read(&mut buffer[filled..]).map_err(|_| "A file could not be read.")? { 0 => break, count => filled += count }
            }
            let last = filled < buffer.len();
            if last {
                let after = file.metadata().map_err(|_| "A file could not be read.")?;
                if (after.len(), after.modified().ok()) != (before.len(), before.modified().ok())
                    || offset + filled as u64 != before.len() { return Err("A file changed while it was copied."); }
            }
            remote.call("write", json!({ "folder": "transfers", "path": item.remote.path, "offset": offset,
                "data": crate::frames::encode(&buffer[..filled]), "last": last, "replace": replace }))
                .await.map_err(|code| explain(&code))?;
            offset += filled as u64;
            self.progress.lock().unwrap_or_else(|error| error.into_inner()).done_bytes += filled as u64;
            if last { return Ok(()); }
        }
    }

    /// Receives one VM file into a partial copy on the Mac, then gives it its name.
    async fn receive(&self, remote: &Remote, item: &Item, replace: bool) -> Result<(), &'static str> {
        let parent = item.mac.parent().ok_or("The destination is invalid.")?;
        fs::create_dir_all(parent).map_err(|_| "The destination folder could not be created.")?;
        let staged = partial(&item.mac);
        let _ = fs::remove_file(&staged);
        let mut file = crate::platform_fs::create_new(&staged, false).map_err(|_| "The destination could not be written.")?;
        let (mut offset, mut seen) = (0u64, None);
        loop {
            if self.cancelled.load(Ordering::Acquire) { return Err("cancelled"); }
            let piece = remote.call("read", json!({ "folder": item.remote.folder, "path": item.remote.path, "offset": offset }))
                .await.map_err(|code| explain(&code))?;
            let stamp = (piece["size"].as_u64(), piece["modified"].as_i64());
            if *seen.get_or_insert(stamp) != stamp { return Err("A file changed while it was copied."); }
            let bytes = crate::frames::decode(piece["data"].as_str().unwrap_or_default())
                .ok_or("The VM sent an unreadable piece.")?;
            file.write_all(&bytes).map_err(|_| "The destination could not be written.")?;
            offset += bytes.len() as u64;
            self.progress.lock().unwrap_or_else(|error| error.into_inner()).done_bytes += bytes.len() as u64;
            if piece["end"] == true {
                if Some(offset) != stamp.0 { return Err("A file changed while it was copied."); }
                file.sync_all().map_err(|_| "The destination could not be written.")?;
                if !replace && fs::symlink_metadata(&item.mac).is_ok() { return Err("The destination changed during the transfer."); }
                drop(file);
                return fs::rename(&staged, &item.mac).map_err(|_| "The destination could not be written.");
            }
            if bytes.is_empty() { return Err("The VM sent an unreadable piece."); }
        }
    }
}

/// Turns a worker's refusal code into words for the user.
fn explain(code: &str) -> &'static str {
    match code {
        "disconnected" => "The VM disconnected during the transfer.",
        "space" => "The VM does not have enough free disk space.",
        "exists" => "The destination changed during the transfer.",
        "sequence" => "The transfer lost a piece and was stopped.",
        "path" => "A path was refused because it leaves the offered folders or is a link.",
        _ => "The VM could not complete the transfer.",
    }
}

/// Names the partial copy beside a Mac target.
fn partial(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_owned();
    name.push(PARTIAL);
    target.with_file_name(name)
}

/// Returns the same item under "name 2.ext", "name 3.ext" and so on.
fn numbered(item: &Item, number: u32, to_vm: bool) -> Item {
    let rename = |path: &str| {
        let (folder, name) = path.rsplit_once('/').map_or(("", path), |(folder, name)| (folder, name));
        let renamed = match name.rsplit_once('.').filter(|(stem, _)| !stem.is_empty()) {
            Some((stem, extension)) => format!("{stem} {number}.{extension}"),
            None => format!("{name} {number}"),
        };
        if folder.is_empty() { renamed } else { format!("{folder}/{renamed}") }
    };
    let relative = rename(&item.relative);
    Item { mac: if to_vm { item.mac.clone() } else {
            item.mac.with_file_name(Path::new(&relative).file_name().unwrap_or_default()) },
        remote: Source { folder: item.remote.folder.clone(),
            path: if to_vm { rename(&item.remote.path) } else { item.remote.path.clone() } },
        relative, size: item.size }
}

/// Lists the regular files under the chosen Mac paths. Links are counted and left behind.
fn plan_mac(sources: &[PathBuf], day: &str) -> Result<(Vec<Item>, u64), &'static str> {
    fn walk(path: &Path, relative: String, day: &str, depth: usize, items: &mut Vec<Item>, skipped: &mut u64)
        -> Result<(), &'static str> {
        let found = fs::symlink_metadata(path).map_err(|_| "A chosen file could not be read.")?;
        if crate::platform_fs::is_link(&found) || !(found.is_dir() || found.is_file()) { *skipped += 1; return Ok(()); }
        if found.is_file() {
            if items.len() >= FILES { return Err("A transfer holds at most 10,000 files."); }
            items.push(Item { mac: path.to_owned(), size: found.len(), relative: relative.clone(),
                remote: Source { folder: "transfers".into(), path: format!("{day}/{relative}") } });
            return Ok(());
        }
        if depth >= DEPTH { return Err("A folder nests too deeply to transfer."); }
        let mut entries: Vec<_> = fs::read_dir(path).map_err(|_| "A chosen folder could not be read.")?
            .flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else { *skipped += 1; continue };
            if name.ends_with(PARTIAL) { continue; }
            walk(&entry.path(), format!("{relative}/{name}"), day, depth + 1, items, skipped)?;
        }
        Ok(())
    }
    let (mut items, mut skipped) = (Vec::new(), 0);
    for source in sources {
        let name = source.file_name().and_then(|name| name.to_str()).ok_or("A chosen name is not valid text.")?;
        walk(source, name.to_owned(), day, 0, &mut items, &mut skipped)?;
    }
    if items.is_empty() { return Err("Nothing to transfer: the selection holds no regular file."); }
    Ok((items, skipped))
}

/// Lists the files under one chosen VM path through the worker's own listing.
fn plan_vm<'a>(remote: &'a Remote, source: &'a Source, relative: &'a str, destination: &'a Path, depth: usize,
    items: &'a mut Vec<Item>) -> std::pin::Pin<Box<dyn Future<Output = Result<(), &'static str>> + Send + 'a>> {
    Box::pin(async move {
        if depth > DEPTH { return Err("A folder nests too deeply to transfer."); }
        if relative.split('/').any(|name| name.is_empty() || name == "." || name == "..") {
            return Err("A path was refused because it leaves the offered folders or is a link.");
        }
        match remote.call("list", json!({ "folder": source.folder, "path": source.path })).await {
            Ok(listing) => for entry in listing["entries"].as_array().into_iter().flatten() {
                let Some(name) = entry["name"].as_str().filter(|name| !name.contains('/')) else { continue };
                let child = Source { folder: source.folder.clone(), path: format!("{}/{name}", source.path) };
                plan_vm(remote, &child, &format!("{relative}/{name}"), destination, depth + 1, items).await?;
            },
            // Not a folder: it must then be a readable file, which the first read proves.
            Err(_) => {
                let piece = remote.call("read", json!({ "folder": source.folder, "path": source.path, "offset": u64::MAX >> 1 }))
                    .await.map_err(|code| explain(&code))?;
                if items.len() >= FILES { return Err("A transfer holds at most 10,000 files."); }
                items.push(Item { mac: destination.join(relative), remote: source.clone(),
                    relative: relative.to_owned(), size: piece["size"].as_u64().unwrap_or(0) });
            }
        }
        Ok(())
    })
}

/// What the user is asked to choose on this Mac.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pick { Files, Folders, Destination }

/// Opens the macOS panel, or under test returns prepared answers in order.
pub enum Picker { Native, Fixed(Mutex<Vec<Option<Vec<PathBuf>>>>) }

impl Default for Picker { fn default() -> Self { Self::Native } }

impl Picker {
    /// Returns the chosen paths, or `None` when the user cancelled. The panel belongs to
    /// macOS: no page and no agent can answer it, which makes the choice the user's own.
    pub async fn choose(&self, pick: Pick) -> Result<Option<Vec<PathBuf>>, &'static str> {
        if let Self::Fixed(answers) = self {
            let mut answers = answers.lock().unwrap_or_else(|error| error.into_inner());
            return Ok(if answers.is_empty() { None } else { answers.remove(0) });
        }
        #[cfg(windows)]
        { let _ = pick; Err("VM file transfers are not available on Windows.") }
        #[cfg(unix)]
        {
        let ask = match pick {
            Pick::Files => "choose file with prompt \"Choose files to send to the VM\" with multiple selections allowed",
            Pick::Folders => "choose folder with prompt \"Choose folders to send to the VM\" with multiple selections allowed",
            Pick::Destination => "choose folder with prompt \"Choose where to save the files from the VM\"",
        };
        let script = format!("set chosen to {ask}\nif class of chosen is not list then set chosen to {{chosen}}\n\
            set out to \"\"\nrepeat with one in chosen\nset out to out & POSIX path of one & linefeed\nend repeat\nreturn out");
        let run = tokio::process::Command::new("/usr/bin/osascript").args(["-e", &script])
            .stdin(std::process::Stdio::null()).kill_on_drop(true).output();
        let output = tokio::time::timeout(std::time::Duration::from_secs(300), run).await
            .map_err(|_| "The file panel was left open too long.")?
            .map_err(|_| "The macOS file panel could not be opened.")?;
        // Cancel ends the script with error -128; that is an answer, not a failure.
        if !output.status.success() { return Ok(None); }
        let paths: Vec<PathBuf> = String::from_utf8_lossy(&output.stdout).lines()
            .filter(|line| line.starts_with('/')).map(|line| PathBuf::from(line.trim_end_matches('/'))).collect();
        Ok(if paths.is_empty() { None } else { Some(paths) })
        }
    }
}

/// Chosen Mac paths wait here under an identifier, so the page never names a Mac path.
#[derive(Default)]
pub struct Selections { saved: Mutex<HashMap<Uuid, (std::time::Instant, Pick, Vec<PathBuf>)>> }

impl Selections {
    /// Keeps one selection for ten minutes and describes it by names only.
    pub fn keep(&self, pick: Pick, paths: Vec<PathBuf>) -> Value {
        let items: Vec<Value> = paths.iter().map(|path| {
            let found = fs::symlink_metadata(path).ok();
            json!({ "name": path.file_name().and_then(|name| name.to_str()).unwrap_or("?"),
                "kind": if found.as_ref().is_some_and(|found| found.is_dir()) { "folder" } else { "file" },
                "size": found.filter(|found| found.is_file()).map(|found| found.len()) })
        }).collect();
        let id = Uuid::new_v4();
        let mut saved = self.saved.lock().unwrap_or_else(|error| error.into_inner());
        saved.retain(|_, (since, _, _)| since.elapsed() < std::time::Duration::from_secs(600));
        saved.insert(id, (std::time::Instant::now(), pick, paths));
        json!({ "selectionId": id, "items": items })
    }

    /// Hands a selection to exactly one transfer.
    pub fn take(&self, id: Uuid, destination: bool) -> Option<Vec<PathBuf>> {
        let mut saved = self.saved.lock().unwrap_or_else(|error| error.into_inner());
        let (since, pick, _) = saved.get(&id)?;
        if since.elapsed() >= std::time::Duration::from_secs(600) || (*pick == Pick::Destination) != destination {
            return None;
        }
        saved.remove(&id).map(|(_, _, paths)| paths)
    }
}
