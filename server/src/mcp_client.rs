//! MiniClaw's MCP client. It starts a local server as a child process, inside the Seatbelt
//! sandbox unless the user marked the server unrestricted, or calls a remote server over
//! Streamable HTTP. Secrets are read from the Keychain here, at the moment of use, and go to
//! the server only: into the child's environment or the `Authorization` header.
//!
//! Follows the MCP specification of 2025-06-18: newline-delimited JSON-RPC on stdio; for
//! HTTP one POST per message, `Accept` of both JSON and an event stream, the session and
//! protocol version headers on every request after `initialize`.

// The saved entry that says how to reach the server.
use crate::mcp_servers::{Kind, Server};
// Reads the server's secrets at the moment of use.
use crate::secrets::{Secrets, env_key, header_key, token_key};
// Confines a local server and opens its approved hosts.
use crate::{egress, seatbelt::{self, Grant}};
// Reads pieces of a streamed HTTP answer.
use futures_util::StreamExt;
// Builds requests and reads answers.
use serde_json::{Value, json};
// Bounds every wait on a server.
use std::time::Duration;
// Talks to the child over its pipes.
use tokio::io::AsyncWriteExt;
#[cfg(unix)]
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
#[cfg(windows)]
use tokio::fs::File as ChildStdin;
#[cfg(windows)]
use tokio::fs::File as ChildStdout;
// Splits the child's output into bounded lines.
use tokio_util::codec::{FramedRead, LinesCodec};

/// The protocol version MiniClaw asks for, and the older ones it can also speak.
const VERSION: &str = "2025-06-18";
const SPOKEN: [&str; 3] = [VERSION, "2025-03-26", "2024-11-05"];
/// The largest single message or HTTP answer that is read.
const MOST: usize = 8 * 1024 * 1024;
/// The most tools one server may offer; the rest are not listed.
const MOST_TOOLS: usize = 128;
/// A PATH for servers that does not depend on how MiniClaw itself was started.
#[cfg(unix)]
const PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

/// Why a server could not be reached or a call failed, in words the user can act on.
#[derive(Debug, Clone)]
pub struct Fault(pub String);

impl Fault {
    pub(crate) fn new(words: impl Into<String>) -> Self { Self(words.into()) }

    /// Whether trying again unchanged can succeed. A sandboxed server that declares listening
    /// ports is refused by its own settings (`declared_listener`), so a retry cannot.
    pub(crate) fn retryable(&self) -> bool { !self.0.contains(LISTEN_REFUSED) }
}

/// The sentence that marks the listening-port refusal, in its words and in `Fault::retryable`.
const LISTEN_REFUSED: &str = "Sandboxed servers cannot listen on Windows yet";

/// What MiniClaw says when MiniClaw's Node refused a sandboxed server a child process and the
/// server then stopped or did not answer (windows_runtimes::NODE_PRELOAD).
#[cfg(any(windows, test))]
const CHILD_PIPES_FAULT: &str = "This server starts a child process, which the Windows sandbox cannot support with the bundled Node yet. \
    MiniClaw's Node 24.21.0 cannot create the pipes for a child's input and output inside the sandbox; a MiniClaw update with a newer Node is needed.";

/// Watches a server's standard error for the refusal code of MiniClaw's Node preload. The end of
/// each read is kept, so a code split between two reads is still found; nothing else is kept.
#[cfg(any(windows, test))]
#[derive(Default)]
struct RefusalWatch { tail: Vec<u8>, seen: bool }

#[cfg(any(windows, test))]
impl RefusalWatch {
    /// Reads one more piece of standard error and says whether the code has been seen.
    fn feed(&mut self, chunk: &[u8]) -> bool {
        if self.seen { return true; }
        let code = crate::windows_runtimes::CHILD_PIPES_CODE.as_bytes();
        self.tail.extend_from_slice(chunk);
        self.seen = self.tail.windows(code.len()).any(|window| window == code);
        let keep = code.len() - 1;
        if self.tail.len() > keep { self.tail.drain(..self.tail.len() - keep); }
        self.seen
    }
}

/// Gives the reason when a server whose child was refused then stopped or did not answer. The
/// server's own refusals of a request stay its own.
#[cfg(any(windows, test))]
fn explain_refusal(fault: Fault, refused: bool) -> Fault {
    let stopped = ["The server stopped before it answered.", "The server did not answer in time.", "The server stopped."];
    if refused && stopped.contains(&fault.0.as_str()) { Fault::new(CHILD_PIPES_FAULT) } else { fault }
}

/// Whether a Windows server's standard error has ended, and whether it named a refused child.
#[cfg(windows)]
#[derive(Default)]
struct ErrorsSeen { refused: std::sync::atomic::AtomicBool, ended: std::sync::atomic::AtomicBool }

/// Reads a Windows server's standard error to its end, as before discarding it, and notes the
/// refusal code of MiniClaw's Node preload.
#[cfg(windows)]
fn watch_errors(mut errors: impl std::io::Read + Send + 'static) -> std::sync::Arc<ErrorsSeen> {
    use std::sync::atomic::Ordering;
    let seen = std::sync::Arc::new(ErrorsSeen::default());
    let shared = seen.clone();
    tokio::task::spawn_blocking(move || {
        let mut watch = RefusalWatch::default();
        let mut buffer = [0u8; 8192];
        while let Ok(count) = errors.read(&mut buffer) {
            if count == 0 { break; }
            if watch.feed(&buffer[..count]) { shared.refused.store(true, Ordering::Release); }
        }
        shared.ended.store(true, Ordering::Release);
    });
    seen
}

/// How long a server may take to start and answer `initialize`: what its entry says. Tests shorten it.
fn start_wait(server: &Server) -> Duration {
    std::env::var("MINICLAW_TEST_MCP_START_MS").ok().and_then(|text| text.parse().ok()).map(Duration::from_millis)
        .unwrap_or_else(|| Duration::from_secs(u64::from(server.startup_seconds)))
}

/// A local server process. Dropping it ends the process and everything it started.
struct Stdio {
    #[cfg(unix)]
    child: Child,
    #[cfg(windows)]
    _process: crate::windows_process::Process,
    #[cfg(windows)]
    _sandbox: Option<crate::windows_sandbox::Sandbox>,
    #[cfg(windows)]
    _network: Option<crate::windows_network_broker::ClientLease>,
    /// What the server wrote to standard error so far, as far as MiniClaw needs to know.
    #[cfg(windows)]
    errors: std::sync::Arc<ErrorsSeen>,
    input: ChildStdin,
    output: FramedRead<ChildStdout, LinesCodec>,
    /// The server's only way to the network, kept alive as long as the server.
    _proxy: Option<egress::Proxy>,
    /// On Linux, the server's ways across its own network namespace, kept as long as the server.
    #[cfg(target_os = "linux")]
    _bridges: Option<crate::sandbox_linux::Bridges>,
}

