//! Starts an MCP server saved as `npx …` or `npm exec …` on Windows without npm's batch
//! launchers. The first start installs the package with MiniClaw's npm into the server's own
//! folder (`<data>\mcp-npm\<server id>`), inside the server's sandbox and through its approved
//! hosts; every start then runs the package's command script with MiniClaw's Node directly.
//! `npm_launch` holds the rules; this file runs them.

// The rules for launchers, packages and npm settings.
use crate::npm_launch::{self, Plan, Shim};
// The saved server and the error the page shows.
use crate::{mcp_client::Fault, mcp_servers::Server};
// Names the server's folders and files.
use std::path::{Path, PathBuf};
// Bounds an install and records how long it took.
use std::time::{Duration, Instant};

/// The longest an install may run. npm's own retries are short, so a dead registry fails sooner.
const INSTALL_SECONDS: u64 = 300;

/// What a recognised server runs: the private Node, its arguments (Node's options from the
/// script's `#!` line, the script, the server's own arguments), and the installed package folder
/// the run reads.
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub package: PathBuf,
}

/// MiniClaw's data folder, where the sandbox journals live too.
fn data() -> Result<PathBuf, Fault> {
    std::env::var_os("MINICLAW_DATA_DIR").map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(|path| PathBuf::from(path).join("MiniClaw")))
        .ok_or_else(|| Fault::new("MiniClaw's data folder could not be found."))
}

/// One lock per server, so two starts never install into the same folder at once.
fn lock(id: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    static LOCKS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>> = std::sync::OnceLock::new();
    let mut locks = LOCKS.get_or_init(Default::default).lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    locks.entry(id.to_owned()).or_default().clone()
}

/// Prepares a recognised server: reads its arguments, installs its package when this server has
/// no install for them yet, and chooses the script to run. `secrets` are the server's saved
/// values; the install gets them too (a private registry's token is one).
pub async fn prepare(server: &Server, secrets: &[(String, String)], shim: Shim) -> Result<Launch, Fault> {
    let mut plan = npm_launch::plan(shim, &server.args).map_err(Fault)?;
    if plan.registry.is_none() {
        plan.registry = secrets.iter().find(|(name, _)| name.eq_ignore_ascii_case("npm_config_registry")).map(|(_, value)| value.clone());
    }
    let node = crate::windows_runtimes::private_node().map_err(|error| Fault(error.to_string()))?;
    let folder = npm_launch::server_folder(&data()?, &server.id).ok_or_else(|| Fault::new("This server has no saved identifier."))?;
    let package = folder.join("pkg");
    let lock = lock(&server.id);
    let _held = lock.lock().await;
    let key = npm_launch::install_key(&plan);
    let marker = folder.join("installed.json");
    let installed = std::fs::read_to_string(&marker).is_ok_and(|text| text == key);
    if !installed {
        install(server, &plan, &folder, &package, secrets).await?;
        std::fs::write(&marker, &key).map_err(|_| Fault::new("MiniClaw could not record the server's install."))?;
    }
    let (options, script) = entry(&package, &plan)?;
    let mut args = options;
    args.push(script.to_string_lossy().into_owned());
    args.extend(plan.args.iter().cloned());
    Ok(Launch { program: node.program.clone(), args, package })
}

/// Removes a deleted server's install. Best effort: a folder still in use stays until the next
/// deletion of a server with the same identifier, which never happens.
pub fn forget(id: &str) {
    let Ok(data) = data() else { return };
    let Some(folder) = npm_launch::server_folder(&data, id) else { return };
    if let Err(error) = std::fs::remove_dir_all(&folder) && error.kind() != std::io::ErrorKind::NotFound {
        tracing::warn!(%error, "A deleted MCP server's npm install could not be removed");
    }
}

