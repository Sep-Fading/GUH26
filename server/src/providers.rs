// Writes the private settings file.
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Weak, atomic::{AtomicBool, Ordering}},
    time::Duration,
};

// Persists each provider's enabled flag.
use serde::{Deserialize, Serialize};
// Serializes provider starts and stops.
use tokio::sync::Mutex;

// Starts and stops provider clients.
use crate::{chat::Backend, environment::Assignment, rpc::Failure};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Id { Codex, Claude }

impl Id {
    pub const ALL: [Id; 2] = [Id::Codex, Id::Claude];

    /// Reads the public provider name used in routes and settings.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "codex" => Some(Id::Codex),
            "claude" => Some(Id::Claude),
            _ => None,
        }
    }

    /// Returns the public provider name.
    pub fn name(self) -> &'static str {
        match self { Id::Codex => "codex", Id::Claude => "claude" }
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(default = "enabled")]
    pub codex: bool,
    #[serde(default = "enabled")]
    pub claude: bool,
}

/// A provider without a saved choice is enabled.
fn enabled() -> bool { true }

impl Default for Settings {
    /// Enables every provider until the user turns one off.
    fn default() -> Self { Self { codex: true, claude: true } }
}

impl Settings {
    /// Reads saved choices; a missing or unreadable file enables every provider.
    pub fn read(path: &Path) -> Self {
        std::fs::read(path).ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }

    /// Replaces the private settings file atomically.
    fn write(&self, path: &Path) -> std::io::Result<()> {
        let temporary = path.with_extension("tmp");
        let mut file = crate::platform_fs::private_file(&temporary, false)?;
        file.write_all(&serde_json::to_vec(self)?)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(temporary, path)
    }

    /// Returns one provider's saved choice.
    pub fn enabled(&self, id: Id) -> bool {
        match id { Id::Codex => self.codex, Id::Claude => self.claude }
    }
}

/// Host-mode facts needed to start a provider after the backend is already serving.
pub struct Runtime {
    pub settings: PathBuf,
    pub codex_bin: Option<String>,
    pub claude_bin: Option<String>,
    pub claude_store: PathBuf,
    // Present when the Mac's Codex must also answer VM control for opt-in VMs.
    pub vm_control: Option<crate::worker::Control>,
    pub gate: Mutex<()>,
    // Set at shutdown so a stopped provider is not brought back.
    pub stopping: AtomicBool,
}

impl Runtime {
    /// Reports whether this provider's executable was found at startup.
    pub fn installed(&self, id: Id) -> bool {
        match id { Id::Codex => self.codex_bin.is_some(), Id::Claude => self.claude_bin.is_some() }
    }
}

impl Backend {
    /// Returns the running client for one provider.
    pub fn provider_client(&self, id: Id) -> Option<crate::rpc::Client> {
        match id {
            // With VM links the Mac's Codex is the Host worker and Master is a VM.
            Id::Codex if self.vm_routing => self.host_worker(),
            Id::Codex => self.primary_client(),
            Id::Claude => self.claude_client(),
        }
    }

    /// Attaches a Codex App Server started after the backend, in host mode only.
    pub async fn attach_codex(self: &Arc<Self>, client: crate::rpc::Client)
        -> Result<(), Failure> {
        if self.vm_routing { return self.attach_host(client).await; }
        if !client.available() || self.primary_client().is_some() {
            return Err(Failure::Rejected);
        }
        *self.master.write().map_err(|_| Failure::Unavailable)? = Some(client.clone());
        Self::listen_client(self, client, Assignment::Host {}, Assignment::Host {});
        self.changed();
        Ok(())
    }

    /// Saves one provider's choice, then starts or stops it to match. Returns the
    /// saved settings; a provider that fails to start stays enabled but unavailable.
    pub async fn set_provider(self: &Arc<Self>, id: Id, enable: bool)
        -> Result<Settings, Failure> {
        let runtime = self.providers.get().ok_or(Failure::Unavailable)?;
        let _gate = runtime.gate.lock().await;
        let mut settings = Settings::read(&runtime.settings);
        match id { Id::Codex => settings.codex = enable, Id::Claude => settings.claude = enable }
        settings.write(&runtime.settings).map_err(|_| Failure::Unavailable)?;
        if !enable {
            if let Some(client) = self.provider_client(id) {
                client.stop().await;
                client.drain_events().await;
            }
        } else if self.provider_client(id).is_none() {
            self.start_provider(runtime, id).await;
        }
        self.changed();
        Ok(settings)
    }

