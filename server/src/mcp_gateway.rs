//! The one tool through which a model reaches MCP servers: `miniclaw_mcp`, with the actions
//! `list_tools` and `call_tool`. A provider fixes a conversation's tools when the conversation
//! starts, while the user may switch a server on or off at any moment; so the tool stays the
//! same and what it reaches is decided at each call, from the permission rules.
//!
//! The order of a call: the conversation and its running turn are checked, the server must be
//! switched on (never asked about), the tool must exist, the user approves the tool with the
//! exact arguments in view, and only then is the server called. A model cannot add, change or
//! switch on a server; no action here does that.

// The running conversation, its approval popup and the activity log.
use crate::chat::{Backend, ScopedApproval, ScopedChoice};
// Connects to servers and calls their tools.
use crate::mcp_client::{Fault, Link};
// The saved entries that say how to reach each server.
use crate::mcp_servers::Server;
// Decides whether a server is on and whether a tool may run.
use crate::permissions::{Context, Decision, Target};
// Reads the server's secrets at the moment of use.
use crate::secrets::Secrets;
// Builds the tool's answers and activity entries.
use serde_json::{Value, json};
// Shares connections between calls and bounds every wait.
use std::{collections::HashMap, future::Future, sync::{Arc, atomic::Ordering}, time::{Duration, Instant}};
// Serialises the calls to one server and carries the user's answer.
use tokio::sync::{Mutex, oneshot};
// Names each approval.
use uuid::Uuid;

/// The tool's name, as both providers see it.
pub const TOOL: &str = "miniclaw_mcp";
/// The most text of one result that is returned to the model.
const MOST: usize = 256 * 1024;

/// Describes the tool to a provider. `type` is required by the real Codex.
pub fn tool_definition() -> Value {
    json!({ "type": "function", "name": TOOL,
        "description": "Your only way to any MCP server (for example when the user says \"use the Blender MCP\" or names another server): MCP servers are not separate tools here. Reaches the MCP servers the user switched on for this conversation. First use action list_tools to see the servers and their tools, a name and one line each (query narrows the list to tools whose name or description carries its words). Then action describe_tool with server and tool gives that tool's full description and inputSchema, and action call_tool with server, tool and arguments runs it. The user approves each tool before it runs. Servers that are off cannot be reached, and you cannot switch a server on. The user allows a server for the conversation's agent in Settings, Agents, and switches it from Tools, beside the message box.",
        "inputSchema": { "type": "object", "additionalProperties": false, "required": ["action"], "properties": {
            "action": { "type": "string", "enum": ["list_tools", "describe_tool", "call_tool"] },
            "query": { "type": "string", "description": "Words to narrow list_tools to matching tools, case does not matter." },
            "server": { "type": "string", "description": "The server's name, from list_tools. Required for describe_tool and call_tool; narrows list_tools to that server." },
            "tool": { "type": "string", "description": "The tool's name, from list_tools. Required for describe_tool and call_tool." },
            "arguments": { "type": "object", "description": "The tool's input, matching its inputSchema." } } } })
}

/// One open connection with the tools it listed, valid for one revision of its server.
struct Open { revision: u64, link: Link, tools: Vec<Value> }

/// Keeps one connection per server, so a local server is started once and not per call.
/// `closing` turns true once, at shutdown: from then on nothing connects and running work ends.
pub struct Hub { open: Mutex<HashMap<String, Arc<Mutex<Option<Open>>>>>, closing: tokio::sync::watch::Sender<bool> }

impl Default for Hub {
    /// Starts with no connection and not closing.
    fn default() -> Self { Self { open: Mutex::default(), closing: tokio::sync::watch::channel(false).0 } }
}

impl Hub {
    /// Returns the server's slot. Calls to one server wait for each other; servers do not.
    async fn slot(&self, id: &str) -> Arc<Mutex<Option<Open>>> {
        self.open.lock().await.entry(id.to_owned()).or_default().clone()
    }

    /// Ends the connection of a server that was changed or deleted.
    pub async fn forget(&self, id: &str) {
        let slot = self.open.lock().await.remove(id);
        if let Some(slot) = slot { *slot.lock().await = None; }
    }

    /// Ends every connection for shutdown, and with it each local server's process and
    /// sandbox. The flag is set first, so a call that still holds a slot cannot connect again
    /// after its connection was dropped here, and work under `run` lets go of its slot.
    pub async fn close_all(&self) {
        self.closing.send_replace(true);
        let slots: Vec<_> = self.open.lock().await.drain().map(|(_, slot)| slot).collect();
        for slot in slots { *slot.lock().await = None; }
    }

