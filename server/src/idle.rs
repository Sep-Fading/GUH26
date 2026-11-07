//! Idle shutdown for VMs. Each VM has its own limit from a closed list, off by default. A VM
//! past its limit is warned about first; with no answer within the grace period it is shut
//! down through the normal clean path. The choice is kept on the Mac.

// Reads the shared backend state and VM control.
use crate::chat::Backend;
// Builds control requests and public views.
use serde_json::{Value, json};
// Keeps per-VM limits, activity and warnings.
use std::{collections::HashMap, fs, io::Write, path::PathBuf,
    sync::{Arc, Mutex, OnceLock}, time::{Duration, Instant}};

/// The idle limits the page offers, in minutes. Zero means off.
pub const CHOICES: [u16; 5] = [0, 15, 30, 60, 120];
/// How long a warning waits for "Keep running" before the VM is shut down.
pub const GRACE: Duration = Duration::from_secs(60);

#[derive(Debug, PartialEq, Eq)]
pub enum Step { Wait, Warn, Stop }

/// Decides what to do with one running VM. A VM with a turn in progress is never idle.
pub fn step(limit: Option<Duration>, idle_for: Duration, busy: bool, warned_for: Option<Duration>,
    grace: Duration) -> Step {
    let Some(limit) = limit else { return Step::Wait };
    if busy || idle_for < limit { return Step::Wait; }
    match warned_for {
        None => Step::Warn,
        Some(waited) if waited >= grace => Step::Stop,
        Some(_) => Step::Wait,
    }
}

#[derive(Default)]
pub struct Idle {
    pub path: OnceLock<PathBuf>,
    limits: Mutex<Option<HashMap<String, u16>>>,
    activity: Mutex<HashMap<String, Instant>>,
    warnings: Mutex<HashMap<String, (Instant, String)>>,
    /// The state each VM was last seen in, to tell a new failure from an old one.
    states: Mutex<HashMap<String, String>>,
}

impl Idle {
    /// Runs a closure over the saved limits, loading them from disk on first use.
    fn with_limits<T>(&self, action: impl FnOnce(&mut HashMap<String, u16>) -> T) -> T {
        let mut limits = self.limits.lock().unwrap_or_else(|error| error.into_inner());
        let loaded = limits.get_or_insert_with(|| self.path.get()
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str::<HashMap<String, u16>>(&text).ok())
            .map(|saved| saved.into_iter().filter(|(_, minutes)| CHOICES.contains(minutes)).collect())
            .unwrap_or_default());
        action(loaded)
    }

    /// Returns one VM's idle limit in minutes; zero means off.
    pub fn minutes(&self, vm_id: &str) -> u16 {
        self.with_limits(|limits| limits.get(vm_id).copied().unwrap_or(0))
    }

    /// Saves one VM's limit from the offered list, in a file only this user can read.
    pub fn set_minutes(&self, vm_id: &str, minutes: u16) -> std::io::Result<()> {
        if !CHOICES.contains(&minutes) { return Err(std::io::ErrorKind::InvalidInput.into()); }
        let text = self.with_limits(|limits| {
            if minutes == 0 { limits.remove(vm_id); } else { limits.insert(vm_id.into(), minutes); }
            serde_json::to_string(limits)
        })?;
        self.touch(vm_id);
        let Some(path) = self.path.get() else { return Ok(()) };
        let staged = path.with_extension(format!("json.{}", uuid::Uuid::new_v4()));
        let mut output = crate::platform_fs::private_file(&staged, true)?;
        output.write_all(text.as_bytes())?;
        output.sync_all()?;
        drop(output);
        fs::rename(staged, path)
    }

    /// Forgets a deleted VM's limit, activity and warning.
    pub fn forget(&self, vm_id: &str) {
        let _ = self.set_minutes(vm_id, 0);
        self.activity.lock().unwrap_or_else(|error| error.into_inner()).remove(vm_id);
        self.warnings.lock().unwrap_or_else(|error| error.into_inner()).remove(vm_id);
    }

    /// Records use of a VM: a turn, a desktop viewer, a desktop action or "Keep running".
    /// Any use also withdraws an open warning.
    pub fn touch(&self, vm_id: &str) {
        self.activity.lock().unwrap_or_else(|error| error.into_inner())
            .insert(vm_id.into(), Instant::now());
        self.warnings.lock().unwrap_or_else(|error| error.into_inner()).remove(vm_id);
    }

    /// Lists open warnings with the seconds left before each shutdown.
    pub fn warnings(&self) -> Vec<Value> {
        let now = Instant::now();
        self.warnings.lock().unwrap_or_else(|error| error.into_inner()).iter()
            .map(|(vm_id, (since, name))| json!({ "vmId": vm_id, "name": name,
                "secondsLeft": GRACE.saturating_sub(now.saturating_duration_since(*since)).as_secs() }))
            .collect()
    }
}

