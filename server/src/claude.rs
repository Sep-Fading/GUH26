// Stores threads, active turns, and workspace paths.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

// Creates short private sockets on Unix.
#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt;
// Selects the platform bridge transport type.
#[cfg(unix)]
use tokio::net::{UnixListener as ToolListener};
#[cfg(windows)]
use tokio::net::{TcpListener as ToolListener};

// Reads bounded JSON lines from streams.
use futures_util::StreamExt;
// Persists saved threads as private JSON.
use serde::{Deserialize, Serialize};
// Builds App Server replies and notifications.
use serde_json::{Value, json};
// Carries frames between bridge and client.
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt, DuplexStream},
    process::Command,
    sync::{mpsc, oneshot},
};
// Frames CLI and client output lines.
use tokio_util::codec::{FramedRead, LinesCodec, LinesCodecError};
// Creates thread and turn identifiers.
use uuid::Uuid;

// Returns the bridge as an ordinary client.
use crate::rpc::{Client, Failure};

/// Marks bridge threads in saved summaries, separate from Codex's `openai` tasks.
pub const PROVIDER: &str = "anthropic";

const BASE: &str = "You are MiniClaw, a helpful conversational assistant. Answer the user directly. No tools are available.";

#[derive(Clone, Serialize, Deserialize)]
struct Item {
    id: String,
    role: String,
    text: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Turn {
    id: String,
    status: String,
    items: Vec<Item>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Thread {
    id: String,
    model: String,
    effort: Option<String>,
    instructions: String,
    created_at: i64,
    // The CLI session exists once one request reached the model.
    started: bool,
    // Set when the backend offered the MiniClaw tool to this chat.
    #[serde(default)]
    tools: bool,
    // The delegation tool as the backend described it, with this agent's recipients by name.
    #[serde(default)]
    delegate: Option<Value>,
    // Set when the backend offered the MCP gateway to this chat.
    #[serde(default)]
    mcp: bool,
    // Set when the backend offered the question tool to this chat.
    #[serde(default)]
    ask: bool,
    // Set when the backend offered the project tool to this chat.
    #[serde(default)]
    project: bool,
    turns: Vec<Turn>,
}

struct Active {
    turn: String,
    // Identifies this turn's MCP shim on the private tool socket.
    token: String,
    stop: Option<oneshot::Sender<()>>,
    // Independent built-in tool choices and fetch rules captured for this turn.
    web: Option<NativeTools>,
    // Set when an MCP server is switched on for this conversation as the turn starts. Without
    // one the turn has no gateway, and a turn with no tool at all still runs in safe mode.
    mcp: bool,
    // Set unless asking questions is switched off for this conversation as the turn starts.
    ask: bool,
}

/// Independent built-in tools and plan mode for one Claude turn. Fetch rules constrain
/// hosts; the CLI also permits its own documentation sites (docs/native-tools-probe).
#[derive(Clone, Default)]
struct NativeTools {
    fetch: bool,
    subagents: bool,
    tracking: bool,
    plan: bool,
    // Search starts on; each additional tool starts off.
    search: bool,
    hosts: Vec<String>,
    domains: Vec<String>,
    blocked_hosts: Vec<String>,
    blocked_domains: Vec<String>,
}

impl NativeTools {
    /// Reads the backend's decision from a turn's parameters; absent means off.
    fn read(params: &Value) -> Option<Self> {
        let web = params.get("miniclawNativeTools")?.as_object()?;
        // A name is passed to the CLI inside a rule, so it must be a plain DNS name.
        let names = |key: &str| web.get(key).and_then(Value::as_array).map(|list| list.iter()
            .filter_map(Value::as_str).filter(|name| crate::permissions::dns_name(name))
            .map(str::to_owned).collect::<Vec<_>>()).unwrap_or_default();
        Some(Self { fetch: web.get("fetch") == Some(&json!(true)),
            subagents: web.get("subagents") == Some(&json!(true)),
            tracking: web.get("tracking") == Some(&json!(true)),
            plan: web.get("plan") == Some(&json!(true)),
            search: web.get("search") == Some(&json!(true)),
            hosts: names("hosts"), domains: names("domains"),
            blocked_hosts: names("blockedHosts"), blocked_domains: names("blockedDomains") })
    }

    /// Lists only the tools selected for this turn; plan mode adds no tool.
    fn enabled_tools(&self) -> Vec<&'static str> {
        let mut tools = Vec::new();
        if self.search { tools.push("WebSearch"); }
        if self.fetch { tools.push("WebFetch"); }
        if self.subagents { tools.push("Task"); }
        if self.tracking { tools.extend(["TaskCreate", "TaskGet", "TaskList", "TaskUpdate"]); }
        tools
    }

    /// Writes the CLI's fetch rules. The CLI matches a host exactly, so a domain needs its
    /// wildcard form as well (both measured: docs/native-tools-probe).
    fn rules(hosts: &[String], domains: &[String]) -> Vec<String> {
        hosts.iter().map(|host| format!("WebFetch(domain:{host})"))
            .chain(domains.iter().flat_map(|domain| [format!("WebFetch(domain:{domain})"),
                format!("WebFetch(domain:*.{domain})")])).collect()
    }
}

/// What one `claude auth login` run has shown so far.
#[derive(Default)]
struct LoginProgress {
    // The CLI's sign-in page, from its "If the browser didn't open, visit:" line.
    url: Option<String>,
    // The CLI's latest error line: why it stopped, or a hint such as an invalid pasted code.
    message: Option<String>,
    // Output streams still being read; the CLI's exit is reported after both have ended.
    reading: u8,
}

/// One running `claude auth login`, with the input that takes a pasted code.
struct LoginAttempt {
    child: crate::provider_process::Child,
    input: Arc<tokio::sync::Mutex<Option<Box<dyn AsyncWrite + Send + Unpin>>>>,
    progress: Arc<Mutex<LoginProgress>>,
    // Set once an account reading has followed the CLI's exit.
    exit_seen: bool,
}

struct Bridge {
    binary: String,
    cwd: String,
    store: PathBuf,
    threads: Mutex<HashMap<String, Thread>>,
    active: Mutex<HashMap<String, Active>>,
    // A short cache keeps per-request sign-in checks from spawning the CLI each time.
    account: Mutex<Option<(Instant, Value)>>,
    // Lets one caller at a time start `claude auth status`; the others reuse its reading.
    account_gate: tokio::sync::Mutex<()>,
    // Discovery is shared by the picker and all model/effort validation paths.
    models: Mutex<Option<(Instant, Value, Vec<Value>)>>,
    model_gate: tokio::sync::Mutex<()>,
    login: Mutex<Option<LoginAttempt>>,
    // Tool calls awaiting the backend's answer, keyed by server-request id.
    calls: Mutex<HashMap<String, oneshot::Sender<Value>>>,
    socket: Option<PathBuf>,
    out: mpsc::Sender<Value>,
}

/// Starts the in-process bridge that presents the Claude Code CLI through the
/// App Server protocol subset MiniClaw uses. No CLI process starts until needed.
pub async fn launch(binary: String, cwd: String, store: PathBuf) -> Result<Client, Failure> {
    crate::platform_fs::private_dir(&store, true)
        .map_err(|_| Failure::Unavailable)?;
    let threads = load(&store).map_err(|_| Failure::Unavailable)?;
    let (near, far) = tokio::io::duplex(64 * 1024);
    let (reader, mut writer) = tokio::io::split(far);
    let (out, mut frames) = mpsc::channel::<Value>(256);
    let listener = tool_socket();
    let bridge = Arc::new(Bridge { binary, cwd, store, threads: Mutex::new(threads),
        active: Mutex::new(HashMap::new()), account: Mutex::new(None), account_gate: tokio::sync::Mutex::new(()),
        models: Mutex::new(None), model_gate: tokio::sync::Mutex::new(()),
        login: Mutex::new(None), calls: Mutex::new(HashMap::new()),
        socket: listener.as_ref().map(|(path, _)| path.clone()), out });
    let accepting = listener.map(|(_, listener)| tokio::spawn(accept(bridge.clone(), listener)));
    tokio::spawn(async move {
        while let Some(frame) = frames.recv().await {
            if writer.write_all(format!("{frame}\n").as_bytes()).await.is_err() { break; }
        }
    });
    tokio::spawn(async move {
        serve(bridge, reader).await;
        if let Some(task) = accepting { task.abort(); }
    });
    Client::attach(near).await
}

/// Binds the private socket the MCP shim calls back on. Unix socket paths are limited
/// to about 100 bytes, so it lives in a short private temp directory, not the data
/// directory. Without a socket, Claude chats run without tools.
#[cfg(unix)]
fn tool_socket() -> Option<(PathBuf, ToolListener)> {
    // A backend killed before cleanup leaves its directory; sweep those of dead processes.
    for entry in std::fs::read_dir(std::env::temp_dir()).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let owner = name.to_str().and_then(|name| name.strip_prefix("mc-tools-"))
            .and_then(|pid| pid.parse::<i32>().ok());
        if let Some(pid) = owner && unsafe { libc::kill(pid, 0) } != 0 {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    let directory = std::env::temp_dir().join(format!("mc-tools-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::DirBuilder::new().mode(0o700).create(&directory).ok()?;
    let path = directory.join(format!("{}.sock", &Uuid::new_v4().simple().to_string()[..8]));
    if path.as_os_str().len() > 100 { return None; }
    let listener = tokio::net::UnixListener::bind(&path).ok()?;
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).ok()?;
    Some((path, listener))
}

/// Binds only numeric localhost; each call must authenticate its active turn.
#[cfg(windows)]
fn tool_socket() -> Option<(PathBuf, ToolListener)> {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).ok()?;
    let address = listener.local_addr().ok()?.to_string();
    listener.set_nonblocking(true).ok()?;
    Some((PathBuf::from(address), ToolListener::from_std(listener).ok()?))
}

/// Accepts authenticated shim connections until the bridge closes.
async fn accept(bridge: Arc<Bridge>, listener: ToolListener) {
    while let Ok((stream, _)) = listener.accept().await {
        if bridge.out.is_closed() { break; }
        tokio::spawn(bridge.clone().tool_call(stream));
    }
}

/// Reads saved threads; a turn left running by a previous process becomes interrupted.
fn load(store: &Path) -> std::io::Result<HashMap<String, Thread>> {
    let mut threads = HashMap::new();
    for entry in std::fs::read_dir(store)? {
        let path = entry?.path();
        if path.extension().is_none_or(|kind| kind != "json") || threads.len() >= 4096 { continue; }
        let Ok(mut thread) = serde_json::from_slice::<Thread>(&std::fs::read(&path)?)
            else { continue; };
        if path.file_stem().and_then(|name| name.to_str()) != Some(thread.id.as_str()) { continue; }
        let mut repaired = false;
        for turn in &mut thread.turns {
            if turn.status == "inProgress" {
                turn.status = "interrupted".into();
                repaired = true;
            }
        }
        if repaired { save(store, &thread)?; }
        threads.insert(thread.id.clone(), thread);
    }
    Ok(threads)
}

/// Replaces one private thread file atomically.
fn save(store: &Path, thread: &Thread) -> std::io::Result<()> {
    let temporary = store.join(format!("{}.tmp", thread.id));
    let mut file = crate::platform_fs::private_file(&temporary, false)?;
    std::io::Write::write_all(&mut file, &serde_json::to_vec(thread)?)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(temporary, store.join(format!("{}.json", thread.id)))
}

/// Answers client requests until the client closes, then stops every running CLI.
async fn serve(bridge: Arc<Bridge>, reader: tokio::io::ReadHalf<DuplexStream>) {
    let mut lines = FramedRead::new(reader, LinesCodec::new_with_max_length(8 * 1024 * 1024));
    while let Some(Ok(line)) = lines.next().await {
        let Ok(frame) = serde_json::from_str::<Value>(&line) else { break; };
        // A reply to one of the bridge's own tool requests.
        if frame.get("method").is_none() && let Some(id) = frame["id"].as_str() {
            if let Some(waiting) = bridge.calls.lock().unwrap().remove(id) {
                let _ = waiting.send(frame["result"].clone());
            }
            continue;
        }
        let (Some(id), Some(method)) = (frame.get("id"), frame["method"].as_str())
            else { continue; };
        // Discovery and usage must not hold up chat or Stop while the network is slow.
        if matches!(method, "account/rateLimits/read" | "model/list") {
            let bridge = bridge.clone();
            let id = id.clone();
            let models = method == "model/list";
            let refresh = frame["params"]["refresh"] == true;
            tokio::spawn(async move {
                let result = if models {
                    bridge.models(refresh).await.map(|data| json!({ "data": data, "nextCursor": null }))
                } else { bridge.usage().await };
                let reply = match result {
                    Some(result) => json!({ "id": id, "result": result }),
                    None => json!({ "id": id, "error": { "code": -32600, "message": "Provider data unavailable" } }),
                };
                let _ = bridge.out.send(reply).await;
            });
            continue;
        }
        let reply = match bridge.clone().handle(method, &frame["params"]).await {
            Some(result) => json!({ "id": id, "result": result }),
            None => json!({ "id": id, "error": { "code": -32600, "message": "Rejected" } }),
        };
        if bridge.out.send(reply).await.is_err() { break; }
        // The acknowledgement precedes the first notification of a new turn.
        if method == "turn/start" { bridge.clone().begin(&frame["params"]); }
    }
    bridge.active.lock().unwrap().clear();
    #[cfg(unix)]
    if let Some(directory) = bridge.socket.as_ref().and_then(|socket| socket.parent()) {
        let _ = std::fs::remove_dir_all(directory);
    }
}

impl Bridge {
    /// Reads structured plan limits through Claude's existing sign-in, without a model
    /// prompt, local transcript scan, or persisted session. Output and runtime are bounded.
    async fn usage(&self) -> Option<Value> {
        self.control(&[("init", json!({ "subtype": "initialize" })),
            ("usage", json!({ "subtype": "get_usage", "skip_behaviors": true }))], "usage").await
    }

    /// Reads the CLI's model metadata, caching successful discovery for five minutes.
    async fn models(&self, refresh: bool) -> Option<Vec<Value>> {
        let _guard = self.model_gate.lock().await;
        let account = self.account().await;
        if account["account"]["type"] != "claude" {
            *self.models.lock().unwrap() = None;
            return Some(Vec::new());
        }
        if !refresh && let Some((read, owner, models)) = self.models.lock().unwrap().as_ref()
            && owner == &account && read.elapsed() < Duration::from_secs(300) {
            return Some(models.clone());
        }
        let response = self.control(&[("models", json!({ "subtype": "initialize" }))], "models").await?;
        let models = catalog(&response)?;
        if self.account().await != account { return None; }
        *self.models.lock().unwrap() = Some((Instant::now(), account, models.clone()));
        Some(models)
    }

    /// Finds a model in the last catalog accepted by the validation path.
    fn model(&self, name: &str) -> Option<Value> {
        self.models.lock().unwrap().as_ref()?.2.iter()
            .find(|entry| entry["model"] == name).cloned()
    }

    /// Returns one bounded control response without a prompt, tools or saved session.
    async fn control(&self, requests: &[(&str, Value)], target: &str) -> Option<Value> {
        tokio::time::timeout(Duration::from_secs(10), async {
            let mut command = Command::new(&self.binary);
            command
                .args(["-p", "--safe-mode", "--tools", "", "--strict-mcp-config", "--setting-sources", "",
                    "--no-session-persistence", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"])
                .current_dir(&self.cwd)
                .env_remove("ANTHROPIC_API_KEY").env_remove("ANTHROPIC_AUTH_TOKEN").env_remove("ANTHROPIC_BASE_URL")
                .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
                .kill_on_drop(true);
            let mut child = crate::provider_process::spawn(&mut command).ok()?;
            let mut input = child.stdin.take()?;
            for (id, request) in requests {
                let frame = json!({ "type": "control_request", "request_id": id, "request": request });
                input.write_all(format!("{frame}\n").as_bytes()).await.ok()?;
            }
            input.flush().await.ok()?;
            drop(input);
            let mut output = FramedRead::new(child.stdout.take()?, LinesCodec::new_with_max_length(256 * 1024));
            let mut result = None;
            let mut bytes = 0;
            while let Some(line) = output.next().await {
                let line = line.ok()?;
                bytes += line.len();
                if bytes > 512 * 1024 { return None; }
                let frame: Value = serde_json::from_str(&line).ok()?;
                if frame["type"] == "control_response" && frame["response"]["request_id"] == target {
                    if frame["response"]["subtype"] != "success" { return None; }
                    result = frame["response"].get("response").cloned();
                }
            }
            if !child.wait().await.ok()?.success() { return None; }
            result
        }).await.ok().flatten()
    }

    /// Returns one method's result, or None for a rejected or unsupported request.
    async fn handle(self: Arc<Self>, method: &str, params: &Value) -> Option<Value> {
        match method {
            "initialize" => Some(json!({})),
            "account/read" => Some(self.account().await),
            "account/login/start" => self.login(),
            "account/login/read" => Some(self.login_read()),
            "account/login/code" => self.login_code(params).await,
            "account/login/cancel" => {
                self.login.lock().unwrap().take();
                Some(json!({}))
            }
            "thread/start" => self.start(params),
            "thread/read" => self.thread(params).map(|thread|
                json!({ "thread": self.view(&thread) })),
            "thread/resume" => self.resume(params),
            "thread/list" => self.list(params),
            "thread/turns/list" => self.turns(params),
            "thread/delete" => self.delete(params),
            "turn/start" => self.reserve(params),
            "turn/interrupt" => self.interrupt(params),
            _ => None,
        }
    }

    /// Relays one shim call as the same `item/tool/call` server request Codex sends, so
    /// the backend applies one consent and sandbox path to both providers.
    async fn tool_call<S>(self: Arc<Self>, stream: S)
    where S: AsyncRead + AsyncWrite + Unpin {
        let (reader, mut writer) = tokio::io::split(stream);
        let mut lines = FramedRead::new(reader, LinesCodec::new_with_max_length(256 * 1024));
        let Ok(Some(Ok(line))) = tokio::time::timeout(Duration::from_secs(5), lines.next()).await
            else { return; };
        let Ok(request) = serde_json::from_str::<Value>(&line) else { return; };
        let owner = self.active.lock().unwrap().iter()
            .find(|(_, active)| Some(active.token.as_str()) == request["token"].as_str())
            .map(|(thread, active)| (thread.clone(), active.turn.clone()));
        let refused = json!({ "success": false, "contentItems": [] });
        let result = match owner {
            Some((thread, turn)) => {
                let id = format!("claude-tool-{}", Uuid::new_v4());
                let (answer, answered) = oneshot::channel();
                self.calls.lock().unwrap().insert(id.clone(), answer);
                // The shim names one of MiniClaw's three tools; anything else is the file tool.
                let tool = if request["tool"] == "miniclaw_delegate" { "miniclaw_delegate" }
                    else if request["tool"] == crate::mcp_gateway::TOOL { crate::mcp_gateway::TOOL }
                    else if request["tool"] == crate::ask::TOOL { crate::ask::TOOL }
                    else if request["tool"] == crate::project_tool::TOOL { crate::project_tool::TOOL }
                    else { "miniclaw_scoped_native" };
                let _ = self.out.send(json!({ "id": id, "method": "item/tool/call", "params": {
                    "tool": tool, "threadId": thread, "turnId": turn,
                    "callId": id, "arguments": request["arguments"] } })).await;
                // The handlers keep their own limits: an approval's wait, a command's time, a
                // delegated run's minutes. This one only ends a call nobody will answer.
                let result = tokio::time::timeout(Duration::from_secs(3900), answered).await;
                self.calls.lock().unwrap().remove(&id);
                result.ok().and_then(Result::ok).unwrap_or(refused)
            }
            None => refused,
        };
        let _ = writer.write_all(format!("{result}\n").as_bytes()).await;
    }

    /// Starts `claude auth login` with its output read by MiniClaw, which reports the sign-in page,
    /// the CLI's errors and its exit through `account/login/read`; the caller polls that and
    /// `account/read`. Returns an error message instead when the CLI cannot start.
    fn login(&self) -> Option<Value> {
        let mut command = Command::new(&self.binary);
        command.args(["auth", "login", "--claudeai"]).stdin(Stdio::piped())
            .stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        // The Windows CLI would open its page with rundll32 inside the kill-on-close job that ends
        // with it, so a browser it starts would close when the sign-in ends. Its BROWSER is MiniClaw's
        // own executable instead, which starts a second copy outside the job (explicit breakaway) to
        // open the page; the CLI's localhost callback still finishes the sign-in by itself.
        #[cfg(windows)]
        {
            if let Ok(exe) = std::env::current_exe() { command.env("BROWSER", exe); }
            command.env(crate::claude_login::OPENER, "1");
            if let Some(log) = std::env::var_os("MINICLAW_TEST_BROWSER_LOG") { command.env("MINICLAW_TEST_BROWSER_LOG", log); }
        }
        let Ok(mut child) = crate::provider_process::spawn_sign_in(&mut command) else {
            return Some(json!({ "error": "Claude Code could not be started. Check that it is installed, then try again." }));
        };
        let progress = Arc::new(Mutex::new(LoginProgress::default()));
        let input = child.stdin.take().map(|input| Box::new(input) as Box<dyn AsyncWrite + Send + Unpin>);
        if let Some(output) = child.stdout.take() {
            progress.lock().unwrap().reading += 1;
            tokio::spawn(read_login(output, progress.clone(), false));
        }
        if let Some(errors) = child.stderr.take() {
            progress.lock().unwrap().reading += 1;
            tokio::spawn(read_login(errors, progress.clone(), true));
        }
        *self.login.lock().unwrap() = Some(LoginAttempt { child,
            input: Arc::new(tokio::sync::Mutex::new(input)), progress, exit_seen: false });
        *self.account.lock().unwrap() = None;
        Some(json!({ "loginId": Uuid::new_v4() }))
    }

    /// Reports the running sign-in: its page, latest message and, once its output has ended,
    /// whether the CLI exited successfully.
    fn login_read(&self) -> Value {
        let mut login = self.login.lock().unwrap();
        let Some(attempt) = login.as_mut() else { return json!({ "running": false }); };
        let progress = attempt.progress.lock().unwrap();
        let exited = if progress.reading == 0 {
            attempt.child.try_wait().ok().flatten().map(|status| status.success())
        } else { None };
        json!({ "running": true, "url": progress.url, "message": progress.message, "exited": exited })
    }

    /// Hands the code Claude's page showed to the waiting CLI, as its "Paste code here" prompt
    /// expects; rejects anything that is not a `code#state` value.
    async fn login_code(&self, params: &Value) -> Option<Value> {
        let code = params["code"].as_str().filter(|code| crate::claude_login::code(code))?;
        let input = self.login.lock().unwrap().as_ref()?.input.clone();
        let mut input = input.lock().await;
        let writer = input.as_mut()?;
        writer.write_all(format!("{code}\n").as_bytes()).await.ok()?;
        writer.flush().await.ok()?;
        Some(json!({}))
    }

    /// Returns the sign-in state, starting `claude auth status` at most once per three seconds,
    /// also while a sign-in waits and the page polls; concurrent callers share one reading. The
    /// first reading after the sign-in CLI exits is fresh, so its success shows at once.
    async fn account(&self) -> Value {
        let _gate = self.account_gate.lock().await;
        let mut finished = false;
        if let Some(attempt) = self.login.lock().unwrap().as_mut()
            && !attempt.exit_seen && matches!(attempt.child.try_wait(), Ok(Some(_))) {
            attempt.exit_seen = true;
            finished = true;
        }
        if !finished && let Some((read, value)) = self.account.lock().unwrap().as_ref()
            && read.elapsed() < Duration::from_secs(3) {
            return value.clone();
        }
        let value = self.read_account().await;
        *self.account.lock().unwrap() = Some((Instant::now(), value.clone()));
        value
    }

    /// Reads the CLI's own sign-in state; credentials never pass through MiniClaw.
    async fn read_account(&self) -> Value {
        let signed_out = json!({ "account": null, "requiresOpenaiAuth": false });
        let mut command = Command::new(&self.binary);
        command.args(["auth", "status", "--json"]).stdin(Stdio::null())
            .stderr(Stdio::null()).kill_on_drop(true);
        let Ok(Ok(output)) = tokio::time::timeout(Duration::from_secs(10), crate::provider_process::output(&mut command)).await
            else { return signed_out; };
        let Ok(status) = serde_json::from_slice::<Value>(&output.stdout) else { return signed_out; };
        if status["loggedIn"] != true { return signed_out; }
        json!({ "account": { "type": "claude", "email": status["email"].as_str(),
            "planType": status["subscriptionType"].as_str() }, "requiresOpenaiAuth": false })
    }

    /// Returns a copy of one saved thread.
    fn thread(&self, params: &Value) -> Option<Thread> {
        self.threads.lock().unwrap().get(params["threadId"].as_str()?).cloned()
    }

    /// Builds the saved-thread shape that history summaries read.
    fn view(&self, thread: &Thread) -> Value {
        let preview = thread.turns.iter().flat_map(|turn| &turn.items)
            .find(|item| item.role == "user")
            .map(|item| item.text.chars().take(60).collect::<String>());
        let active = self.active.lock().unwrap().contains_key(&thread.id);
        json!({ "id": thread.id, "name": null, "preview": preview, "ephemeral": false,
            "modelProvider": PROVIDER, "cwd": self.cwd, "source": "appServer",
            "model": thread.model, "reasoningEffort": thread.effort,
            "createdAt": thread.created_at, "updatedAt": thread.created_at,
            "status": { "type": if active { "active" } else { "idle" } } })
    }

    /// Joins the fixed chat policy with pinned agent or project instructions.
    fn instructions(params: &Value) -> String {
        let base = params["baseInstructions"].as_str().filter(|text| !text.is_empty())
            .unwrap_or(BASE);
        match params["developerInstructions"].as_str().filter(|text| !text.is_empty()) {
            Some(developer) => format!("{base}\n\n{developer}"),
            None => base.into(),
        }
    }

    /// Returns the offered tool of this name. Only MiniClaw's own five tools are ever accepted:
    /// one unknown name among them and nothing is offered at all.
    fn offered_tool<'a>(params: &'a Value, name: &str) -> Option<&'a Value> {
        let tools = params["dynamicTools"].as_array()?;
        let file = crate::scoped::tool_definition()["name"].clone();
        let known = |tool: &Value| tool["name"] == file || tool["name"] == "miniclaw_delegate"
            || tool["name"] == crate::mcp_gateway::TOOL || tool["name"] == crate::ask::TOOL
            || tool["name"] == crate::project_tool::TOOL;
        if tools.len() > 5 || !tools.iter().all(known) { return None; }
        tools.iter().find(|tool| tool["name"] == name)
    }

    /// Reports whether the backend offered the MiniClaw file and command tool to this chat.
    fn offered(params: &Value) -> bool {
        Self::offered_tool(params, crate::scoped::tool_definition()["name"].as_str().unwrap_or_default()).is_some()
    }

    /// Returns the delegation tool when the backend offered it to this chat.
    fn delegate_offered(params: &Value) -> Option<Value> {
        Self::offered_tool(params, "miniclaw_delegate").cloned()
    }

    /// Reports whether the backend offered the question tool to this chat.
    fn ask_offered(params: &Value) -> bool { Self::offered_tool(params, crate::ask::TOOL).is_some() }

    /// Reports whether the backend offered the project tool to this chat.
    fn project_offered(params: &Value) -> bool { Self::offered_tool(params, crate::project_tool::TOOL).is_some() }

    /// Reports whether the backend offered the MCP gateway to this chat.
    fn mcp_offered(params: &Value) -> bool { Self::offered_tool(params, crate::mcp_gateway::TOOL).is_some() }

    /// Creates and saves an empty thread with its model, effort, and instructions.
    fn start(&self, params: &Value) -> Option<Value> {
        let model = params["model"].as_str()?;
        let entry = self.model(model)?;
        let effort = params["config"]["model_reasoning_effort"].as_str().map(str::to_owned);
        if effort.as_ref().is_some_and(|effort| !supports(&entry, effort)) { return None; }
        let mut threads = self.threads.lock().unwrap();
        if threads.len() >= 4096 { return None; }
        let thread = Thread { id: Uuid::new_v4().to_string(), model: model.into(), effort,
            instructions: Self::instructions(params),
            created_at: chrono::Utc::now().timestamp(), started: false,
            tools: Self::offered(params), delegate: Self::delegate_offered(params),
            mcp: Self::mcp_offered(params), ask: Self::ask_offered(params), project: Self::project_offered(params), turns: Vec::new() };
        save(&self.store, &thread).ok()?;
        threads.insert(thread.id.clone(), thread.clone());
        drop(threads);
        Some(json!({ "thread": self.view(&thread), "model": thread.model,
            "modelProvider": PROVIDER, "reasoningEffort": thread.effort }))
    }

    /// Reopens a saved thread and refreshes its pinned instructions.
    fn resume(&self, params: &Value) -> Option<Value> {
        let mut threads = self.threads.lock().unwrap();
        let thread = threads.get_mut(params["threadId"].as_str()?)?;
        let instructions = Self::instructions(params);
        let tools = Self::offered(params);
        let delegate = Self::delegate_offered(params);
        let mcp = Self::mcp_offered(params);
        let ask = Self::ask_offered(params);
        let project = Self::project_offered(params);
        if thread.instructions != instructions || thread.tools != tools || thread.delegate != delegate || thread.mcp != mcp
            || thread.ask != ask || thread.project != project {
            thread.ask = ask;
            thread.project = project;
            thread.instructions = instructions;
            thread.tools = tools;
            thread.delegate = delegate;
            thread.mcp = mcp;
            save(&self.store, thread).ok()?;
        }
        let thread = thread.clone();
        drop(threads);
        Some(json!({ "thread": self.view(&thread), "model": thread.model,
            "modelProvider": PROVIDER, "reasoningEffort": thread.effort }))
    }

    /// Reads an offset cursor and page size, both bounded to the saved-thread limit.
    fn window(params: &Value) -> Option<(usize, usize)> {
        let offset = match &params["cursor"] {
            Value::Null => 0,
            Value::String(cursor) => cursor.parse().ok().filter(|offset| *offset <= 1 << 20)?,
            _ => return None,
        };
        Some((offset, params["limit"].as_u64().unwrap_or(50).clamp(1, 50) as usize))
    }

    /// Lists saved threads newest first. Bridge threads never have descendants or archives.
    fn list(&self, params: &Value) -> Option<Value> {
        if !params["ancestorThreadId"].is_null() || params["archived"] == true {
            return Some(json!({ "data": [], "nextCursor": null }));
        }
        let (offset, limit) = Self::window(params)?;
        let mut threads: Vec<Thread> = self.threads.lock().unwrap().values().cloned().collect();
        threads.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| a.id.cmp(&b.id)));
        let next = (offset + limit < threads.len()).then(|| (offset + limit).to_string());
        let data: Vec<_> = threads.iter().skip(offset).take(limit)
            .map(|thread| self.view(thread)).collect();
        Some(json!({ "data": data, "nextCursor": next }))
    }

