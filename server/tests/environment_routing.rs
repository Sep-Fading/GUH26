// Exercises the same API with a synthetic VM-owned Codex process.
use miniclaw_server::{agents::Store, api, chat::Backend,
    environment::Assignment, rpc::Client};
// Sends guarded in-process requests without opening a listener.
use axum::{body::{Body, to_bytes}, http::Request};
// Builds and checks machine choices as JSON.
use serde_json::{Value, json};
// Owns disposable metadata and checks worker calls.
use std::{fs, path::PathBuf, time::Duration};
// Runs the fake Codex peer with no live credentials.
use tokio::process::Command;
// Calls the actual Axum routes under test.
use tower::ServiceExt;
// Reads the first emitted server-sent event.
use futures_util::StreamExt;
// Generates private fixture paths and thread IDs.
use uuid::Uuid;

struct Directory(PathBuf);
impl Directory {
    /// Creates a disposable directory and returns its owned path.
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("mc-route-{}", Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    /// Removes only this test's disposable metadata and fake workspace.
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

/// Starts a fake App Server that acknowledges only the methods needed here.
fn peer(cwd: &str, id: &str) -> Command {
    let mut command = Command::new("node");
    command.env("MC_TEST_CWD", cwd).env("MC_TEST_ID", id)
        .args(["--input-type=module", "-e", r#"
        import { createInterface } from 'node:readline';
        import { appendFileSync, existsSync } from 'node:fs';
        const cwd = process.env.MC_TEST_CWD;
        const id = process.env.MC_TEST_ID;
        const send = value => console.log(JSON.stringify(value));
        createInterface({ input: process.stdin }).on('line', line => {
            const msg = JSON.parse(line);
            if (!msg.method && msg.id === 'scope-1') {
                appendFileSync(`${cwd}/tool-replies`, `${JSON.stringify(msg.result)}\n`);
                return;
            }
            if (msg.method === 'initialized') return;
            if (msg.method === 'turn/interrupt') appendFileSync(`${cwd}/interrupts`, `${msg.params.threadId}\n`);
            if (msg.method === 'account/login/start') appendFileSync(`${cwd}/logins`, 'device\n');
            if (msg.method === 'thread/start') appendFileSync(`${cwd}/starts`, `${JSON.stringify(msg.params)}\n`);
            if (msg.method === 'thread/delete') appendFileSync(`${cwd}/deletes`, `${msg.params.threadId}\n`);
            if (msg.method === 'thread/resume') appendFileSync(`${cwd}/resumes`, `${JSON.stringify(msg.params)}\n`);
            if ((msg.params?.threadId && msg.params.threadId !== id) ||
                (msg.params?.ancestorThreadId && msg.params.ancestorThreadId !== id)) {
                send({ id: msg.id, error: { code: -32000, message: 'Foreign task' } });
                return;
            }
            const thread = { id, cwd, source: 'appServer', modelProvider: 'openai',
                name: id, preview: id,
                createdAt: 1900000000, updatedAt: 1900000000,
                status: { type: 'idle' }, turns: [] };
            const result = msg.method === 'account/read' ? { account:
                process.env.MC_TEST_SIGNED_OUT ? null : {
                    type: 'chatgpt', email: 'test@example.invalid', planType: 'plus' } }
                : msg.method === 'miniclaw/vm/delete' ? { state: 'missing' }
                : msg.method === 'miniclaw/vm/stop' ? { state: 'stopping' }
                : msg.method === 'miniclaw/vm/show' ? { state: 'running' }
                : msg.method === 'miniclaw/vm/status' ? (msg.params?.vmId
                    ? (existsSync(`${cwd}/vm-failed`)
                        ? { state: 'failed', reason: 'Project VM stopped before it was ready.' }
                        : existsSync(`${cwd}/vm-deleted`)
                        ? { state: 'missing', reason: 'Assigned VM does not exist.' }
                        : process.env.MC_TEST_DEDICATED_OFF || existsSync(`${cwd}/vm-off`)
                        ? { state: 'off' }
                        : process.env.MC_TEST_DEDICATED_SETUP
                            ? { state: 'starting', stage: 'installing', percent: 42 }
                        : process.env.MC_TEST_DEDICATED_ID === msg.params.vmId
                            ? { state: 'running' }
                        : { state: 'missing', reason: 'Assigned VM does not exist.' })
                    : { state: existsSync(`${cwd}/master-running`) ? 'running' : 'off' })
                : msg.method === 'miniclaw/vm/start' ? (process.env.MC_TEST_VM_START_FAIL
                    ? { state: 'failed', reason: 'VM resource limit exceeded.' }
                    : { state: 'starting' })
                : msg.method === 'miniclaw/vm/list' ? { machines: process.env.MC_TEST_DEDICATED_ID
                    && !existsSync(`${cwd}/vm-deleted`)
                    ? [{ vmId: process.env.MC_TEST_DEDICATED_ID, slot: 0,
                        key: 'a'.repeat(64), cwd: '/Users/Shared/MiniClaw/Workspace' }] : [] }
                : msg.method === 'miniclaw/vm/provision' ? (process.env.MC_TEST_SETUP_FULL
                    ? { state: 'failed', reason: 'Project VM limit reached.' }
                    : { vmId: process.env.MC_TEST_DEDICATED_ID,
                        state: 'starting', stage: 'preparing' })
                : msg.method === 'miniclaw/vm/retry' ? { state: 'starting', stage: 'installing' }
                : msg.method === 'miniclaw/vm/cancel' ? { state: 'failed', stage: 'failed',
                    reason: 'VM setup was cancelled. Its files were kept for retry.' }
                : msg.method === 'account/login/start' ? { loginId: 'dedicated-login',
                    verificationUrl: 'https://auth.openai.com/codex/device', userCode: 'TEST-CODE' }
                : msg.method === 'thread/list' ? { data: msg.params?.ancestorThreadId ? [] : [thread], nextCursor: null }
                : msg.method === 'thread/start' ? { thread, model: 'fixture-model',
                    reasoningEffort: null }
                : msg.method === 'thread/read' ? { thread }
                : msg.method === 'thread/unarchive' ? { thread }
                : msg.method === 'thread/resume' ? { thread, model: 'fixture-model',
                    reasoningEffort: null }
                : msg.method === 'thread/turns/list' ? { data: [], nextCursor: null }
                : msg.method === 'miniclaw/scoped/status' ? {
                    ready: process.env.MC_TEST_SCOPED_OFF !== '1' }
                : msg.method === 'miniclaw/computer/read' ? { available: process.env.MC_TEST_COMPUTER === '1' }
                : msg.method === 'miniclaw/scoped/execute' ? { status: 'ok', text: 'approved fixture' }
                : msg.method === 'turn/start' ? { turn: { id: `${id}-turn`,
                    status: 'inProgress', items: [] } } : {};
            if (['miniclaw/vm/delete', 'miniclaw/vm/stop', 'miniclaw/vm/show'].includes(msg.method)) {
                appendFileSync(`${cwd}/vm-actions`, `${msg.method.slice(12)} ${msg.params?.vmId ?? 'master'}\n`);
                if (msg.method === 'miniclaw/vm/delete') appendFileSync(`${cwd}/vm-deleted`, '');
            }
            if (msg.method === 'miniclaw/vm/start' && msg.params?.vmId) {
                appendFileSync(`${cwd}/vm-starts`, `${msg.params.vmId}\n`);
            }
            if (msg.method === 'miniclaw/desktop/frame') {
                appendFileSync(`${cwd}/frame-requests`, `${JSON.stringify(msg.params)}\n`);
                // An older worker accepts only empty parameters.
                if (process.env.MC_TEST_OLD_WORKER === '1' && msg.params?.viewer) {
                    return send({ id: msg.id, error: { code: -32602, message: 'Desktop capture denied' } });
                }
                return send({ id: msg.id, result: { jpeg: '/9j/4AAQSkZJRgABAQAASABIAAD/4QBMRXhpZgAATU0AKgAAAAgAAYdpAAQAAAABAAAAGgAAAAAAA6ABAAMAAAABAAEAAKACAAQAAAABAAAACKADAAQAAAABAAAACAAAAAD/7QA4UGhvdG9zaG9wIDMuMAA4QklNBAQAAAAAAAA4QklNBCUAAAAAABDUHYzZjwCyBOmACZjs+EJ+/8AAEQgACAAIAwEiAAIRAQMRAf/EAB8AAAEFAQEBAQEBAAAAAAAAAAABAgMEBQYHCAkKC//EALUQAAIBAwMCBAMFBQQEAAABfQECAwAEEQUSITFBBhNRYQcicRQygZGhCCNCscEVUtHwJDNicoIJChYXGBkaJSYnKCkqNDU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6g4SFhoeIiYqSk5SVlpeYmZqio6Slpqeoqaqys7S1tre4ubrCw8TFxsfIycrS09TV1tfY2drh4uPk5ebn6Onq8fLz9PX29/j5+v/EAB8BAAMBAQEBAQEBAQEAAAAAAAABAgMEBQYHCAkKC//EALURAAIBAgQEAwQHBQQEAAECdwABAgMRBAUhMQYSQVEHYXETIjKBCBRCkaGxwQkjM1LwFWJy0QoWJDThJfEXGBkaJicoKSo1Njc4OTpDREVGR0hJSlNUVVZXWFlaY2RlZmdoaWpzdHV2d3h5eoKDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uLj5OXm5+jp6vLz9PX29/j5+v/bAEMAAgICAgICAwICAwUDAwMFBgUFBQUGCAYGBgYGCAoICAgICAgKCgoKCgoKCgwMDAwMDA4ODg4ODw8PDw8PDw8PD//bAEMBAgICBAQEBwQEBxALCQsQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEP/dAAQAAf/aAAwDAQACEQMRAD8A9o1v4h/BDw38Pz/wkXwwFjqTWjwJeSThma5ERHmLHErsxDANgAnFfK//AAuH4X/9Az/yXvP/AJHrtf2iv+Ra0X/r5m/9EmvjavWhn+L1/eP73/mfFYbJaFWnGUor5KP/AMif/9k=' } });
            }
            send({ id: msg.id, result });
            if (msg.method === 'miniclaw/scoped/execute') {
                appendFileSync(`${cwd}/executions`, `${JSON.stringify(msg.params)}\n`);
            }
            if (msg.method === 'turn/start' && process.env.MC_TEST_SCOPED_TOOL === '1') {
                setTimeout(() => {
                    send({ method: 'turn/started', params: {
                        threadId: id, turn: { id: `${id}-turn`, status: 'inProgress' },
                    } });
                    send({ id: 'scope-1', method: 'item/tool/call', params: {
                        tool: 'miniclaw_scoped_native', threadId: id,
                        turnId: `${id}-turn`, callId: 'fixture-call',
                        arguments: { action: 'read_file', path: `${cwd.replace(/\/[^/]+$/, '')}/approved.txt` },
                    } });
                }, Number(process.env.MC_TEST_SCOPED_TOOL_DELAY_MS || 10));
            }
            // A hostile worker claims that another machine's turn has finished.
            if (msg.method === 'turn/start' && process.env.MC_TEST_FORGE_COMPLETE) {
                const forged = process.env.MC_TEST_FORGE_COMPLETE;
                setTimeout(() => send({ method: 'turn/completed', params: {
                    threadId: forged, turn: { id: `${forged}-turn`, status: 'completed' },
                } }), 10);
            }
            if (msg.method === 'turn/start' && process.env.MC_TEST_COMPLETE === '1') {
                setTimeout(() => send({ method: 'turn/completed', params: {
                    threadId: id, turn: { id: `${id}-turn`, status: 'completed' },
                } }), 10);
            }
        });
    "#]);
    command
}

/// A VM agent chat is offered the owned tool, and its own worker runs it.
#[tokio::test]
async fn master_agent_receives_scoped_tool() {
    let root = Directory::new();
    let workspace = root.0.join("workspace");
    fs::create_dir(&workspace).unwrap();
    let cwd = workspace.to_str().unwrap().to_owned();
    let id = Uuid::new_v4().to_string();
    let client = Client::spawn(peer(&cwd, &id)).await.unwrap();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let backend = Backend::new(Some(client.clone()), cwd, origin.into(),
        root.0.join("recovery"), store, None, true);
    let router = api::router(backend);
    let (status, saved) = post(router.clone(), origin, "/api/agents", json!({
        "name":"Scoped", "instructions":"Use only the provided tool for files or apps.",
        "model":null,"effort":null,"active":true
    })).await;
    assert_eq!(status, 201, "{saved}");
    let agent = saved["agent"]["id"].as_str().unwrap();
    let (status, created) = post(router, origin, "/api/threads", json!({
        "agentId":agent,"environment":{"kind":"vm"}
    })).await;
    assert_eq!(status, 201, "{created}");
    let starts = fs::read_to_string(workspace.join("starts")).unwrap();
    let first: Value = serde_json::from_str(starts.lines().next().unwrap()).unwrap();
    assert_eq!(first["dynamicTools"][0]["name"], "miniclaw_scoped_native");
    client.stop().await;
}

/// Rejecting a model-requested file read never calls the guest executor.
#[tokio::test]
async fn scoped_reject_never_executes() {
    let root = Directory::new();
    let workspace = root.0.join("workspace");
    fs::create_dir(&workspace).unwrap();
    let cwd = workspace.to_str().unwrap().to_owned();
    let id = Uuid::new_v4().to_string();
    let mut command = peer(&cwd, &id);
    command.env("MC_TEST_SCOPED_TOOL", "1");
    let client = Client::spawn(command).await.unwrap();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let backend = Backend::new(Some(client.clone()), cwd, origin.into(),
        root.0.join("recovery"), store, None, true);
    let router = api::router(backend);
    let (status, saved) = post(router.clone(), origin, "/api/agents", json!({
        "name":"Scoped", "instructions":"Use only the provided tool.",
        "model":null,"effort":null,"active":true
    })).await;
    assert_eq!(status, 201, "{saved}");
    let agent = saved["agent"]["id"].as_str().unwrap();
    let (status, created) = post(router.clone(), origin, "/api/threads", json!({
        "agentId":agent,"environment":{"kind":"vm"}
    })).await;
    assert_eq!(status, 201, "{created}");
    let thread = created["thread"]["id"].as_str().unwrap();
    let (status, sent) = post(router.clone(), origin,
        &format!("/api/threads/{thread}/messages"), json!({
            "text":"Read the approved file", "clientMessageId":Uuid::new_v4()
        })).await;
    assert_eq!(status, 202, "{sent}");
    let approval = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let (_, snapshot) = get(router.clone(), origin,
                &format!("/api/threads/{thread}")).await;
            if snapshot["scopedApproval"]["id"].is_string() {
                break snapshot["scopedApproval"]["id"].as_str().unwrap().to_owned();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.unwrap();
    assert!(!workspace.join("executions").exists());
    let (status, _) = post(router, origin,
        &format!("/api/threads/{thread}/scoped-approvals/{approval}"),
        json!({"action":"reject"})).await;
    assert_eq!(status, 200);
    tokio::time::timeout(Duration::from_secs(3), async {
        while !workspace.join("tool-replies").exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.unwrap();
    assert!(!workspace.join("executions").exists());
    let reply: Value = serde_json::from_str(&fs::read_to_string(
        workspace.join("tool-replies")).unwrap()).unwrap();
    assert_eq!(reply["success"], false);
    client.stop().await;
}

/// One-off and eight-hour decisions both gate the exact guest request.
#[tokio::test]
async fn scoped_approvals_dispatch_and_store_only_sessions() {
    for (action, session) in [("approve_once", false), ("approve_session", true)] {
        let root = Directory::new();
        let workspace = root.0.join("workspace");
        fs::create_dir(&workspace).unwrap();
        let cwd = workspace.to_str().unwrap().to_owned();
        let id = Uuid::new_v4().to_string();
        let mut command = peer(&cwd, &id);
        command.env("MC_TEST_SCOPED_TOOL", "1");
        let client = Client::spawn(command).await.unwrap();
        let origin = "http://127.0.0.1:8787";
        let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
        let backend = Backend::new(Some(client.clone()), cwd, origin.into(),
            root.0.join("recovery"), store, None, true);
        let router = api::router(backend);
        let (status, saved) = post(router.clone(), origin, "/api/agents", json!({
            "name":"Scoped", "instructions":"Use only the provided tool.",
            "model":null,"effort":null,"active":true
        })).await;
        assert_eq!(status, 201, "{saved}");
        let agent = saved["agent"]["id"].as_str().unwrap();
        let (status, created) = post(router.clone(), origin, "/api/threads", json!({
            "agentId":agent,"environment":{"kind":"vm"}
        })).await;
        assert_eq!(status, 201, "{created}");
        let thread = created["thread"]["id"].as_str().unwrap();
        let (status, sent) = post(router.clone(), origin,
            &format!("/api/threads/{thread}/messages"), json!({
                "text":"Read the approved file", "clientMessageId":Uuid::new_v4()
            })).await;
        assert_eq!(status, 202, "{sent}");
        let approval = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let (_, snapshot) = get(router.clone(), origin,
                    &format!("/api/threads/{thread}")).await;
                if let Some(id) = snapshot["scopedApproval"]["id"].as_str() {
                    break id.to_owned();
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.unwrap();
        assert!(!workspace.join("executions").exists());
        let (status, _) = post(router.clone(), origin,
            &format!("/api/threads/{thread}/scoped-approvals/{approval}"),
            json!({"action":action})).await;
        assert_eq!(status, 200);
        tokio::time::timeout(Duration::from_secs(3), async {
            while !workspace.join("executions").exists() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.unwrap();
        let execution: Value = serde_json::from_str(&fs::read_to_string(
            workspace.join("executions")).unwrap()).unwrap();
        assert_eq!(execution["operation"]["action"], "read_file");
        assert_eq!(execution["operation"]["path"],
            format!("{}/approved.txt", workspace.parent().unwrap().display()));
        assert!(execution["grants"].is_array(), "the chat's own worker gets the grants");
        let (_, permissions) = get(router, origin,
            &format!("/api/permissions/conversation/{thread}")).await;
        assert_eq!(permissions["sources"]["session"].as_array().unwrap().len(),
            usize::from(session));
        client.stop().await;
    }
}

/// A rule saved while the approval is open wins over the answer given to that approval.
#[tokio::test]
async fn a_block_saved_while_the_approval_is_open_stops_the_operation() {
    let root = Directory::new();
    let workspace = root.0.join("workspace");
    fs::create_dir(&workspace).unwrap();
    let cwd = workspace.to_str().unwrap().to_owned();
    let mut command = peer(&cwd, &Uuid::new_v4().to_string());
    command.env("MC_TEST_SCOPED_TOOL", "1");
    let client = Client::spawn(command).await.unwrap();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let backend = Backend::new(Some(client.clone()), cwd, origin.into(),
        root.0.join("recovery"), store, None, true);
    let router = api::router(backend);
    let (status, saved) = post(router.clone(), origin, "/api/agents", json!({
        "name":"Scoped", "instructions":"Use only the provided tool.",
        "model":null,"effort":null,"active":true })).await;
    assert_eq!(status, 201, "{saved}");
    let (status, created) = post(router.clone(), origin, "/api/threads", json!({
        "agentId":saved["agent"]["id"],"environment":{"kind":"vm"} })).await;
    assert_eq!(status, 201, "{created}");
    let thread = created["thread"]["id"].as_str().unwrap();
    let (status, _) = post(router.clone(), origin, &format!("/api/threads/{thread}/messages"),
        json!({ "text":"Read the approved file", "clientMessageId":Uuid::new_v4() })).await;
    assert_eq!(status, 202);
    let approval = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let (_, snapshot) = get(router.clone(), origin, &format!("/api/threads/{thread}")).await;
            if let Some(id) = snapshot["scopedApproval"]["id"].as_str() { break id.to_owned(); }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.unwrap();
    let target = json!({"target_type":"file","action":"read",
        "path":format!("{}/approved.txt", workspace.parent().unwrap().display())});
    let (status, _) = post(router.clone(), origin,
        &format!("/api/permissions/conversation/{thread}"),
        json!({"target":target,"effect":"block"})).await;
    assert_eq!(status, 200);
    // The answer may be refused as stale or accepted; either way nothing may run.
    let _ = post(router.clone(), origin,
        &format!("/api/threads/{thread}/scoped-approvals/{approval}"),
        json!({"action":"approve_once"})).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while !workspace.join("tool-replies").exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.unwrap();
    assert!(!workspace.join("executions").exists(), "a blocked operation ran");
    client.stop().await;
}

/// Removing a grant before a queued call reaches the guest fails that call.
#[tokio::test]
async fn scoped_revocation_stops_queued_call() {
    let root = Directory::new();
    let workspace = root.0.join("workspace");
    fs::create_dir(&workspace).unwrap();
    let cwd = workspace.to_str().unwrap().to_owned();
    let id = Uuid::new_v4().to_string();
    let mut command = peer(&cwd, &id);
    command.env("MC_TEST_SCOPED_TOOL", "1")
        .env("MC_TEST_SCOPED_TOOL_DELAY_MS", "200");
    let client = Client::spawn(command).await.unwrap();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let backend = Backend::new(Some(client.clone()), cwd, origin.into(),
        root.0.join("recovery"), store, None, true);
    let router = api::router(backend);
    let (status, saved) = post(router.clone(), origin, "/api/agents", json!({
        "name":"Scoped", "instructions":"Use only the provided tool.",
        "model":null,"effort":null,"active":true
    })).await;
    assert_eq!(status, 201, "{saved}");
    let agent = saved["agent"]["id"].as_str().unwrap();
    let (status, created) = post(router.clone(), origin, "/api/threads", json!({
        "agentId":agent,"environment":{"kind":"vm"}
    })).await;
    assert_eq!(status, 201, "{created}");
    let thread = created["thread"]["id"].as_str().unwrap();
    let target = json!({"target_type":"file","action":"read",
        "path":format!("{}/approved.txt", workspace.parent().unwrap().display())});
    let (status, _) = post(router.clone(), origin,
        &format!("/api/permissions/conversation/{thread}"),
        json!({"target":target,"effect":"allow"})).await;
    assert_eq!(status, 200);
    let (status, sent) = post(router.clone(), origin,
        &format!("/api/threads/{thread}/messages"), json!({
            "text":"Read the approved file", "clientMessageId":Uuid::new_v4()
        })).await;
    assert_eq!(status, 202, "{sent}");
    let (status, _) = post(router.clone(), origin,
        &format!("/api/permissions/conversation/{thread}"),
        json!({"target":target,"effect":"remove"})).await;
    assert_eq!(status, 200);
    tokio::time::timeout(Duration::from_secs(3), async {
        while !workspace.join("tool-replies").exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.unwrap();
    assert!(!workspace.join("executions").exists());
    let reply: Value = serde_json::from_str(&fs::read_to_string(
        workspace.join("tool-replies")).unwrap()).unwrap();
    assert_eq!(reply["success"], false);
    let (_, snapshot) = get(router, origin, &format!("/api/threads/{thread}")).await;
    assert!(snapshot["scopedApproval"].is_null());
    client.stop().await;
}

/// Calls one guarded API mutation and returns its status and JSON body.
async fn post(router: axum::Router, origin: &str, path: &str, body: Value)
    -> (u16, Value)
{
    let request = Request::builder().method("POST").uri(path)
        .header("host", origin.strip_prefix("http://").unwrap())
        .header("origin", origin).header("sec-fetch-site", "same-origin")
        .header("content-type", "application/json")
        .header("x-miniclaw-request", "1")
        .body(Body::from(body.to_string())).unwrap();
    let response = router.oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// Reads one guarded API resource and returns its status and JSON body.
async fn get(router: axum::Router, origin: &str, path: &str) -> (u16, Value) {
    let request = Request::builder().method("GET").uri(path)
        .header("host", origin.strip_prefix("http://").unwrap())
        .header("origin", origin).body(Body::empty()).unwrap();
    let response = router.oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// A VM-owned client accepts VM work and refuses Host work without fallback.
#[tokio::test]
async fn guest_only_routes_vm_choice() {
    let root = Directory::new();
    let workspace = root.0.join("workspace");
    fs::create_dir(&workspace).unwrap();
    let cwd = workspace.to_str().unwrap().to_owned();
    let client = Client::spawn(peer(&cwd, "019a1111-1111-7111-8111-111111111111")).await.unwrap();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let backend = Backend::new(Some(client.clone()), cwd, origin.into(),
        root.0.join("recovery"), store, None, true);
    let router = api::router(backend.clone());
    let (control_status, control) = get(router.clone(), origin,
        "/api/vms/master/status").await;
    assert_eq!(control_status, 503, "{control}");
    assert_eq!(control["error"]["code"], "vm_control_unavailable");
    let (host_status, host) = post(router.clone(), origin, "/api/threads",
        json!({ "environment": { "kind": "host" } })).await;
    assert_eq!(host_status, 503);
    assert_eq!(host["error"]["code"], "host_unavailable");
    let (vm_status, vm) = post(router.clone(), origin, "/api/threads",
        json!({ "environment": { "kind": "vm" } })).await;
    assert_eq!(vm_status, 201);
    assert_eq!(vm["thread"]["environment"], json!({ "kind": "vm" }));
    assert_eq!(vm["thread"]["vmId"], "master");
    let id = vm["thread"]["id"].as_str().unwrap();
    let (send_status, _) = post(router, origin, &format!("/api/threads/{id}/messages"),
        json!({ "text": "Work in the VM", "clientMessageId": Uuid::new_v4() })).await;
    assert_eq!(send_status, 202);
    client.stop().await;
}

/// One backend routes each selected conversation to its own worker and workspace.
#[tokio::test]
async fn one_backend_keeps_host_and_vm_separate() {
    let root = Directory::new();
    let host_path = root.0.join("host");
    let vm_path = root.0.join("vm");
    fs::create_dir(&host_path).unwrap();
    fs::create_dir(&vm_path).unwrap();
    let host_cwd = host_path.to_str().unwrap().to_owned();
    let vm_cwd = vm_path.to_str().unwrap().to_owned();
    let host_id = "019a2222-2222-7222-8222-222222222222";
    let vm_id = "019a3333-3333-7333-8333-333333333333";
    let host = Client::spawn(peer(&host_cwd, host_id)).await.unwrap();
    let vm = Client::spawn(peer(&vm_cwd, vm_id)).await.unwrap();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let backend = Backend::new_with_host(Some(vm.clone()), Some(host.clone()),
        vm_cwd.clone(), host_cwd.clone(), origin.into(), root.0.join("recovery"),
        store.clone(), None);
    let router = api::router(backend.clone());
    let (host_status, host_reply) = post(router.clone(), origin, "/api/threads",
        json!({ "environment": { "kind": "host" } })).await;
    assert_eq!(host_status, 201);
    assert_eq!(host_reply["thread"]["id"], host_id);
    let (vm_status, vm_reply) = post(router.clone(), origin, "/api/threads",
        json!({ "environment": { "kind": "vm" } })).await;
    assert_eq!(vm_status, 201);
    assert_eq!(vm_reply["thread"]["id"], vm_id);
    for (id, expected) in [(host_id, host_id), (vm_id, vm_id)] {
        let (status, reply) = post(router.clone(), origin,
            &format!("/api/threads/{id}/messages"),
            json!({ "text": "Stay assigned", "clientMessageId": Uuid::new_v4() })).await;
        assert_eq!(status, 202);
        assert_eq!(reply["turnId"], format!("{expected}-turn"));
    }
    let restored = Backend::new_with_host(Some(vm.clone()), Some(host.clone()),
        vm_cwd, host_cwd, origin.into(), root.0.join("recovery"), store,
        Some(miniclaw_server::computer::Target::Guest));
    let restored = api::router(restored);
    let (status, list) = get(restored.clone(), origin, "/api/threads").await;
    assert_eq!(status, 200, "{list}");
    let ids: Vec<_> = list["threads"].as_array().unwrap().iter()
        .filter_map(|task| task["id"].as_str()).collect();
    assert!(ids.contains(&host_id));
    assert!(ids.contains(&vm_id));
    for (id, kind) in [(host_id, "host"), (vm_id, "vm")] {
        let (status, snapshot) = get(restored.clone(), origin,
            &format!("/api/threads/{id}")).await;
        assert_eq!(status, 200, "{snapshot}");
        assert_eq!(snapshot["thread"]["environment"]["kind"], kind);
        let (status, older) = get(restored.clone(), origin,
            &format!("/api/threads/{id}/history?cursor=older")).await;
        assert_eq!(status, 200, "{older}");
    }
    backend.cancel(host_id, "synthetic-turn");
    tokio::time::timeout(Duration::from_secs(2), async {
        while !host_path.join("interrupts").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    assert_eq!(fs::read_to_string(host_path.join("interrupts")).unwrap(),
        format!("{host_id}\n"));
    assert!(!vm_path.join("interrupts").exists());
    let (computer_status, computer) = post(restored.clone(), origin,
        &format!("/api/threads/{host_id}/computer"), json!({ "enabled": true })).await;
    assert_eq!(computer_status, 503, "{computer}");
    assert_eq!(computer["error"]["code"], "computer_unavailable");
    let (delete_status, delete) = post(restored.clone(), origin,
        &format!("/api/threads/{host_id}/delete"), json!({})).await;
    assert_eq!(delete_status, 200, "{delete}");
    let (status, vm_snapshot) = get(restored.clone(), origin,
        &format!("/api/threads/{vm_id}")).await;
    assert_eq!(status, 200, "{vm_snapshot}");
    drop(restored);
    host.stop().await;
    vm.stop().await;
}

/// A worker can only speak for its own chats: a VM worker that reports another machine's
/// turn as finished is ignored, so it cannot end, unlock or alter a Mac conversation.
#[tokio::test]
async fn a_worker_cannot_finish_another_machines_turn() {
    let root = Directory::new();
    let host_path = root.0.join("host");
    let vm_path = root.0.join("vm");
    fs::create_dir(&host_path).unwrap();
    fs::create_dir(&vm_path).unwrap();
    let host_cwd = host_path.to_str().unwrap().to_owned();
    let vm_cwd = vm_path.to_str().unwrap().to_owned();
    let host_id = "019a2a2a-2a2a-72a2-82a2-2a2a2a2a2a2a";
    let vm_id = "019a3b3b-3b3b-73b3-83b3-3b3b3b3b3b3b";
    let host = Client::spawn(peer(&host_cwd, host_id)).await.unwrap();
    let mut hostile = peer(&vm_cwd, vm_id);
    hostile.env("MC_TEST_FORGE_COMPLETE", host_id);
    let vm = Client::spawn(hostile).await.unwrap();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let backend = Backend::new_with_host(Some(vm.clone()), Some(host.clone()),
        vm_cwd.clone(), host_cwd.clone(), origin.into(), root.0.join("recovery"), store, None);
    let router = api::router(backend);
    for kind in ["host", "vm"] {
        let (status, _) = post(router.clone(), origin, "/api/threads",
            json!({ "environment": { "kind": kind } })).await;
        assert_eq!(status, 201);
    }
    // The Mac chat's turn is left running; then the VM worker starts and forges its end.
    for id in [host_id, vm_id] {
        let (status, _) = post(router.clone(), origin, &format!("/api/threads/{id}/messages"),
            json!({ "text": "Work", "clientMessageId": Uuid::new_v4() })).await;
        assert_eq!(status, 202);
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
    let (_, snapshot) = get(router.clone(), origin, &format!("/api/threads/{host_id}")).await;
    assert_eq!(snapshot["activeTurn"]["id"], format!("{host_id}-turn"),
        "another machine's worker ended this turn: {snapshot}");
    host.stop().await;
    vm.stop().await;
}

/// Mac work remains usable when the Master worker has not started.
#[tokio::test]
async fn host_works_with_master_absent() {
    let root = Directory::new();
    let host_path = root.0.join("host");
    let vm_path = root.0.join("vm");
    fs::create_dir(&host_path).unwrap();
    fs::create_dir(&vm_path).unwrap();
    let host_cwd = host_path.to_str().unwrap().to_owned();
    let vm_cwd = vm_path.to_str().unwrap().to_owned();
    let host_id = "019a4444-4444-7444-8444-444444444444";
    let host = Client::spawn(peer(&host_cwd, host_id)).await.unwrap();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let offline_id = Uuid::new_v4().to_string();
    store.assign_existing(offline_id.clone(), Assignment::Vm { vm_id: "master".into() })
        .await.unwrap();
    let backend = Backend::new_with_host(None, Some(host.clone()), vm_cwd, host_cwd,
        origin.into(), root.0.join("recovery"), store, None);
    let router = api::router(backend.clone());
    let (machine_status, machine) = get(router.clone(), origin,
        "/api/vms/master/status").await;
    assert_eq!(machine_status, 200, "{machine}");
    assert_eq!(machine["state"], "off");
    let (list_status, list) = get(router.clone(), origin, "/api/threads").await;
    assert_eq!(list_status, 200, "{list}");
    assert!(list["threads"].as_array().unwrap().iter().any(|task|
        task["id"] == offline_id && task["vmOffline"] == true), "{list}");
    let (snapshot_status, snapshot) = get(router.clone(), origin,
        &format!("/api/threads/{offline_id}")).await;
    assert_eq!(snapshot_status, 200, "{snapshot}");
    assert_eq!(snapshot["vmOffline"], true);
    assert_eq!(snapshot["thread"]["id"], offline_id);
    let message_id = Uuid::new_v4();
    let (wrong_status, wrong) = post(router.clone(), origin,
        "/api/vms/master/prepare", json!({ "environment": { "kind": "host" },
            "clientMessageId": message_id })).await;
    assert_eq!(wrong_status, 400, "{wrong}");
    let (cancel_status, cancel_ready) = post(router.clone(), origin,
        "/api/vms/master/prepare", json!({ "environment": { "kind": "vm" },
            "clientMessageId": Uuid::new_v4() })).await;
    assert_eq!(cancel_status, 200, "{cancel_ready}");
    let (decline_status, _) = post(router.clone(), origin,
        "/api/vms/master/decline", json!({ "approvalId": cancel_ready["approvalId"] })).await;
    assert_eq!(decline_status, 200);
    let (denied_status, _) = post(router.clone(), origin,
        "/api/vms/master/start", json!({ "approvalId": cancel_ready["approvalId"],
            "clientMessageId": Uuid::new_v4() })).await;
    assert_eq!(denied_status, 409);
    let (prepare_status, prepared) = post(router.clone(), origin,
        "/api/vms/master/prepare", json!({ "environment": { "kind": "vm" },
            "clientMessageId": message_id })).await;
    assert_eq!(prepare_status, 200, "{prepared}");
    let approval = prepared["approvalId"].as_str().unwrap();
    let (stale_status, stale) = post(router.clone(), origin,
        "/api/vms/master/start", json!({ "approvalId": approval,
            "clientMessageId": Uuid::new_v4() })).await;
    assert_eq!(stale_status, 409, "{stale}");
    let (start_status, started) = post(router.clone(), origin,
        "/api/vms/master/start", json!({ "approvalId": approval,
            "clientMessageId": message_id })).await;
    assert_eq!(start_status, 202, "{started}");
    assert_eq!(started["state"], "starting");
    let (duplicate_status, duplicate) = post(router.clone(), origin,
        "/api/vms/master/start", json!({ "approvalId": approval,
            "clientMessageId": message_id })).await;
    assert_eq!(duplicate_status, 409, "{duplicate}");
    let (vm_status, vm) = post(router.clone(), origin, "/api/threads",
        json!({ "environment": { "kind": "vm" } })).await;
    assert_eq!(vm_status, 503, "{vm}");
    let (host_status, created) = post(router.clone(), origin, "/api/threads",
        json!({ "environment": { "kind": "host" } })).await;
    assert_eq!(host_status, 201, "{created}");
    assert_eq!(created["thread"]["id"], host_id);
    let (send_status, sent) = post(router.clone(), origin,
        &format!("/api/threads/{host_id}/messages"),
        json!({ "text": "Mac only", "clientMessageId": Uuid::new_v4() })).await;
    assert_eq!(send_status, 202, "{sent}");
    let (list_status, list) = get(router.clone(), origin, "/api/threads").await;
    assert_eq!(list_status, 200, "{list}");
    assert!(list["threads"].as_array().unwrap().iter()
        .any(|thread| thread["id"] == host_id));
    let request = Request::builder()
        .uri(format!("/api/threads/{host_id}/events"))
        .header("host", "127.0.0.1:8787")
        .header("origin", origin)
        .body(Body::empty()).unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);
    let mut frames = response.into_body().into_data_stream();
    let first = tokio::time::timeout(Duration::from_secs(1), frames.next()).await.unwrap();
    let first = first.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&first).contains("event: snapshot"));
    host.stop().await;
}

/// A failed Master admission names the bounded cause and never starts Codex work.
#[tokio::test]
async fn master_start_failure_is_visible() {
    let root = Directory::new();
    let host_path = root.0.join("host");
    let vm_path = root.0.join("vm");
    fs::create_dir(&host_path).unwrap();
    fs::create_dir(&vm_path).unwrap();
    let mut command = peer(host_path.to_str().unwrap(),
        "019a7777-7777-7777-8777-777777777777");
    command.env("MC_TEST_VM_START_FAIL", "1");
    let host = Client::spawn(command).await.unwrap();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let backend = Backend::new_with_host(None, Some(host.clone()),
        vm_path.to_str().unwrap().into(), host_path.to_str().unwrap().into(),
        origin.into(), root.0.join("recovery"), store, None);
    let router = api::router(backend);
    let message_id = Uuid::new_v4();
    let (prepare_status, prepared) = post(router.clone(), origin,
        "/api/vms/master/prepare", json!({ "environment": { "kind": "vm" },
            "clientMessageId": message_id })).await;
    assert_eq!(prepare_status, 200, "{prepared}");
    let (status, reply) = post(router, origin,
        "/api/vms/master/start", json!({ "approvalId": prepared["approvalId"],
            "clientMessageId": message_id })).await;
    assert_eq!(status, 503, "{reply}");
    assert_eq!(reply["error"]["message"], "VM resource limit exceeded.");
    host.stop().await;
}

/// A dedicated VM binding never runs through Master or the Mac worker.
#[tokio::test]
async fn dedicated_vm_routes_only_its_id() {
    let root = Directory::new();
    let host_path = root.0.join("host");
    let master_path = root.0.join("master");
    for path in [&host_path, &master_path] {
        fs::create_dir(path).unwrap();
    }
    let dedicated_vm = Uuid::new_v4().to_string();
    let thread = "019a8888-8888-7888-8888-888888888888";
    let mut host_command = peer(host_path.to_str().unwrap(),
        "019a9999-9999-7999-8999-999999999999");
    host_command.env("MC_TEST_DEDICATED_OFF", "1");
    let host = Client::spawn(host_command).await.unwrap();
    let master = Client::spawn(peer(master_path.to_str().unwrap(),
        "019aaaaa-aaaa-7aaa-8aaa-aaaaaaaaaaaa")).await.unwrap();
    // Each macOS guest presents the same workspace path inside its own filesystem.
    let dedicated = Client::spawn(peer(master_path.to_str().unwrap(), thread))
        .await.unwrap();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    store.assign_existing(thread.into(), Assignment::Vm { vm_id: dedicated_vm.clone() })
        .await.unwrap();
    let backend = Backend::new_with_host(Some(master.clone()), Some(host.clone()),
        master_path.to_str().unwrap().into(), host_path.to_str().unwrap().into(),
        origin.into(), root.0.join("recovery"), store, None);
    backend.attach_dedicated(dedicated_vm.clone(), dedicated.clone()).await.unwrap();
    let router = api::router(backend.clone());
    let (list_status, list) = get(router.clone(), origin, "/api/threads").await;
    assert_eq!(list_status, 200, "{list}");
    assert!(list["threads"].as_array().unwrap().iter().any(|task|
        task["id"] == thread && task["vmId"] == dedicated_vm), "{list}");
    let (status, snapshot) = get(router.clone(), origin,
        &format!("/api/threads/{thread}")).await;
    assert_eq!(status, 200, "{snapshot}");
    assert_eq!(snapshot["thread"]["vmId"], dedicated_vm);
    let (status, sent) = post(router.clone(), origin, &format!("/api/threads/{thread}/messages"),
        json!({ "text": "Only this VM", "clientMessageId": Uuid::new_v4() })).await;
    assert_eq!(status, 202, "{sent}");
    assert_eq!(sent["turnId"], format!("{thread}-turn"));
    dedicated.stop().await;
    let (offline_status, offline) = get(router.clone(), origin, "/api/threads").await;
    assert_eq!(offline_status, 200, "{offline}");
    assert!(offline["threads"].as_array().unwrap().iter().any(|task|
        task["id"] == thread && task["vmOffline"] == true), "{offline}");
    let (view_status, view) = get(router, origin,
        &format!("/api/threads/{thread}")).await;
    assert_eq!(view_status, 200, "{view}");
    assert_eq!(view["vmOffline"], true);
    let (machine_status, machine) = get(api::router(backend.clone()), origin,
        &format!("/api/vms/{dedicated_vm}/status")).await;
    assert_eq!(machine_status, 200, "{machine}");
    assert_eq!(machine["state"], "off");
    let message_id = Uuid::new_v4();
    let (prepare_status, prepared) = post(api::router(backend.clone()), origin,
        &format!("/api/vms/{dedicated_vm}/prepare"),
        json!({ "threadId": thread, "clientMessageId": message_id })).await;
    assert_eq!(prepare_status, 200, "{prepared}");
    let (start_status, started) = post(api::router(backend.clone()), origin,
        &format!("/api/vms/{dedicated_vm}/start"),
        json!({ "approvalId": prepared["approvalId"], "clientMessageId": message_id })).await;
    assert_eq!(start_status, 202, "{started}");
    assert_eq!(fs::read_to_string(host_path.join("vm-starts")).unwrap(),
        format!("{dedicated_vm}\n"));
    host.stop().await;
    master.stop().await;
}

/// Deleting a VM task refuses an absent VM even when the Mac worker is healthy.
#[tokio::test]
async fn vm_delete_never_uses_mac() {
    let root = Directory::new();
    let host_path = root.0.join("host");
    let vm_path = root.0.join("vm");
    fs::create_dir(&host_path).unwrap();
    fs::create_dir(&vm_path).unwrap();
    let id = "019a1919-1919-7191-8191-191919191919";
    let host = Client::spawn(peer(host_path.to_str().unwrap(), id)).await.unwrap();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    store.assign_existing(id.into(), Assignment::Vm { vm_id: "master".into() })
        .await.unwrap();
    let backend = Backend::new_with_host(None, Some(host.clone()),
        vm_path.to_str().unwrap().into(), host_path.to_str().unwrap().into(),
        origin.into(), root.0.join("recovery"), store, None);
    let (status, body) = post(api::router(backend), origin,
        &format!("/api/threads/{id}/delete"), json!({})).await;
    assert_eq!(status, 503, "{body}");
    assert_eq!(body["error"]["code"], "vm_unavailable");
    host.stop().await;
}

/// A new task in a dedicated project uses its saved VM, while a Mac override stays on Mac.
#[tokio::test]
async fn dedicated_project_pins_new_task() {
    let root = Directory::new();
    let host_path = root.0.join("host");
    let master_path = root.0.join("master");
    fs::create_dir(&host_path).unwrap();
    fs::create_dir(&master_path).unwrap();
    let vm_id = Uuid::new_v4().to_string();
    let host_id = "019a1212-1212-7121-8121-121212121212";
    let dedicated_id = "019a1313-1313-7131-8131-131313131313";
    let mut host_command = peer(host_path.to_str().unwrap(), host_id);
    host_command.env("MC_TEST_DEDICATED_ID", &vm_id);
    let host = Client::spawn(host_command).await.unwrap();
    let master = Client::spawn(peer(master_path.to_str().unwrap(),
        "019a1414-1414-7141-8141-141414141414")).await.unwrap();
    let dedicated = Client::spawn(peer(master_path.to_str().unwrap(), dedicated_id))
        .await.unwrap();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let backend = Backend::new_with_host(Some(master.clone()), Some(host.clone()),
        master_path.to_str().unwrap().into(), host_path.to_str().unwrap().into(),
        origin.into(), root.0.join("recovery"), store.clone(), None);
    backend.attach_dedicated(vm_id.clone(), dedicated.clone()).await.unwrap();
    let router = api::router(backend);
    let (project_status, project) = post(router.clone(), origin, "/api/projects",
        json!({ "name": "Separate workspace", "environment": {
            "kind": "vm", "dedicated": true, "vmId": vm_id } })).await;
    assert_eq!(project_status, 201, "{project}");
    let project_id = project["project"]["id"].as_str().unwrap();
    let (status, machine) = get(router.clone(), origin,
        &format!("/api/vms/{vm_id}/status")).await;
    assert_eq!(status, 200, "{machine}");
    assert_eq!(machine["ready"], true);
    fs::write(host_path.join("vm-off"), "").unwrap();
    let message_id = Uuid::new_v4();
    let (wrong_status, wrong) = post(router.clone(), origin,
        "/api/vms/master/prepare", json!({ "environment": { "kind": "vm" },
            "projectId": project_id, "clientMessageId": message_id })).await;
    assert_eq!(wrong_status, 400, "{wrong}");
    let (prepare_status, prepared) = post(router.clone(), origin,
        &format!("/api/vms/{vm_id}/prepare"),
        json!({ "environment": { "kind": "vm" }, "projectId": project_id,
            "clientMessageId": message_id })).await;
    assert_eq!(prepare_status, 200, "{prepared}");
    let (start_status, started) = post(router.clone(), origin,
        &format!("/api/vms/{vm_id}/start"),
        json!({ "approvalId": prepared["approvalId"],
            "clientMessageId": message_id })).await;
    assert_eq!(start_status, 202, "{started}");
    assert_eq!(fs::read_to_string(host_path.join("vm-starts")).unwrap(),
        format!("{vm_id}\n"));
    fs::remove_file(host_path.join("vm-off")).unwrap();
    let (vm_status, vm_task) = post(router.clone(), origin, "/api/threads",
        json!({ "projectId": project_id, "environment": { "kind": "vm" } })).await;
    assert_eq!(vm_status, 201, "{vm_task}");
    assert_eq!(vm_task["thread"]["id"], dedicated_id);
    assert_eq!(vm_task["thread"]["vmId"], vm_id);
    let (mac_status, mac_task) = post(router.clone(), origin, "/api/threads",
        json!({ "projectId": project_id, "environment": { "kind": "host" } })).await;
    assert_eq!(mac_status, 201, "{mac_task}");
    assert_eq!(mac_task["thread"]["id"], host_id);
    assert_eq!(store.assignment(dedicated_id.into()).await.unwrap(),
        Some(Assignment::Vm { vm_id }));
    dedicated.stop().await;
    let (edit_status, edited) = post(router.clone(), origin,
        &format!("/api/projects/{project_id}"),
        json!({ "name": "Renamed while off", "revision": 1,
            "environment": { "kind": "vm", "dedicated": true,
                "vmId": project["project"]["environment"]["vmId"] } })).await;
    assert_eq!(edit_status, 200, "{edited}");
    master.stop().await;
    host.stop().await;
}

/// Builds a backend with one ready project VM and a project that owns it.
async fn project_with_vm(root: &Directory)
    -> (axum::Router, Store, String, Value, Vec<Client>, std::sync::Arc<Backend>) {
    let host_path = root.0.join("host");
    let master_path = root.0.join("master");
    fs::create_dir(&host_path).unwrap();
    fs::create_dir(&master_path).unwrap();
    let vm_id = Uuid::new_v4().to_string();
    let mut host_command = peer(host_path.to_str().unwrap(), "019a1212-1212-7121-8121-121212121212");
    host_command.env("MC_TEST_DEDICATED_ID", &vm_id);
    let host = Client::spawn(host_command).await.unwrap();
    let master = Client::spawn(peer(master_path.to_str().unwrap(),
        "019a1414-1414-7141-8141-141414141414")).await.unwrap();
    let dedicated = Client::spawn(peer(master_path.to_str().unwrap(),
        "019a1313-1313-7131-8131-131313131313")).await.unwrap();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let backend = Backend::new_with_host(Some(master.clone()), Some(host.clone()),
        master_path.to_str().unwrap().into(), host_path.to_str().unwrap().into(),
        origin.into(), root.0.join("recovery"), store.clone(), None);
    backend.attach_dedicated(vm_id.clone(), dedicated.clone()).await.unwrap();
    let router = api::router(backend.clone());
    let (status, project) = post(router.clone(), origin, "/api/projects",
        json!({ "name": "Separate workspace", "environment": {
            "kind": "vm", "dedicated": true, "vmId": vm_id } })).await;
    assert_eq!(status, 201, "{project}");
    (router, store, vm_id, project["project"].clone(), vec![host, master, dedicated], backend)
}

/// Deleting a project deletes its conversations and keeps its VM as an orphan, which Settings
/// can then control and delete.
#[tokio::test]
async fn deleted_project_leaves_an_orphaned_vm() {
    let root = Directory::new();
    let origin = "http://127.0.0.1:8787";
    let (router, _store, vm_id, project, clients, _backend) = project_with_vm(&root).await;
    let id = project["id"].as_str().unwrap();
    let (status, view) = get(router.clone(), origin, "/api/virtualisation").await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["projectVms"], json!([{ "vmId": vm_id, "state": "running",
        "projectId": id, "projectName": "Separate workspace" }]));
    // A VM that a project owns is deleted through its project, never on its own.
    let (owned, _) = post(router.clone(), origin, &format!("/api/vms/{vm_id}/delete"), json!({})).await;
    assert_eq!(owned, 409);
    let (task_status, task) = post(router.clone(), origin, "/api/threads",
        json!({ "projectId": id, "environment": { "kind": "vm" } })).await;
    assert_eq!(task_status, 201, "{task}");

    let (stale, _) = post(router.clone(), origin, &format!("/api/projects/{id}/delete"),
        json!({ "revision": 7 })).await;
    assert_eq!(stale, 409);
    let (status, body) = post(router.clone(), origin, &format!("/api/projects/{id}/delete"),
        json!({ "revision": 1 })).await;
    assert_eq!(status, 200, "{body}");
    let (_, list) = get(router.clone(), origin, "/api/projects").await;
    assert_eq!(list["projects"], json!([]));
    // The project's conversations are deleted with it, at their provider as well.
    let (_, tasks) = get(router.clone(), origin, "/api/threads").await;
    assert!(tasks["threads"].as_array().unwrap_or_else(|| panic!("{tasks}")).iter()
        .all(|entry| entry["id"] != task["thread"]["id"]), "{tasks}");
    assert!(fs::read_to_string(root.0.join("master/deletes")).unwrap()
        .contains(task["thread"]["id"].as_str().unwrap()));

    let (_, view) = get(router.clone(), origin, "/api/virtualisation").await;
    assert_eq!(view["projectVms"], json!([{ "vmId": vm_id, "state": "running",
        "projectId": null, "projectName": null }]));
    // Power controls work without a conversation asking for the VM.
    for (action, state) in [("show", "running"), ("stop", "stopping"), ("start", "starting")] {
        let (status, reply) = post(router.clone(), origin, &format!("/api/vms/{vm_id}/power"),
            json!({ "action": action })).await;
        assert_eq!((status, reply["state"].as_str()), (200, Some(state)), "{action}: {reply}");
    }
    let (bad, _) = post(router.clone(), origin, &format!("/api/vms/{vm_id}/power"),
        json!({ "action": "delete" })).await;
    assert_eq!(bad, 400);
    // A running VM keeps its disk; a stopped orphan can be deleted, Master never.
    let (running, reply) = post(router.clone(), origin, &format!("/api/vms/{vm_id}/delete"), json!({})).await;
    assert_eq!(running, 409, "{reply}");
    assert_eq!(reply["error"]["code"], "vm_running");
    fs::write(root.0.join("host/vm-off"), "").unwrap();
    let (master, _) = post(router.clone(), origin, "/api/vms/master/delete", json!({})).await;
    assert_eq!(master, 400);
    let (status, reply) = post(router.clone(), origin, &format!("/api/vms/{vm_id}/delete"), json!({})).await;
    assert_eq!(status, 200, "{reply}");
    let actions = fs::read_to_string(root.0.join("host/vm-actions")).unwrap();
    assert_eq!(actions, format!("show {vm_id}\nstop {vm_id}\ndelete {vm_id}\n"));
    let (_, view) = get(router.clone(), origin, "/api/virtualisation").await;
    assert_eq!(view["projectVms"], json!([]));
    for client in clients { client.stop().await; }
}

/// A VM that fails to start, or stops by itself, is reported once; a requested stop is not.
#[tokio::test]
async fn vm_failures_reach_notifications_once() {
    use miniclaw_server::idle::{self, failure};
    // The rule: only a change into `failed` counts, and what came before names the kind.
    assert_eq!(failure(Some("starting"), "failed"), Some("vm_start_failed"));
    assert_eq!(failure(Some("running"), "failed"), Some("vm_stopped_unexpectedly"));
    assert_eq!(failure(Some("failed"), "failed"), None, "reported once");
    assert_eq!(failure(None, "failed"), None, "a failure from before this run is old news");
    assert_eq!(failure(Some("running"), "off"), None, "a clean stop is not a failure");
    assert_eq!(failure(Some("stopping"), "failed"), None, "a stop the user asked for is not reported");

    let root = Directory::new();
    let origin = "http://127.0.0.1:8787";
    let (router, _store, _vm_id, _project, clients, backend) = project_with_vm(&root).await;
    let kinds = || async { get(router.clone(), origin, "/api/notifications").await.1["notifications"]
        .as_array().unwrap().iter().map(|notice| notice["kind"].as_str().unwrap().to_owned()).collect::<Vec<_>>() };
    idle::watch_failures(&backend).await;
    assert!(kinds().await.is_empty(), "a running VM is no news");
    fs::write(root.0.join("host/vm-failed"), "").unwrap();
    idle::watch_failures(&backend).await;
    idle::watch_failures(&backend).await;
    assert_eq!(kinds().await, vec!["vm_stopped_unexpectedly"], "once, however often it is seen");
    for client in clients { client.stop().await; }
}

/// A transfer starts only from what the user chose in the macOS panel, and only once.
#[tokio::test]
async fn transfer_routes_use_the_panel_selection_once() {
    use miniclaw_server::transfer_jobs::Picker;
    let root = Directory::new();
    let origin = "http://127.0.0.1:8787";
    let (router, _store, vm_id, _project, clients, backend) = project_with_vm(&root).await;
    let chosen = root.0.join("report.txt");
    fs::write(&chosen, "quarterly").unwrap();
    let _ = backend.picker.set(Picker::Fixed(std::sync::Mutex::new(vec![Some(vec![chosen.clone()]), None])));
    let base = format!("/api/vms/{vm_id}/transfers");
    let (bad, _) = post(router.clone(), origin, &format!("{base}/pick"), json!({ "kind": "anything" })).await;
    assert_eq!(bad, 400);
    let (status, picked) = post(router.clone(), origin, &format!("{base}/pick"), json!({ "kind": "files" })).await;
    assert_eq!(status, 200, "{picked}");
    assert_eq!(picked["items"], json!([{ "name": "report.txt", "kind": "file", "size": 9 }]));
    assert!(!picked.to_string().contains(root.0.to_str().unwrap()), "the page never sees a Mac path");
    let (_, cancelled) = post(router.clone(), origin, &format!("{base}/pick"), json!({ "kind": "files" })).await;
    assert_eq!(cancelled, json!({ "cancelled": true }));

    // A request that names a Mac path itself is malformed, and an invented selection is refused.
    let (forged, _) = post(router.clone(), origin, &base,
        json!({ "direction": "to_vm", "selectionId": Uuid::new_v4(), "paths": ["/etc/passwd"] })).await;
    assert_eq!(forged, 400);
    let (unknown, reply) = post(router.clone(), origin, &base,
        json!({ "direction": "to_vm", "selectionId": Uuid::new_v4() })).await;
    assert_eq!((unknown, reply["error"]["code"].as_str()), (409, Some("selection_expired")));
    // A files selection cannot serve as a destination.
    let (wrong, _) = post(router.clone(), origin, &base, json!({ "direction": "to_mac",
        "selectionId": picked["selectionId"], "sources": [{ "folder": "workspace", "path": "a" }] })).await;
    assert_eq!(wrong, 409);

    let (status, started) = post(router.clone(), origin, &base,
        json!({ "direction": "to_vm", "selectionId": picked["selectionId"] })).await;
    assert_eq!(status, 202, "{started}");
    let job = started["jobId"].as_str().unwrap();
    let mut view = json!(null);
    for _ in 0..200 {
        view = get(router.clone(), origin, &format!("/api/transfers/{job}")).await.1;
        if view["state"] != "running" { break; }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!((view["state"].as_str(), view["doneBytes"].as_u64()), (Some("done"), Some(9)), "{view}");
    let (again, _) = post(router.clone(), origin, &base,
        json!({ "direction": "to_vm", "selectionId": picked["selectionId"] })).await;
    assert_eq!(again, 409, "a selection serves one transfer");
    let (none, _) = get(router.clone(), origin, &format!("/api/transfers/{}", Uuid::new_v4())).await;
    assert_eq!(none, 404);
    for client in clients { client.stop().await; }
}

/// Backups are off for every VM until the user changes that, and only offered values are saved.
#[tokio::test]
async fn backup_routes_start_switched_off() {
    use miniclaw_server::backups::Backups;
    let root = Directory::new();
    let origin = "http://127.0.0.1:8787";
    let (router, _store, vm_id, _project, clients, backend) = project_with_vm(&root).await;
    let _ = backend.backups.set(Backups::new(root.0.join("vm-backups.json"), root.0.join("Backups"), root.0.join("Trash")));
    for id in ["master", vm_id.as_str()] {
        let (status, view) = get(router.clone(), origin, &format!("/api/vms/{id}/backups")).await;
        assert_eq!(status, 200, "{view}");
        assert_eq!((view["experimental"].as_bool(), view["settings"]["schedule"].as_str(), view["backups"].as_array().map(Vec::len)),
            (Some(true), Some("off"), Some(0)), "{view}");
    }
    assert!(!root.0.join("Backups").exists() && !root.0.join("vm-backups.json").exists());
    let path = format!("/api/vms/{vm_id}/backups/settings");
    for bad in [json!({ "schedule": "hourly", "keep": 3 }), json!({ "schedule": "daily", "keep": 0 }),
        json!({ "schedule": "daily", "keep": 11 }), json!({ "schedule": "daily", "keep": 3, "folders": ["/etc"] })] {
        assert_eq!(post(router.clone(), origin, &path, bad.clone()).await.0, 400, "{bad}");
    }
    let (status, saved) = post(router.clone(), origin, &path, json!({ "schedule": "weekly", "keep": 5 })).await;
    assert_eq!((status, saved["settings"]["schedule"].as_str(), saved["settings"]["keep"].as_u64()),
        (200, Some("weekly"), Some(5)), "{saved}");
    let (_, master) = get(router.clone(), origin, "/api/vms/master/backups").await;
    assert_eq!(master["settings"]["schedule"], "off", "switching one VM on leaves the others off");
    let (missing, reply) = post(router.clone(), origin, &format!("/api/vms/{vm_id}/backups/nope/restore"), json!({})).await;
    assert_eq!((missing, reply["error"]["code"].as_str()), (409, Some("backup_unusable")));
    for client in clients { client.stop().await; }
}

/// An idle VM is warned about first and shut down only when nobody answers.
#[tokio::test]
async fn idle_vm_is_warned_then_shut_down_unless_kept() {
    use miniclaw_server::idle::{self, Step};
    use std::time::Duration;
    let (limit, grace) = (Duration::from_secs(900), Duration::from_secs(60));
    // The rule itself: busy or recently used VMs wait, an unanswered warning ends in a stop.
    assert_eq!(idle::step(None, Duration::from_secs(9999), false, None, grace), Step::Wait);
    assert_eq!(idle::step(Some(limit), Duration::from_secs(899), false, None, grace), Step::Wait);
    assert_eq!(idle::step(Some(limit), Duration::from_secs(900), true, None, grace), Step::Wait);
    assert_eq!(idle::step(Some(limit), Duration::from_secs(900), false, None, grace), Step::Warn);
    assert_eq!(idle::step(Some(limit), Duration::from_secs(930), false,
        Some(Duration::from_secs(30)), grace), Step::Wait);
    assert_eq!(idle::step(Some(limit), Duration::from_secs(960), false,
        Some(Duration::from_secs(60)), grace), Step::Stop);

    let root = Directory::new();
    let origin = "http://127.0.0.1:8787";
    let (router, _store, vm_id, _project, clients, backend) = project_with_vm(&root).await;
    let path = format!("/api/vms/{vm_id}/idle");
    let (status, saved) = get(router.clone(), origin, &path).await;
    assert_eq!((status, saved["minutes"].as_u64()), (200, Some(0)), "off by default: {saved}");
    let (bad, _) = post(router.clone(), origin, &path, json!({ "minutes": 20 })).await;
    assert_eq!(bad, 400, "only the offered times are accepted");
    let (status, saved) = post(router.clone(), origin, &path, json!({ "minutes": 15 })).await;
    assert_eq!((status, saved["minutes"].as_u64()), (200, Some(15)), "{saved}");
    let (_, master) = get(router.clone(), origin, "/api/vms/master/idle").await;
    assert_eq!(master["minutes"], 0, "each VM has its own setting");
    let (_, none) = get(router.clone(), origin, "/api/vms/idle").await;
    assert_eq!(none["warnings"], json!([]));

    // One pass of the watcher with a clock 15 minutes ahead warns; "Keep running" clears it.
    idle::tick(&backend, Duration::from_secs(900)).await;
    let (_, warned) = get(router.clone(), origin, "/api/vms/idle").await;
    assert_eq!(warned["warnings"][0]["vmId"], vm_id, "{warned}");
    assert_eq!(warned["warnings"][0]["name"], "Separate workspace");
    assert!(warned["warnings"][0]["secondsLeft"].as_u64().is_some_and(|left| left <= 60));
    let (kept, _) = post(router.clone(), origin, &format!("/api/vms/{vm_id}/idle/keep"), json!({})).await;
    assert_eq!(kept, 200);
    let (_, cleared) = get(router.clone(), origin, "/api/vms/idle").await;
    assert_eq!(cleared["warnings"], json!([]));
    assert!(!root.0.join("host/vm-actions").exists(), "a kept VM is not stopped");

    // Warned again and left alone, the VM is stopped once and a notice records it.
    idle::tick(&backend, Duration::from_secs(900)).await;
    idle::tick(&backend, Duration::from_secs(961)).await;
    assert_eq!(fs::read_to_string(root.0.join("host/vm-actions")).unwrap(), format!("stop {vm_id}\n"));
    let (_, after) = get(router.clone(), origin, "/api/vms/idle").await;
    assert_eq!(after["warnings"], json!([]));
    let (_, notices) = get(router.clone(), origin, "/api/notifications").await;
    assert!(notices["notifications"].as_array().unwrap().iter()
        .any(|notice| notice["kind"] == "vm_idle_stopped"), "{notices}");
    for client in clients { client.stop().await; }
}

/// Deleting a project together with its VM needs the typed name and a stopped VM.
#[tokio::test]
async fn project_delete_can_take_its_vm_along() {
    let root = Directory::new();
    let origin = "http://127.0.0.1:8787";
    let (router, _store, vm_id, project, clients, _backend) = project_with_vm(&root).await;
    let path = format!("/api/projects/{}/delete", project["id"].as_str().unwrap());
    let (task_status, task) = post(router.clone(), origin, "/api/threads",
        json!({ "projectId": project["id"], "environment": { "kind": "vm" } })).await;
    assert_eq!(task_status, 201, "{task}");
    let (unnamed, reply) = post(router.clone(), origin, &path,
        json!({ "revision": 1, "deleteVm": true, "confirmName": "separate" })).await;
    assert_eq!(unnamed, 400, "{reply}");
    assert_eq!(reply["error"]["code"], "confirmation_required");
    let (running, reply) = post(router.clone(), origin, &path,
        json!({ "revision": 1, "deleteVm": true, "confirmName": "Separate workspace" })).await;
    assert_eq!(running, 409, "{reply}");
    assert_eq!(reply["error"]["code"], "vm_running");
    assert!(!root.0.join("host/vm-actions").exists(), "nothing was deleted");
    let (_, list) = get(router.clone(), origin, "/api/projects").await;
    assert_eq!(list["projects"].as_array().unwrap().len(), 1, "the project is kept");

    fs::write(root.0.join("host/vm-off"), "").unwrap();
    clients[2].stop().await;
    // With the VM off and kept, its conversations cannot be reached, so nothing is deleted.
    let (kept, reply) = post(router.clone(), origin, &path, json!({ "revision": 1 })).await;
    assert_eq!((kept, reply["error"]["code"].as_str()), (409, Some("conversations_unreachable")), "{reply}");
    let (status, reply) = post(router.clone(), origin, &path,
        json!({ "revision": 1, "deleteVm": true, "confirmName": "Separate workspace" })).await;
    assert_eq!(status, 200, "{reply}");
    // The conversation lived on the deleted disk; MiniClaw drops its own record of it.
    let (_, tasks) = get(router.clone(), origin, "/api/threads").await;
    assert!(tasks["threads"].as_array().unwrap_or_else(|| panic!("{tasks}")).iter()
        .all(|entry| entry["id"] != task["thread"]["id"]), "{tasks}");
    assert_eq!(fs::read_to_string(root.0.join("host/vm-actions")).unwrap(), format!("delete {vm_id}\n"));
    let (_, list) = get(router.clone(), origin, "/api/projects").await;
    assert_eq!(list["projects"], json!([]));
    let (_, view) = get(router.clone(), origin, "/api/virtualisation").await;
    assert_eq!(view["projectVms"], json!([]));
    for client in clients { client.stop().await; }
}

/// A Mac project has no VM to delete, and Master is never deleted with a project.
#[tokio::test]
async fn project_delete_never_touches_master() {
    let root = Directory::new();
    let origin = "http://127.0.0.1:8787";
    let (router, _store, _vm, _project, clients, _backend) = project_with_vm(&root).await;
    let (_, shared) = post(router.clone(), origin, "/api/projects",
        json!({ "name": "On Master", "environment": { "kind": "vm", "dedicated": false } })).await;
    let path = format!("/api/projects/{}/delete", shared["project"]["id"].as_str().unwrap());
    let (status, reply) = post(router.clone(), origin, &path,
        json!({ "revision": 1, "deleteVm": true, "confirmName": "On Master" })).await;
    assert_eq!(status, 400, "{reply}");
    let (status, _) = post(router.clone(), origin, &path, json!({ "revision": 1 })).await;
    assert_eq!(status, 200);
    assert!(!root.0.join("host/vm-actions").exists());
    for client in clients { client.stop().await; }
}

/// New project setup shows an unsaved VM's progress without a task or Master route.
#[tokio::test]
async fn dedicated_setup_is_visible_before_project_save() {
    let root = Directory::new();
    let host_path = root.0.join("host");
    let vm_path = root.0.join("vm");
    fs::create_dir(&host_path).unwrap();
    fs::create_dir(&vm_path).unwrap();
    let vm_id = Uuid::new_v4().to_string();
    let mut host_command = peer(host_path.to_str().unwrap(),
        "019a1515-1515-7151-8151-151515151515");
    host_command.env("MC_TEST_DEDICATED_ID", &vm_id)
        .env("MC_TEST_DEDICATED_SETUP", "1");
    let host = Client::spawn(host_command).await.unwrap();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let backend = Backend::new_with_host(None, Some(host.clone()),
        vm_path.to_str().unwrap().into(), host_path.to_str().unwrap().into(),
        origin.into(), root.0.join("recovery"), store, None);
    let router = api::router(backend);
    let request_id = Uuid::new_v4();
    let (created_status, created) = post(router.clone(), origin, "/api/vms/projects",
        json!({ "requestId": request_id })).await;
    assert_eq!(created_status, 202, "{created}");
    assert_eq!(created["vmId"], vm_id);
    assert!(!created.to_string().contains(&"a".repeat(64)));
    let (status_code, status) = get(router.clone(), origin,
        &format!("/api/vms/{vm_id}/status")).await;
    assert_eq!(status_code, 200, "{status}");
    assert_eq!(status["stage"], "installing");
    assert_eq!(status["percent"], 42);
    assert_eq!(status["ready"], false);
    let (retry_status, retried) = post(router.clone(), origin,
        &format!("/api/vms/{vm_id}/retry"), json!({})).await;
    assert_eq!(retry_status, 202, "{retried}");
    let (cancel_status, cancelled) = post(router.clone(), origin,
        &format!("/api/vms/{vm_id}/cancel"), json!({})).await;
    assert_eq!(cancel_status, 200, "{cancelled}");
    assert_eq!(cancelled["state"], "failed");
    let (missing_status, _) = get(router, origin,
        &format!("/api/vms/{}/status", Uuid::new_v4())).await;
    assert_eq!(missing_status, 404);
    host.stop().await;
}

/// A provisioning refusal reaches the browser as its bounded cause, not controller loss.
#[tokio::test]
async fn dedicated_setup_refusal_shows_cause() {
    let root = Directory::new();
    let host_path = root.0.join("host");
    let vm_path = root.0.join("vm");
    fs::create_dir(&host_path).unwrap();
    fs::create_dir(&vm_path).unwrap();
    let mut host_command = peer(host_path.to_str().unwrap(),
        "019a1818-1818-7181-8181-181818181818");
    host_command.env("MC_TEST_SETUP_FULL", "1");
    let host = Client::spawn(host_command).await.unwrap();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let backend = Backend::new_with_host(None, Some(host.clone()),
        vm_path.to_str().unwrap().into(), host_path.to_str().unwrap().into(),
        origin.into(), root.0.join("recovery"), store, None);
    let (status, body) = post(api::router(backend), origin, "/api/vms/projects",
        json!({ "requestId": Uuid::new_v4() })).await;
    assert_eq!(status, 503, "{body}");
    assert_eq!(body["error"]["message"], "Project VM limit reached.");
    host.stop().await;
}

/// A running guest without guest Codex sign-in is still not a ready project VM.
#[tokio::test]
async fn dedicated_project_waits_for_guest_auth() {
    let root = Directory::new();
    let host_path = root.0.join("host");
    let vm_path = root.0.join("vm");
    fs::create_dir(&host_path).unwrap();
    fs::create_dir(&vm_path).unwrap();
    let vm_id = Uuid::new_v4().to_string();
    let mut host_command = peer(host_path.to_str().unwrap(),
        "019a1616-1616-7161-8161-161616161616");
    host_command.env("MC_TEST_DEDICATED_ID", &vm_id);
    let host = Client::spawn(host_command).await.unwrap();
    let mut guest_command = peer(vm_path.to_str().unwrap(),
        "019a1717-1717-7171-8171-171717171717");
    guest_command.env("MC_TEST_SIGNED_OUT", "1");
    let guest = Client::spawn(guest_command).await.unwrap();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let backend = Backend::new_with_host(None, Some(host.clone()),
        vm_path.to_str().unwrap().into(), host_path.to_str().unwrap().into(),
        origin.into(), root.0.join("recovery"), store, None);
    backend.attach_dedicated(vm_id.clone(), guest.clone()).await.unwrap();
    let router = api::router(backend);
    let (status_code, status) = get(router.clone(), origin,
        &format!("/api/vms/{vm_id}/status")).await;
    assert_eq!(status_code, 200, "{status}");
    assert_eq!(status["state"], "running");
    assert_eq!(status["ready"], false);
    assert_eq!(status["workerConnected"], true);
    let (auth_status, auth) = post(router.clone(), origin,
        &format!("/api/vms/{vm_id}/auth/start"),
        json!({ "method": "device" })).await;
    assert_eq!(auth_status, 200, "{auth}");
    assert_eq!(auth["status"], "pending");
    assert_eq!(auth["userCode"], "TEST-CODE");
    assert_eq!(fs::read_to_string(vm_path.join("logins")).unwrap(), "device\n");
    assert!(!host_path.join("logins").exists());
    let (save_status, saved) = post(router.clone(), origin, "/api/projects",
        json!({ "name": "Not signed in", "environment": {
            "kind": "vm", "dedicated": true, "vmId": vm_id } })).await;
    assert_eq!(save_status, 503, "{saved}");
    assert_eq!((get(router, origin, "/api/projects").await.1)["projects"], json!([]));
    guest.stop().await;
    host.stop().await;
}

/// A VM chat carries either the MiniClaw tool or Computer Use, never both: the VM's
/// worker refuses the pair, which broke switching Computer Use on.
#[tokio::test]
async fn vm_chat_swaps_the_miniclaw_tool_for_computer_use() {
    let root = Directory::new();
    let workspace = root.0.join("workspace");
    fs::create_dir(&workspace).unwrap();
    let cwd = workspace.to_str().unwrap().to_owned();
    let client = Client::spawn(peer(&cwd, "019a2828-2828-7282-8282-282828282828")).await.unwrap();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let backend = Backend::new(Some(client.clone()), cwd, origin.into(),
        root.0.join("recovery"), store, Some(miniclaw_server::computer::Target::Guest), true);
    let router = api::router(backend);
    let (status, created) = post(router.clone(), origin, "/api/threads",
        json!({ "environment": { "kind": "vm" } })).await;
    assert_eq!(status, 201, "{created}");
    let thread = created["thread"]["id"].as_str().unwrap();
    let start: Value = serde_json::from_str(fs::read_to_string(workspace.join("starts"))
        .unwrap().lines().last().unwrap()).unwrap();
    assert!(start["dynamicTools"].is_array(), "a plain VM chat starts with the MiniClaw tool");
    let (status, switched) = post(router.clone(), origin,
        &format!("/api/threads/{thread}/computer"), json!({ "enabled": true })).await;
    assert_eq!(status, 200, "{switched}");
    let resume: Value = serde_json::from_str(fs::read_to_string(workspace.join("resumes"))
        .unwrap().lines().last().unwrap()).unwrap();
    assert_eq!(resume["miniclawComputerUse"], true);
    assert!(resume.get("dynamicTools").is_none(), "{resume}");
    let (status, _) = post(router, origin,
        &format!("/api/threads/{thread}/computer"), json!({ "enabled": false })).await;
    assert_eq!(status, 200);
    let resume: Value = serde_json::from_str(fs::read_to_string(workspace.join("resumes"))
        .unwrap().lines().last().unwrap()).unwrap();
    assert!(resume["dynamicTools"].is_array(), "the tool returns when Computer Use is off");
    client.stop().await;
}

/// A stream names itself to the worker so two viewers do not split frames, and falls back
/// to the plain request for a worker installed before viewers existed.
#[tokio::test]
async fn desktop_stream_names_its_viewer_and_still_works_with_an_older_worker() {
    use futures_util::StreamExt;
    for old_worker in [false, true] {
        let root = Directory::new();
        let workspace = root.0.join("workspace");
        fs::create_dir(&workspace).unwrap();
        let cwd = workspace.to_str().unwrap().to_owned();
        let mut command = peer(&cwd, "019a2929-2929-7292-8292-292929292929");
        command.env("MC_TEST_DEDICATED_ID", "unused");
        if old_worker { command.env("MC_TEST_OLD_WORKER", "1"); }
        let client = Client::spawn(command).await.unwrap();
        let origin = "http://127.0.0.1:8787";
        let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
        let backend = Backend::new_with_host(Some(client.clone()), Some(client.clone()), cwd.clone(),
            cwd.clone(), origin.into(), root.0.join("recovery"), store, None);
        fs::write(workspace.join("master-running"), "").unwrap();
        let request = Request::builder().method("GET").uri("/api/vms/master/desktop?stream=0")
            .header("host", "127.0.0.1:8787").header("origin", origin).body(Body::empty()).unwrap();
        let response = api::router(backend).oneshot(request).await.unwrap();
        assert_eq!(response.status().as_u16(), 200, "old worker: {old_worker}");
        let mut body = response.into_body().into_data_stream();
        let part = tokio::time::timeout(Duration::from_secs(3), body.next()).await
            .unwrap().unwrap().unwrap();
        assert!(part.starts_with(b"--frame\r\nContent-Type: image/jpeg\r\nContent-Length: "));
        drop(body);
        let requests: Vec<Value> = fs::read_to_string(workspace.join("frame-requests")).unwrap()
            .lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        let viewer = requests[0]["viewer"].as_str().expect("the first request names a viewer");
        assert_eq!(Uuid::parse_str(viewer).unwrap().to_string(), viewer);
        if old_worker {
            assert_eq!(requests[1], json!({}), "an older worker is asked again without a viewer");
        }
        client.stop().await;
    }
}

/// Each VM reports whether its own Computer Use helper is installed.
#[tokio::test]
async fn project_vm_reports_its_own_computer_use_helper() {
    let root = Directory::new();
    let host_path = root.0.join("host");
    fs::create_dir(&host_path).unwrap();
    let with_helper = Uuid::new_v4().to_string();
    let without = Uuid::new_v4().to_string();
    let mut host_command = peer(host_path.to_str().unwrap(),
        "019a1616-1616-7161-8161-161616161616");
    host_command.env("MC_TEST_DEDICATED_ID", &with_helper);
    let host = Client::spawn(host_command).await.unwrap();
    let mut guests = Vec::new();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let backend = Backend::new_with_host(None, Some(host.clone()),
        host_path.to_str().unwrap().into(), host_path.to_str().unwrap().into(),
        origin.into(), root.0.join("recovery"), store,
        Some(miniclaw_server::computer::Target::Guest));
    for (vm_id, helper) in [(&with_helper, "1"), (&without, "0")] {
        let path = root.0.join(vm_id);
        fs::create_dir(&path).unwrap();
        let mut command = peer(path.to_str().unwrap(), "019a1717-1717-7171-8171-171717171717");
        command.env("MC_TEST_COMPUTER", helper);
        let guest = Client::spawn(command).await.unwrap();
        backend.attach_dedicated(vm_id.clone(), guest.clone()).await.unwrap();
        guests.push(guest);
    }
    assert!(backend.vm_computer_ready(&with_helper));
    assert!(!backend.vm_computer_ready(&without));
    let router = api::router(backend.clone());
    let (code, status) = get(router, origin, &format!("/api/vms/{with_helper}/status")).await;
    assert_eq!(code, 200, "{status}");
    assert_eq!(status["computerReady"], true);
    // A VM that has gone away no longer counts as ready.
    guests[0].stop().await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while backend.vm_computer_ready(&with_helper) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.unwrap();
    for guest in guests { guest.stop().await; }
    host.stop().await;
}

/// An older Mac task is verified on Mac before its permanent binding is saved.
#[tokio::test]
async fn legacy_host_task_binds_on_host() {
    let root = Directory::new();
    let host_path = root.0.join("host");
    let vm_path = root.0.join("vm");
    fs::create_dir(&host_path).unwrap();
    fs::create_dir(&vm_path).unwrap();
    let host_cwd = host_path.to_str().unwrap().to_owned();
    let vm_cwd = vm_path.to_str().unwrap().to_owned();
    let host_id = "019a5555-5555-7555-8555-555555555555";
    let vm_id = "019a6666-6666-7666-8666-666666666666";
    let host = Client::spawn(peer(&host_cwd, host_id)).await.unwrap();
    let vm = Client::spawn(peer(&vm_cwd, vm_id)).await.unwrap();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let backend = Backend::new_with_host(Some(vm.clone()), Some(host.clone()), vm_cwd,
        host_cwd, origin.into(), root.0.join("recovery"), store.clone(), None);
    let router = api::router(backend);
    let (status, selected) = post(router.clone(), origin,
        &format!("/api/threads/{host_id}/environment"), json!({ "kind": "host" })).await;
    assert_eq!(status, 200, "{selected}");
    assert_eq!(store.assignment(host_id.into()).await.unwrap(),
        Some(miniclaw_server::environment::Assignment::Host {}));
    let (status, snapshot) = get(router, origin, &format!("/api/threads/{host_id}")).await;
    assert_eq!(status, 200, "{snapshot}");
    host.stop().await;
    vm.stop().await;
}

/// Losing Master does not mark an active Mac response as uncertain.
#[tokio::test]
async fn master_disconnect_keeps_host_turn() {
    let root = Directory::new();
    let host_path = root.0.join("host");
    let vm_path = root.0.join("vm");
    fs::create_dir(&host_path).unwrap();
    fs::create_dir(&vm_path).unwrap();
    let host_cwd = host_path.to_str().unwrap().to_owned();
    let vm_cwd = vm_path.to_str().unwrap().to_owned();
    let host_id = "019a7777-7777-7777-8777-777777777777";
    let vm_id = "019a8888-8888-7888-8888-888888888888";
    let host = Client::spawn(peer(&host_cwd, host_id)).await.unwrap();
    let vm = Client::spawn(peer(&vm_cwd, vm_id)).await.unwrap();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let backend = Backend::new_with_host(Some(vm.clone()), Some(host.clone()), vm_cwd,
        host_cwd, origin.into(), root.0.join("recovery"), store, None);
    let router = api::router(backend);
    let (status, created) = post(router.clone(), origin, "/api/threads",
        json!({ "environment": { "kind": "host" } })).await;
    assert_eq!(status, 201, "{created}");
    let (status, sent) = post(router.clone(), origin,
        &format!("/api/threads/{host_id}/messages"),
        json!({ "text": "Keep working", "clientMessageId": Uuid::new_v4() })).await;
    assert_eq!(status, 202, "{sent}");
    vm.stop().await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    let (status, snapshot) = get(router, origin, &format!("/api/threads/{host_id}")).await;
    assert_eq!(status, 200, "{snapshot}");
    assert_eq!(snapshot["outcomeUnknown"], false);
    assert_eq!(snapshot["activeTurn"]["id"], format!("{host_id}-turn"));
    host.stop().await;
}

/// Starting Master later binds VM work without replacing the Mac worker.
#[tokio::test]
async fn master_attaches_after_host_ready() {
    let root = Directory::new();
    let host_path = root.0.join("host");
    let vm_path = root.0.join("vm");
    fs::create_dir(&host_path).unwrap();
    fs::create_dir(&vm_path).unwrap();
    let host_cwd = host_path.to_str().unwrap().to_owned();
    let vm_cwd = vm_path.to_str().unwrap().to_owned();
    let host_id = "019aaaaa-aaaa-7aaa-8aaa-aaaaaaaaaaaa";
    let vm_id = "019abbbb-bbbb-7bbb-8bbb-bbbbbbbbbbbb";
    let host = Client::spawn(peer(&host_cwd, host_id)).await.unwrap();
    let mut vm_command = peer(&vm_cwd, vm_id);
    vm_command.env("MC_TEST_COMPLETE", "1");
    let vm = Client::spawn(vm_command).await.unwrap();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let backend = Backend::new_with_host(None, Some(host.clone()), vm_cwd,
        host_cwd, origin.into(), root.0.join("recovery"), store, None);
    let router = api::router(backend.clone());
    let (status, host_created) = post(router.clone(), origin, "/api/threads",
        json!({ "environment": { "kind": "host" } })).await;
    assert_eq!(status, 201, "{host_created}");
    let (status, unavailable) = post(router.clone(), origin, "/api/threads",
        json!({ "environment": { "kind": "vm" } })).await;
    assert_eq!(status, 503, "{unavailable}");
    backend.attach_master(vm.clone()).await.unwrap();
    let (status, vm_created) = post(router.clone(), origin, "/api/threads",
        json!({ "environment": { "kind": "vm" } })).await;
    assert_eq!(status, 201, "{vm_created}");
    assert_eq!(vm_created["thread"]["id"], vm_id);
    let (status, vm_sent) = post(router.clone(), origin,
        &format!("/api/threads/{vm_id}/messages"),
        json!({ "text": "VM after startup", "clientMessageId": Uuid::new_v4() })).await;
    assert_eq!(status, 202, "{vm_sent}");
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let (status, snapshot) = get(router.clone(), origin,
                &format!("/api/threads/{vm_id}")).await;
            assert_eq!(status, 200, "{snapshot}");
            if snapshot["activeTurn"].is_null() { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    let (status, host_sent) = post(router.clone(), origin,
        &format!("/api/threads/{host_id}/messages"),
        json!({ "text": "Mac stays live", "clientMessageId": Uuid::new_v4() })).await;
    assert_eq!(status, 202, "{host_sent}");
    vm.stop().await;
    host.stop().await;
}

/// A late Mac worker makes Mac work available without starting Master.
#[tokio::test]
async fn host_attaches_without_master() {
    let root = Directory::new();
    let host_path = root.0.join("host");
    let vm_path = root.0.join("vm");
    fs::create_dir(&host_path).unwrap();
    fs::create_dir(&vm_path).unwrap();
    let host_cwd = host_path.to_str().unwrap().to_owned();
    let vm_cwd = vm_path.to_str().unwrap().to_owned();
    let host_id = "019acccc-cccc-7ccc-8ccc-cccccccccccc";
    let host = Client::spawn(peer(&host_cwd, host_id)).await.unwrap();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let backend = Backend::new_with_host(None, None, vm_cwd, host_cwd,
        origin.into(), root.0.join("recovery"), store, None);
    let router = api::router(backend.clone());
    let (status, unavailable) = post(router.clone(), origin, "/api/threads",
        json!({ "environment": { "kind": "host" } })).await;
    assert_eq!(status, 503, "{unavailable}");
    backend.attach_host(host.clone()).await.unwrap();
    let (status, created) = post(router.clone(), origin, "/api/threads",
        json!({ "environment": { "kind": "host" } })).await;
    assert_eq!(status, 201, "{created}");
    assert_eq!(created["thread"]["id"], host_id);
    host.stop().await;
}

/// Setup signs in to Codex inside Master through Master's own worker, never the Mac's.
#[tokio::test]
async fn master_guest_signs_in_through_its_own_worker() {
    let root = Directory::new();
    let host_path = root.0.join("host");
    let master_path = root.0.join("master");
    fs::create_dir(&host_path).unwrap();
    fs::create_dir(&master_path).unwrap();
    let host = Client::spawn(peer(host_path.to_str().unwrap(),
        "019a9999-9999-7999-8999-999999999999")).await.unwrap();
    let mut guest = peer(master_path.to_str().unwrap(), "019aaaaa-aaaa-7aaa-8aaa-aaaaaaaaaaaa");
    guest.env("MC_TEST_SIGNED_OUT", "1");
    let master = Client::spawn(guest).await.unwrap();
    let origin = "http://127.0.0.1:8787";
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    let backend = Backend::new_with_host(Some(master.clone()), Some(host.clone()),
        master_path.to_str().unwrap().into(), host_path.to_str().unwrap().into(),
        origin.into(), root.0.join("recovery"), store, None);
    let router = api::router(backend);
    let (status, before) = get(router.clone(), origin, "/api/vms/master/status").await;
    assert_eq!(status, 200, "{before}");
    assert_eq!(before["signedIn"], false);
    let (status, login) = post(router.clone(), origin, "/api/vms/master/auth/start",
        json!({ "method": "device" })).await;
    assert_eq!(status, 200, "{login}");
    assert_eq!(login["userCode"], "TEST-CODE");
    assert_eq!(fs::read_to_string(master_path.join("logins")).unwrap(), "device\n");
    assert!(!host_path.join("logins").exists(), "the Mac's Codex was asked to sign in");
    let (status, _) = post(router, origin, "/api/vms/master/auth/start",
        json!({ "method": "browser" })).await;
    assert_eq!(status, 400);
    host.stop().await;
    master.stop().await;
}