    /// Runs gateway work until shutdown begins; `None` says shutdown dropped it unfinished,
    /// together with any slot it held.
    async fn run<T>(&self, work: impl Future<Output = T>) -> Option<T> {
        let mut closing = self.closing.subscribe();
        tokio::select! {
            biased;
            _ = closing.wait_for(|closing| *closing) => None,
            value = work => Some(value),
        }
    }
}

/// Connects if there is no connection for this revision of the server, and lists its tools.
/// The caller holds the slot's lock, which is what makes the shutdown check here final.
async fn ensure<'a>(hub: &Hub, slot: &'a mut Option<Open>, server: &Server) -> Result<&'a mut Open, Fault> {
    if *hub.closing.borrow() {
        *slot = None;
        return Err(Fault("MiniClaw is shutting down, so the server was not started.".into()));
    }
    if slot.as_ref().is_none_or(|open| open.revision != server.revision) {
        *slot = None;
        let mut link = Link::open(server, &Secrets::from_env()).await?;
        let tools = link.tools().await?;
        *slot = Some(Open { revision: server.revision, link, tools });
    }
    Ok(slot.as_mut().expect("just connected"))
}

/// An answer for the model: success or not, and words.
fn answer(success: bool, text: impl Into<String>) -> Value {
    json!({ "success": success, "contentItems": [{ "type": "inputText", "text": text.into() }] })
}

/// Cuts text at a character boundary and says how much is missing.
fn bounded(text: &str, most: usize) -> String {
    if text.len() <= most { return text.to_owned(); }
    let mut end = most;
    while !text.is_char_boundary(end) { end -= 1; }
    format!("{}\n... {} more bytes not shown", &text[..end], text.len() - end)
}

/// The most pictures of one result that are passed on, and the most base64 text one may hold.
const MOST_IMAGES: usize = 4;
const MOST_IMAGE: usize = 6_000_000;

/// Picks the pictures of a tool result that can be passed to a model: a known image type, a
/// bounded size, and base64 that is only base64, so the data URL cannot carry anything else.
fn pictures(result: &Value) -> Vec<String> {
    result["content"].as_array().into_iter().flatten().filter(|item| item["type"] == "image").filter_map(|item| {
        let (kind, data) = (item["mimeType"].as_str()?, item["data"].as_str()?);
        let fits = matches!(kind, "image/png" | "image/jpeg" | "image/webp" | "image/gif") && !data.is_empty() && data.len() <= MOST_IMAGE
            && data.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'='));
        fits.then(|| format!("data:{kind};base64,{data}"))
    }).take(MOST_IMAGES).collect()
}

/// Turns a tool result's content into text. Pictures travel beside it (`pictures`); audio and
/// anything else is named, not passed on.
fn words(result: &Value) -> String {
    let mut parts: Vec<String> = result["content"].as_array().into_iter().flatten().map(|item| match item["type"].as_str() {
        Some("text") => item["text"].as_str().unwrap_or_default().to_owned(),
        Some("image") => String::new(),
        Some("resource") => item["resource"]["text"].as_str().map(str::to_owned)
            .unwrap_or_else(|| format!("[resource {}]", item["resource"]["uri"].as_str().unwrap_or_default())),
        Some("resource_link") => format!("[link {}]", item["uri"].as_str().unwrap_or_default()),
        Some(kind) => format!("[{kind} content is not passed on]"),
        None => String::new(),
    }).filter(|part| !part.is_empty()).collect();
    if parts.is_empty() && let Some(structured) = result.get("structuredContent").filter(|value| !value.is_null()) {
        parts.push(structured.to_string());
    }
    bounded(&parts.join("\n"), MOST)
}

/// The servers that are switched on for this conversation and meant for its provider.
pub async fn switched_on(backend: &Backend, context: &Context, claude: bool) -> Vec<Server> {
    let mut on = Vec::new();
    for server in backend.agents.mcp_servers().await.unwrap_or_default().into_iter().filter(|server| server.provider.serves(claude)) {
        let switch = Target::McpServer { server: server.id.clone() };
        if matches!(backend.agents.permission(context, &switch, chrono::Utc::now().timestamp()).await, Ok(Decision::Allow(_))) {
            on.push(server);
        }
    }
    on
}

/// Reports whether any server is switched on for a conversation, as a turn starts.
pub async fn any_on(backend: &Backend, thread: &str) -> bool {
    match backend.agents.permission_context(thread.to_owned()).await {
        Ok(context) if context.machine() == "host" => !switched_on(backend, &context, backend.is_claude(thread)).await.is_empty(),
        _ => false,
    }
}