#[cfg(unix)]
impl Drop for Stdio {
    /// Ends the whole process group, since a launcher such as `npx` starts the real server as its child.
    fn drop(&mut self) {
        if let Some(id) = self.child.id() { unsafe { libc::killpg(id as i32, libc::SIGKILL); } }
    }
}

/// A remote server and what its session needs on every request.
struct Http {
    client: reqwest::Client,
    url: String,
    token: Option<String>,
    /// The user's extra headers, read from the Keychain for this connection.
    headers: Vec<(String, String)>,
    session: Option<String>,
    version: Option<String>,
}

enum Transport { Stdio(Box<Stdio>), Http(Box<Http>) }

/// One initialized connection to one server.
pub struct Link {
    transport: Transport,
    next: u64,
    /// How long `initialize` and a listing may take.
    wait: Duration,
    /// The `serverInfo` the server gave: its name and version.
    pub info: Value,
}

/// The folder a command outside the system paths is installed in, so the sandbox can read it:
/// `<prefix>` for `<prefix>/bin/<command>`, otherwise the command's own folder.
#[cfg(unix)]
fn install_prefix(command: &str) -> Option<String> {
    let real = std::fs::canonicalize(command).ok()?;
    let parent = real.parent()?;
    let prefix = if parent.file_name().is_some_and(|name| name == "bin") { parent.parent()? } else { parent };
    let text = prefix.to_str()?;
    let home = std::env::var("HOME").unwrap_or_default();
    // Never the whole disk or the whole home folder.
    (text.len() > 1 && text != home).then(|| text.to_owned())
}

/// Starts the server's process with its secrets in its environment.
#[cfg(unix)]
async fn spawn(server: &Server, secrets: &Secrets) -> Result<Stdio, Fault> {
    let program = server.command.as_deref().ok_or_else(|| Fault::new("This server has no command."))?;
    let folder = match &server.folder {
        Some(folder) => Some(std::fs::canonicalize(folder).ok().and_then(|path| path.to_str().map(str::to_owned))
            .ok_or_else(|| Fault::new("The server's folder does not exist."))?),
        None => None,
    };
    let mut proxy = None;
    #[cfg(target_os = "linux")]
    let mut bridges = None;
    let mut command = if server.unrestricted {
        Command::new(program)
    } else {
        if !seatbelt::available() { return Err(Fault::new("This Mac cannot enforce the sandbox, so the server was not started.")); }
        let mut grants = Vec::new();
        if let Some(folder) = &folder { grants.push(Grant { path: folder.clone(), write: true, folder: true, blocked: false }); }
        grants.extend(runtime_grants(&server.runtime_paths).map_err(|_| Fault::new("The server runtime folders must be existing installation directories."))?);
        if let Some(prefix) = install_prefix(program) { grants.push(Grant { path: prefix, write: false, folder: true, blocked: false }); }
        if !server.hosts.is_empty() {
            proxy = Some(egress::Proxy::start(server.hosts.clone(), egress::system_resolver()).await
                .map_err(|_| Fault::new("The server's network proxy could not start."))?);
        }
        // The server's own local ports, and its proxy's, are the only connections it can open; its
        // listening ports the only ones it can accept on.
        let ports: Vec<u16> = server.ports.iter().copied().chain(proxy.as_ref().map(|proxy| proxy.port)).collect();
        #[cfg(target_os = "linux")]
        { bridges = Some(crate::sandbox_linux::Bridges::open(&server.ports, &server.listen).await
            .map_err(|_| Fault::new("The server's local ports could not be bridged into its sandbox."))?); }
        Command::from(seatbelt::command_net(&grants, program, &ports, &server.listen))
    };
    command.args(&server.args).env_clear().env("PATH", PATH).env("LANG", "en_US.UTF-8")
        .current_dir(folder.as_deref().unwrap_or("/"))
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null())
        .process_group(0).kill_on_drop(true);
    if server.unrestricted {
        if let Ok(home) = std::env::var("HOME") { command.env("HOME", home); }
    } else {
        // A confined server has no home to read; its folder stands in for it.
        let place = folder.as_deref().unwrap_or("/var/empty");
        command.env("HOME", place).env("TMPDIR", place);
    }
    if let Some(proxy) = &proxy {
        let address = format!("http://127.0.0.1:{}", proxy.port);
        command.env("HTTPS_PROXY", &address).env("https_proxy", &address).env("NODE_USE_ENV_PROXY", "1");
    }
    for name in &server.env {
        let value = secrets.get(&env_key(&server.id, name)).map_err(|_| Fault::new("The credential store could not be read."))?
            .ok_or_else(|| Fault::new(format!("The value of {name} is missing. Enter it again in the server's settings.")))?;
        command.env(name, value);
    }
    let mut child = command.spawn().map_err(|_| Fault::new("The server's command could not be started."))?;
    let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take())
        else { return Err(Fault::new("The server's command could not be started.")) };
    Ok(Stdio { child, input, output: FramedRead::new(output, LinesCodec::new_with_max_length(MOST)), _proxy: proxy,
        #[cfg(target_os = "linux")] _bridges: bridges })
}

/// Whether a confined server needs read access to the installation of the program it names. An npm-launched
/// server runs its installed package; a `node.exe` server runs on MiniClaw's private Node when this installation
/// has one (`Sandbox::program_for`), so the Node it named never runs and its folder is not granted.
#[cfg(any(windows, test))]
fn install_grant_wanted(program: &std::path::Path, npm_launch: bool, private_node: bool) -> bool {
    !npm_launch && !(private_node && crate::windows_runtimes::node_image(program))
}

/// Selects a bounded installation grant without exposing the whole profile or Windows tree.
#[cfg(windows)]
fn windows_install_grant(program: &std::path::Path) -> std::io::Result<Option<Grant>> {
    let program = std::fs::canonicalize(program)?;
    let system = std::env::var_os("SystemRoot").map(std::path::PathBuf::from)
        .and_then(|path| path.canonicalize().ok());
    if system.as_ref().is_some_and(|root| program.starts_with(root)) { return Ok(None); }
    let parent = program.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
    let home = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from)
        .and_then(|path| path.canonicalize().ok());
    let file_only = parent.parent().is_none() || home.as_deref() == Some(parent);
    Ok(Some(Grant { path: if file_only { &program } else { parent }.to_string_lossy().into_owned(),
        write: false, folder: !file_only, blocked: false }))
}