    /// Waits while a provider has nothing to supervise: it is switched off or not installed.
    /// Only a backend change can alter that, so the task sleeps until one happens instead of
    /// waking every second to read the settings file. A short pause afterwards folds a burst
    /// of changes into one check. Returns false once the backend is gone.
    async fn idle(changes: &mut tokio::sync::watch::Receiver<u64>) -> bool {
        if changes.changed().await.is_err() { return false; }
        tokio::time::sleep(Duration::from_secs(1)).await;
        changes.borrow_and_update();
        true
    }

    /// Keeps each enabled provider running. When one exits on its own it is started
    /// again after 1, 2, 5 and then every 30 seconds. A provider the user disabled, a
    /// missing executable, and shutdown all stop the retries. Nothing is resent; turns
    /// that were in flight are already marked uncertain by the disconnect event.
    pub fn supervise(self: &Arc<Self>) {
        for id in Id::ALL {
            let owner: Weak<Self> = Arc::downgrade(self);
            let mut changes = self.changes.subscribe();
            tokio::spawn(async move {
                let mut attempt = 0usize;
                loop {
                    // A completion observer does not keep the provider's process alive.
                    let running = match owner.upgrade() {
                        Some(backend) => backend.provider_client(id).map(|client| client.completion()),
                        None => break,
                    };
                    if let Some(mut completion) = running {
                        completion.closed().await;
                        attempt = 0;
                    }
                    let delay = [1, 2, 5, 30][attempt.min(3)];
                    tokio::time::sleep(Duration::from_secs(delay)).await;
                    let Some(backend) = owner.upgrade() else { break; };
                    let Some(runtime) = backend.providers.get() else { break; };
                    if runtime.stopping.load(Ordering::Acquire) { break; }
                    let gate = runtime.gate.lock().await;
                    if backend.provider_client(id).is_some() { attempt = 0; continue; }
                    if !Settings::read(&runtime.settings).enabled(id) || !runtime.installed(id) {
                        attempt = 0;
                        // Release everything held before the long wait.
                        drop(gate);
                        drop(backend);
                        if !Self::idle(&mut changes).await { break; }
                        continue;
                    }
                    backend.start_provider(runtime, id).await;
                    attempt += 1;
                    backend.changed();
                }
            });
        }
    }

    /// Launches and attaches one installed provider; failures leave it unavailable.
    async fn start_provider(self: &Arc<Self>, runtime: &Runtime, id: Id) {
        match id {
            Id::Codex => {
                let Some(binary) = &runtime.codex_bin else { return; };
                let launched = match &runtime.vm_control {
                    Some(control) => crate::runtime::launch_with_control(binary,
                        self.local_cwd(), crate::worker::Control { path: control.path.clone(),
                            key: control.key }).await,
                    None => crate::runtime::launch(binary, &self.cwd).await,
                };
                if let Ok(client) = launched
                    && self.attach_codex(client.clone()).await.is_err() {
                    client.stop().await;
                }
            }
            Id::Claude => {
                let Some(binary) = &runtime.claude_bin else { return; };
                if let Ok(client) = crate::claude::launch(binary.clone(), self.local_cwd().into(),
                    runtime.claude_store.clone()).await
                    && self.attach_claude(client.clone()).await.is_err() {
                    client.stop().await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A switched-off provider costs nothing until something in the backend changes.
    #[tokio::test]
    async fn an_idle_supervisor_sleeps_until_the_backend_changes() {
        let (sender, mut changes) = tokio::sync::watch::channel(0u64);
        let quiet = tokio::time::timeout(Duration::from_millis(300), Backend::idle(&mut changes)).await;
        assert!(quiet.is_err(), "nothing changed, so the supervisor must still be waiting");
        sender.send(1).unwrap();
        let woke = tokio::time::timeout(Duration::from_secs(3), Backend::idle(&mut changes)).await;
        assert_eq!(woke, Ok(true));
        drop(sender);
        assert!(!Backend::idle(&mut changes).await, "a backend that is gone ends the supervisor");
    }

    /// Provider choices remain writable while the database pins their shared parent directory.
    #[test]
    fn provider_choice_roundtrip() {
        let root = std::env::temp_dir().join(format!("miniclaw-provider-settings-{}", uuid::Uuid::new_v4()));
        crate::platform_fs::private_dir(&root, false).unwrap();
        let _database = crate::agents::Store::open(&root.join("agents.sqlite3")).unwrap();
        let path = root.join("providers.json");
        let off = Settings { codex: false, claude: true };
        off.write(&path).expect("initial provider settings write");
        assert!(!Settings::read(&path).codex);
        let on = Settings { codex: true, claude: false };
        on.write(&path).expect("replacement provider settings write");
        assert!(!Settings::read(&path).claude);
        drop(_database);
        std::fs::remove_dir_all(root).unwrap();
    }
}