    /// Returns saved turns newest first in the item shape the history reader expects.
    fn turns(&self, params: &Value) -> Option<Value> {
        let thread = self.thread(params)?;
        let (offset, limit) = Self::window(params)?;
        let next = (offset + limit < thread.turns.len()).then(|| (offset + limit).to_string());
        let data: Vec<_> = thread.turns.iter().rev().skip(offset).take(limit).map(|turn| {
            let items: Vec<_> = turn.items.iter().map(|item| if item.role == "user" {
                json!({ "id": item.id, "type": "userMessage",
                    "content": [{ "type": "text", "text": item.text }] })
            } else {
                json!({ "id": item.id, "type": "agentMessage", "text": item.text })
            }).collect();
            json!({ "id": turn.id, "status": turn.status, "items": items, "itemsView": "full" })
        }).collect();
        Some(json!({ "data": data, "nextCursor": next }))
    }

    /// Deletes an idle thread's MiniClaw transcript.
    fn delete(&self, params: &Value) -> Option<Value> {
        let id = params["threadId"].as_str()?;
        if self.active.lock().unwrap().contains_key(id) { return None; }
        let mut threads = self.threads.lock().unwrap();
        threads.get(id)?;
        std::fs::remove_file(self.store.join(format!("{id}.json"))).ok()?;
        threads.remove(id);
        self.forget_session(id);
        Some(json!({}))
    }