/// Installs the plan's packages into `package` with MiniClaw's npm: in the server's sandbox with
/// the server's approved hosts, or unsandboxed for a server the user marked unrestricted.
async fn install(server: &Server, plan: &Plan, folder: &Path, package: &Path, secrets: &[(String, String)]) -> Result<(), Fault> {
    let names = plan.packages.join(" and ");
    if !plan.install {
        return Err(Fault(format!("{names} is not installed for this server, and its arguments say not to install it.")));
    }
    let host = npm_launch::registry_host(plan.registry.as_deref()).map_err(Fault)?;
    if !server.unrestricted && !server.hosts.iter().any(|approved| *approved == host) {
        return Err(Fault(format!("{names} is not installed for this server yet. Add {host} to the server's approved hosts so MiniClaw's npm \
            can download it; later starts need no network.")));
    }
    let npm = crate::windows_runtimes::private_npm().map_err(|error| Fault(error.to_string()))?;
    let node = crate::windows_runtimes::private_node().map_err(|error| Fault(error.to_string()))?;
    let prepare_folder = || -> std::io::Result<()> {
        crate::platform_fs::private_dir(folder, true)?;
        for name in ["cache", "global"] { crate::platform_fs::private_dir(&folder.join(name), true)?; }
        let _ = std::fs::remove_file(folder.join("installed.json"));
        if package.exists() { std::fs::remove_dir_all(package)?; }
        crate::platform_fs::private_dir(package, false)?;
        std::fs::write(package.join("package.json"), "{\"private\":true,\"description\":\"MiniClaw MCP server install\"}\n")?;
        if !folder.join("npmrc").exists() { drop(crate::platform_fs::private_file(&folder.join("npmrc"), true)?); }
        Ok(())
    };
    if let Err(error) = prepare_folder() {
        // A finished run's cleanup may still hold the old files for a moment.
        tracing::debug!(%error, "Waiting for sandbox cleanup before preparing an npm install");
        let _ = crate::windows_sandbox::drain_cleanup(Duration::from_secs(35));
        prepare_folder().map_err(|_| Fault::new("The server's previous install is still in use or its folder cannot be prepared. Stop the server and try again."))?;
    }
    let args = npm_launch::install_arguments(&npm.cli, plan);
    let mut values: Vec<(String, String)> = secrets.to_vec();
    let started = Instant::now();
    let captured = if server.unrestricted {
        values.extend(node.environment().into_iter().map(|(name, value)| (name.to_string_lossy().into_owned(), value.to_string_lossy().into_owned())));
        values.extend(npm_launch::install_environment(folder, &npm.shell, plan));
        let process = crate::windows_process::stdio(&node.program, &args, package, &values)
            .map_err(|_| Fault::new("MiniClaw's npm could not be started."))?;
        tokio::time::timeout(Duration::from_secs(INSTALL_SECONDS), crate::windows_process::capture_stdio(process)).await
    } else {
        let prepare = |error: std::io::Error| {
            tracing::warn!(%error, "An npm install's sandbox could not be prepared");
            Fault::new(crate::windows_sandbox::refusal(&error).unwrap_or("The sandbox for the server's npm install could not be prepared."))
        };
        let mut sandbox = crate::windows_sandbox::Sandbox::with_private_cwd(&[]).map_err(prepare)?;
        sandbox.add_private_npm().map_err(prepare)?.ok_or_else(|| Fault::new("MiniClaw's npm is unavailable in this installation."))?;
        sandbox.grant_owned(folder, true).map_err(prepare)?;
        sandbox.enable_network().map_err(|_| Fault::new("The Windows network service is not available for this server's approved hosts."))?;
        let proxy = crate::egress::Proxy::start(server.hosts.clone(), crate::egress::system_resolver()).await
            .map_err(|_| Fault::new("The server's network proxy could not start."))?;
        let mut base: Vec<(String, String)> = sandbox.environment().into_iter()
            .map(|(name, value)| (name.to_string_lossy().into_owned(), value.to_string_lossy().into_owned())).collect();
        base.append(&mut values);
        let address = format!("http://127.0.0.1:{}", proxy.port);
        base.extend([("HTTP_PROXY".into(), address.clone()), ("HTTPS_PROXY".into(), address.clone()), ("ALL_PROXY".into(), address),
            ("NO_PROXY".into(), String::new()), ("NODE_USE_ENV_PROXY".into(), "1".into())]);
        base.extend(npm_launch::install_environment(folder, &npm.shell, plan));
        let pending = crate::windows_process::pending_stdio_restricted(&node.program, &args, package, &base, &sandbox.security())
            .map_err(|_| Fault::new("MiniClaw's npm could not be started in the sandbox."))?;
        sandbox.bind_process(&pending.server.process).map_err(|error| Fault(error.to_string()))?;
        let _lease = crate::windows_network_broker::connect_ports(&pending.server.process, &sandbox.profile_name(), proxy.port, &[], &[]).await
            .map_err(|error| Fault(error.to_string()))?;
        let process = pending.resume().map_err(|_| Fault::new("MiniClaw's npm could not be started in the sandbox."))?;
        let result = tokio::time::timeout(Duration::from_secs(INSTALL_SECONDS), crate::windows_process::capture_stdio(process)).await;
        let refused = proxy.refused();
        if !refused.is_empty() { tracing::info!(?refused, "Hosts an npm install was refused"); }
        result
    };
    let elapsed = started.elapsed();
    let (output, errors, code) = match captured {
        Err(_) => return Err(Fault(format!("Installing {names} took longer than {INSTALL_SECONDS} seconds and was stopped."))),
        Ok(Err(_)) => return Err(Fault::new("MiniClaw's npm stopped unexpectedly.")),
        Ok(Ok(captured)) => captured,
    };
    tracing::info!(packages = %names, ?elapsed, code, "Installed an MCP server's npm package");
    if code != 0 {
        let text = format!("{}{}", String::from_utf8_lossy(&errors), String::from_utf8_lossy(&output));
        tracing::debug!(%text, "npm install output");
        return Err(Fault(format!("MiniClaw's npm could not install {names} (exit code {code}): {}", summary(&text))));
    }
    Ok(())
}