/// Handles one call of the tool to its answer, and shows it in Activity whatever its outcome.
pub async fn handle(backend: Arc<Backend>, request: crate::rpc::ServerRequest) {
    let params = request.params.clone();
    let thread = params["threadId"].as_str().unwrap_or_default().to_owned();
    let arguments = &params["arguments"];
    let action = arguments["action"].as_str().unwrap_or_default();
    let known = matches!(action, "list_tools" | "describe_tool" | "call_tool");
    let entry = match (params["callId"].as_str(), params["turnId"].as_str()) {
        (Some(id), Some(turn)) => {
            let agent = backend.data.lock().await.chats.get(&thread)
                .and_then(|chat| chat.summary["agentName"].as_str().map(str::to_owned));
            let (title, detail) = match action {
                "list_tools" => ("List MCP tools".to_owned(), arguments["query"].as_str().map(|query| bounded(query, 200))),
                "describe_tool" => (format!("Describe {}: {}", arguments["server"].as_str().unwrap_or("?"), arguments["tool"].as_str().unwrap_or("?")), None),
                "call_tool" => (format!("{}: {}", arguments["server"].as_str().unwrap_or("?"), arguments["tool"].as_str().unwrap_or("?")),
                    Some(bounded(&arguments.get("arguments").cloned().unwrap_or(json!({})).to_string(), 4096))),
                _ => (bounded(&arguments.to_string(), 4096), None),
            };
            Some(json!({ "id": id, "turnId": turn, "kind": if known { "mcp" } else { "other" }, "title": title, "detail": detail,
                "agent": agent, "status": "running", "startedAt": chrono::Utc::now().timestamp_millis() }))
        }
        _ => None,
    };
    if let Some(entry) = &entry { backend.publish_activity(&thread, entry).await; }
    // Shutdown drops a call in flight, so its server's slot is free to be closed.
    let (status, result, kept) = backend.mcp.run(run(&backend, &params)).await
        .unwrap_or_else(|| ("failed", answer(false, "MiniClaw is shutting down, so this call was not finished."), Vec::new()));
    if let Some(mut entry) = entry {
        entry["status"] = json!(status);
        entry["endedAt"] = json!(chrono::Utc::now().timestamp_millis());
        // Links to the pictures MiniClaw kept, never their bytes.
        if !kept.is_empty() { entry["pictures"] = json!(kept); }
        // A listing is long and repeats what Settings shows; a call's result is what the user wants to see.
        if action == "call_tool" || status != "ok" {
            entry["output"] = json!(bounded(result["contentItems"][0]["text"].as_str().unwrap_or_default(), 16 * 1024));
        }
        backend.publish_activity(&thread, &entry).await;
    }
    let _ = request.answer.send(result);
}

/// Keeps each picture of a result in the conversation's store and returns how Activity links it.
/// A picture that cannot be kept still reaches the model; it is only missing from Activity.
async fn keep(backend: &Arc<Backend>, thread: &str, server: &str, tool: &str, images: &[String]) -> Vec<(Value, String)> {
    let safe = |text: &str| text.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') { c } else { '_' }).take(60).collect::<String>();
    let mut kept = Vec::new();
    for (index, image) in images.iter().enumerate() {
        let Some((kind, data)) = image.strip_prefix("data:").and_then(|rest| rest.split_once(";base64,")) else { continue };
        let Some(bytes) = crate::frames::decode(data) else { continue };
        let extension = kind.strip_prefix("image/").unwrap_or("png").replace("jpeg", "jpg");
        let name = format!("{}-{}-{}.{extension}", safe(server), safe(tool), index + 1);
        let (store, owner) = (backend.clone(), thread.to_owned());
        let Ok(Ok(record)) = tokio::task::spawn_blocking(move || store.attachments.keep(&owner, &name, &bytes)).await else { continue };
        kept.push((json!({ "id": record.id, "name": record.name, "url": format!("/api/threads/{thread}/attachments/{}", record.id) }),
            record.id.to_string()));
    }
    kept
}