    /// Removes the CLI's own session file for this chat, best effort, under every spelling of
    /// the working folder it may have been started in.
    fn forget_session(&self, id: &str) {
        let Some(projects) = projects() else { return; };
        for cwd in crate::history::workspace_spellings(&self.cwd) {
            let path = session_file(&projects, &cwd, id);
            if std::fs::symlink_metadata(&path).is_ok_and(|info| info.is_file()) {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    /// Claims the thread for one turn and records the user's message before any reply.
    fn reserve(&self, params: &Value) -> Option<Value> {
        let id = params["threadId"].as_str()?;
        let text = prompt(params)?;
        let mut active = self.active.lock().unwrap();
        if active.contains_key(id) { return None; }
        let mut threads = self.threads.lock().unwrap();
        let thread = threads.get_mut(id)?;
        if let Some(model) = params["model"].as_str() {
            let entry = self.model(model)?;
            let effort = params["effort"].as_str().map(str::to_owned);
            if effort.as_ref().is_some_and(|effort| !supports(&entry, effort)) { return None; }
            thread.model = model.into();
            thread.effort = effort;
        }
        let turn = Uuid::new_v4().to_string();
        let user = params["clientUserMessageId"].as_str().map(str::to_owned)
            .unwrap_or_else(|| format!("{turn}-user"));
        thread.turns.push(Turn { id: turn.clone(), status: "inProgress".into(),
            items: vec![Item { id: user, role: "user".into(), text }] });
        save(&self.store, thread).ok()?;
        active.insert(id.into(), Active { turn: turn.clone(),
            token: Uuid::new_v4().to_string(), stop: None, web: NativeTools::read(params),
            mcp: params.get("miniclawMcp") == Some(&json!(true)),
            ask: params.get("miniclawAsk") == Some(&json!(true)) });
        Some(json!({ "turn": { "id": turn, "status": "inProgress", "items": [] } }))
    }

    /// Starts the reserved turn's CLI run in the background.
    fn begin(self: Arc<Self>, params: &Value) {
        let Some(id) = params["threadId"].as_str().map(str::to_owned) else { return; };
        // The environment note rarely changes, so it sits in the system prompt beside the skills.
        let skills: Vec<&str> = ["miniclaw_environment", "miniclaw_skills"].iter()
            .filter_map(|key| params["additionalContext"][*key]["value"].as_str()).collect();
        let skills = (!skills.is_empty()).then(|| skills.join("\n\n"));
        // What belongs to this one message: a called skill, and delegation results. It goes with
        // the message, not into the system prompt. The system prompt heads the cached prefix, so
        // text that comes and goes there would make Claude re-read the whole conversation at full
        // price twice (platform.claude.com/docs/en/build-with-claude/prompt-caching).
        let extras: Vec<&str> = ["miniclaw_project", "miniclaw_slash", "miniclaw_delegations"].iter()
            .filter_map(|key| params["additionalContext"][*key]["value"].as_str()).collect();
        let mut extras: Vec<String> = extras.into_iter().map(str::to_owned).collect();
        // The text of attached files belongs to this message too.
        if let Some(attached) = params["additionalContext"]["miniclaw_attachments"]["value"].as_str() { extras.push(attached.to_owned()); }
        let extras = (!extras.is_empty()).then(|| extras.join("\n\n"));
        let pictures: Vec<std::path::PathBuf> = params["input"].as_array().into_iter().flatten()
            .filter(|part| part["type"] == "localImage").filter_map(|part| part["path"].as_str().map(Into::into)).collect();
        let (stop, stopped) = oneshot::channel();
        let turn = {
            let mut active = self.active.lock().unwrap();
            let Some(entry) = active.get_mut(&id).filter(|entry| entry.stop.is_none())
                else { return; };
            entry.stop = Some(stop);
            entry.turn.clone()
        };
        tokio::spawn(async move { self.run(id, turn, skills, extras, pictures, stopped).await; });
    }

    /// Requests a stop; the running turn reports `interrupted` when its CLI has exited.
    fn interrupt(&self, params: &Value) -> Option<Value> {
        let mut active = self.active.lock().unwrap();
        let entry = active.get_mut(params["threadId"].as_str()?)?;
        if Some(entry.turn.as_str()) != params["turnId"].as_str() { return None; }
        if let Some(stop) = entry.stop.take() { let _ = stop.send(()); }
        Some(json!({}))
    }

    /// Sends one notification to the client.
    async fn notify(&self, method: &str, params: Value) {
        let _ = self.out.send(json!({ "method": method, "params": params })).await;
    }

    /// Runs one CLI turn, streams its text, then saves and announces the final status.
    async fn run(self: Arc<Self>, id: String, turn: String, skills: Option<String>, extras: Option<String>,
        pictures: Vec<std::path::PathBuf>, mut stopped: oneshot::Receiver<()>) {
        self.notify("turn/started", json!({ "threadId": id,
            "turn": { "id": turn, "status": "inProgress", "items": [] } })).await;
        let thread = self.threads.lock().unwrap().get(&id).cloned();
        let mut replies = Vec::new();
        let mut reached = false;
        let status = match thread {
            Some(thread) => {
                let run = self.stream(&thread, &turn, skills, extras, &pictures, &mut replies, &mut reached);
                tokio::select! {
                    // A dropped sender means the bridge is closing.
                    _ = &mut stopped => "interrupted",
                    succeeded = run => if succeeded { "completed" } else { "failed" },
                }
            }
            None => "failed",
        };
        {
            let mut threads = self.threads.lock().unwrap();
            if let Some(thread) = threads.get_mut(&id) {
                thread.started |= reached;
                if let Some(saved) = thread.turns.iter_mut().find(|saved| saved.id == turn) {
                    saved.status = status.into();
                    saved.items.extend(replies.into_iter().filter(|item| !item.text.is_empty()));
                }
                let _ = save(&self.store, thread);
            }
        }
        self.active.lock().unwrap().remove(&id);
        self.notify("turn/completed", json!({ "threadId": id,
            "turn": { "id": turn, "status": status } })).await;
    }

    /// Streams text blocks from one CLI process and returns whether it reported success.
    /// Dropping this future kills the process; text received so far stays in `replies`.
    async fn stream(&self, thread: &Thread, turn: &str, skills: Option<String>, extras: Option<String>,
        pictures: &[std::path::PathBuf], replies: &mut Vec<Item>, reached: &mut bool) -> bool {
        let text = thread.turns.iter().find(|saved| saved.id == turn)
            .and_then(|saved| saved.items.first()).map(|item| item.text.clone());
        let Some(text) = text else { return false; };
        let (token, web, mcp, ask) = self.active.lock().unwrap().get(&thread.id)
            .map(|active| (Some(active.token.clone()), active.web.clone(), active.mcp, active.ask)).unwrap_or_default();
        // The gateway belongs to this turn only when the backend offered it and a server is on, and
        // the question tool only while asking is switched on.
        let mut thread = thread.clone();
        thread.mcp = thread.mcp && mcp;
        thread.ask = thread.ask && ask;
        let thread = &thread;
        let Some(mut command) = self.command(thread, skills, token, web.as_ref()) else { return false; };
        // A message with pictures is written as one line of content blocks, pictures first, which
        // CLI 2.1.278 was seen to accept; a message of words alone stays plain text.
        if !pictures.is_empty() { command.args(["--input-format", "stream-json"]); }
        let Ok(mut child) = crate::provider_process::spawn(&mut command) else { return false; };
        let mut input = child.stdin.take().unwrap();
        // The saved message stays as typed; only what the CLI reads carries this turn's extras.
        let text = match extras {
            Some(extras) => format!("<miniclaw-context>\n{extras}\n</miniclaw-context>\n\n{text}"),
            None => text,
        };
        let text = if pictures.is_empty() { text } else {
            let mut content = Vec::new();
            for path in pictures {
                let Ok(bytes) = tokio::fs::read(path).await else { return false; };
                let Some(media) = crate::attachments::picture(&bytes) else { return false; };
                content.push(json!({ "type": "image", "source": { "type": "base64", "media_type": media,
                    "data": crate::frames::encode(&bytes) } }));
            }
            content.push(json!({ "type": "text", "text": text }));
            format!("{}\n", json!({ "type": "user", "message": { "role": "user", "content": content } }))
        };
        if input.write_all(text.as_bytes()).await.is_err() || input.flush().await.is_err() { return false; }
        drop(input);
        let mut succeeded = false;
        let mut open = false;
        let mut lines = lines(child.stdout.take().unwrap());
        while let Some(Ok(line)) = lines.next().await {
            let Ok(frame) = serde_json::from_str::<Value>(&line) else { return false; };
            if frame["type"] == "result" {
                succeeded = frame["subtype"] == "success" && frame["is_error"] != true;
                // The CLI reports what the turn cost in its last frame. It is sent on as the
                // notification Codex sends, so MiniClaw counts both providers the same way.
                let usage = &frame["usage"];
                let count = |key: &str| usage[key].as_i64().unwrap_or(0);
                let (input, output) = (count("input_tokens"), count("output_tokens"));
                if input + output > 0 {
                    let cached = count("cache_read_input_tokens");
                    self.notify("thread/tokenUsage/updated", json!({ "threadId": thread.id, "turnId": turn,
                        "tokenUsage": { "last": { "inputTokens": input, "cachedInputTokens": cached, "outputTokens": output,
                            "reasoningOutputTokens": 0, "cacheWriteInputTokens": count("cache_creation_input_tokens"),
                            "totalTokens": input + output } } })).await;
                }
                continue;
            }
            if frame["type"] != "stream_event" || !frame["parent_tool_use_id"].is_null() {
                continue;
            }
            *reached = true;
            let event = &frame["event"];
            // Thinking blocks stay private; only text blocks become messages.
            match event["type"].as_str() {
                Some("content_block_start") if event["content_block"]["type"] == "text" => {
                    replies.push(Item { id: format!("{turn}-{}", replies.len()),
                        role: "assistant".into(), text: String::new() });
                    open = true;
                }
                Some("content_block_delta") if open && event["delta"]["type"] == "text_delta" => {
                    let (Some(item), Some(delta)) = (replies.last_mut(),
                        event["delta"]["text"].as_str()) else { continue; };
                    if item.text.len() + delta.len() > 2 * 1024 * 1024 { return false; }
                    item.text.push_str(delta);
                    self.notify("item/agentMessage/delta", json!({ "threadId": thread.id,
                        "turnId": turn, "itemId": item.id, "delta": delta })).await;
                }
                Some("content_block_stop") if open => {
                    open = false;
                    let Some(item) = replies.last() else { continue; };
                    self.notify("item/completed", json!({ "threadId": thread.id, "turnId": turn,
                        "item": { "id": item.id, "type": "agentMessage", "text": item.text },
                    })).await;
                }
                _ => {}
            }
        }
        child.wait().await.is_ok_and(|exit| exit.success()) && succeeded
    }

    /// Builds the CLI invocation with selected tools and pinned instructions. Excludes
    /// inherited customizations and the provider's file and shell tools.
    fn command(&self, thread: &Thread, skills: Option<String>, token: Option<String>,
        web: Option<&NativeTools>) -> Option<Command> {
        let mut system = thread.instructions.clone();
        if let Some(skills) = skills {
            system.push_str("\n\n");
            system.push_str(&skills);
        }
        // Arguments share the OS limit; refuse rather than truncate instructions.
        if system.len() > 256 * 1024 { return None; }
        let native = web.map(NativeTools::enabled_tools).unwrap_or_default();
        let mut command = Command::new(&self.binary);
        command.args(["-p", "--output-format", "stream-json", "--verbose",
            "--include-partial-messages", "--strict-mcp-config",
            "--disable-slash-commands", "--system-prompt-snapshot", "off"])
            // The providers' shell and file tools are never named here.
            .arg("--tools").arg(native.join(","))
            .arg("--system-prompt").arg(system)
            .arg("--model").arg(&thread.model);
        if let Some(effort) = &thread.effort { command.arg("--effort").arg(effort); }
        // Safe mode also disables MCP (checked against CLI 2.1.278), so a tool chat
        // instead loads no settings files: no hooks, plugins or inherited permissions.
        let shim = if thread.tools || thread.delegate.is_some() || thread.mcp || thread.ask || thread.project { self.socket.as_ref().zip(token) } else { None };
        // Safe mode removes built-in tools, so selected tools use the no-settings path.
        if shim.is_none() && native.is_empty() { command.arg("--safe-mode"); }
        else { command.args(["--setting-sources", ""]); }
        let mut allowed: Vec<String> = native.iter().filter(|name| **name != "WebFetch")
            .map(|name| (*name).to_owned()).collect();
        // Plan mode makes the model plan without acting; it names no tool by itself.
        if web.is_some_and(|web| web.plan) { command.args(["--permission-mode", "plan"]); }
        if let Some(web) = web.filter(|web| web.fetch) {
            // Fetch uses host rules; Claude also pre-approves some documentation sites.
            allowed.extend(NativeTools::rules(&web.hosts, &web.domains));
            let blocked = NativeTools::rules(&web.blocked_hosts, &web.blocked_domains);
            if !blocked.is_empty() { command.arg("--disallowedTools").args(blocked); }
        }
        // The only tool is MiniClaw's own, served by this executable's MCP shim.
        if let Some((socket, token)) = shim {
            let shim = std::env::current_exe().ok()?;
            let mut env = json!({ "MINICLAW_TOOL_SOCKET": socket, "MINICLAW_TOOL_TOKEN": token });
            if !thread.tools { env["MINICLAW_NO_FILE_TOOL"] = json!("1"); }
            if let Some(delegate) = &thread.delegate {
                env["MINICLAW_DELEGATE_TOOL"] = json!(delegate.to_string());
                allowed.push("mcp__miniclaw__miniclaw_delegate".to_owned());
            }
            if thread.ask {
                env["MINICLAW_ASK_TOOL"] = json!("1");
                allowed.push(format!("mcp__miniclaw__{}", crate::ask::TOOL));
            }
            if thread.project {
                env["MINICLAW_PROJECT_TOOL"] = json!("1");
                allowed.push(format!("mcp__miniclaw__{}", crate::project_tool::TOOL));
            }
            if thread.mcp {
                env["MINICLAW_MCP_TOOL"] = json!("1");
                allowed.push(format!("mcp__miniclaw__{}", crate::mcp_gateway::TOOL));
            }
            // A delegated run or an unattended approval may keep one call open for an hour.
            // Claude Code ends a silent stdio tool call after 30 minutes by default; the
            // per-server `timeout` raises that window (code.claude.com/docs/en/env-vars).
            let config = json!({ "mcpServers": { "miniclaw": {
                "command": shim, "args": ["--tool-bridge"], "env": env, "timeout": 3_900_000,
            } } });
            command.arg("--mcp-config").arg(config.to_string());
            command.env("CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT", "3900000");
            if thread.tools { allowed.push("mcp__miniclaw__miniclaw_scoped_native".to_owned()); }
        }
        if !allowed.is_empty() { command.arg("--allowedTools").args(&allowed); }
        let cwd = if thread.started { projects().map_or_else(|| self.cwd.clone(), |projects| resume_cwd(&projects, &self.cwd, &thread.id)) }
            else { self.cwd.clone() };
        command.arg(if thread.started { "--resume" } else { "--session-id" }).arg(&thread.id)
            .current_dir(cwd)
            .env_remove("ANTHROPIC_API_KEY").env_remove("ANTHROPIC_AUTH_TOKEN")
            .env_remove("ANTHROPIC_BASE_URL")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
            .kill_on_drop(true);
        Some(command)
    }
}

/// Reads one output stream of `claude auth login`: the first sign-in page it names and, from the
/// error stream, the latest message.
async fn read_login<R: AsyncRead + Unpin>(output: R, progress: Arc<Mutex<LoginProgress>>, errors: bool) {
    let mut lines = FramedRead::new(output, LinesCodec::new_with_max_length(16 * 1024));
    while let Some(line) = lines.next().await {
        let line = match line {
            Ok(line) => line,
            Err(LinesCodecError::MaxLineLengthExceeded) => continue,
            Err(_) => break,
        };
        if let Some(url) = crate::claude_login::sign_in_url(&line) {
            progress.lock().unwrap().url.get_or_insert(url);
        } else if errors && let Some(message) = crate::claude_login::message(&line) {
            progress.lock().unwrap().message = Some(message);
        }
    }
    progress.lock().unwrap().reading -= 1;
}

/// Frames CLI output into bounded lines.
fn lines<R: AsyncRead>(output: R) -> FramedRead<R, LinesCodec> {
    FramedRead::new(output, LinesCodec::new_with_max_length(8 * 1024 * 1024))
}

/// Joins the text parts of a turn's input; an empty message is rejected.
fn prompt(params: &Value) -> Option<String> {
    let parts: Vec<&str> = params["input"].as_array()?.iter()
        .filter(|part| part["type"] == "text").filter_map(|part| part["text"].as_str()).collect();
    let text = parts.join("\n");
    (!text.trim().is_empty()).then_some(text)
}

/// Returns whether a catalog entry offers this effort level.
fn supports(entry: &Value, effort: &str) -> bool {
    entry["supportedReasoningEfforts"].as_array().is_some_and(|efforts|
        efforts.iter().any(|option| option["reasoningEffort"] == effort))
}

/// Converts CLI picker metadata into the shared catalog without a built-in model list.
fn catalog(response: &Value) -> Option<Vec<Value>> {
    let rows = response["models"].as_array()?;
    if rows.is_empty() || rows.len() > 1000 { return None; }
    let mut models: Vec<Value> = Vec::new();
    for row in rows {
        let value = row["value"].as_str()?;
        // A full picker ID retains context suffixes; aliases resolve to a pinned model.
        let model = if value.starts_with("claude-") { value }
            else { row["resolvedModel"].as_str().unwrap_or(value) };
        if model.is_empty() || model.len() > 512 { return None; }
        let default = value == "default";
        let efforts: Vec<Value> = row["supportedEffortLevels"].as_array().into_iter().flatten()
            .filter(|_| row["supportsEffort"] != false)
            .filter_map(Value::as_str).filter(|level| !level.is_empty())
            .map(|level| json!({ "reasoningEffort": level, "description": "" })).collect();
        let effort = efforts.iter().find(|entry| entry["reasoningEffort"] == "medium")
            .or_else(|| efforts.first()).map(|entry| entry["reasoningEffort"].clone())
            .unwrap_or(json!(""));
        let mut entry = json!({ "model": model, "displayName": row["displayName"].as_str()?,
            "description": row["description"].as_str().unwrap_or_default(), "hidden": false,
            "isDefault": default, "provider": "claude", "defaultReasoningEffort": effort,
            "supportedReasoningEfforts": efforts, "serviceTiers": [] });
        if let Some(existing) = models.iter_mut().find(|entry| entry["model"] == model) {
            if default { existing["isDefault"] = json!(true); }
            else {
                entry["isDefault"] = existing["isDefault"].clone();
                *existing = entry;
            }
        } else { models.push(entry); }
    }
    Some(models)
}

/// The CLI's folder of saved sessions.
fn projects() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from)
        .or_else(|| std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .map(|home| PathBuf::from(home).join(".claude")))
        .map(|config| config.join("projects"))
}

/// Where the CLI keeps one chat's session: a folder named after the working directory with every
/// other character turned into `-`.
fn session_file(projects: &Path, cwd: &str, id: &str) -> PathBuf {
    let folder: String = cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    projects.join(folder).join(format!("{id}.jsonl"))
}

/// The working folder a started chat resumes in. Windows chats started before 2026-10-04 ran in
/// the verbatim `\\?\C:\...` folder, which Node reports as the working directory as given (checked
/// natively), so the CLI filed their sessions under that spelling; such a chat resumes there.
fn resume_cwd(projects: &Path, cwd: &str, id: &str) -> String {
    crate::history::workspace_spellings(cwd).into_iter()
        .find(|spelling| session_file(projects, spelling, id).is_file())
        .unwrap_or_else(|| cwd.to_owned())
}

#[cfg(test)]
mod session_folder_tests {
    use super::*;

