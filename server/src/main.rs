// Reads configuration and manages private instance storage.
use std::{
    collections::HashMap,
    env,
    error::Error,
    fs::{File, TryLockError},
    io,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
    time::Duration,
};
// Preserves existing Unix state directory restrictions.
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
// Binds HTTP and tracks explicit shutdown.
use tokio::net::TcpListener;

// Connects startup with runtime, routes, and helpers.
use miniclaw_server::{agents::Store, api, chat::Backend, computer, providers, remote,
    remote::Route, runtime};
// Validates stable dedicated VM identities from the host supervisor.
use uuid::Uuid;

/// Runs the requested helper mode or the single loopback UI/API server.
/// Returns after shutdown or a startup/serving error.
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // The Claude sign-in CLI runs this executable as its browser opener (claude_login::opener).
    #[cfg(windows)]
    if let Some(code) = miniclaw_server::claude_login::opener(&env::args_os().skip(1).collect::<Vec<_>>()) {
        std::process::exit(code);
    }
    // Inside the Linux sandbox this binary only relays proxy connections and runs the command.
    #[cfg(target_os = "linux")]
    {
        let all: Vec<String> = env::args().skip(1).collect();
        if all.first().map(String::as_str) == Some(miniclaw_server::sandbox_linux::RELAY_MODE) {
            std::process::exit(miniclaw_server::sandbox_linux::relay(&all[1..]));
        }
    }
    let mut args = env::args_os().skip(1);
    let mode = args.next();
    if let Some(mode) = mode {
        if mode == "--write-private-file" {
            use std::io::{Read, Write};
            let path = PathBuf::from(args.next().ok_or("Missing private file path")?);
            if args.next().is_some() || !path.is_absolute() { return Err("Expected one absolute private file path".into()); }
            let parent = path.parent().ok_or("Missing private parent")?;
            let _parent = miniclaw_server::platform_fs::open_existing(parent, false, true)?;
            #[cfg(windows)]
            miniclaw_server::platform_fs::private_dir(parent, true)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let meta = _parent.metadata()?;
                if meta.mode() & 0o077 != 0 || meta.uid() != rustix::process::geteuid().as_raw() {
                    return Err("Setup parent is not private".into());
                }
            }
            let mut bytes = Vec::new();
            io::stdin().take(65537).read_to_end(&mut bytes)?;
            if bytes.len() > 65536 { return Err("Private setup payload exceeds limit".into()); }
            match miniclaw_server::platform_fs::create_new(&path, true) {
                Ok(mut file) => { file.write_all(&bytes)?; file.sync_all()?; }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let file = miniclaw_server::platform_fs::open_read_nofollow(&path)?;
                    if !miniclaw_server::platform_fs::private_regular_file(&file)? { return Err("Existing setup file is not private".into()); }
                    let mut current = Vec::new();
                    file.take(65537).read_to_end(&mut current)?;
                    if current != bytes { return Err("Existing setup file differs".into()); }
                }
                Err(error) => return Err(error.into()),
            }
            return Ok(());
        }
        if mode == "--initialize-data" || mode == "--secure-data-dir" {
            let data = PathBuf::from(args.next().ok_or("Missing new data directory")?);
            if args.next().is_some() || !data.is_absolute() {
                return Err("Expected one absolute data directory".into());
            }
            let _instance = instance(&data)?;
            if mode == "--secure-data-dir" { return Ok(()); }
            if data.join("agents.sqlite3").symlink_metadata().is_ok() {
                return Err("An agents database already exists".into());
            }
            Store::open(&data.join("agents.sqlite3"))
                .map_err(|_| "Could not initialize profile storage")?;
            return Ok(());
        }
        // Setup adds a bundle's catalog (JSON on standard input) to an existing profile database.
        // The instance lock refuses while MiniClaw runs for this folder; the report goes to stdout.
        if mode == "--add-bundle" {
            use std::io::Read;
            let data = PathBuf::from(args.next().ok_or("Missing data directory")?);
            if args.next().is_some() || !data.is_absolute() {
                return Err("Expected one absolute data directory".into());
            }
            let _instance = instance(&data)?;
            if data.join("agents.sqlite3").symlink_metadata().is_err() {
                return Err("No agents database exists yet; initialize the data directory first".into());
            }
            let mut text = String::new();
            io::stdin().take(4 << 20 | 1).read_to_string(&mut text)?;
            if text.len() > 4 << 20 { return Err("Bundle catalog exceeds limit".into()); }
            let catalog: miniclaw_server::bundles::Catalog = serde_json::from_str(&text)
                .map_err(|error| format!("Invalid bundle catalog: {error}"))?;
            let agents = Store::open(&data.join("agents.sqlite3"))
                .map_err(|_| "Agent storage could not be opened")?;
            let report = agents.add_bundle(catalog).await?;
            println!("{}", serde_json::to_string(&report)?);
            return Ok(());
        }
        // Names what a profile database holds, read-only, also while MiniClaw runs.
        if mode == "--bundle-status" {
            let data = PathBuf::from(args.next().ok_or("Missing data directory")?);
            if args.next().is_some() || !data.is_absolute() {
                return Err("Expected one absolute data directory".into());
            }
            let found = miniclaw_server::bundles::inventory(&data.join("agents.sqlite3"))
                .map_err(|_| "Agent storage could not be read")?;
            println!("{}", serde_json::to_string(&found)?);
            return Ok(());
        }
        #[cfg(windows)]
        if mode == "--network-epoch" {
            if args.next().is_some() { return Err("Unexpected network diagnostic argument".into()); }
            let epoch = tokio::task::spawn_blocking(miniclaw_server::windows_network_broker::verified_boot_epoch).await??;
            println!("{}", serde_json::to_string(&epoch)?);
            return Ok(());
        }
        #[cfg(windows)]
        if mode == "--network-service" {
            let path = PathBuf::from(args.next().ok_or("Missing protected network service configuration")?);
            if args.next().is_some() || !path.is_absolute() { return Err("Expected one absolute service configuration path".into()); }
            return miniclaw_server::windows_network_broker::service(&path).map_err(Into::into);
        }
        if mode == "--tool-bridge" {
            return miniclaw_server::tool_bridge::run().map_err(Into::into);
        }
        if mode == "--computer-filter" && args.next().is_none() {
            return computer::filter(true).await.map_err(Into::into);
        }
        if mode != "--computer-helper" {
            return Err("Unknown command".into());
        }
        let helper = args.next().ok_or("Missing computer helper path")?;
        if args.next().is_some() {
            return Err("Unexpected computer helper argument".into());
        }
        #[cfg(windows)]
        return computer::bridge_windows(&helper).await.map_err(Into::into);
        #[cfg(unix)]
        return computer::bridge(&helper).map_err(Into::into);
    }
    // MiniClaw's own warnings go to stderr: by default on Windows, only when asked on macOS.
    miniclaw_server::logging::install()?;
    let assets = env::var_os("MINICLAW_WEB_DIR").map(PathBuf::from);
    if let Some(path) = &assets
        && (!path.is_absolute() || !path.join("index.html").is_file())
    {
        return Err("MINICLAW_WEB_DIR must contain the packaged index.html".into());
    }
    let port = match env::var("MINICLAW_PORT") {
        Ok(value) => value
            .parse::<u16>()
            .map_err(|_| "MINICLAW_PORT must be 0..=65535")?,
        Err(env::VarError::NotPresent) => 8787,
        Err(error) => return Err(error.into()),
    };

    // Both overrides are checked before a port is opened or any provider is started.
    let binary = codex_binary()?;
    let claude_bin = claude_binary()?;
    let data = miniclaw_server::platform_fs::data_directory();
    // Reject duplicate instances before opening a port or starting Codex.
    let _instance = instance(&data)?;
    // Unresolved records of earlier runs pause sandboxed work only: the gate keeps the reason and
    // refuses each sandboxed launch with it, while chat and everything else keep working.
    #[cfg(windows)]
    if let Err(error) = tokio::task::spawn_blocking(miniclaw_server::windows_sandbox::recover).await? {
        tracing::warn!(%error, "Windows sandbox recovery needs attention; sandboxed work is paused");
        eprintln!("Windows sandbox recovery needs attention before sandboxed work can start: {error} Existing records and permissions were preserved. Chat keeps working.");
    }
    // The private Node, npm and Python hook are checked now, in the background, not at the first launch.
    #[cfg(windows)]
    miniclaw_server::windows_runtimes::warm_up();
    let agents = Store::open(&data.join("agents.sqlite3"))
        .map_err(|_| "Agent storage could not be opened. No server was started.")?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
    let address = listener.local_addr()?;
    println!("MiniClaw listening on http://{address}");
    tracing::info!(%address, "Backend listening");

    // Chat work has its own directory; authentication still uses each provider's own store.
    let cwd = env::var_os("MINICLAW_CHAT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| data.join("chat-workspace"));
    if cfg!(windows) && env::var_os("MINICLAW_CHAT_DIR").is_some() {
        // Explicit workspaces retain the user's existing directory permissions.
        tokio::fs::create_dir_all(&cwd).await?;
    } else {
        miniclaw_server::platform_fs::private_dir(&cwd, true)?;
    }
    #[cfg(unix)]
    std::fs::set_permissions(&cwd, std::fs::Permissions::from_mode(0o700))?;
    // Windows gets the plain drive form: Codex, Claude and their shells run in this folder, and
    // `canonicalize`'s verbatim form is one other programs may refuse (cmd.exe does). History
    // still matches tasks saved under the verbatim form (`history::workspace_spellings`).
    let cwd = miniclaw_server::platform_fs::plain_disk_form(&cwd.canonicalize()?.to_string_lossy())
        .to_owned();
    let origin = format!("http://{address}");
    let settings = providers::Settings::read(&data.join("providers.json"));
    // Opt-in VMs: a VM session leaves private link files; without them the Mac runs alone.
    let links = match env::var_os("MINICLAW_VM_LINKS") {
        Some(path) => Some(remote::Links::read(Path::new(&path))
            .map_err(|_| "MINICLAW_VM_LINKS must name a private VM link file")?),
        None => None,
    };
    let vm_control = links.as_ref().map(|links| links.control()).transpose()
        .map_err(|_| "The VM control key is missing or not private")?;
    let client = match (&binary, &vm_control) {
        (Some(binary), Some(control)) if settings.codex => runtime::launch_with_control(binary,
            &cwd, miniclaw_server::worker::Control { path: control.path.clone(),
                key: control.key }).await.ok(),
        (Some(binary), None) if settings.codex =>
            runtime::launch(binary, &cwd).await.ok(),
        _ => None,
    };
    if client.is_none() && settings.codex {
        eprintln!("Codex App Server is unavailable.");
    }
    let computer_helper = computer::target();
    // An installed Windows helper is looked up once at start, which also records its signature check.
    if let Some(target @ computer::Target::Installed(_)) = computer_helper.clone() {
        tokio::spawn(async move {
            match target.current().await {
                Ok(computer::Target::Local(path)) => eprintln!("Computer Use helper: {path}"),
                Ok(_) => {}
                Err(reason) => eprintln!("Computer Use helper unavailable: {}", reason.message()),
            }
        });
    }
    let backend = if let Some(links) = &links {
        // The Mac's Codex is the Host worker; Master and project VMs attach when they boot.
        let worker = remote::Settings::read(&links.worker)
            .map_err(|_| "The VM worker settings are missing or not private")?;
        let backend = Backend::new_with_host(None, client.clone(), worker.cwd.clone(), cwd,
            origin, data.join("computer-recovery"), agents, Some(computer::Target::Guest));
        backend.set_local();
        if let Some(helper) = computer::helper() { backend.set_local_helper(helper); }
        tokio::spawn(monitor_worker(worker, Route::Socket(links.master.clone()),
            Arc::downgrade(&backend), true));
        tokio::spawn(monitor_dedicated_registry(Arc::downgrade(&backend),
            links.dedicated.clone()));
        backend
    } else {
        Backend::new(client.clone(), cwd, origin, data.join("computer-recovery"), agents,
            computer_helper, false)
    };

    // Each provider starts on its own; either one alone is a working app.
    {
        if let Some(binary) = claude_bin.clone().filter(|_| settings.claude) {
            match miniclaw_server::claude::launch(binary, backend.local_cwd().to_owned(),
                data.join("claude-threads")).await {
                Ok(claude) if backend.attach_claude(claude.clone()).await.is_ok() => {}
                _ => eprintln!("Claude Code is unavailable."),
            }
        }
        let _ = backend.transcripts.set(data.join("vm-transcripts"));
        let _ = backend.appearance.set(data.join("appearance.json"));
        let _ = backend.idle.path.set(data.join("vm-idle.json"));
        let _ = backend.activity.dir.set(data.join("activity"));
        let _ = backend.attachments.dir.set(data.join("attachments"));
        let _ = backend.chat_folders.path.set(data.join("conversation-folders.json"));
        // A run that a stopped backend left open is closed as failed; it is never run again.
        let _ = backend.agents.close_interrupted_runs().await;
        // Swarms that were working carry on by themselves, once a provider is signed in.
        tokio::spawn(miniclaw_server::api::resume_swarms(backend.clone()));
        // What this Mac is and which common tools it has, for every conversation's briefing.
        tokio::spawn(miniclaw_server::briefing::detect());
        let _ = backend.agents.close_interrupted_schedules().await;
        // No turn of this run has ended, so no queued message may leave by itself.
        let _ = backend.agents.pause_queues_after_restart().await;
        let _ = backend.backups.set(miniclaw_server::backups::Backups::new(data.join("vm-backups.json"),
            data.join("backups"), env::var_os("HOME").map(std::path::PathBuf::from)
                .unwrap_or_else(|| data.clone()).join(".Trash")));
        miniclaw_server::idle::watch(backend.clone());
        let _ = backend.providers.set(providers::Runtime {
            settings: data.join("providers.json"), codex_bin: binary.clone(), claude_bin,
            claude_store: data.join("claude-threads"), vm_control,
            gate: tokio::sync::Mutex::new(()),
            stopping: std::sync::atomic::AtomicBool::new(false),
        });
        backend.supervise();
    }
    let owner = backend.clone();
    #[cfg(windows)]
    let cleanup_owner = backend.clone();

    let router = match assets {
        Some(path) => api::router_at(backend, &path),
        None => api::router(backend),
    };
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            owner.shell_grants.lock().await.clear();
            if let Some(runtime) = owner.providers.get() {
                runtime.stopping.store(true, std::sync::atomic::Ordering::Release);
            }
            // Stop every client so its streams can close during HTTP shutdown. Providers may have been restarted since launch; stop whichever run now.
            let mut running = vec![owner.primary_client(), owner.host_worker(),
                owner.claude_client()];
            running.extend(owner.dedicated_workers().into_iter().map(|(_, client)| Some(client)));
            for client in running.into_iter().flatten() {
                client.stop().await;
                client.drain_events().await;
            }
        }).await?;
    // Every owner of a sandbox lets go of it before the cleanup queue is drained: one-shot
    // commands, terminals, and the MCP servers the gateway keeps running. A sandbox that
    // outlived this would leave a journal the next start cannot resolve without a reboot.
    #[cfg(windows)]
    {
        let runs = miniclaw_server::executor::end_runs(Duration::from_secs(10)).await;
        cleanup_owner.terminals.close_all().await;
        let servers = tokio::time::timeout(Duration::from_secs(10), cleanup_owner.mcp.close_all()).await;
        tokio::task::spawn_blocking(|| miniclaw_server::windows_sandbox::drain_cleanup(
            Duration::from_secs(35))).await??;
        runs?;
        servers.map_err(|_| "MCP servers did not stop in time; their sandbox records were kept for recovery.")?;
    }
    Ok(())
}

