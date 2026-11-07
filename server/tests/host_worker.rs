#![cfg(unix)]

// Drives the Mac's own Codex through the in-process worker link.
use miniclaw_server::{runtime, worker};
// Verifies the supervisor request proof independently.
use hmac::{Hmac, Mac};
// Hashes signed status and start requests.
use sha2::Sha256;
// Checks the authenticated worker's Codex replies.
use serde_json::json;
// Creates disposable settings, workspace, and fake Codex.
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, time::Duration};
// Accepts only the worker process started by this test.
use tokio::{io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixListener};
// Names each disposable test root independently.
use uuid::Uuid;

struct Directory(PathBuf);

impl Directory {
    /// Creates a private fixture root and returns its owned path.
    fn new() -> Self {
        let path = PathBuf::from("/private/tmp")
            .join(format!("mchw-{}", Uuid::new_v4().simple()));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    /// Removes only the files created by this test fixture.
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

/// The Mac's Codex link forwards tasks to Codex and VM control to the supervisor socket.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn host_worker_forwards_its_workspace() {
    let root = Directory::new();
    let cwd = root.0.join("workspace");
    fs::create_dir(&cwd).unwrap();
    let control_socket = root.0.join("control.sock");
    let control_listener = UnixListener::bind(&control_socket).unwrap();
    fs::set_permissions(&control_socket, fs::Permissions::from_mode(0o600)).unwrap();
    let control_key = [42_u8; 32];
    let fake = root.0.join("fake-codex.mjs");
    let node = std::process::Command::new("/usr/bin/which").arg("node")
        .output().unwrap();
    assert!(node.status.success());
    let mut script = format!("#!{}\n", String::from_utf8_lossy(&node.stdout).trim());
    script.push_str(r#"import { createInterface } from 'node:readline';
createInterface({ input: process.stdin }).on('line', line => {
    const msg = JSON.parse(line);
    if (msg.method === 'initialized') return;
    const result = msg.method === 'config/read' ? { config: { mcp_servers: {} } }
      : msg.method === 'account/read' ? { account: { type: 'chatgpt',
          email: 'host@example.invalid', planType: 'plus' } }
      : msg.method === 'thread/start' ? { thread: { id: '019a9999-9999-7999-8999-999999999999',
          cwd: msg.params.cwd }, model: 'fixture', reasoningEffort: null }
      : {};
    process.stdout.write(`${JSON.stringify({ id: msg.id, result })}\n`);
});
"#);
    fs::write(&fake, script).unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
    let control = tokio::spawn(async move {
        for (index, method) in ["status", "start", "start", "status", "list",
            "provision", "provision", "retry", "cancel"]
            .iter().enumerate() {
            let (stream, _) = control_listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], *method);
            let vm_id = if index == 3 || index >= 7 {
                "019a1111-1111-7111-8111-111111111111"
            } else { "" };
            if index == 3 || index >= 7 { assert_eq!(request["vmId"], vm_id); }
            let request_id = if index == 5 || index == 6 {
                if index == 5 { "019a2222-2222-7222-8222-222222222222" }
                else { "019a3333-3333-7333-8333-333333333333" }
            } else { "" };
            if index == 5 || index == 6 { assert_eq!(request["requestId"], request_id); }
            let mut mac = <Hmac<Sha256> as hmac::digest::KeyInit>::new_from_slice(&control_key)
                .unwrap();
            mac.update(format!("{}:{method}{}", request["nonce"].as_str().unwrap(),
                format!("{}{}", if vm_id.is_empty() { String::new() }
                    else { format!(":{vm_id}") }, if request_id.is_empty() { String::new() }
                    else { format!(":{request_id}") })).as_bytes());
            let proof = request["mac"].as_str().unwrap().as_bytes().chunks_exact(2)
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect::<Vec<_>>();
            mac.verify_slice(&proof).unwrap();
            let reply = if index == 4 {
                json!({ "ok": true, "machines": [{
                    "vmId": "019a1111-1111-7111-8111-111111111111",
                    "slot": 0, "key": "2a".repeat(32),
                    "cwd": "/Users/Shared/MiniClaw/Workspace" }] })
            } else if index == 5 {
                json!({ "ok": true, "machine": { "vmId": "019a1111-1111-7111-8111-111111111111",
                    "state": "starting", "stage": "preparing" } })
            } else if index == 6 {
                json!({ "ok": false, "error": { "code": "setup_failed",
                    "message": "Project VM limit reached." } })
            } else if index >= 7 {
                json!({ "ok": true, "status": { "state": "failed",
                    "stage": "failed", "reason": "VM setup was cancelled. Its files were kept for retry." } })
            } else if index == 3 {
                json!({ "ok": true, "status": { "state": "missing",
                    "reason": "Assigned VM does not exist." } })
            } else if index == 2 {
                json!({ "ok": false, "error": { "code": "start_failed",
                    "message": "VM resource limit exceeded." } })
            } else {
                json!({ "ok": true, "status": { "state":
                    if *method == "status" { "off" } else { "starting" } } })
            };
            stream.get_mut().write_all(format!("{reply}\n").as_bytes()).await.unwrap();
        }
    });
    let client = tokio::time::timeout(Duration::from_secs(10),
        runtime::launch_with_control(fake.to_str().unwrap(), cwd.to_str().unwrap(),
            worker::Control { path: control_socket.clone(), key: control_key })).await.unwrap().unwrap();
    let account = client.call("account/read", json!({})).await.unwrap();
    assert_eq!(account["account"]["email"], "host@example.invalid");
    let started = client.call("thread/start", json!({
        "cwd": cwd, "approvalPolicy": "never", "sandbox": "read-only",
        "environments": [], "selectedCapabilityRoots": [], "baseInstructions": "No tools",
    })).await.unwrap();
    assert_eq!(started["thread"]["cwd"], cwd.to_str().unwrap());
    let status = client.call("miniclaw/vm/status", json!({})).await.unwrap();
    assert_eq!(status["state"], "off");
    let started = client.call("miniclaw/vm/start", json!({})).await.unwrap();
    assert_eq!(started["state"], "starting");
    let failed = client.call("miniclaw/vm/start", json!({})).await.unwrap();
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["reason"], "VM resource limit exceeded.");
    let missing = client.call("miniclaw/vm/status", json!({
        "vmId": "019a1111-1111-7111-8111-111111111111" })).await.unwrap();
    assert_eq!(missing["state"], "missing");
    let listed = client.call("miniclaw/vm/list", json!({})).await.unwrap();
    assert_eq!(listed["machines"][0]["slot"], 0);
    let provisioned = client.call("miniclaw/vm/provision", json!({
        "requestId": "019a2222-2222-7222-8222-222222222222" })).await.unwrap();
    assert_eq!(provisioned["vmId"], "019a1111-1111-7111-8111-111111111111");
    let full = client.call("miniclaw/vm/provision", json!({
        "requestId": "019a3333-3333-7333-8333-333333333333" })).await.unwrap();
    assert_eq!(full["state"], "failed");
    assert_eq!(full["reason"], "Project VM limit reached.");
    let retried = client.call("miniclaw/vm/retry", json!({
        "vmId": "019a1111-1111-7111-8111-111111111111" })).await.unwrap();
    assert_eq!(retried["stage"], "failed");
    let cancelled = client.call("miniclaw/vm/cancel", json!({
        "vmId": "019a1111-1111-7111-8111-111111111111" })).await.unwrap();
    assert_eq!(cancelled["state"], "failed");
    control.await.unwrap();
    let lost = client.call("miniclaw/vm/status", json!({})).await.unwrap();
    assert_eq!(lost["state"], "unknown");
    assert_eq!(lost["reason"], "Mac VM controller is unavailable.");
    client.stop().await;
}