/// Runs one call and answers its Activity status with what the model receives, and the pictures
/// a tool returned that MiniClaw kept.
async fn run(backend: &Arc<Backend>, params: &Value) -> (&'static str, Value, Vec<Value>) {
    let refused = |text: &str| ("refused", answer(false, text), Vec::new());
    let (Some(thread), Some(turn)) = (params["threadId"].as_str(), params["turnId"].as_str())
        else { return refused("MiniClaw could not tell which conversation this call belongs to.") };
    {
        let data = backend.data.lock().await;
        let Some(chat) = data.chats.get(thread) else { return refused("This conversation is not open.") };
        // MCP servers run on this Mac, so only a conversation on this Mac reaches them.
        if !backend.is_local() || chat.summary["environment"]["kind"] == "vm"
            || chat.stopping || chat.active.as_deref() != Some(turn) {
            return refused("MCP servers are not available to this conversation right now.");
        }
    }
    let Ok(context) = backend.agents.permission_context(thread.to_owned()).await
        else { return refused("This conversation's permissions could not be read.") };
    if context.machine() != "host" || backend.agents.admit(thread.to_owned()).await.is_err() {
        return refused("MCP servers are not available to this conversation right now.");
    }
    let arguments = &params["arguments"];
    let on = switched_on(backend, &context, backend.is_claude(thread)).await;
    let plain = |(status, value): (&'static str, Value)| (status, value, Vec::new());
    match arguments["action"].as_str() {
        Some("list_tools") => plain(list(backend, on, arguments).await),
        Some("describe_tool") => plain(describe(backend, on, arguments).await),
        Some("call_tool") => call(backend, thread, turn, &context, on, arguments).await,
        _ => refused("MiniClaw could not read this request, so nothing was done. Use action list_tools, describe_tool with server and tool, or call_tool with server, tool and arguments."),
    }
}

/// The most a tool list may run to. Claude Code files a tool result past its own limit on disk,
/// at a path no grant reaches, so the list stays far below that and schemas come one at a time.
const LIST_MOST: usize = 48 * 1024;
/// The most of a description that is listed beside a tool's name.
const SUMMARY_MOST: usize = 160;

/// The first sentence of a description, bounded, for the list.
fn summary(description: &str) -> String {
    let line = description.trim().lines().next().unwrap_or_default();
    let sentence = match line.find(". ") { Some(end) => &line[..=end], None => line };
    let sentence = sentence.trim();
    if sentence.chars().count() <= SUMMARY_MOST { return sentence.to_owned(); }
    format!("{}…", sentence.chars().take(SUMMARY_MOST - 1).collect::<String>().trim_end())
}

/// Reports whether every word of a query appears in a tool's name or description, case aside.
fn matches(tool: &Value, query: &str) -> bool {
    let text = format!("{}\n{}", tool["name"].as_str().unwrap_or_default(), tool["description"].as_str().unwrap_or_default()).to_lowercase();
    query.split_whitespace().all(|word| text.contains(&word.to_lowercase()))
}

/// The offered tools of a server whose name is close to a wanted name: sharing a piece of at
/// least three letters, so a guess such as `play_test` finds `play_test_start`.
fn near<'a>(server: &Server, tools: &'a [Value], wanted: &str) -> Vec<&'a str> {
    let wanted = wanted.to_lowercase();
    let pieces: Vec<&str> = wanted.split(|c: char| !c.is_ascii_alphanumeric()).filter(|piece| piece.len() >= 3).collect();
    let mut names: Vec<&str> = tools.iter().filter_map(|tool| tool["name"].as_str()).filter(|name| server.offers(name))
        .filter(|name| { let lower = name.to_lowercase(); pieces.iter().any(|piece| lower.contains(piece)) }).collect();
    names.sort_unstable();
    names.truncate(8);
    names
}

/// Words the refusal of a tool name nobody has, naming the close ones.
fn unknown_tool(server: &Server, tools: &[Value], wanted: &str) -> String {
    let close = near(server, tools, wanted);
    let hint = if close.is_empty() { "Use list_tools, with query, to see its tools.".to_owned() }
        else { format!("Close names: {}. Use describe_tool to see one before calling it.", close.join(", ")) };
    format!("The server \"{}\" has no tool named \"{}\". {hint}", server.name, bounded(wanted, 128))
}