/// Finds the Codex executable in either desktop app. `CODEX_BIN` overrides discovery;
/// an empty value disables Codex.
/// On Windows an override that is not the provider's native `.exe` is a startup error.
fn codex_binary() -> Result<Option<String>, String> {
    if let Some(value) = env::var_os("CODEX_BIN") {
        return miniclaw_server::provider_process::provider_override("CODEX_BIN", &value, cfg!(windows));
    }
    #[cfg(windows)]
    { return Ok(native_binary("codex.exe")); }
    // Linux packages and npm put the CLI on PATH; the standalone installer uses ~/.local/bin.
    #[cfg(target_os = "linux")]
    { return Ok(native_binary("codex")); }
    // The desktop app's bundled CLI first; otherwise a CLI installed with npm or Homebrew.
    #[cfg(not(any(windows, target_os = "linux")))]
    Ok(["/Applications/Codex.app/Contents/Resources/codex-cli/bin/codex",
        "/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex",
        "/Applications/Codex.app/Contents/Resources/codex",
        "/Applications/ChatGPT.app/Contents/Resources/codex"].into_iter()
        .find(|path| Path::new(path).is_file()).map(str::to_owned)
        .or_else(|| native_binary("codex")))
}

/// Finds the Claude Code CLI. `CLAUDE_BIN` overrides discovery; an empty value disables it.
/// On Windows an override that is not the provider's native `.exe` is a startup error.
fn claude_binary() -> Result<Option<String>, String> {
    if let Some(value) = env::var_os("CLAUDE_BIN") {
        let path = miniclaw_server::provider_process::provider_override("CLAUDE_BIN", &value, cfg!(windows))?
            .map(PathBuf::from);
        return Ok(path.filter(|path| path.is_absolute() && path.is_file())
            .map(|path| path.to_string_lossy().into_owned()));
    }
    Ok(native_binary(if cfg!(windows) { "claude.exe" } else { "claude" }))
}

