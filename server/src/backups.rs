//! Experimental: folder backups of a VM's Workspace to this Mac. Everything is off until the
//! user switches a schedule on or presses "Back up now". A backup copies through the file
//! transfer code, gets its name only when its manifest is written, and is never whole-disk.

// Copies files and reports progress.
use crate::transfer_jobs::{Remote, Source, Transfers};
// Stores settings and manifests.
use serde::{Deserialize, Serialize};
use serde_json::json;
// Hashes what arrived.
use sha2::{Digest, Sha256};
// Reads and writes backup folders on the Mac.
use std::{collections::HashMap, fs, io::{Read, Write},     path::{Path, PathBuf}, sync::Mutex, time::Duration};
// Names transfer jobs.
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Schedule { Off, Daily, Weekly }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub schedule: Schedule,
    /// How many whole backups stay before the oldest goes to the Trash.
    pub keep: u8,
    /// Folders under the VM's Workspace; an empty name is the whole Workspace.
    pub folders: Vec<String>,
    #[serde(default)] pub last_run: Option<i64>,
    #[serde(default)] pub last_error: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { schedule: Schedule::Off, keep: 3, folders: vec![String::new()], last_run: None, last_error: None }
    }
}

impl Settings {
    /// Accepts 1 to 10 kept backups and up to 16 plain relative folders.
    fn valid(&self) -> bool {
        (1..=10).contains(&self.keep) && (1..=16).contains(&self.folders.len())
            && self.folders.iter().all(|folder| folder.len() <= 512 && !folder.starts_with('/')
                && (folder.is_empty() || folder.split('/').all(|name| !name.is_empty() && name != "." && name != "..")))
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Backup { pub id: String, pub created: i64, pub files: u64, pub bytes: u64 }

#[derive(Debug)]
pub struct Failed { pub attempts: u32, pub reason: String }

pub struct Backups { file: PathBuf, root: PathBuf, trash: PathBuf, saved: Mutex<Option<HashMap<String, Settings>>>,
    running: Mutex<Vec<String>> }

impl Backups {
    /// `file` holds the settings, `root` the backups, and removed backups move to `trash`.
    pub fn new(file: PathBuf, root: PathBuf, trash: PathBuf) -> Self {
        Self { file, root, trash, saved: Mutex::new(None), running: Mutex::new(Vec::new()) }
    }

    fn with<T>(&self, action: impl FnOnce(&mut HashMap<String, Settings>) -> T) -> T {
        let mut saved = self.saved.lock().unwrap_or_else(|error| error.into_inner());
        action(saved.get_or_insert_with(|| fs::read_to_string(&self.file).ok()
            .and_then(|text| serde_json::from_str::<HashMap<String, Settings>>(&text).ok())
            .map(|all| all.into_iter().filter(|(_, settings)| settings.valid()).collect()).unwrap_or_default()))
    }

    /// Returns one VM's settings; a VM nobody configured has backups off.
    pub fn settings(&self, vm_id: &str) -> Settings {
        self.with(|all| all.get(vm_id).cloned().unwrap_or_default())
    }

    /// Saves valid settings in a file only this user can read.
    pub fn save(&self, vm_id: &str, settings: Settings) -> std::io::Result<()> {
        if !settings.valid() { return Err(std::io::ErrorKind::InvalidInput.into()); }
        let text = self.with(|all| { all.insert(vm_id.into(), settings); serde_json::to_string(all) })?;
        let staged = self.file.with_extension(format!("json.{}", Uuid::new_v4()));
        let mut output = crate::platform_fs::private_file(&staged, true)?;
        output.write_all(text.as_bytes())?;
        output.sync_all()?;
        drop(output);
        fs::rename(staged, &self.file)
    }

    /// Reports whether a scheduled backup should run at `now` (seconds). Off is never due.
    pub fn due(&self, vm_id: &str, now: i64) -> bool {
        let settings = self.settings(vm_id);
        let period = match settings.schedule { Schedule::Off => return false, Schedule::Daily => 86_400, Schedule::Weekly => 604_800 };
        settings.last_run.is_none_or(|last| now - last >= period)
            && !self.running.lock().unwrap_or_else(|error| error.into_inner()).iter().any(|id| id == vm_id)
    }

    /// Lists whole backups, newest first. A folder without a manifest is not a backup.
    pub fn list(&self, vm_id: &str) -> Vec<Backup> {
        let mut found: Vec<Backup> = fs::read_dir(self.root.join(vm_id)).into_iter().flatten().flatten()
            .filter_map(|entry| {
                let id = entry.file_name().into_string().ok()?;
                let manifest: serde_json::Value = serde_json::from_slice(&fs::read(entry.path().join("manifest.json")).ok()?).ok()?;
                let files = manifest["files"].as_array()?;
                Some(Backup { id, created: manifest["created"].as_i64()?, files: files.len() as u64,
                    bytes: files.iter().filter_map(|file| file["bytes"].as_u64()).sum() })
            }).collect();
        found.sort_by(|left, right| right.created.cmp(&left.created).then(right.id.cmp(&left.id)));
        found
    }

    /// Makes one backup. It appears under its name only after every file arrived and the
    /// manifest with their checksums was written; then older backups beyond `keep` are moved
    /// to the Trash. One backup per VM runs at a time.
    pub async fn run(&self, transfers: &Transfers, remote: Remote, vm_id: &str, now: i64) -> Result<Backup, String> {
        if !safe(vm_id) { return Err("The VM is not valid.".into()); }
        {
            let mut running = self.running.lock().unwrap_or_else(|error| error.into_inner());
            if running.iter().any(|id| id == vm_id) { return Err("A backup of this VM is already running.".into()); }
            running.push(vm_id.into());
        }
        let result = self.copy(transfers, remote, vm_id, now).await;
        self.running.lock().unwrap_or_else(|error| error.into_inner()).retain(|id| id != vm_id);
        let mut settings = self.settings(vm_id);
        settings.last_error = result.as_ref().err().cloned();
        if result.is_ok() { settings.last_run = Some(now); }
        let _ = self.save(vm_id, settings);
        result
    }

    async fn copy(&self, transfers: &Transfers, remote: Remote, vm_id: &str, now: i64) -> Result<Backup, String> {
        let settings = self.settings(vm_id);
        let home = self.root.join(vm_id);
        let id = format!("{now}-{}", &Uuid::new_v4().simple().to_string()[..6]);
        let staging = home.join(format!("{id}.incomplete"));
        fs::create_dir_all(staging.join("files/Workspace")).map_err(|_| "The backup folder could not be created.")?;
        let outcome = async {
            let sources = settings.folders.iter().map(|folder| Source { folder: "workspace".into(), path: folder.clone() }).collect();
            // The whole Workspace has no name of its own, so its contents land in `Workspace`.
            let job = transfers.to_mac_into(remote, sources, staging.join("files/Workspace"));
            let view = loop {
                let view = transfers.view(job).ok_or("The backup was lost.")?;
                if view["state"] != "running" { break view; }
                tokio::time::sleep(Duration::from_millis(20)).await;
            };
            if view["state"] != "done" {
                return Err(view["error"].as_str().unwrap_or("The backup did not finish.").to_owned());
            }
            let files = tokio::task::spawn_blocking({ let base = staging.join("files"); move || manifest(&base) })
                .await.map_err(|_| "The backup could not be checked.")??;
            fs::write(staging.join("manifest.json"), serde_json::to_vec_pretty(&json!({
                "created": now, "vm": vm_id, "files": files })).map_err(|_| "The manifest could not be written.")?)
                .map_err(|_| "The manifest could not be written.")?;
            fs::rename(&staging, home.join(&id)).map_err(|_| "The backup could not be finished.")?;
            Ok(())
        }.await;
        if let Err(reason) = outcome { let _ = fs::remove_dir_all(&staging); return Err(reason); }
        for old in self.list(vm_id).into_iter().skip(usize::from(settings.keep)) {
            let _ = fs::create_dir_all(&self.trash);
            let _ = fs::rename(home.join(&old.id), self.trash.join(format!("MiniClaw backup {vm_id} {}", old.id)));
        }
        self.list(vm_id).into_iter().find(|backup| backup.id == id).ok_or_else(|| "The backup could not be read back.".into())
    }

    /// Tries once and then three more times, waiting `pause` between attempts.
    pub async fn run_with_retries(&self, transfers: &Transfers, remote: Remote, vm_id: &str, now: i64, pause: Duration)
        -> Result<Backup, Failed> {
        let mut reason = String::new();
        for attempt in 1..=4 {
            match self.run(transfers, remote.clone(), vm_id, now).await {
                Ok(backup) => return Ok(backup),
                Err(error) => reason = error,
            }
            if attempt < 4 { tokio::time::sleep(pause).await; }
        }
        Err(Failed { attempts: 4, reason })
    }

    /// Sends a backup to the VM's Transfers folder as `restore-<id>`, after checking every file
    /// against the manifest. The Workspace itself is never overwritten.
    pub async fn restore(&self, transfers: &Transfers, remote: Remote, vm_id: &str, id: &str) -> Result<Uuid, String> {
        if !safe(vm_id) || !safe(id) { return Err("The backup is not valid.".into()); }
        let home = self.root.join(vm_id).join(id);
        let recorded: serde_json::Value = serde_json::from_slice(&fs::read(home.join("manifest.json"))
            .map_err(|_| "The backup does not exist.")?).map_err(|_| "The backup's manifest is unreadable.")?;
        let base = home.join("files");
        let found = tokio::task::spawn_blocking({ let base = base.clone(); move || manifest(&base) })
            .await.map_err(|_| "The backup could not be checked.")??;
        if json!(found) != recorded["files"] { return Err("The backup no longer matches its checksums and was not restored.".into()); }
        let sources = fs::read_dir(&base).map_err(|_| "The backup is unreadable.")?.flatten().map(|entry| entry.path()).collect();
        Ok(transfers.to_vm(remote, sources, format!("restore-{id}")))
    }
}

/// Accepts a plain single name, so an identifier can never leave the backup folder.
fn safe(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// Lists every regular file under `base` with its size and SHA-256, sorted by path.
fn manifest(base: &Path) -> Result<Vec<serde_json::Value>, String> {
    fn walk(base: &Path, folder: &Path, files: &mut Vec<serde_json::Value>) -> Result<(), String> {
        let mut entries: Vec<_> = fs::read_dir(folder).map_err(|_| "A backup folder is unreadable.")?.flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let kind = entry.file_type().map_err(|_| "A backup file is unreadable.")?;
            if kind.is_dir() { walk(base, &entry.path(), files)?; continue; }
            if !kind.is_file() { continue; }
            let mut file = fs::File::open(entry.path()).map_err(|_| "A backup file is unreadable.")?;
            let (mut hash, mut buffer, mut bytes) = (Sha256::new(), vec![0u8; 1 << 16], 0u64);
            loop {
                let count = file.read(&mut buffer).map_err(|_| "A backup file is unreadable.")?;
                if count == 0 { break; }
                hash.update(&buffer[..count]); bytes += count as u64;
            }
            files.push(json!({ "path": entry.path().strip_prefix(base).unwrap_or(&entry.path()).to_string_lossy(),
                "bytes": bytes, "sha256": hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect::<String>() }));
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(base, base, &mut files)?;
    Ok(files)
}

/// Returns the running, signed-in worker of one VM, which a backup needs.
fn worker(backend: &crate::chat::Backend, vm_id: &str) -> Option<Remote> {
    if vm_id == "master" { backend.primary_client() } else { backend.dedicated_client(vm_id) }.map(Remote::Worker)
}

/// Runs one backup in the background with its retries, and reports a final failure as a
/// notification. `pause` separates the attempts.
pub fn start(backend: std::sync::Arc<crate::chat::Backend>, vm_id: String, pause: Duration) -> bool {
    let (Some(backups), Some(remote)) = (backend.backups.get(), worker(&backend, &vm_id)) else { return false };
    if backups.running.lock().unwrap_or_else(|error| error.into_inner()).iter().any(|id| *id == vm_id) { return false; }
    tokio::spawn(async move {
        let Some(backups) = backend.backups.get() else { return };
        backend.idle.touch(&vm_id);
        let now = chrono::Utc::now().timestamp();
        if backups.run_with_retries(&backend.transfers, remote, &vm_id, now, pause).await.is_err() {
            // A failed schedule waits a full period before it tries again.
            let mut settings = backups.settings(&vm_id);
            settings.last_run = Some(now);
            let _ = backups.save(&vm_id, settings);
            let owner = backend.data.lock().await.owner();
            if let Some(owner) = owner { let _ = backend.agents.backup_failed(owner, vm_id.clone()).await; }
        }
    });
    true
}

/// Starts every scheduled backup that is due and whose VM is running. It never starts a VM.
pub fn scheduled(backend: &std::sync::Arc<crate::chat::Backend>) {
    let Some(backups) = backend.backups.get() else { return };
    let now = chrono::Utc::now().timestamp();
    let configured: Vec<String> = backups.with(|all| all.keys().cloned().collect());
    for vm_id in configured.into_iter().filter(|vm_id| backups.due(vm_id, now)) {
        start(backend.clone(), vm_id, Duration::from_secs(60));
    }
}
