// Exercises bounded transport with synthetic notifications.
use miniclaw_server::{agents::Store, chat::{Backend, Chat, Receipt}, models::Choice, rpc::Client};
// Sends explicit fixture commands without private data.
use serde_json::json;
// Bounds fixture progress without timing sleeps.
use std::{fs, time::Duration};
// Starts an isolated newline-delimited protocol peer.
use tokio::process::Command;
// Gives disposable private fixture directories unique names.
use uuid::Uuid;
// Exercises public mutation responses without a listener.
use axum::{body::{Body, to_bytes}, http::Request};
// Invokes the production router with test requests.
use tower::ServiceExt;

/// Creates a peer whose supported methods and generated payload sizes are explicit.
async fn client() -> Client {
    logged_client(None).await
}

/// Adds an optional private journal for verifying received synthetic turn starts.
async fn logged_client(log: Option<&std::path::Path>) -> Client {
    let mut command = Command::new("node");
    if let Some(log) = log { command.env("FIXTURE_WRITES", log); }
    command.args(["--input-type=module", "-e", r#"
        import { createInterface } from 'node:readline';
        import { appendFileSync } from 'node:fs';
        const write = value => new Promise(resolve =>
            process.stdout.write(JSON.stringify(value) + '\n', resolve));
        let first;
        createInterface({ input: process.stdin }).on('line', async line => {
            const msg = JSON.parse(line);
            if (!msg.method) return;
            if (msg.method === 'initialized') return;
            if (msg.method === 'fixture/emit') {
                for (let index = 0; index < msg.params.count; index++) {
                    await write({ method: 'fixture/notice', params: {
                        index, padding: 'x'.repeat(msg.params.size) } });
                }
                await write({ id: msg.id, result: {} });
            } else if (msg.method === 'fixture/approve') {
                await write({ id: 'a'.repeat(msg.params.size),
                    method: 'mcpServer/elicitation/request', params: { threadId: 'fixture' } });
                await write({ id: msg.id, result: {} });
            } else if (msg.method === 'fixture/first') {
                first = msg.id;
            } else if (msg.method === 'fixture/second') {
                await write({ id: msg.id, result: 2 });
                await write({ id: first, result: 1 });
            } else if (msg.method === 'account/read') {
                await write({ id: msg.id, result: { account: {
                    type: 'chatgpt', email: 'budget@example.test' } } });
            } else if (msg.method === 'turn/start' && process.env.FIXTURE_WRITES) {
                appendFileSync(process.env.FIXTURE_WRITES, 'turn/start\n');
                // Leave the acknowledgement pending while overload is injected.
            } else if (['initialize', 'turn/interrupt'].includes(msg.method)) {
                await write({ id: msg.id, result: {} });
            } else {
                await write({ id: msg.id, error: { code: -32601,
                    message: 'Unsupported fixture method' } });
            }
        });
    "#]);
    Client::spawn(command).await.unwrap()
}

/// Fewer than 1024 large notifications exhaust the aggregate payload budget.
#[tokio::test]
async fn byte_overflow_closes_transport() {
    let client = client().await;
    let _held = client.subscribe();
    let result = tokio::time::timeout(Duration::from_secs(3), client.call(
        "fixture/emit", json!({ "count": 40, "size": 1024 * 1024 }))).await.unwrap();
    assert!(result.is_err(), "large notification queue remained unbounded by bytes");
    assert!(!client.available());
    client.stop().await;
}

/// Values already received remain charged while any consumer clone retains them.
#[tokio::test]
async fn received_clones_keep_budget() {
    let client = client().await;
    let mut receiver = client.subscribe();
    client.call("fixture/emit", json!({ "count": 20, "size": 1024 * 1024 }))
        .await.unwrap();
    let mut retained = Vec::new();
    for _ in 0..20 {
        let value = receiver.recv().await.unwrap();
        retained.push(value.clone());
        drop(value);
    }
    let result = client.call("fixture/emit", json!({ "count": 20, "size": 1024 * 1024 }))
        .await;
    assert!(result.is_err(), "receiver clones released their charge too early");
    drop(retained);
    client.stop().await;
}

/// Dropping receivers or their consumed values releases capacity for later batches.
#[tokio::test]
async fn dropped_values_release_budget() {
    let client = client().await;
    for _ in 0..3 {
        let mut receiver = client.subscribe();
        client.call("fixture/emit", json!({ "count": 20, "size": 1024 * 1024 }))
            .await.unwrap();
        for _ in 0..20 { drop(receiver.recv().await.unwrap()); }
        drop(receiver);
    }
    // Sending without any receiver must not consume retained capacity.
    client.call("fixture/emit", json!({ "count": 40, "size": 1024 * 1024 }))
        .await.unwrap();
    let _receiver = client.subscribe();
    client.call("fixture/emit", json!({ "count": 20, "size": 1024 * 1024 }))
        .await.unwrap();
    client.stop().await;
}

/// A paused consumer below budget does not block reply routing or Stop.
#[tokio::test]
async fn held_consumer_keeps_rpc_usable() {
    let client = client().await;
    let _held = client.subscribe();
    client.call("fixture/emit", json!({ "count": 8, "size": 1024 * 1024 }))
        .await.unwrap();
    let (first, second) = tokio::join!(
        client.call("fixture/first", json!({})),
        client.call("fixture/second", json!({})),
    );
    assert_eq!(first.unwrap(), 1);
    assert_eq!(second.unwrap(), 2);
    tokio::time::timeout(Duration::from_secs(1), client.call("turn/interrupt",
        json!({ "threadId": "fixture", "turnId": "active" }))).await.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(1), client.stop()).await.unwrap();
}

/// Lag or byte exhaustion persists terminal failure and keeps active sends uncertain.
#[tokio::test]
async fn lost_events_preserve_uncertainty() {
    for large in [false, true] {
        let dir = std::env::temp_dir().join(format!("mc-rpc-budget-{}", Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let client = client().await;
        let store = Store::open(&dir.join("agents.sqlite3")).unwrap();
        let backend = Backend::new(Some(client.clone()), dir.to_string_lossy().into(),
            "http://127.0.0.1:8787".into(), dir.join("recovery"), store.clone(), None, false);
        let mut data = backend.data.lock().await;
        data.account = json!({ "status": "signed_in", "email": "budget@example.test" });
        let owner = miniclaw_server::notifications::owner(&data.account).unwrap();
        let mut chat = Chat::new(json!({ "id": "active" }));
        chat.active = Some("response".into());
        chat.sending = true;
        chat.receipts.insert("client-id".into(), Receipt {
            text: "one question".into(), choice: Choice::default(), turn: None, attachments: Vec::new(),
        });
        data.chats.insert("active".into(), chat);
        let result = client.call("fixture/emit", if large {
            json!({ "count": 40, "size": 1024 * 1024 })
        } else { json!({ "count": 1100, "size": 8 }) }).await;
        if large { assert!(result.is_err(), "byte overload did not end transport"); }
        drop(data);
        tokio::time::timeout(Duration::from_secs(3), client.closed()).await.unwrap();
        // A later orderly cleanup must not relabel the already-observed failure.
        client.stop().await;
        tokio::time::timeout(Duration::from_secs(3), client.drain_events()).await.unwrap();
        let data = backend.data.lock().await;
        assert!(data.chats["active"].uncertain, "lost events left send outcome certain");
        assert!(data.chats["active"].receipts["client-id"].turn.is_none());
        assert!(data.account.is_null());
        drop(data);
        assert_eq!(store.notices(owner).await.unwrap().len(), 1);
        drop(backend);
        fs::remove_dir_all(dir).unwrap();
    }
}

/// Generated approval events use ordinary capacity and cannot suppress terminal delivery.
#[tokio::test]
async fn approval_events_share_budget() {
    let client = client().await;
    let mut receiver = client.subscribe();
    let mut approvals = client.take_requests().unwrap();
    client.call("fixture/emit", json!({ "count": 31, "size": 1024 * 1024 }))
        .await.unwrap();
    client.call("fixture/approve", json!({ "size": 2 * 1024 * 1024 }))
        .await.unwrap();
    approvals.recv().await.unwrap().answer.send(json!({ "action": "decline" })).unwrap();
    tokio::time::timeout(Duration::from_secs(3), client.closed()).await.unwrap();
    client.stop().await;
    for _ in 0..31 { assert_eq!(receiver.recv().await.unwrap().value()["method"], "fixture/notice"); }
    let terminal = receiver.recv().await.unwrap().value();
    assert_eq!(terminal["method"], "miniclaw/disconnected");
    assert_eq!(terminal["params"]["expected"], false);
    assert!(receiver.try_recv().is_err(), "ordinary event followed terminal state");
}

/// Overload leaves an unacknowledged HTTP send uncertain and rejects its repeated ID.
#[tokio::test]
async fn public_send_is_not_repeated() {
    let dir = std::env::temp_dir().join(format!("mc-rpc-send-{}", Uuid::new_v4()));
    fs::create_dir(&dir).unwrap();
    let log = dir.join("starts");
    fs::write(&log, "").unwrap();
    let client = logged_client(Some(&log)).await;
    let store = Store::open(&dir.join("agents.sqlite3")).unwrap();
    let id = Uuid::new_v4().to_string();
    // Sending needs a pinned machine; this chat runs on the Mac.
    store.assign_existing(id.clone(), miniclaw_server::environment::Assignment::Host {})
        .await.unwrap();
    let backend = Backend::new(Some(client.clone()), dir.to_string_lossy().into(),
        "http://127.0.0.1:8787".into(), dir.join("recovery"), store, None, false);
    let client_id = Uuid::new_v4().to_string();
    backend.data.lock().await.chats.insert(id.clone(), Chat::new(json!({ "id": id })));
    let request = || Request::builder().method("POST")
        .uri(format!("/api/threads/{id}/messages"))
        .header("host", "127.0.0.1:8787").header("content-type", "application/json")
        .header("x-miniclaw-request", "1")
        .body(Body::from(json!({ "text": "One question", "clientMessageId": client_id })
            .to_string())).unwrap();
    let router = miniclaw_server::api::router(backend.clone());
    let sending = tokio::spawn(router.clone().oneshot(request()));
    tokio::time::timeout(Duration::from_secs(3), async {
        while fs::read_to_string(&log).unwrap().is_empty() { tokio::task::yield_now().await; }
    }).await.unwrap();
    let data = backend.data.lock().await;
    assert!(data.chats[&id].sending);
    assert!(client.call("fixture/emit", json!({ "count": 40, "size": 1024 * 1024 }))
        .await.is_err());
    drop(data);
    let failed = sending.await.unwrap().unwrap();
    assert_eq!(failed.status(), 503);
    let body = to_bytes(failed.into_body(), 4096).await.unwrap();
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap()
        ["error"]["outcomeUnknown"], true);
    client.drain_events().await;
    let repeated = router.oneshot(request()).await.unwrap();
    assert_eq!(repeated.status(), 503);
    assert_eq!(fs::read_to_string(&log).unwrap(), "turn/start\n");
    let data = backend.data.lock().await;
    assert!(data.chats[&id].uncertain);
    assert!(data.chats[&id].receipts[&client_id].turn.is_none());
    drop(data);
    drop(backend);
    fs::remove_dir_all(dir).unwrap();
}
