// Exercises the host backend's opt-in VM links without any virtual machine.
#![cfg(target_os = "macos")]
use miniclaw_server::{agents::Store, api, chat::Backend, environment::Assignment,
    remote::Settings, rpc::{Client, Failure}, runtime, worker};
// Sends guarded in-process API requests.
use axum::{body::{Body, to_bytes}, http::Request};
// Builds requests and reads replies.
use serde_json::{Value, json};
// Creates private sockets, scripts, and workspaces.
use std::{fs, os::unix::fs::PermissionsExt, path::{Path, PathBuf}};
// Accepts worker and supervisor connections.
use tokio::{io::{AsyncBufReadExt, AsyncWriteExt, BufReader}, net::UnixListener,
    process::Command};
// Calls the real Axum routes.
use tower::ServiceExt;
// Names disposable fixtures and tasks.
use uuid::Uuid;

struct Directory(PathBuf);
impl Directory {
    /// Creates a short private root; Unix socket paths must stay under 104 bytes.
    fn new() -> Self {
        let path = PathBuf::from("/private/tmp").join(format!("mchv-{}", Uuid::new_v4().simple()));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    /// Removes only this fixture.
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

/// Writes an executable fake Codex that records `thread/start` and issues a new id per chat.
fn fake_codex(root: &Path, name: &str, cwd: &str, id: &str) -> PathBuf {
    let node = std::process::Command::new("/usr/bin/which").arg("node").output().unwrap();
    let path = root.join(name);
    let script = format!(r#"#!{node}
import {{ createInterface }} from 'node:readline';
import {{ appendFileSync }} from 'node:fs';
const cwd = {cwd:?}, id = {id:?};
const thread = {{ id, cwd, source: 'appServer', modelProvider: 'openai', name: id, preview: id,
  createdAt: 1900000000, updatedAt: 1900000000, status: {{ type: 'idle' }}, turns: [] }};
createInterface({{ input: process.stdin }}).on('line', line => {{
  const msg = JSON.parse(line);
  if (msg.method === 'initialized' || !msg.method) return;
  if (msg.method === 'thread/start') appendFileSync(`${{cwd}}/starts`, `${{JSON.stringify(msg.params)}}\n`);
  if (msg.method === 'thread/resume') appendFileSync(`${{cwd}}/resumes`, `${{JSON.stringify(msg.params)}}\n`);
  // A saved task answers under its own id; a resume reports the folder it was given.
  const saved = {{ ...thread, id: msg.params?.threadId, cwd: msg.params?.cwd ?? cwd }};
  const result = msg.method === 'config/read' ? {{ config: {{ mcp_servers: {{}} }} }}
    : msg.method === 'account/read' ? {{ account: {{ type: 'chatgpt',
        email: 'host@example.invalid', planType: 'plus' }} }}
    : msg.method === 'thread/list' ? {{ data: [], nextCursor: null }}
    : ['thread/read', 'thread/unarchive'].includes(msg.method) ? {{ thread: saved }}
    : msg.method === 'thread/resume' ? {{ thread: saved, model: 'fixture-model', reasoningEffort: null }}
    : msg.method === 'thread/turns/list' ? {{ data: [], nextCursor: null }}
    : msg.method === 'thread/start' ? {{ thread: {{ ...thread, id: crypto.randomUUID() }},
        model: 'fixture-model', reasoningEffort: null }}
    : {{}};
  process.stdout.write(`${{JSON.stringify({{ id: msg.id, result }})}}\n`);
}});
"#, node = String::from_utf8_lossy(&node.stdout).trim());
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

/// Sends one guarded JSON POST and returns its status and body.
async fn post(router: axum::Router, path: &str, body: Value) -> (u16, Value) {
    let origin = "http://127.0.0.1:8787";
    let request = Request::builder().method("POST").uri(path)
        .header("host", "127.0.0.1:8787").header("origin", origin)
        .header("sec-fetch-site", "same-origin").header("content-type", "application/json")
        .header("x-miniclaw-request", "1").body(Body::from(body.to_string())).unwrap();
    let response = router.oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// The local Codex passes ordinary calls through and answers VM control itself, so
/// no separate Mac worker process is needed.
#[tokio::test]
async fn local_codex_answers_vm_control() {
    let root = Directory::new();
    let cwd = root.0.join("workspace");
    fs::create_dir(&cwd).unwrap();
    let binary = fake_codex(&root.0, "codex", cwd.to_str().unwrap(), &Uuid::new_v4().to_string());
    let socket = root.0.join("control.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let supervisor = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = BufReader::new(stream);
        let mut line = String::new();
        stream.read_line(&mut line).await.unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        stream.get_mut().write_all(b"{\"ok\":true,\"machines\":[]}\n").await.unwrap();
        request
    });
    let control = worker::Control { path: socket, key: [7; 32] };
    let client = runtime::launch_with_control(binary.to_str().unwrap(),
        cwd.to_str().unwrap(), control).await.unwrap();
    let account = client.call("account/read", json!({})).await.unwrap();
    assert_eq!(account["account"]["email"], "host@example.invalid");
    let listed = client.call("miniclaw/vm/list", json!({})).await.unwrap();
    assert_eq!(listed["machines"], json!([]));
    let request = supervisor.await.unwrap();
    assert_eq!(request["method"], "list");
    assert_eq!(request["mac"].as_str().unwrap().len(), 64);
    client.stop().await;
}

/// A VM worker is reached through its private Unix socket; a wrong key is refused.
/// Budget requests are signed like VM requests; the percentages are part of what is signed.
#[tokio::test]
async fn local_codex_relays_signed_ledger_requests() {
    use hmac::{Hmac, KeyInit, Mac};
    let root = Directory::new();
    let cwd = root.0.join("workspace");
    fs::create_dir(&cwd).unwrap();
    let binary = fake_codex(&root.0, "codex", cwd.to_str().unwrap(), &Uuid::new_v4().to_string());
    let socket = root.0.join("control.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let supervisor = tokio::spawn(async move {
        let mut seen = Vec::new();
        for _ in 0..2 {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            seen.push(serde_json::from_str::<Value>(&line).unwrap());
            stream.get_mut().write_all(
                b"{\"ok\":true,\"ledger\":{\"vms\":[],\"limits\":{\"cpuPercent\":40}}}\n").await.unwrap();
        }
        seen
    });
    let control = worker::Control { path: socket, key: [7; 32] };
    let client = runtime::launch_with_control(binary.to_str().unwrap(),
        cwd.to_str().unwrap(), control).await.unwrap();
    let view = client.call("miniclaw/vm/ledger", json!({})).await.unwrap();
    assert_eq!(view["limits"]["cpuPercent"], 40);
    client.call("miniclaw/vm/limits", json!({ "limits": "40,60,30" })).await.unwrap();
    for bad in [json!({ "limits": "0,60,30" }), json!({ "limits": "40,60" }), json!({}),
        json!({ "limits": "40,60,30", "extra": 1 })] {
        assert!(client.call("miniclaw/vm/limits", bad.clone()).await.is_err(), "{bad}");
    }
    assert!(client.call("miniclaw/vm/ledger", json!({ "limits": "40,60,30" })).await.is_err());
    let seen = supervisor.await.unwrap();
    assert_eq!(seen[0]["method"], "ledger");
    assert!(seen[0].get("limits").is_none());
    assert_eq!(seen[1]["limits"], "40,60,30");
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(&[7; 32]).unwrap();
    mac.update(format!("{}:limits:40,60,30", seen[1]["nonce"].as_str().unwrap()).as_bytes());
    let expected: String = mac.finalize().into_bytes().iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(seen[1]["mac"], expected, "the percentages are covered by the signature");
    client.stop().await;
}

#[tokio::test]
async fn vm_worker_connects_over_a_unix_socket() {
    let root = Directory::new();
    let cwd = root.0.join("workspace");
    fs::create_dir(&cwd).unwrap();
    let binary = fake_codex(&root.0, "codex", cwd.to_str().unwrap(), &Uuid::new_v4().to_string());
    let socket = root.0.join("controller.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let key = [9; 32];
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let binary = binary.clone();
            tokio::spawn(worker::serve(stream, key, move || Command::new(binary)));
        }
    });
    let settings = Settings { key, cwd: cwd.to_str().unwrap().into() };
    let client = settings.connect_unix(&socket).await.unwrap();
    let account = client.call("account/read", json!({})).await.unwrap();
    assert_eq!(account["account"]["type"], "chatgpt");
    client.stop().await;
    let wrong = Settings { key: [1; 32], cwd: cwd.to_str().unwrap().into() };
    assert_eq!(wrong.connect_unix(&socket).await.err(), Some(Failure::Rejected));
    let absent = settings.connect_unix(&root.0.join("none.sock")).await;
    assert_eq!(absent.err(), Some(Failure::Unavailable));
}

/// With VM links configured, the Mac keeps its own features: host agent chats still get
/// the Seatbelt tool, Claude still attaches, and a VM task never falls back to the Mac.
#[tokio::test]
async fn vm_capable_host_keeps_local_features() {
    let root = Directory::new();
    let host_cwd = root.0.join("host");
    fs::create_dir(&host_cwd).unwrap();
    let binary = fake_codex(&root.0, "codex", host_cwd.to_str().unwrap(),
        &Uuid::new_v4().to_string());
    let host = Client::spawn(Command::new(&binary)).await.unwrap();
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let vm_task = Uuid::new_v4().to_string();
    store.assign_existing(vm_task.clone(), Assignment::Vm { vm_id: "master".into() })
        .await.unwrap();
    let backend = Backend::new_with_host(None, Some(host.clone()),
        "/Users/Shared/MiniClaw/Workspace".into(), host_cwd.to_str().unwrap().into(),
        "http://127.0.0.1:8787".into(), root.0.join("recovery"), store, None);
    backend.set_local();
    backend.set_local_helper("/fixture/SkyComputerUseClient".into());
    let claude = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/support/fake-claude.mjs");
    let bridge = miniclaw_server::claude::launch(claude.into(),
        host_cwd.to_str().unwrap().into(), root.0.join("claude-threads")).await.unwrap();
    backend.attach_claude(bridge.clone()).await.unwrap();
    let router = api::router(backend.clone());
    let (status, saved) = post(router.clone(), "/api/agents", json!({
        "name": "Editor", "instructions": "Edit files.", "active": true })).await;
    assert_eq!(status, 201, "{saved}");
    let (status, created) = post(router.clone(), "/api/threads", json!({
        "agentId": saved["agent"]["id"], "environment": { "kind": "host" } })).await;
    assert_eq!(status, 201, "{created}");
    let starts = fs::read_to_string(host_cwd.join("starts")).unwrap();
    let start: Value = serde_json::from_str(starts.lines().next().unwrap()).unwrap();
    assert_eq!(start["dynamicTools"][0]["name"], "miniclaw_scoped_native");
    // Computer Use on the Mac uses the Mac's own helper form, never the guest flag.
    let (status, computer) = post(router.clone(), "/api/threads", json!({
        "computerUse": true, "environment": { "kind": "host" } })).await;
    assert_eq!(status, 201, "{computer}");
    let starts = fs::read_to_string(host_cwd.join("starts")).unwrap();
    let start: Value = serde_json::from_str(starts.lines().last().unwrap()).unwrap();
    assert!(start["config"]["mcp_servers.miniclaw_computer"].is_object(), "{start}");
    assert!(start.get("miniclawComputerUse").is_none(), "{start}");
    // Switching Computer Use on a Mac chat resumes it in the Mac's folder, not the VM's.
    let (status, plain) = post(router.clone(), "/api/threads", json!({
        "environment": { "kind": "host" } })).await;
    assert_eq!(status, 201, "{plain}");
    let plain_id = plain["thread"]["id"].as_str().unwrap();
    let (status, switched) = post(router.clone(), &format!("/api/threads/{plain_id}/computer"),
        json!({ "enabled": true })).await;
    assert_eq!(status, 200, "{switched}");
    let resumes = fs::read_to_string(host_cwd.join("resumes")).unwrap();
    let resume: Value = serde_json::from_str(resumes.lines().last().unwrap()).unwrap();
    assert_eq!(resume["cwd"], host_cwd.to_str().unwrap(), "{resume}");
    let (status, refused) = post(router, &format!("/api/threads/{vm_task}/delete"),
        json!({})).await;
    assert_eq!(status, 503, "{refused}");
    assert_eq!(refused["error"]["code"], "vm_unavailable");
    bridge.stop().await;
    host.stop().await;
}
