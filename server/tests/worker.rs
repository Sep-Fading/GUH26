// Checks worker authentication and RPC ownership.
use miniclaw_server::{rpc::Failure, worker};
// Creates protocol requests and expected replies.
use serde_json::{Value, json};
// Builds independent controller authentication proof fixtures.
use hmac::{Hmac, KeyInit, Mac};
// Hashes the documented worker handshake transcript.
use sha2::Sha256;
// Bounds test waits and tracks startup.
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
// Builds private streams and owned fixtures.
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, duplex},
    process::Command,
};

const KEY: [u8; 32] = [71; 32];

/// Returns a fake App Server without reading credentials or contacting OpenAI.
fn peer() -> Command {
    let mut command = Command::new("node");
    command.args([
        "--input-type=module",
        "-e",
        r#"
        import { createInterface } from 'node:readline';
        const send = value => console.log(JSON.stringify(value));
        let initialized = false;
        let first;
        createInterface({ input: process.stdin }).on('line', line => {
            const msg = JSON.parse(line);
            if (msg.method === 'initialize') send({ id: msg.id, result: {} });
            else if (msg.method === 'initialized') initialized = true;
            else if (msg.method === 'hold') first = msg;
            else if (msg.method === 'pid') send({ id: msg.id, result: process.pid });
            else if (msg.method === 'exit') process.exit(0);
            else if (msg.method === 'finish') {
                process.stdout.write(JSON.stringify({ id: msg.id,
                    result: 'x'.repeat(128 * 1024) }) + '\n', () => process.exit(0));
            }
            else if (msg.method === 'second') {
                send({ id: msg.id, result: 2 });
                send({ id: first.id, result: 1 });
            } else if (msg.method === 'stream') {
                send({ method: 'item/agentMessage/delta', params: { delta: 'hello' } });
                send({ id: msg.id, result: {} });
            } else if (msg.method === 'approve') {
                send({ id: 'approval', method: 'mcpServer/elicitation/request',
                    params: { threadId: 'thread' } });
                send({ id: msg.id, result: {} });
            } else if (msg.method) send({ id: msg.id, result: { initialized } });
        });
    "#,
    ]);
    command
}