/// Says why a confined local server cannot start while the Windows sandbox is unavailable:
/// `closed` holds the gate's reason, and is `None` while the gate is open.
/// A server the user marked unrestricted, and a remote one, need no sandbox.
#[cfg(any(windows, test))]
fn closed_gate(server: &Server, closed: Option<&str>) -> Option<Fault> {
    let reason = closed.filter(|_| matches!(server.kind, Kind::Stdio) && !server.unrestricted)?;
    let refusal = "This PC cannot enforce the sandbox, so the server was not started.";
    Some(Fault(if reason.is_empty() { refusal.to_owned() } else { format!("{refusal} {reason}") }))
}

/// Refuses a sandboxed local server that declares listening ports, before any sandbox exists:
/// Windows sandboxes cannot listen yet (`windows_network_broker::LISTEN_UNSUPPORTED`), and
/// preparing one first would stamp the server's folder for nothing. The words name the one way to
/// run it, outside the sandbox, which the user switches on (and confirms) in the server's settings.
/// `arm64` adds that x64 programs cannot run in the sandbox on this PC either (Roblox Studio's
/// StudioMCP.exe is x64).
#[cfg(any(windows, test))]
pub(crate) fn declared_listener(server: &Server, arm64: bool) -> Option<Fault> {
    if server.unrestricted || server.listen.is_empty() || !matches!(server.kind, Kind::Stdio) { return None; }
    let ports = server.listen.iter().map(u16::to_string).collect::<Vec<_>>().join(", ");
    let mut words = format!("{} listens on port {ports} for an app to connect to it. {LISTEN_REFUSED}, \
        so MiniClaw did not start it and changed no folder permissions. To use it, open Settings > MCP servers, edit {}, \
        and turn on \"Run outside the sandbox\" (it then runs with your own access to your files).", server.name, server.name);
    if arm64 {
        words.push_str(" On this ARM64 PC, x64 programs such as Roblox Studio's StudioMCP.exe cannot run inside the sandbox either.");
    }
    Some(Fault(words))
}

/// Says why a confined server's sandbox was not prepared: the sandbox's own words when it refused
/// for a reason the user can act on (an item in the folder they cannot manage, earlier records
/// still unresolved), and the general sentence otherwise.
#[cfg(windows)]
fn unprepared(error: &std::io::Error) -> Fault {
    Fault::new(crate::windows_sandbox::refusal(error)
        .unwrap_or("The server's Windows sandbox could not be prepared. Check its folder and installation grants."))
}

/// Asks the gate for this installation and refuses a confined local server while it is closed.
#[cfg(windows)]
fn refused_by_gate(server: &Server) -> Option<Fault> {
    let closed = (!crate::executor::sandbox_available()).then(|| crate::seatbelt::unavailable_reason().unwrap_or_default());
    closed_gate(server, closed.as_deref())
}

/// Gives a child process its working folder in the form programs expect. `canonicalize`
/// answers `\\?\C:\...`, which Rust's documentation warns other applications may not accept;
/// a drive path loses that prefix, as `windows_shell::native_path` does for the shell.
#[cfg(any(windows, test))]
fn child_folder(folder: &std::path::Path) -> std::path::PathBuf {
    let plain = folder.to_str().and_then(|text| text.strip_prefix(r"\\?\")).filter(|rest| {
        let bytes = rest.as_bytes();
        bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
    });
    plain.map_or_else(|| folder.to_path_buf(), std::path::PathBuf::from)
}

/// Uses only runtime folders explicitly saved during setup; mutable venv files never widen grants.
fn runtime_grants(paths: &[String]) -> std::io::Result<Vec<Grant>> {
    if paths.len() > 8 { return Err(std::io::ErrorKind::InvalidInput.into()); }
    paths.iter().map(|path| {
        #[cfg(windows)]
        let normalized = crate::platform_fs::permission_path(std::path::Path::new(path))?;
        #[cfg(unix)]
        let normalized = std::fs::canonicalize(path)?.to_string_lossy().into_owned();
        let target = std::path::Path::new(&normalized);
        if target.parent().is_none() || !target.is_dir() { return Err(std::io::ErrorKind::InvalidInput.into()); }
        Ok(Grant { path: path.clone(), write: false, folder: true, blocked: false })
    }).collect()
}