/// Lists the tools of every server that is on, a name and one line each, narrowed by `server`
/// and `query` when given. A server that cannot be reached is listed with the reason, so the
/// model can tell the user. Schemas are left to `describe_tool`, so a server with hundreds of
/// tools never floods a turn.
async fn list(backend: &Arc<Backend>, on: Vec<Server>, arguments: &Value) -> (&'static str, Value) {
    if on.is_empty() {
        return ("ok", answer(true, "No MCP server is switched on for this conversation. The user allows a server for this conversation's agent in Settings, Agents, and switches it from Tools, beside the message box."));
    }
    let only = arguments["server"].as_str().filter(|name| !name.is_empty());
    let query = arguments["query"].as_str().map(str::trim).filter(|query| !query.is_empty());
    let mut servers = Vec::new();
    for server in on.into_iter().filter(|server| only.is_none_or(|name| server.name == name)) {
        let slot = backend.mcp.slot(&server.id).await;
        let mut slot = slot.lock().await;
        servers.push(match ensure(&backend.mcp, &mut slot, &server).await {
            Ok(open) => json!({ "server": server.name, "tools": open.tools.iter()
                .filter(|tool| server.offers(tool["name"].as_str().unwrap_or_default()) && query.is_none_or(|query| matches(tool, query)))
                .map(|tool| json!({ "name": tool["name"], "summary": summary(tool["description"].as_str().unwrap_or_default()) })).collect::<Vec<_>>() }),
            Err(Fault(reason)) => json!({ "server": server.name, "unreachable": reason }),
        });
    }
    if servers.is_empty() && let Some(name) = only {
        return ("refused", answer(false, format!("The MCP server \"{}\" is not switched on for this conversation, or does not exist. Use list_tools without server to see the servers that are on.", bounded(name, 80))));
    }
    let note = "describe_tool with server and tool gives a tool's full description and inputSchema; list_tools with query narrows this list.";
    let mut text = json!({ "servers": servers, "note": note }).to_string();
    // Past the bound, names alone are listed; past it still, the model is told to narrow the list.
    if text.len() > LIST_MOST {
        for server in &mut servers { if let Some(tools) = server["tools"].as_array_mut() { for tool in tools { tool.as_object_mut().map(|tool| tool.remove("summary")); } } }
        text = json!({ "servers": servers, "note": note }).to_string();
    }
    if text.len() > LIST_MOST {
        let mut counts = servers.iter().map(|server| json!({ "server": server["server"], "toolCount": server["tools"].as_array().map_or(0, Vec::len),
            "unreachable": server["unreachable"] })).collect::<Vec<_>>();
        counts.retain(|server| !server["unreachable"].is_null() || server["toolCount"] != 0);
        text = json!({ "servers": counts, "note": "Too many tools to list at once. Use list_tools with server and query to narrow the list." }).to_string();
    }
    ("ok", answer(true, text))
}

/// Answers one tool's full description and input schema.
async fn describe(backend: &Arc<Backend>, on: Vec<Server>, arguments: &Value) -> (&'static str, Value) {
    let refused = |text: String| ("refused", answer(false, text));
    let (Some(name), Some(tool)) = (arguments["server"].as_str(), arguments["tool"].as_str())
        else { return refused("describe_tool needs server and tool. Use list_tools to see them.".into()) };
    let Some(server) = on.into_iter().find(|server| server.name == name) else {
        return refused(format!("The MCP server \"{}\" is not switched on for this conversation, or does not exist. Use list_tools to see the servers that are on.", bounded(name, 80)));
    };
    let slot = backend.mcp.slot(&server.id).await;
    let mut slot = slot.lock().await;
    match ensure(&backend.mcp, &mut slot, &server).await {
        Ok(open) => match open.tools.iter().find(|known| known["name"] == tool).filter(|_| server.offers(tool)) {
            Some(found) => ("ok", answer(true, bounded(&json!({ "server": server.name, "tool": tool,
                "description": found["description"].as_str().map(|text| bounded(text, 8 * 1024)), "inputSchema": found["inputSchema"] }).to_string(), MOST))),
            None => refused(unknown_tool(&server, &open.tools, tool)),
        },
        Err(Fault(reason)) => ("failed", answer(false, format!("The server \"{}\" could not be reached. {reason}", server.name))),
    }
}