/// Checks initialization, correlation and streaming over an authenticated worker.
#[tokio::test]
async fn routes_worker_rpc() {
    let (controller, guest) = duplex(4096);
    let serving = tokio::spawn(worker::serve(guest, KEY, peer));
    let client = worker::connect(controller, KEY).await.unwrap();
    let mut events = client.subscribe();
    assert_eq!(
        client.call("check", json!({})).await.unwrap(),
        json!({ "initialized": true })
    );
    let (first, second) = tokio::join!(
        client.call("hold", json!({})),
        client.call("second", json!({})),
    );
    assert_eq!(first.unwrap(), json!(1));
    assert_eq!(second.unwrap(), json!(2));
    client.call("stream", json!({})).await.unwrap();
    assert_eq!(events.recv().await.unwrap().value()["params"]["delta"], "hello");
    client.stop().await;
    tokio::time::timeout(Duration::from_secs(2), serving)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

/// A VM worker runs the tool itself, under a Seatbelt profile built from the grants that
/// arrive with the call; Codex never sees the request, and Apple Events stay refused.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn worker_runs_tool_operations_under_the_sent_grants() {
    let root = std::env::temp_dir().canonicalize().unwrap()
        .join(format!("mc-worker-tool-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("granted")).unwrap();
    std::fs::create_dir_all(root.join("secret")).unwrap();
    std::fs::write(root.join("granted/ok.txt"), "granted-content").unwrap();
    std::fs::write(root.join("secret/no.txt"), "secret-content").unwrap();
    let (controller, guest) = duplex(65536);
    let serving = tokio::spawn(worker::serve(guest, KEY, peer));
    let client = worker::connect(controller, KEY).await.unwrap();
    let granted = root.join("granted");
    let result = client.call("miniclaw/scoped/execute", json!({
        "operation": { "action": "run_shell", "cwd": granted,
            "text": format!("cat ok.txt; cat {}", root.join("secret/no.txt").display()) },
        "grants": [{ "path": granted, "write": false, "folder": true, "blocked": false }],
    })).await.unwrap();
    assert_eq!(result["status"], "ok", "{result}");
    assert!(result["text"].as_str().unwrap().contains("granted-content"), "{result}");
    assert!(!result["text"].as_str().unwrap().contains("secret-content"), "{result}");
    assert!(result["stderr"].as_str().unwrap().contains("Operation not permitted"), "{result}");
    for refused in [
        json!({ "operation": { "action": "run_applescript", "text": "return 1", "cwd": granted },
            "grants": [] }),
        json!({ "operation": { "action": "read_file", "path": "/tmp/../etc/hosts" }, "grants": [] }),
        json!({ "action": "read_file", "path": granted.join("ok.txt") }),
    ] {
        assert_eq!(client.call("miniclaw/scoped/execute", refused.clone()).await.unwrap(),
            json!({ "status": "denied" }), "{refused}");
    }
    client.stop().await;
    tokio::time::timeout(Duration::from_secs(2), serving).await.unwrap().unwrap().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

/// Rejects the wrong key without invoking the worker's process factory.
#[tokio::test]
async fn rejects_wrong_key() {
    let (controller, guest) = duplex(4096);
    let started = Arc::new(AtomicBool::new(false));
    let observed = started.clone();
    let serving = tokio::spawn(worker::serve(guest, KEY, move || {
        started.store(true, Ordering::SeqCst);
        peer()
    }));
    let result = worker::connect(controller, [72; 32]).await;
    assert!(matches!(result, Err(Failure::Rejected)));
    assert!(serving.await.unwrap().is_err());
    assert!(!observed.load(Ordering::SeqCst));
}

/// Closes pending calls promptly when the guest process exits.
#[tokio::test]
async fn fails_pending_on_exit() {
    let (controller, guest) = duplex(4096);
    let serving = tokio::spawn(worker::serve(guest, KEY, peer));
    let client = worker::connect(controller, KEY).await.unwrap();
    let mut events = client.subscribe();
    let replies = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(
            client.call("hold", json!({})),
            client.call("exit", json!({}))
        )
    })
    .await
    .unwrap();
    assert_eq!(
        replies,
        (Err(Failure::Unavailable), Err(Failure::Unavailable))
    );
    assert_eq!(
        events.recv().await.unwrap().value()["method"],
        "miniclaw/disconnected"
    );
    assert!(!client.available());
    client.stop().await;
    serving.await.unwrap().unwrap();
}

/// Keeps Stop usable while an approval remains unanswered.
#[tokio::test]
async fn stops_pending_approval() {
    let (controller, guest) = duplex(4096);
    let serving = tokio::spawn(worker::serve(guest, KEY, peer));
    let client = worker::connect(controller, KEY).await.unwrap();
    let mut requests = client.take_requests().unwrap();
    client.call("approve", Value::Null).await.unwrap();
    let request = requests.recv().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), client.stop())
        .await
        .unwrap();
    assert!(!client.available());
    serving.await.unwrap().unwrap();
    tokio::task::yield_now().await;
    assert!(request.answer.send(json!({ "action": "accept" })).is_err());
}

/// Refuses malformed and prematurely closed peers before starting a process.
#[tokio::test]
async fn rejects_invalid_handshake() {
    for bytes in [vec![0; 40], vec![1; 4]] {
        let (mut controller, guest) = duplex(4096);
        let serving = tokio::spawn(worker::serve(guest, KEY, || panic!("must not spawn")));
        controller.write_all(&bytes).await.unwrap();
        controller.shutdown().await.unwrap();
        assert!(serving.await.unwrap().is_err());
        let mut reply = Vec::new();
        controller.read_to_end(&mut reply).await.unwrap();
        assert!(reply.is_empty());
    }
}

/// Rejects reflected worker proofs and captured proofs from earlier connections.
#[tokio::test]
async fn rejects_replayed_proofs() {
    let mut previous: Option<[u8; 64]> = None;
    for _ in 0..2 {
        let (mut controller, guest) = duplex(4096);
        let serving = tokio::spawn(worker::serve(guest, KEY, || panic!("must not spawn")));
        controller.write_all(b"MCWORK01").await.unwrap();
        controller.write_all(&[12; 32]).await.unwrap();
        let mut challenge = [0; 64];
        controller.read_exact(&mut challenge).await.unwrap();
        assert!(!challenge.windows(32).any(|part| part == KEY));
        if let Some(old) = previous {
            assert_ne!(old, challenge);
            controller.write_all(&old[32..]).await.unwrap();
        } else {
            controller.write_all(&challenge[32..]).await.unwrap();
        }
        assert_eq!(serving.await.unwrap(), Err(Failure::Rejected));
        previous = Some(challenge);
    }
}