/// The lines of npm's output that say what went wrong, bounded for the page.
fn summary(text: &str) -> String {
    let errors: Vec<&str> = text.lines().filter(|line| line.starts_with("npm error") || line.starts_with("npm ERR!")).take(6).collect();
    let lines = if errors.is_empty() { text.lines().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect() } else { errors };
    lines.join(" | ").chars().take(700).collect()
}

/// Finds the command script of the installed package: npx's choice of command, a script path
/// that stays inside the package, and a script Node can run. Returns Node's options and the path.
fn entry(package: &Path, plan: &Plan) -> Result<(Vec<String>, PathBuf), Fault> {
    let candidates: Vec<&String> = if plan.command.is_some() { plan.packages.iter().collect() } else { plan.packages.iter().take(1).collect() };
    let mut reasons = Vec::new();
    for spec in candidates {
        let name = npm_launch::package_name(spec).ok_or_else(|| Fault::new("A package name is not valid."))?;
        let root = name.split('/').fold(package.join("node_modules"), |path, part| path.join(part));
        let manifest = std::fs::File::open(root.join("package.json")).ok().and_then(|file| {
            use std::io::Read;
            let mut text = String::new();
            file.take(1024 * 1024).read_to_string(&mut text).ok()?;
            serde_json::from_str::<serde_json::Value>(&text).ok()
        }).ok_or_else(|| Fault(format!("The install of {name} has no readable package.json. Remove and add the server again.")))?;
        let (command, relative) = match npm_launch::bin(&manifest, plan.command.as_deref()) {
            Ok(found) => found,
            Err(reason) => { reasons.push(reason); continue; }
        };
        let relative = npm_launch::entry_path(&relative).ok_or_else(|| Fault(format!("{name}'s command {command} points outside the package.")))?;
        let script = root.join(relative);
        let inside = std::fs::canonicalize(&script).ok().zip(std::fs::canonicalize(&root).ok()).is_some_and(|(script, root)| script.starts_with(root));
        if !inside { return Err(Fault(format!("{name}'s command {command} is missing or points outside the package."))); }
        let mut head = Vec::new();
        std::fs::File::open(&script).and_then(|file| { use std::io::Read; file.take(512).read_to_end(&mut head) })
            .map_err(|_| Fault(format!("{name}'s command {command} could not be read.")))?;
        let file = script.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let options = npm_launch::runner(&head, &file).map_err(Fault)?;
        return Ok((options, script));
    }
    Err(Fault(reasons.join(" ")))
}