/// Starts native MCP servers with explicit AppContainer resources unless the user chose unrestricted.
#[cfg(windows)]
async fn spawn(server: &Server, secrets: &Secrets) -> Result<Stdio, Fault> {
    // A closed gate creates no sandbox, whichever way the caller came.
    if let Some(refused) = refused_by_gate(server) { return Err(refused); }
    if let Some(refused) = declared_listener(server, cfg!(target_arch = "aarch64")) { return Err(refused); }
    let command = server.command.as_deref().ok_or_else(|| Fault::new("This server has no command."))?;
    // The saved values are read first: an npm install may need them too (a private registry's token).
    let mut secret_values = Vec::new();
    for name in &server.env {
        let value = secrets.get(&env_key(&server.id, name)).map_err(|_| Fault::new("The credential store could not be read."))?
            .ok_or_else(|| Fault::new(format!("The value of {name} is missing. Enter it again in the server's settings.")))?;
        secret_values.push((name.clone(), value));
    }
    // MiniClaw's runtime files are checked once, on a thread of their own (`windows_runtimes::warm_up`). A launch
    // waits for the checks it uses off the async workers and within the server's start wait, so its own calls
    // below find them answered and a slow or stuck check cannot hold up this or any other request.
    let npm = crate::npm_launch::shim(command).is_some();
    if !server.unrestricted || npm {
        crate::windows_runtimes::ready(npm, start_wait(server)).await.map_err(Fault)?;
    }
    // `npx` and `npm exec` never run npm's batch launchers: MiniClaw's npm installs the package and
    // MiniClaw's Node runs its command script with the arguments as a real argument list.
    let launch = match crate::npm_launch::shim(command) {
        Some(shim) => Some(crate::mcp_npm::prepare(server, &secret_values, shim).await?),
        None => None,
    };
    let program = launch.as_ref().map_or_else(|| std::path::PathBuf::from(command), |launch| launch.program.clone());
    let program = program.as_path();
    let args = launch.as_ref().map_or_else(|| server.args.clone(), |launch| launch.args.clone());
    if !program.is_absolute() || !program.extension().and_then(|v| v.to_str())
        .is_some_and(|v| v.eq_ignore_ascii_case("exe") || v.eq_ignore_ascii_case("com")) {
        return Err(Fault::new("Windows MCP commands require a native executable. Batch files need an explicit cmd.exe launcher."));
    }
    let configured_folder = server.folder.as_ref().map(|path| {
        std::path::PathBuf::from(path).canonicalize().ok().filter(|path| path.is_dir())
            .ok_or_else(|| Fault::new("The server's folder does not exist."))
    }).transpose()?;
    // Where a launch spends its time, for MINICLAW_LOG=debug (the first one after an install was slow).
    let started = std::time::Instant::now();
    let mut sandbox = if server.unrestricted { None } else {
        let mut grants = Vec::new();
        if let Some(folder) = &configured_folder {
            grants.push(Grant { path: folder.to_string_lossy().into_owned(), write: true, folder: true, blocked: false });
        }
        // Asked first: whether the private Node replaces the program decides the grant (`install_grant_wanted`).
        let private_node = crate::windows_runtimes::private_node().is_ok();
        if install_grant_wanted(program, launch.is_some(), private_node)
            && let Some(grant) = windows_install_grant(program).map_err(|_| Fault::new("The server's installed executable could not be checked."))? { grants.push(grant); }
        grants.extend(runtime_grants(&server.runtime_paths).map_err(|_| Fault::new("The server runtime folders must be existing installation directories."))?);
        let sandbox = match &configured_folder {
            Some(folder) => crate::windows_sandbox::Sandbox::new(&grants, folder),
            None => crate::windows_sandbox::Sandbox::with_private_cwd(&grants),
        };
        let mut sandbox = sandbox.map_err(|error| {
            tracing::warn!(%error, "A confined server's sandbox could not be prepared");
            #[cfg(test)]
            eprintln!("Sandbox preparation failed: {error:?}");
            unprepared(&error)
        })?;
        // The installed package is read-only to the server: its code cannot change itself for a later start.
        if let Some(launch) = &launch {
            sandbox.grant_owned(&launch.package, false).map_err(|error| {
                tracing::warn!(%error, "A server's npm install could not be granted");
                Fault::new("The server's installed package could not be opened to its sandbox.")
            })?;
        }
        Some(sandbox)
    };
    let folder = child_folder(&configured_folder.or_else(|| sandbox.as_ref().map(|sandbox| sandbox.scratch().to_path_buf()))
        .unwrap_or_else(std::env::temp_dir));
    // A sandboxed Python server always gets the networked token, under a lease whose proxy allows
    // only its approved hosts, possibly none: asyncio cannot be imported without a socket
    // (windows_runtimes::python_needs_lease).
    let network_needed = !server.unrestricted && (!server.hosts.is_empty() || !server.ports.is_empty() || !server.listen.is_empty()
        || crate::windows_runtimes::python_needs_lease(program));
    let proxy = if network_needed {
        Some(egress::Proxy::start(server.hosts.clone(), egress::system_resolver()).await
            .map_err(|_| Fault::new("The server's network proxy could not start."))?)
    } else { None };
    let mut values = sandbox.as_ref().map(|sandbox| sandbox.environment().into_iter()
        .map(|(name, value)| (name.to_string_lossy().into_owned(), value.to_string_lossy().into_owned())).collect::<Vec<_>>()).unwrap_or_default();
    if let Some(proxy) = &proxy {
        let address = format!("http://127.0.0.1:{}", proxy.port);
        values.extend([("HTTP_PROXY".into(), address.clone()), ("HTTPS_PROXY".into(), address.clone()),
            ("ALL_PROXY".into(), address), ("NODE_USE_ENV_PROXY".into(), "1".into())]);
    }
    values.extend(secret_values);
    let prepared = started.elapsed();
    let (process, network) = if let Some(sandbox) = &mut sandbox {
        // A `node.exe` server runs with MiniClaw's private Node, which can seed OpenSSL in the sandbox.
        let program = &sandbox.program_for(program).map_err(|error| {
            tracing::warn!(%error, "A confined server's private runtimes could not be granted");
            unprepared(&error)
        })?;
        let runtimes = started.elapsed();
        // Ahead of the server's own values, which still win as before.
        let node_env: Vec<(String, String)> = sandbox.environment().into_iter()
            .filter(|(name, _)| ["PATH", "OPENSSL_CONF", "OPENSSL_MODULES", crate::windows_runtimes::NODE_PRELOAD_PATH, crate::windows_runtimes::PYTHON_HOOK_FOLDER].iter().any(|wanted| name.eq_ignore_ascii_case(wanted)))
            .map(|(name, value)| (name.to_string_lossy().into_owned(), value.to_string_lossy().into_owned())).collect();
        values.splice(0..0, node_env);
        // Only a server with approved hosts or ports, or a Python server, gets the capabilities Winsock needs.
        if proxy.is_some() {
            sandbox.enable_network().map_err(|_| Fault::new("The Windows network service is not available for this server's approved hosts and ports."))?;
        }
        let pending = crate::windows_process::pending_stdio_restricted(program, &args, &folder, &values, &sandbox.security())
            .map_err(|_| Fault::new("The server's restricted process could not be created."))?;
        sandbox.bind_process(&pending.server.process).map_err(|error| Fault(error.to_string()))?;
        let network = if let Some(proxy) = &proxy {
            let mut ports = server.ports.clone(); ports.sort_unstable(); ports.dedup();
            let mut listen = server.listen.clone(); listen.sort_unstable(); listen.dedup();
            Some(crate::windows_network_broker::connect_ports(&pending.server.process, &sandbox.profile_name(), proxy.port, &ports, &listen).await
                .map_err(|error| Fault(if error.kind() == std::io::ErrorKind::Unsupported || error.to_string().starts_with("Network access for sandboxed commands is unavailable") { error.to_string() }
                    else { "The Windows network service could not enforce this server's approved ports and hosts.".into() }))?)
        } else { None };
        let created = started.elapsed();
        let resumed = pending.resume().map_err(|_| Fault::new("The restricted server could not be resumed."))?;
        tracing::debug!(?prepared, ?runtimes, ?created, resumed = ?started.elapsed(), "Started a confined MCP server");
        (resumed, network)
    } else {
        (crate::windows_process::stdio(program, &args, &folder, &values)
            .map_err(|_| Fault::new("The server's command could not be started or contained."))?, None)
    };
    let errors = watch_errors(process.errors);
    Ok(Stdio { _process: process.process, input: tokio::fs::File::from_std(process.input),
        output: FramedRead::new(tokio::fs::File::from_std(process.output), LinesCodec::new_with_max_length(MOST)),
        _proxy: proxy, _sandbox: sandbox, _network: network, errors })
}

