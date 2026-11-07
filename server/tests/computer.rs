use miniclaw_server::computer::handshake;
use serde_json::json;

#[cfg(unix)]
#[test]
fn preserves_helper_parent() {
    use std::{fs, os::unix::fs::PermissionsExt, process::Command};

    let helper = std::env::temp_dir().join(format!("mc-parent-{}", uuid::Uuid::new_v4()));
    fs::write(&helper, "#!/bin/sh\nprintf '%s %s' \"$$\" \"$PPID\"\n").unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_miniclaw-server"))
        .arg("--computer-helper")
        .arg(&helper)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let expected = format!("{} {}", child.id(), std::process::id());
    let _input = child.stdin.take().unwrap();
    let output = child.wait_with_output().unwrap();
    fs::remove_file(helper).unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
}

#[cfg(unix)]
#[test]
fn filter_exits_with_helper() {
    use std::{
        os::{fd::OwnedFd, unix::net::UnixStream},
        process::{Command, Stdio},
    };
    let (helper, filter) = UnixStream::pair().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_miniclaw-server"))
        .arg("--computer-filter")
        .stdin(Stdio::piped())
        .stdout(Stdio::from(OwnedFd::from(filter)))
        .spawn()
        .unwrap();
    let _input = child.stdin.take().unwrap();
    drop(helper);
    for _ in 0..100 {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    panic!("Filter remained alive after helper exit");
}

#[test]
fn preserves_approval_capabilities() {
    let mut frame = json!({ "id": 1, "method": "initialize", "params": {
        "capabilities": { "experimental": { "codex/auth-change": {} },
            "elicitation": { "form": {}, "url": {} } },
        "protocolVersion": "2025-06-18",
    } });
    handshake(&mut frame);
    assert_eq!(
        frame,
        json!({ "id": 1, "method": "initialize", "params": {
        "capabilities": { "elicitation": { "form": {}, "url": {} } },
        "protocolVersion": "2025-06-18",
    } })
    );
}

#[test]
fn leaves_other_messages_unchanged() {
    for frame in [
        json!({ "id": 1, "method": "initialize" }),
        json!({ "id": 1, "method": "initialize", "params": "invalid" }),
        json!({ "id": "a", "result": { "action": "decline", "content": null } }),
        json!({ "id": 2, "method": "tools/call", "params": {
            "name": "get_app_state", "arguments": { "app": "com.apple.calculator" }
        } }),
        json!({ "id": 1, "method": "initialize", "params": {
            "capabilities": { "experimental": { "different-extension": {} } }
        } }),
        json!({ "id": 1, "method": "initialize", "params": {
            "capabilities": { "experimental": {} }
        } }),
    ] {
        let mut adapted = frame.clone();
        handshake(&mut adapted);
        assert_eq!(adapted, frame);
    }
}

#[cfg(unix)]
#[test]
fn exits_when_helper_exits() {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    let mut child = Command::new(env!("CARGO_BIN_EXE_miniclaw-server"))
        .args(["--computer-helper", "/usr/bin/true"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("Bridge waited for stdin after the helper exited");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Starts a signed-in Codex stand-in that answers only what the status route asks.
async fn signed_in() -> miniclaw_server::rpc::Client {
    let mut command = tokio::process::Command::new("node");
    command.args(["--input-type=module", "-e", r#"
        import { createInterface } from 'node:readline';
        const write = value => process.stdout.write(JSON.stringify(value) + '\n');
        createInterface({ input: process.stdin }).on('line', line => {
            const msg = JSON.parse(line);
            if (!msg.method || msg.id === undefined) return;
            if (msg.method === 'account/read') write({ id: msg.id, result: { account: { type: 'chatgpt', email: 'helper@example.test' } } });
            else if (msg.method === 'initialize') write({ id: msg.id, result: {} });
            else write({ id: msg.id, error: { code: -32601, message: 'Unsupported fixture method' } });
        });
    "#]);
    miniclaw_server::rpc::Client::spawn(command).await.unwrap()
}

/// A Windows helper chosen at setup is looked up again for each status; when no Codex app has it,
/// the Tools menu gets the reason instead of a path.
#[tokio::test]
async fn an_installed_helper_that_does_not_resolve_reports_why() {
    use axum::{body::{Body, to_bytes}, http::Request};
    use miniclaw_server::computer::{Installed, Target, Unresolved};
    use tower::ServiceExt;
    let dir = std::env::temp_dir().join(format!("mc-helper-status-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let store = miniclaw_server::agents::Store::open(&dir.join("agents.sqlite3")).unwrap();
    // A family no machine has installed: the shape is valid, the publisher ID is not Codex's.
    let helper = Installed::new("OpenAI.Codex_0000000000000", "codex-computer-use-arm64.exe").unwrap();
    let backend = miniclaw_server::chat::Backend::new(Some(signed_in().await), dir.to_string_lossy().into(),
        "http://127.0.0.1:8787".into(), dir.join("recovery"), store, Some(Target::Installed(helper)), false);
    let request = Request::builder().uri("/api/computer").header("host", "127.0.0.1:8787")
        .body(Body::empty()).unwrap();
    let response = miniclaw_server::api::router(backend).oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body, json!({ "available": false, "host": false, "reason": Unresolved::NotInstalled.message() }));
    std::fs::remove_dir_all(dir).unwrap();
}

/// A helper path given directly (the Mac's, or an explicit Windows path) is used as it is.
#[tokio::test]
async fn a_fixed_helper_is_current_as_it_is() {
    use miniclaw_server::computer::Target;
    let target = Target::Local("/fixed/helper".into());
    assert_eq!(target.current().await, Ok(target.clone()));
    assert_eq!(Target::Guest.current().await, Ok(Target::Guest));
}