/// Checks every VM with a limit once. `ahead` moves the clock forward, which only tests use.
pub async fn tick(backend: &Arc<Backend>, ahead: Duration) {
    let Some(host) = backend.host_worker().filter(|_| backend.vm_routing) else { return };
    let limited: Vec<(String, u16)> = backend.idle.with_limits(|limits|
        limits.iter().map(|(id, minutes)| (id.clone(), *minutes)).collect());
    let now = Instant::now() + ahead;
    for (vm_id, minutes) in limited {
        let params = if vm_id == "master" { json!({}) } else { json!({ "vmId": vm_id }) };
        let running = host.call("miniclaw/vm/status", params.clone()).await
            .is_ok_and(|status| status["state"] == "running");
        if !running {
            // A stopped VM starts a fresh idle period when it next runs.
            backend.idle.activity.lock().unwrap_or_else(|error| error.into_inner()).remove(&vm_id);
            backend.idle.warnings.lock().unwrap_or_else(|error| error.into_inner()).remove(&vm_id);
            continue;
        }
        let (busy, owner) = {
            let data = backend.data.lock().await;
            (data.chats.values().any(|chat| chat.active.is_some() && chat.summary["vmId"] == vm_id.as_str()),
                data.owner())
        };
        if busy { backend.idle.touch(&vm_id); }
        let last = *backend.idle.activity.lock().unwrap_or_else(|error| error.into_inner())
            .entry(vm_id.clone()).or_insert_with(Instant::now);
        let warned = backend.idle.warnings.lock().unwrap_or_else(|error| error.into_inner())
            .get(&vm_id).map(|(since, _)| now.saturating_duration_since(*since));
        match step(Some(Duration::from_secs(u64::from(minutes) * 60)),
            now.saturating_duration_since(last), busy, warned, GRACE) {
            Step::Wait => {}
            Step::Warn => {
                let name = if vm_id == "master" { "Master VM".to_owned() } else {
                    backend.agents.projects().await.ok().into_iter().flatten()
                        .find(|project| project.definition.environment.as_ref()
                            .and_then(crate::environment::ProjectChoice::dedicated_id) == Some(vm_id.as_str()))
                        .map_or_else(|| "Orphaned VM".to_owned(), |project| project.definition.name)
                };
                backend.idle.warnings.lock().unwrap_or_else(|error| error.into_inner())
                    .insert(vm_id.clone(), (now, name));
            }
            Step::Stop => {
                backend.idle.warnings.lock().unwrap_or_else(|error| error.into_inner()).remove(&vm_id);
                backend.idle.activity.lock().unwrap_or_else(|error| error.into_inner()).remove(&vm_id);
                let stopped = host.call("miniclaw/vm/stop", params).await
                    .is_ok_and(|status| status["state"] == "stopping");
                if let (true, Some(owner)) = (stopped, owner) {
                    let _ = backend.agents.vm_idle_stopped(owner, vm_id.clone()).await;
                }
            }
        }
    }
}

/// Names a VM failure worth telling the user about. Only a change into `failed` counts, what
/// came before names the kind, and a stop somebody asked for is not a failure.
pub fn failure(before: Option<&str>, now: &str) -> Option<&'static str> {
    if now != "failed" { return None; }
    match before {
        Some("starting") => Some("vm_start_failed"),
        Some("running") => Some("vm_stopped_unexpectedly"),
        _ => None,
    }
}

/// Looks at Master and every project VM once and reports a new failure as a notification.
pub async fn watch_failures(backend: &Arc<Backend>) {
    let Some(host) = backend.host_worker().filter(|_| backend.vm_routing) else { return };
    let mut machines = vec![("master".to_owned(), json!({}))];
    if let Ok(list) = host.call("miniclaw/vm/list", json!({})).await {
        machines.extend(list["machines"].as_array().into_iter().flatten().take(64)
            .filter_map(|machine| machine["vmId"].as_str().map(|id| (id.to_owned(), json!({ "vmId": id })))));
    }
    for (vm_id, params) in machines {
        let Ok(status) = host.call("miniclaw/vm/status", params).await else { continue };
        let Some(now) = status["state"].as_str().map(str::to_owned) else { continue };
        let before = backend.idle.states.lock().unwrap_or_else(|error| error.into_inner()).insert(vm_id.clone(), now.clone());
        if let Some(kind) = failure(before.as_deref(), &now) {
            let owner = backend.data.lock().await.owner();
            if let Some(owner) = owner { let _ = backend.agents.vm_failed(owner, vm_id, kind).await; }
        }
    }
}

/// Checks the VMs every 20 s for as long as the backend lives.
pub fn watch(backend: Arc<Backend>) {
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(20));
        loop {
            timer.tick().await;
            tick(&backend, Duration::ZERO).await;
            watch_failures(&backend).await;
            crate::backups::scheduled(&backend);
            if let Some(check) = backend.schedule_tick.get() { check(backend.clone()); }
        }
    });
}