/// Builds the HTTP client for one remote server. A public name is resolved here and pinned
/// to public addresses, so it cannot lead to this Mac or the local network; redirects are
/// not followed, so the token goes to the saved address only.
async fn http(server: &Server, secrets: &Secrets) -> Result<Http, Fault> {
    let url = server.url.clone().ok_or_else(|| Fault::new("This server has no address."))?;
    let parsed = reqwest::Url::parse(&url).map_err(|_| Fault::new("The server's address is not valid."))?;
    let host = parsed.host_str().unwrap_or_default().to_owned();
    let mut builder = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10)).user_agent(concat!("MiniClaw/", env!("CARGO_PKG_VERSION")));
    let local = matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]" | "::1");
    if !local {
        let port = parsed.port_or_known_default().unwrap_or(443);
        let found: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), port)).await
            .map(|found| found.filter(|address| !egress::private(&address.ip())).collect()).unwrap_or_default();
        if found.is_empty() { return Err(Fault::new("The server's name does not lead to a public address.")); }
        builder = builder.resolve_to_addrs(&host, &found);
    }
    let token = if server.has_token {
        secrets.get(&token_key(&server.id)).map_err(|_| Fault::new("The credential store could not be read."))?
    } else { None };
    let mut headers = Vec::new();
    for name in &server.headers {
        let value = secrets.get(&header_key(&server.id, name)).map_err(|_| Fault::new("The credential store could not be read."))?
            .ok_or_else(|| Fault::new(format!("The value of the header {name} is missing. Enter it again in the server's settings.")))?;
        headers.push((name.clone(), value));
    }
    Ok(Http { client: builder.build().map_err(|_| Fault::new("The HTTP client could not be prepared."))?,
        url, token, headers, session: None, version: None })
}

impl Stdio {
    /// Writes one message as one line.
    async fn write(&mut self, message: &Value) -> Result<(), Fault> {
        let mut line = message.to_string();
        line.push('\n');
        self.input.write_all(line.as_bytes()).await.map_err(|_| Fault::new("The server stopped."))?;
        self.input.flush().await.map_err(|_| Fault::new("The server stopped."))
    }

    /// Sends a request and reads until its answer, answering the server's own requests on the way.
    async fn request(&mut self, id: u64, message: &Value) -> Result<Value, Fault> {
        self.write(message).await?;
        loop {
            let line = match self.output.next().await {
                Some(Ok(line)) => line,
                Some(Err(_)) => return Err(Fault::new("The server sent a message that is too large or not text.")),
                None => return Err(Fault::new("The server stopped before it answered.")),
            };
            // A line that is not JSON breaks the protocol, but servers do print by mistake; it is skipped.
            let Ok(frame) = serde_json::from_str::<Value>(&line) else { continue };
            if frame.get("method").is_some() {
                if let Some(theirs) = frame.get("id") {
                    let answer = if frame["method"] == "ping" { json!({ "jsonrpc": "2.0", "id": theirs, "result": {} }) }
                        else { json!({ "jsonrpc": "2.0", "id": theirs, "error": { "code": -32601, "message": "Method not found" } }) };
                    self.write(&answer).await?;
                }
                continue;
            }
            if frame["id"].as_u64() == Some(id) { return Ok(frame); }
        }
    }
}

/// Pulls the JSON-RPC answer with this id out of an event stream's text so far, if it is complete.
fn answer_in(events: &str, id: u64) -> Option<Value> {
    events.replace("\r\n", "\n").split("\n\n").find_map(|event| {
        let data: Vec<&str> = event.lines().filter_map(|line| line.strip_prefix("data:")).map(str::trim_start).collect();
        let frame: Value = serde_json::from_str(&data.join("\n")).ok()?;
        (frame.get("method").is_none() && frame["id"].as_u64() == Some(id)).then_some(frame)
    })
}

impl Http {
    /// Posts one message. A request's answer comes back as JSON or as an event stream.
    async fn post(&mut self, id: Option<u64>, message: &Value) -> Result<Value, Fault> {
        let mut request = self.client.post(&self.url).header("Accept", "application/json, text/event-stream").json(message);
        for (name, value) in &self.headers { request = request.header(name, value); }
        if let Some(token) = &self.token { request = request.bearer_auth(token); }
        if let Some(session) = &self.session { request = request.header("Mcp-Session-Id", session); }
        if let Some(version) = &self.version { request = request.header("MCP-Protocol-Version", version); }
        let response = request.send().await.map_err(|_| Fault::new("The server could not be reached."))?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(Fault::new("The server refused the token. Enter a valid token in the server's settings."));
        }
        if status.is_redirection() { return Err(Fault::new("The server's address redirects elsewhere. Save the final address instead.")); }
        if !status.is_success() { return Err(Fault::new(format!("The server answered with HTTP {}.", status.as_u16()))); }
        if let Some(session) = response.headers().get("mcp-session-id").and_then(|value| value.to_str().ok())
            && self.session.is_none() && session.len() <= 512 { self.session = Some(session.to_owned()); }
        let Some(id) = id else { return Ok(Value::Null) };
        let stream = response.headers().get("content-type").and_then(|value| value.to_str().ok())
            .is_some_and(|kind| kind.starts_with("text/event-stream"));
        let mut body = response.bytes_stream();
        let mut text = Vec::new();
        while let Some(piece) = body.next().await {
            text.extend_from_slice(&piece.map_err(|_| Fault::new("The server's answer was cut off."))?);
            if text.len() > MOST { return Err(Fault::new("The server's answer is too large.")); }
            if stream && let Some(frame) = answer_in(&String::from_utf8_lossy(&text), id) { return Ok(frame); }
        }
        if stream { return Err(Fault::new("The server closed its stream before it answered.")); }
        serde_json::from_slice(&text).map_err(|_| Fault::new("The server's answer is not JSON."))
    }
}

impl Link {
    /// Connects for the product. On Windows a confined local server is refused, before any
    /// sandbox exists, while the availability gate is closed; the Mac checks in its own spawn.
    pub async fn open(server: &Server, secrets: &Secrets) -> Result<Self, Fault> {
        #[cfg(windows)]
        if let Some(refused) = refused_by_gate(server) { return Err(refused); }
        Self::connect(server, secrets).await
    }