/// Validates a signed inventory row before using its dedicated worker route.
fn dedicated_entry(value: &serde_json::Value) -> Option<(String, u32, remote::Settings)> {
    let id = value["vmId"].as_str()?;
    if Uuid::parse_str(id).ok()?.to_string() != id { return None; }
    let slot = value["slot"].as_u64()?;
    if slot >= 64 || value["cwd"] != "/Users/Shared/MiniClaw/Workspace" { return None; }
    let text = value["key"].as_str()?;
    if text.len() != 64 || !text.bytes().all(|byte|
        byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()) { return None; }
    let mut key = [0; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some((id.into(), 10300 + slot as u32,
        remote::Settings { key, cwd: "/Users/Shared/MiniClaw/Workspace".into() }))
}

/// Discovers retained dedicated VMs without exposing their worker keys to HTTP.
async fn monitor_dedicated_registry(owner: Weak<Backend>, sockets: PathBuf) {
    let mut known: HashMap<String, (u32, [u8; 32])> = HashMap::new();
    loop {
        let Some(backend) = owner.upgrade() else { break; };
        if let Some(host) = backend.host_worker()
            && let Ok(result) = host.call("miniclaw/vm/list", serde_json::json!({})).await
            && let Some(machines) = result["machines"].as_array() {
            for machine in machines {
                let Some((id, port, settings)) = dedicated_entry(machine) else {
                    eprintln!("Dedicated VM inventory is invalid; no new route was attached.");
                    break;
                };
                if let Some(previous) = known.get(&id) {
                    if previous != &(port, settings.key) {
                        eprintln!("Dedicated VM route changed; refusing its replacement.");
                        return;
                    }
                    continue;
                }
                known.insert(id.clone(), (port, settings.key));
                // Each project VM slot has its own private socket on the Mac.
                let route = Route::Socket(sockets.join(format!("d{:02}", port - 10300))
                    .join("controller.sock"));
                tokio::spawn(monitor_dedicated_worker(settings, route, owner.clone(), id));
            }
        }
        drop(backend);
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Reconnects one authenticated project VM only on its pinned virtual port.
async fn monitor_dedicated_worker(settings: remote::Settings, route: Route,
    owner: Weak<Backend>, vm_id: String) {
    loop {
        if owner.strong_count() == 0 { break; }
        match settings.connect_route(&route).await {
            Ok(client) => {
                let Some(backend) = owner.upgrade() else { client.stop().await; break; };
                let _ = backend.attach_dedicated(vm_id.clone(), client.clone()).await;
                drop(backend);
                client.closed().await;
            }
            Err(miniclaw_server::rpc::Failure::Rejected) => {
                eprintln!("Dedicated VM worker authentication failed on its private route.");
                break;
            }
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Reconnects one authenticated worker while the independent UI remains available.
async fn monitor_worker(settings: remote::Settings, route: Route,
    owner: Weak<Backend>, master: bool)
{
    loop {
        if owner.strong_count() == 0 { break; }
        match settings.connect_route(&route).await {
            Ok(client) => {
                let Some(backend) = owner.upgrade() else { client.stop().await; break; };
                if master {
                    let ready = client.call("miniclaw/computer/read", serde_json::json!({})).await
                        .ok().is_some_and(|status| status["available"] == true);
                    if backend.attach_master(client.clone()).await.is_ok() {
                        backend.set_master_computer_ready(ready);
                    }
                } else {
                    let _ = backend.attach_host(client.clone()).await;
                }
                drop(backend);
                client.closed().await;
                if master && let Some(backend) = owner.upgrade() {
                    backend.set_master_computer_ready(false);
                }
            }
            Err(miniclaw_server::rpc::Failure::Rejected) => {
                eprintln!("Worker authentication failed on its private route.");
                break;
            }
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Creates private app storage and returns a locked file held for the backend lifetime.
/// Fails if another instance already owns this data directory.
fn instance(data: &Path) -> io::Result<File> {
    let running = || io::Error::new(io::ErrorKind::AddrInUse,
        "MiniClaw is already running for this data directory.");
    let claim = || -> io::Result<File> {
        miniclaw_server::platform_fs::private_dir(data, true)?;
        #[cfg(unix)]
        std::fs::set_permissions(data, std::fs::Permissions::from_mode(0o700))?;
        miniclaw_server::platform_fs::private_lock_file(&data.join("instance.lock"))
    };
    // On Windows the running instance's open handles refuse a second opener before any lock is
    // tried: ERROR_SHARING_VIOLATION (32), seen natively on VM2 on 2026-10-04.
    let file = claim()
        .map_err(|error| if cfg!(windows) && error.raw_os_error() == Some(32) { running() } else { error })?;
    // Keep the descriptor open; the OS releases this lock when the process exits.
    file.try_lock().map_err(|error| match error {
        TryLockError::WouldBlock => running(),
        TryLockError::Error(error) => error,
    })?;
    Ok(file)
}

#[cfg(test)]
mod dedicated_tests {
    use super::*;

    /// A discovered VM must have one bounded slot and an exact worker key.
    #[test]
    fn validates_dedicated_registration() {
        let id = "019a1111-1111-7111-8111-111111111111";
        let good = serde_json::json!({ "vmId": id, "slot": 4,
            "key": "2a".repeat(32), "cwd": "/Users/Shared/MiniClaw/Workspace" });
        let (found, port, settings) = dedicated_entry(&good).unwrap();
        assert_eq!(found, id);
        assert_eq!(port, 10304);
        assert_eq!(settings.key, [42; 32]);
        for bad in [
            serde_json::json!({ "vmId": id, "slot": 64,
                "key": "2a".repeat(32), "cwd": "/Users/Shared/MiniClaw/Workspace" }),
            serde_json::json!({ "vmId": id, "slot": 4,
                "key": "2A".repeat(32), "cwd": "/Users/Shared/MiniClaw/Workspace" }),
            serde_json::json!({ "vmId": id, "slot": 4,
                "key": "2a".repeat(32), "cwd": "/Users/Shared/Other" }),
        ] { assert!(dedicated_entry(&bad).is_none()); }
    }
}

/// Resolves native provider executables without passing arguments through a shell.
fn native_binary(name: &str) -> Option<String> {
    let home = env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from).filter(|path| path.is_absolute());
    let mut candidates = Vec::new();
    if let Some(home) = home {
        candidates.push(home.join(".local/bin").join(name));
        if name == "claude" { candidates.push(home.join(".claude/local/claude")); }
    }
    candidates.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .filter(|directory| directory.is_absolute()).map(|directory| directory.join(name)));
    candidates.into_iter().find(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned())
}

/// Drains the backend when its Windows launcher exits or requests shutdown.
async fn shutdown_signal() {
    #[cfg(windows)]
    if env::var("MINICLAW_LAUNCHER_STDIN").as_deref() == Ok("1") {
        let (closed, wait) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
            let _ = closed.send(());
        });
        tokio::select! { _ = wait => {}, _ = platform_shutdown_signal() => {} }
        return;
    }
    platform_shutdown_signal().await;
}

/// Waits for platform shutdown events before draining provider connections.
async fn platform_shutdown_signal() {
    #[cfg(unix)]
    {
        if let Ok(mut term) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
            return;
        }
    }
    #[cfg(windows)]
    {
        if let (Ok(mut close), Ok(mut shutdown)) =
            (tokio::signal::windows::ctrl_close(), tokio::signal::windows::ctrl_shutdown()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = close.recv() => {},
                _ = shutdown.recv() => {},
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