/// A valid controller proof must not authenticate a second, freshly challenged session.
#[tokio::test]
async fn rejects_previous_session() {
    let mut previous = None;
    for index in 0..2 {
        let (mut controller, guest) = duplex(4096);
        let started = Arc::new(AtomicBool::new(false));
        let observed = started.clone();
        let serving = tokio::spawn(worker::serve(guest, KEY, move || {
            started.store(true, Ordering::SeqCst);
            peer()
        }));
        controller.write_all(b"MCWORK01").await.unwrap();
        controller.write_all(&[12; 32]).await.unwrap();
        let mut challenge = [0; 64];
        controller.read_exact(&mut challenge).await.unwrap();
        let mut mac = Hmac::<Sha256>::new_from_slice(&KEY).unwrap();
        mac.update(b"MCWORK01controller");
        mac.update(&[12; 32]);
        mac.update(&challenge[..32]);
        let valid = mac.finalize().into_bytes();
        let sent = previous.unwrap_or(valid);
        controller.write_all(&sent).await.unwrap();
        if index == 0 {
            assert_eq!(controller.read_u8().await.unwrap(), 1);
            drop(controller);
            serving.await.unwrap().unwrap();
            assert!(observed.load(Ordering::SeqCst));
        } else {
            assert_eq!(serving.await.unwrap(), Err(Failure::Rejected));
            assert!(!observed.load(Ordering::SeqCst));
        }
        previous = Some(sent);
    }
}

/// Exceeds the existing 32 MiB frame limit before the guest App Server can accept a request.
#[tokio::test]
async fn bounds_request_frames() {
    let (controller, guest) = duplex(4096);
    let serving = tokio::spawn(worker::serve(guest, KEY, peer));
    let client = worker::connect(controller, KEY).await.unwrap();
    let result = client
        .call("check", json!({ "text": "x".repeat(33 * 1024 * 1024) }))
        .await;
    client.stop().await;
    let _ = serving.await.unwrap();
    assert_eq!(result, Err(Failure::Unavailable));
}

/// Drains a child's final response before reporting that its stream disconnected.
#[tokio::test]
async fn drains_final_reply() {
    for _ in 0..10 {
        let (controller, guest) = duplex(256);
        let serving = tokio::spawn(worker::serve(guest, KEY, peer));
        let client = worker::connect(controller, KEY).await.unwrap();
        let reply = client.call("finish", Value::Null).await;
        client.stop().await;
        serving.await.unwrap().unwrap();
        assert_eq!(reply.unwrap(), json!("x".repeat(128 * 1024)));
    }
}

/// Rejects peers that stop halfway through authentication within three seconds.
#[tokio::test]
async fn bounds_handshake_wait() {
    let (mut controller, guest) = duplex(4096);
    let serving = tokio::spawn(worker::serve(guest, KEY, || panic!("must not spawn")));
    controller.write_all(b"MC").await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(4), serving)
        .await
        .unwrap();
    assert_eq!(result.unwrap(), Err(Failure::Timeout));
}

/// Reaps the guest child before returning from its connection handler.
#[tokio::test]
async fn reaps_child_after_stop() {
    let (controller, guest) = duplex(4096);
    let serving = tokio::spawn(worker::serve(guest, KEY, peer));
    let client = worker::connect(controller, KEY).await.unwrap();
    let pid = client
        .call("pid", Value::Null)
        .await
        .unwrap()
        .as_u64()
        .unwrap();
    client.stop().await;
    serving.await.unwrap().unwrap();
    let status = Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .unwrap();
    assert!(!status.success(), "worker child is still alive");
}

/// Cancelling setup during initialize closes the stream and stops the guest process.
#[tokio::test]
async fn cancels_initialization() {
    let (controller, guest) = duplex(4096);
    let (started, ready) = tokio::sync::oneshot::channel();
    let mut serving = tokio::spawn(worker::serve(guest, KEY, move || {
        let _ = started.send(());
        let mut command = Command::new("node");
        command.args(["-e", "process.stdin.resume(); setInterval(() => {}, 1000)"]);
        command
    }));
    let connecting = tokio::spawn(worker::connect(controller, KEY));
    ready.await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    connecting.abort();
    assert!(matches!(connecting.await, Err(error) if error.is_cancelled()));
    let result = tokio::time::timeout(Duration::from_secs(2), &mut serving).await;
    if result.is_err() {
        serving.abort();
        let _ = serving.await;
    }
    assert!(
        result.is_ok(),
        "cancelled initialization left the worker running"
    );
}