    /// Starts or reaches the server and completes `initialize`, within the start wait.
    pub async fn connect(server: &Server, secrets: &Secrets) -> Result<Self, Fault> {
        let transport = match server.kind {
            Kind::Stdio => Transport::Stdio(Box::new(spawn(server, secrets).await?)),
            Kind::Http => Transport::Http(Box::new(http(server, secrets).await?)),
        };
        let mut link = Self { transport, next: 1, wait: start_wait(server), info: Value::Null };
        let hello = json!({ "protocolVersion": VERSION, "capabilities": {},
            "clientInfo": { "name": "MiniClaw", "version": env!("CARGO_PKG_VERSION") } });
        let asked = std::time::Instant::now();
        let result = link.request("initialize", hello, link.wait).await;
        tracing::debug!(elapsed = ?asked.elapsed(), ok = result.is_ok(), "An MCP server answered initialize");
        let result = result?;
        let version = result["protocolVersion"].as_str().unwrap_or_default().to_owned();
        if !SPOKEN.contains(&version.as_str()) { return Err(Fault::new("The server speaks an MCP version MiniClaw does not.")); }
        if let Transport::Http(http) = &mut link.transport { http.version = Some(version); }
        link.info = json!({ "name": result["serverInfo"]["name"].as_str().unwrap_or_default().chars().take(80).collect::<String>(),
            "version": result["serverInfo"]["version"].as_str().unwrap_or_default().chars().take(40).collect::<String>() });
        link.notify("notifications/initialized").await?;
        Ok(link)
    }

    /// Says why a Windows server stopped or did not answer when MiniClaw's Node refused it a child
    /// process. Its standard error may still be arriving: up to one second is given for it to end.
    fn explain(&self, fault: Fault) -> impl std::future::Future<Output = Fault> + Send + use<> {
        #[cfg(windows)]
        let errors = match &self.transport { Transport::Stdio(stdio) => Some(stdio.errors.clone()), Transport::Http(_) => None };
        async move {
            #[cfg(windows)]
            if let Some(errors) = errors {
                use std::sync::atomic::Ordering;
                let deadline = std::time::Instant::now() + Duration::from_secs(1);
                while !errors.refused.load(Ordering::Acquire) && !errors.ended.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                return explain_refusal(fault, errors.refused.load(Ordering::Acquire));
            }
            fault
        }
    }

    /// Sends a notification, which has no answer.
    async fn notify(&mut self, method: &str) -> Result<(), Fault> {
        let message = json!({ "jsonrpc": "2.0", "method": method });
        match &mut self.transport {
            Transport::Stdio(stdio) => stdio.write(&message).await,
            Transport::Http(http) => http.post(None, &message).await.map(|_| ()),
        }
    }

    /// Sends one request and returns its `result`, or the server's error in words.
    pub async fn request(&mut self, method: &str, params: Value, wait: Duration) -> Result<Value, Fault> {
        let id = self.next;
        self.next += 1;
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let answer = tokio::time::timeout(wait, async {
            match &mut self.transport {
                Transport::Stdio(stdio) => stdio.request(id, &message).await,
                Transport::Http(http) => http.post(Some(id), &message).await,
            }
        }).await.unwrap_or_else(|_| Err(Fault::new("The server did not answer in time.")));
        let answer = match answer { Ok(answer) => answer, Err(fault) => return Err(self.explain(fault).await) };
        if let Some(error) = answer.get("error") {
            let words: String = error["message"].as_str().unwrap_or("no reason given").chars().take(500).collect();
            return Err(Fault::new(format!("The server refused: {words}")));
        }
        Ok(answer["result"].clone())
    }

    /// Lists the server's tools across its pages, up to the limit.
    pub async fn tools(&mut self) -> Result<Vec<Value>, Fault> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..16 {
            let params = cursor.as_ref().map_or_else(|| json!({}), |cursor| json!({ "cursor": cursor }));
            let page = self.request("tools/list", params, self.wait).await?;
            tools.extend(page["tools"].as_array().cloned().unwrap_or_default().into_iter()
                .filter(|tool| tool["name"].as_str().is_some_and(|name| !name.is_empty() && name.len() <= 128)));
            cursor = page["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() || tools.len() >= MOST_TOOLS { break; }
        }
        tools.truncate(MOST_TOOLS);
        Ok(tools)
    }