/// Asks the user about one tool and waits for the answer. `None` means the conversation moved on.
async fn ask(backend: &Arc<Backend>, thread: &str, turn: &str, target: &Target, extra: Value) -> Option<ScopedChoice> {
    let (answer, decision) = oneshot::channel();
    let id = Uuid::new_v4();
    // An unattended run waits as long as its schedule says; otherwise the user has a minute.
    let unattended = backend.unattended.lock().ok().and_then(|runs| runs.get(thread).copied());
    let window = unattended.unwrap_or(Duration::from_secs(60));
    {
        let mut data = backend.data.lock().await;
        let chat = data.chats.get_mut(thread)?;
        if chat.stopping || chat.active.as_deref() != Some(turn) || chat.scoped_approval.is_some() { return None; }
        chat.scoped_approval = Some(ScopedApproval { id, turn: turn.into(), target: target.clone(), hosts: Vec::new(),
            extra: Some(extra), expires: Instant::now() + window, answer });
        chat.emit("snapshot", chat.snapshot());
    }
    backend.changed();
    let answered = tokio::time::timeout(window, decision).await.ok().and_then(Result::ok);
    if answered.is_none() && unattended.is_some() && let Ok(mut expired) = backend.unattended_expired.lock() {
        expired.insert(thread.to_owned(), serde_json::to_string(target).unwrap_or_default());
    }
    {
        let mut data = backend.data.lock().await;
        if let Some(chat) = data.chats.get_mut(thread) && chat.scoped_approval.as_ref().is_some_and(|open| open.id == id) {
            chat.scoped_approval = None;
            chat.emit("snapshot", chat.snapshot());
        }
    }
    backend.changed();
    Some(answered.unwrap_or(ScopedChoice::Reject))
}

/// Calls one tool of one server that is on, after the user approved it.
async fn call(backend: &Arc<Backend>, thread: &str, turn: &str, context: &Context, on: Vec<Server>, arguments: &Value)
    -> (&'static str, Value, Vec<Value>) {
    let refused = |text: String| ("refused", answer(false, text), Vec::new());
    let (Some(name), Some(tool)) = (arguments["server"].as_str(), arguments["tool"].as_str())
        else { return refused("call_tool needs server and tool. Use list_tools to see them.".into()) };
    let input = match arguments.get("arguments") {
        None | Some(Value::Null) => json!({}),
        Some(input) if input.is_object() => input.clone(),
        Some(_) => return refused("arguments must be an object that matches the tool's inputSchema.".into()),
    };
    let Some(server) = on.into_iter().find(|server| server.name == name) else {
        // A server kept to the other provider is said to be so, which is more use than "not switched on".
        if let Some(other) = backend.agents.mcp_servers().await.unwrap_or_default().into_iter()
            .find(|server| server.name == name && !server.provider.serves(backend.is_claude(thread))) {
            return refused(format!("The MCP server \"{}\" is for {}. The user set that in Settings, MCP servers.", other.name, other.provider.only()));
        }
        return refused(format!("The MCP server \"{}\" is not switched on for this conversation, or does not exist. The user allows a server for this conversation's agent in Settings, Agents, and switches it from Tools, beside the message box.",
            bounded(name, 80)));
    };
    let revision = backend.permission_revision.load(Ordering::Acquire);
    let slot = backend.mcp.slot(&server.id).await;
    // The tool must exist before the user is asked about it.
    {
        let mut slot = slot.lock().await;
        match ensure(&backend.mcp, &mut slot, &server).await {
            Ok(open) if server.offers(tool) && open.tools.iter().any(|known| known["name"] == tool) => {}
            Ok(open) => return refused(unknown_tool(&server, &open.tools, tool)),
            Err(Fault(reason)) => return ("failed", answer(false, format!("The server \"{}\" could not be reached. {reason}", server.name)), Vec::new()),
        }
    }
    let target = Target::McpTool { server: server.id.clone(), tool: tool.to_owned() };
    if target.validate().is_err() { return refused("That tool name cannot be used.".into()); }
    // Full access: a tool that would have asked runs, and a coding session on beside it does not ask.
    let full_access = matches!(backend.agents.permission(context, &Target::Feature { name: "full_access".into() },
        chrono::Utc::now().timestamp()).await, Ok(Decision::Allow(_)));
    let coding = !full_access && matches!(backend.agents.permission(context, &Target::Feature { name: "coding_session".into() },
        chrono::Utc::now().timestamp()).await, Ok(Decision::Allow(_)));
    let decision = backend.agents.permission(context, &target, chrono::Utc::now().timestamp()).await;
    let must_ask = match decision {
        // A coding session reviews everything, whatever was approved before.
        Ok(Decision::Allow(_)) => coding,
        Ok(Decision::Ask) => !full_access,
        _ => return refused("This tool is blocked for this conversation.".into()),
    };
    if must_ask {
        let extra = json!({ "serverName": server.name, "arguments": input, "unrestricted": server.unrestricted,
            "remote": server.url.is_some() });
        match ask(backend, thread, turn, &target, extra).await {
            None => return refused("The conversation moved on before this call was approved.".into()),
            Some(ScopedChoice::Reject) => return refused("The user rejected this tool call, so it was not made.".into()),
            // Programs belong to shell commands; the popup never offers them for a tool.
            Some(ScopedChoice::Once | ScopedChoice::Programs) => {}
            Some(ScopedChoice::Session | ScopedChoice::SessionDomains) => {
                let _gate = backend.agent_gate.read().await;
                if backend.permission_revision.load(Ordering::Acquire) != revision
                    || backend.agents.approve_session(context, &target, chrono::Utc::now().timestamp()).await.is_err() {
                    return refused("Permissions changed while this call was waiting, so it was not made.".into());
                }
            }
        }
    }
    // The server may have been switched off, changed or deleted while the user decided.
    let Some(server) = switched_on(backend, context, backend.is_claude(thread)).await.into_iter().find(|current| current.id == server.id)
        else { return refused(format!("The MCP server \"{name}\" is no longer switched on for this conversation.")) };
    let mut slot = slot.lock().await;
    let open = match ensure(&backend.mcp, &mut slot, &server).await {
        Ok(open) => open,
        Err(Fault(reason)) => return ("failed", answer(false, format!("The server \"{}\" could not be reached. {reason}", server.name)), Vec::new()),
    };
    match open.link.call(tool, input, Duration::from_secs(u64::from(server.tool_seconds))).await {
        Ok(result) => {
            let failed = result["isError"] == true;
            let (text, images) = (words(&result), pictures(&result));
            let sent = result["content"].as_array().map_or(0, |items| items.iter().filter(|item| item["type"] == "image").count());
            // The model is told how many pictures follow, and how many could not be passed on.
            let mut text = match (text.is_empty(), images.len()) {
                (true, 0) => "The tool returned nothing.".to_owned(),
                (true, _) => String::new(),
                (false, _) => text,
            };
            // Kept pictures are named by id, which is how the model hands one to another agent.
            let kept = keep(backend, thread, &server.name, tool, &images).await;
            if !images.is_empty() {
                let ids = kept.iter().map(|(view, id)| format!("attachment {id} ({})", view["name"].as_str().unwrap_or_default())).collect::<Vec<_>>();
                let named = if ids.is_empty() { String::new() } else { format!(" MiniClaw kept {}: {}.", if ids.len() == 1 { "it" } else { "them" }, ids.join(", ")) };
                text.push_str(&format!("{}[{} image{} returned, shown below.{named}]", if text.is_empty() { "" } else { "\n" }, images.len(),
                    if images.len() == 1 { "" } else { "s" }));
            }
            if sent > images.len() { text.push_str(&format!("\n[{} image(s) were too large or of an unsupported type and were not passed on.]", sent - images.len())); }
            let mut reply = answer(!failed, text);
            if let Some(items) = reply["contentItems"].as_array_mut() {
                items.extend(images.into_iter().map(|image| json!({ "type": "inputImage", "imageUrl": image })));
            }
            (if failed { "failed" } else { "ok" }, reply, kept.into_iter().map(|(view, _)| view).collect())
        }
        Err(Fault(reason)) => {
            // A connection that failed a call is not reused.
            *slot = None;
            ("failed", answer(false, reason), Vec::new())
        }
    }
}