    /// A chat whose session was filed under the old verbatim folder resumes there; any other chat,
    /// and every macOS chat, resumes in the backend's folder.
    #[test]
    fn an_old_windows_session_resumes_in_its_own_folder() {
        let projects = std::env::temp_dir().join(format!("mc-claude-projects-{}", Uuid::new_v4()));
        let plain = r"C:\Users\S\chat";
        let old = session_file(&projects, r"\\?\C:\Users\S\chat", "old");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, "{}").unwrap();
        assert_eq!(old.parent().unwrap().file_name().unwrap(), "----C--Users-S-chat");
        assert_eq!(resume_cwd(&projects, plain, "old"), r"\\?\C:\Users\S\chat");
        assert_eq!(resume_cwd(&projects, plain, "new"), plain);
        let current = session_file(&projects, plain, "both");
        std::fs::create_dir_all(current.parent().unwrap()).unwrap();
        std::fs::write(&current, "{}").unwrap();
        std::fs::write(session_file(&projects, r"\\?\C:\Users\S\chat", "both"), "{}").unwrap();
        assert_eq!(resume_cwd(&projects, plain, "both"), plain, "the current spelling wins");
        assert_eq!(resume_cwd(&projects, "/Users/sepehr/chat", "old"), "/Users/sepehr/chat");
        let _ = std::fs::remove_dir_all(projects);
    }
}

#[cfg(test)]
mod bridge_tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    /// Unknown tokens are refused before any backend approval or tool request.
    #[tokio::test]
    async fn bridge_rejects_unknown_token() {
        let (out, mut frames) = mpsc::channel(4);
        let bridge = Arc::new(Bridge {
            binary: String::new(), cwd: String::new(), store: PathBuf::new(),
            threads: Mutex::new(HashMap::new()), active: Mutex::new(HashMap::new()),
            account: Mutex::new(None), account_gate: tokio::sync::Mutex::new(()), models: Mutex::new(None),
            model_gate: tokio::sync::Mutex::new(()), login: Mutex::new(None),
            calls: Mutex::new(HashMap::new()), socket: None, out,
        });
        let (mut client, server) = tokio::io::duplex(4096);
        let request = tokio::spawn(bridge.tool_call(server));
        client.write_all(b"{\"token\":\"wrong\",\"tool\":\"miniclaw_scoped_native\"}\n")
            .await.unwrap();
        let mut reply = String::new();
        client.read_to_string(&mut reply).await.unwrap();
        request.await.unwrap();
        assert_eq!(serde_json::from_str::<Value>(&reply).unwrap()["success"], false);
        assert!(frames.try_recv().is_err());
    }
}