    /// Calls one tool and returns the server's result object unchanged.
    pub async fn call(&mut self, name: &str, arguments: Value, wait: Duration) -> Result<Value, Fault> {
        self.request("tools/call", json!({ "name": name, "arguments": arguments }), wait).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_grants_are_explicit_read_only_directories() {
        let root = std::env::temp_dir().join(format!("mc-runtime-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("python.exe"), b"fixture").unwrap();
        let paths = vec![root.to_string_lossy().into_owned()];
        let grants = runtime_grants(&paths).unwrap();
        assert_eq!(grants.len(), 1);
        assert!(grants[0].folder && !grants[0].write && !grants[0].blocked);
        assert!(runtime_grants(&[root.join("python.exe").to_string_lossy().into_owned()]).is_err());
        assert!(runtime_grants(&vec![paths[0].clone(); 9]).is_err());
        #[cfg(windows)]
        assert!(runtime_grants(&["C:\\".into()]).is_err());
        #[cfg(unix)]
        assert!(runtime_grants(&["/".into()]).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_confined_local_server_needs_the_windows_sandbox() {
        let server = |extra: Value| -> Server {
            let mut entry = json!({ "name": "fixture", "kind": "stdio", "command": r"C:\Tools\server.exe" });
            entry.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            serde_json::from_value(entry).unwrap()
        };
        // Closed: a confined server is refused with the gate's own reason after the fixed sentence.
        let reason = "The MiniClaw network service is not running. Start it.";
        let refused = closed_gate(&server(json!({})), Some(reason)).expect("a confined server is refused");
        assert_eq!(refused.0, format!("This PC cannot enforce the sandbox, so the server was not started. {reason}"));
        assert_eq!(closed_gate(&server(json!({})), Some("")).unwrap().0, "This PC cannot enforce the sandbox, so the server was not started.");
        // The user's own choice to run a server unrestricted, and remote servers, need no sandbox.
        assert!(closed_gate(&server(json!({ "unrestricted": true })), Some(reason)).is_none());
        assert!(closed_gate(&server(json!({ "kind": "http", "command": null, "url": "https://mcp.example.com/mcp" })), Some(reason)).is_none());
        // Open: nothing is refused here, confined or not.
        for extra in [json!({}), json!({ "unrestricted": true }), json!({ "kind": "http", "command": null, "url": "https://mcp.example.com/mcp" })] {
            assert!(closed_gate(&server(extra), None).is_none());
        }
    }

    /// A sandboxed server that declares listening ports (Roblox Studio's 13469) is refused before any
    /// sandbox exists, with the way to run it named; an unrestricted one, or one without, is not.
    #[test]
    fn a_sandboxed_listener_is_refused_before_preparation_with_the_way_out() {
        let server = |extra: Value| -> Server {
            let mut entry = json!({ "name": "roblox_studio", "kind": "stdio", "command": r"C:\WINDOWS\System32\cmd.exe",
                "args": ["/d", "/s", "/c", ".\\mcp.bat"], "listen": [13469] });
            entry.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            serde_json::from_value(entry).unwrap()
        };
        let refused = declared_listener(&server(json!({})), false).expect("a sandboxed listener is refused").0;
        for words in ["roblox_studio listens on port 13469", "cannot listen on Windows yet", "changed no folder permissions",
            "Settings > MCP servers", "\"Run outside the sandbox\""] {
            assert!(refused.contains(words), "{words}: {refused}");
        }
        assert!(!refused.contains("ARM64"), "{refused}");
        let arm = declared_listener(&server(json!({})), true).unwrap().0;
        assert!(arm.starts_with(&refused) && arm.contains("ARM64 PC, x64 programs such as Roblox Studio's StudioMCP.exe"), "{arm}");
        assert!(declared_listener(&server(json!({ "unrestricted": true })), true).is_none());
        assert!(declared_listener(&server(json!({ "listen": [] })), true).is_none());
    }

    /// A `node.exe` server runs on MiniClaw's private Node, so the folder of the Node it named is not granted:
    /// that grant stamped every file of a Node installation (npm included) at each launch, only for a program
    /// that never runs. Without the private Node, or for any other program, the installation is granted as before.
    #[test]
    fn a_node_server_on_the_private_node_gets_no_grant_for_the_users_node() {
        let node = std::path::Path::new(r"C:\Program Files\nodejs\node.exe");
        let python = std::path::Path::new(r"C:\Python313\python.exe");
        assert!(!install_grant_wanted(node, false, true));
        assert!(install_grant_wanted(node, false, false));
        assert!(install_grant_wanted(python, false, true));
        // An npm-launched server runs its installed package with the private Node and never had this grant.
        assert!(!install_grant_wanted(node, true, true) && !install_grant_wanted(python, true, false));
    }

    #[test]
    fn a_child_folder_is_a_plain_drive_path() {
        assert_eq!(child_folder(std::path::Path::new(r"\\?\C:\Users\S\server folder")), std::path::PathBuf::from(r"C:\Users\S\server folder"));
        assert_eq!(child_folder(std::path::Path::new(r"C:\Users\S\server")), std::path::PathBuf::from(r"C:\Users\S\server"));
        // A verbatim network or device path has no plain drive form and is left as it is.
        assert_eq!(child_folder(std::path::Path::new(r"\\?\UNC\host\share\x")), std::path::PathBuf::from(r"\\?\UNC\host\share\x"));
        assert_eq!(child_folder(std::path::Path::new(r"\\?\Volume{1b3b1146-4076-11e1-84aa-806e6f6e6963}\x")),
            std::path::PathBuf::from(r"\\?\Volume{1b3b1146-4076-11e1-84aa-806e6f6e6963}\x"));
    }

    #[test]
    fn a_refused_child_is_found_on_standard_error_and_explains_the_failure() {
        let code = crate::windows_runtimes::CHILD_PIPES_CODE;
        // Found in one read, split across reads, or not at all.
        let mut watch = RefusalWatch::default();
        assert!(!watch.feed(b"starting\n"));
        assert!(watch.feed(format!("MiniClaw {code}: refused\n").as_bytes()));
        assert!(watch.feed(b"later output"), "once seen, it stays seen");
        let mut split = RefusalWatch::default();
        let (head, tail) = code.split_at(7);
        assert!(!split.feed(format!("x MiniClaw {head}").as_bytes()));
        assert!(split.feed(format!("{tail}: refused").as_bytes()));
        let mut other = RefusalWatch::default();
        for _ in 0..1000 { assert!(!other.feed(b"ERR_MINICLAW_SANDBOX_CHILD_PIP\nnoise ")); }
        // Only a server that stopped or did not answer gets the reason; its own refusals stay its own.
        for words in ["The server stopped before it answered.", "The server did not answer in time.", "The server stopped."] {
            assert_eq!(explain_refusal(Fault::new(words), true).0, CHILD_PIPES_FAULT);
            assert_eq!(explain_refusal(Fault::new(words), false).0, words);
        }
        assert_eq!(explain_refusal(Fault::new("The server refused: bad input"), true).0, "The server refused: bad input");
        assert!(CHILD_PIPES_FAULT.starts_with("This server starts a child process, which the Windows sandbox cannot support with the bundled Node yet."));
    }

    #[test]
    fn an_answer_is_found_after_other_events() {
        let events = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\n\
            event: message\r\nid: 4\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"ok\":true}}\r\n\r\n";
        assert_eq!(answer_in(events, 7).unwrap()["result"]["ok"], true);
        assert!(answer_in(events, 8).is_none());
        assert!(answer_in("data: {\"jsonrpc\":\"2.0\",\"id\":7,\"resu", 7).is_none());
    }
}

#[cfg(all(test, windows))]
pub(crate) mod windows_tests {
    use super::*;

    /// A stdio MCP server written for the pinned shell, which starts inside the sandbox where
    /// Windows PowerShell cannot. It answers `initialize`, `write` (a file in its folder),
    /// `read` and `probe` (whether a path and a loopback port are out of reach), and
    /// anything else with an empty tool list. Its one saved value is `HOSTNAME`: without it
    /// the shell asks Winsock for the name, which the sandbox refuses, and exits.
    pub(crate) fn shell_server(extra: Value, forbidden: &str, port: u16) -> (Server, Secrets) {
        let runtime = crate::windows_shell::runtime().expect("the pinned shell is staged beside the test binary");
        let script = format!(r#"
denied() {{ if cat "$1" >/dev/null 2>&1; then printf false; else printf true; fi; }}
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  [ -z "$id" ] && continue
  case "$line" in
    *'"method":"initialize"'*) result='{{"protocolVersion":"2025-06-18","capabilities":{{}},"serverInfo":{{"name":"fixture","version":"1"}}}}' ;;
    *'"method":"write"'*) printf RUN_SECRET > private.txt; result="{{\"path\":\"$(pwd)/private.txt\"}}" ;;
    *'"method":"read"'*) path=$(printf '%s' "$line" | sed -n 's/.*"path":"\([^"]*\)".*/\1/p'); result="{{\"denied\":$(denied "$path")}}" ;;
    *'"method":"proxy"'*) answer=$(printf 'CONNECT probe.invalid:443 HTTP/1.1\r\n\r\n' | nc 127.0.0.1 "${{HTTP_PROXY##*:}}" 2>/dev/null | head -n 1)
      case "$answer" in *403*) net=true ;; *) net=false ;; esac
      result="{{\"reached\":$net,\"name\":\"$(env -u HOSTNAME hostname 2>/dev/null)\"}}" ;;
    *'"method":"probe"'*) if nc 127.0.0.1 {port} </dev/null >/dev/null 2>&1; then net=false; else net=true; fi
      result="{{\"fileDenied\":$(denied '{forbidden}'),\"networkDenied\":$net}}" ;;
    *) result='{{"tools":[]}}' ;;
  esac
  printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":$result}}"