#[cfg(all(test, unix))]
mod shutdown_tests {
    use super::*;

    /// A local server that lists one tool and then never answers; it leaves its process id in a file.
    fn fixture(root: &std::path::Path) -> (Server, std::path::PathBuf) {
        let pid = root.join("pid");
        let script = format!(concat!("echo $$ > '{}'\n",
            "read -r line; printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{{}},\"serverInfo\":{{\"name\":\"fixture\",\"version\":\"1\"}}}}}}'\n",
            "read -r line\n",
            "read -r line; printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{{\"tools\":[{{\"name\":\"wait\",\"description\":\"Never answers.\",\"inputSchema\":{{\"type\":\"object\"}}}}]}}}}'\n",
            "while read -r line; do :; done\n"), pid.display());
        let server: Server = serde_json::from_value(json!({ "id": "fixture", "name": "fixture", "kind": "stdio", "command": "/bin/sh",
            "args": ["-c", script], "unrestricted": true, "startupSeconds": 10 })).unwrap();
        (server, pid)
    }

    /// Reports whether the fixture's process is gone, or only waits to be reaped.
    async fn ended(pid: &std::path::Path) -> bool {
        let id: i32 = std::fs::read_to_string(pid).unwrap().trim().parse().unwrap();
        for _ in 0..50 {
            let state = std::process::Command::new("/bin/ps").args(["-o", "stat=", "-p", &id.to_string()]).output().unwrap();
            let state = String::from_utf8_lossy(&state.stdout).trim().to_owned();
            if state.is_empty() || state.starts_with('Z') { return true; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    /// Shutdown ends every running server and no later call can start one again.
    #[tokio::test]
    async fn closing_ends_every_server_and_refuses_new_ones() {
        let root = std::env::temp_dir().join(format!("mc-hub-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let (server, pid) = fixture(&root);
        let hub = Hub::default();
        let slot = hub.slot(&server.id).await;
        assert_eq!(ensure(&hub, &mut *slot.lock().await, &server).await.map(|open| open.tools.len()).map_err(|fault| fault.0), Ok(1));
        let id: i32 = std::fs::read_to_string(&pid).unwrap().trim().parse().unwrap();
        assert_eq!(unsafe { libc::kill(id, 0) }, 0, "the server runs while its connection is kept");
        hub.close_all().await;
        assert!(slot.lock().await.is_none(), "the connection was dropped");
        assert!(ended(&pid).await, "the server's process was ended");
        // A call that was already on its way cannot start the server again.
        let late = ensure(&hub, &mut *slot.lock().await, &server).await.map(|_| ()).map_err(|fault| fault.0);
        assert_eq!(late, Err("MiniClaw is shutting down, so the server was not started.".to_owned()));
        let fresh = hub.slot(&server.id).await;
        assert!(ensure(&hub, &mut *fresh.lock().await, &server).await.is_err());
        assert!(fresh.lock().await.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Shutdown does not wait for a tool call that is still running: the call is dropped.
    #[tokio::test]
    async fn closing_ends_a_call_in_flight() {
        let root = std::env::temp_dir().join(format!("mc-hub-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let (server, pid) = fixture(&root);
        let hub = Arc::new(Hub::default());
        let (started, running) = oneshot::channel();
        let (owner, entry) = (hub.clone(), server.clone());
        let call = tokio::spawn(async move {
            owner.run(async {
                let slot = owner.slot(&entry.id).await;
                let mut slot = slot.lock().await;
                let open = ensure(&owner, &mut slot, &entry).await.map_err(|fault| fault.0).unwrap();
                let _ = started.send(());
                open.link.call("wait", json!({}), Duration::from_secs(120)).await.map_err(|fault| fault.0)
            }).await
        });
        running.await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), hub.close_all()).await.expect("shutdown must not wait for the call");
        assert_eq!(tokio::time::timeout(Duration::from_secs(5), call).await.unwrap().unwrap(), None);
        assert!(ended(&pid).await, "the server's process was ended");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The open sandbox journals of this run's isolated data folder.
    fn journals() -> BTreeSet<String> {
        let data = std::path::PathBuf::from(std::env::var_os("MINICLAW_DATA_DIR").expect("Isolated native test data root"));
        std::fs::read_dir(data.join("sandbox-journals")).into_iter().flatten().flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned()).filter(|name| name.ends_with(".jsonl")).collect()
    }

    /// Shutdown with a live sandboxed MCP server: the hub lets go of it, the cleanup queue
    /// resolves its journal, and the recovery of the next start finds nothing to refuse.
    #[tokio::test]
    async fn shutdown_resolves_a_live_sandboxed_server() {
        let (server, secrets) = crate::mcp_client::windows_tests::shell_server(json!({}), "unused", 9);
        let before = journals();
        let hub = Hub::default();
        // The acceptance primitive connects while the product gate is still closed.
        let mut link = Link::connect(&server, &secrets).await.map_err(|fault| fault.0).unwrap();
        let tools = link.tools().await.map_err(|fault| fault.0).unwrap();
        *hub.slot(&server.id).await.lock().await = Some(Open { revision: server.revision, link, tools });
        let live: Vec<String> = journals().difference(&before).cloned().collect();
        assert_eq!(live.len(), 1, "the running server's sandbox has an open journal: {live:?}");
        // Without the hub's part of shutdown the queue is empty and the journal stays open.
        tokio::task::spawn_blocking(|| crate::windows_sandbox::drain_cleanup(Duration::from_secs(35))).await.unwrap().unwrap();
        assert_eq!(journals().difference(&before).count(), 1, "draining alone does not end a live server");
        hub.close_all().await;
        tokio::task::spawn_blocking(|| crate::windows_sandbox::drain_cleanup(Duration::from_secs(35))).await.unwrap().unwrap();
        assert_eq!(journals(), before, "the server's journal was resolved");
        tokio::task::spawn_blocking(crate::windows_sandbox::recover).await.unwrap()
            .expect("the next start's recovery has nothing left to refuse");
    }
}