done
"#);
        let mut entry = json!({"id":"fixture","name":"shell fixture","kind":"stdio","command":runtime.program,"args":["sh","-c",script],
            "env":["HOSTNAME"],"startupSeconds":30});
        entry.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let server: Server = serde_json::from_value(entry).unwrap();
        let secrets = Secrets::File(std::env::temp_dir().join(format!("mc-fixture-secrets-{}", uuid::Uuid::new_v4())));
        secrets.set(&env_key(&server.id, "HOSTNAME"), "miniclaw").expect("the fixture's value is saved");
        (server, secrets)
    }

    /// A confined server starts exactly when the gate is open. Closed, both entry points refuse it
    /// with the gate's reason before any sandbox exists, and the user's own unrestricted server
    /// still starts. Asserts whichever state this run is in.
    #[tokio::test]
    async fn a_confined_server_follows_the_gate() {
        let (server, secrets) = shell_server(json!({}), "unused", 9);
        let journals = || std::fs::read_dir(std::path::PathBuf::from(std::env::var_os("MINICLAW_DATA_DIR").expect("Isolated native test data root"))
            .join("sandbox-journals")).into_iter().flatten().flatten().filter(|entry| entry.file_name().to_string_lossy().ends_with(".jsonl")).count();
        match crate::seatbelt::unavailable_reason() {
            None => {
                let mut link = Link::open(&server, &secrets).await.unwrap_or_else(|fault| panic!("an open gate starts the server: {}", fault.0));
                let result = link.request("write", json!({}), Duration::from_secs(10)).await.unwrap();
                assert!(result["path"].as_str().is_some_and(|path| path.contains("sandbox-scratch")), "{result}");
            }
            Some(reason) => {
                let before = journals();
                let expected = format!("This PC cannot enforce the sandbox, so the server was not started. {reason}");
                for (name, attempt) in [("open", Link::open(&server, &secrets).await), ("connect", Link::connect(&server, &secrets).await)] {
                    match attempt {
                        Ok(_) => panic!("{name} started a confined server behind a closed gate"),
                        Err(fault) => assert_eq!(fault.0, expected, "{name}"),
                    }
                }
                assert_eq!(journals(), before, "no sandbox was created");
                let (free, secrets) = shell_server(json!({"unrestricted":true}), "unused", 9);
                let mut link = Link::open(&free, &secrets).await.unwrap_or_else(|fault| panic!("an unrestricted server needs no sandbox: {}", fault.0));
                let answer = link.request("tools/list", json!({}), Duration::from_secs(10)).await.unwrap();
                assert_eq!(answer["tools"], json!([]));
            }
        }
    }

    /// Default server workspaces belong to protected per-run scratch, inaccessible to peer packages.
    #[tokio::test]
    async fn default_mcp_workspaces_are_private_between_runs() {
        let (server, secrets) = shell_server(json!({}), "unused", 9);
        let mut first = Link::connect(&server, &secrets).await.unwrap();
        let result = first.request("write", json!({}), Duration::from_secs(10)).await.unwrap();
        let path = std::path::PathBuf::from(result["path"].as_str().unwrap());
        let data = std::path::PathBuf::from(std::env::var_os("MINICLAW_DATA_DIR").expect("Isolated native test data root"));
        assert!(path.starts_with(data.join("sandbox-scratch")), "{}", path.display());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "RUN_SECRET");
        // The owner of the file can be refused nothing here, so a denial below is the sandbox's.
        let own = first.request("read", json!({"path":path}), Duration::from_secs(10)).await.unwrap();
        assert_eq!(own["denied"], false);
        let mut second = Link::connect(&server, &secrets).await.unwrap();
        let denied = second.request("read", json!({"path":path}), Duration::from_secs(10)).await.unwrap();
        assert_eq!(denied["denied"], true);
    }

    /// A confined stdio server initializes and answers while unrelated files/network stay denied.
    #[tokio::test]
    async fn sandboxed_mcp_roundtrip_denies_ungranted_resources() {
        let root = std::env::temp_dir().join(format!("mc-mcp-native-{}", uuid::Uuid::new_v4()));
        let allowed = root.join("allowed");
        std::fs::create_dir_all(&allowed).unwrap();
        let forbidden = root.join("forbidden.txt");
        std::fs::write(&forbidden, "PRIVATE_SENTINEL").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (server, secrets) = shell_server(json!({"folder":allowed}), &forbidden.to_string_lossy().replace('\\', "/"), port);
        let mut link = Link::connect(&server, &secrets).await.unwrap();
        let result = link.request("probe", json!({}), Duration::from_secs(10)).await.unwrap();
        assert_eq!(result["fileDenied"], true);
        assert_eq!(result["networkDenied"], true);
        // The same probe of a file inside the granted folder is not refused.
        std::fs::write(allowed.join("inside.txt"), "inside").unwrap();
        let inside = link.request("read", json!({"path":allowed.join("inside.txt").to_string_lossy().replace('\\', "/")}), Duration::from_secs(10)).await.unwrap();
        assert_eq!(inside["denied"], false);
        drop(link);
        assert!(tokio::time::timeout(Duration::from_millis(100), listener.accept()).await.is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    /// A confined server with an approved host initialises Winsock and reaches its proxy, and only
    /// its proxy: another live loopback port stays out of reach.
    #[tokio::test]
    async fn networked_mcp_server_reaches_only_its_proxy() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (server, secrets) = shell_server(json!({"hosts":["example.com"]}), "unused", port);
        let mut link = Link::connect(&server, &secrets).await.unwrap();
        // The proxy answers a host that was not approved with 403; the answer proves the round trip.
        let reached = link.request("proxy", json!({}), Duration::from_secs(10)).await.unwrap();
        assert_eq!(reached["reached"], true, "{reached}");
        // The shell's own host-name lookup needs Winsock; it works in a networked run.
        assert_ne!(reached["name"], "", "{reached}");
        let probe = link.request("probe", json!({}), Duration::from_secs(10)).await.unwrap();
        assert_eq!(probe["networkDenied"], true, "{probe}");
        drop(link);
        assert!(tokio::time::timeout(Duration::from_millis(100), listener.accept()).await.is_err());
    }
}
