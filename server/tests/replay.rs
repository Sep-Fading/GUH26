// Builds direct HTTP requests and bounded responses.
use axum::{body::Body, http::Request, response::Response};
// Polls exactly one HTTP body frame.
use futures_util::StreamExt;
// Exercises replay through production reducers and routes.
use miniclaw_server::{agents::Store, api, chat::{Backend, Chat}, rpc::Client};
// Constructs synthetic transcript and protocol data.
use serde_json::{Value, json};
// Owns disposable storage and shared backend fixtures.
use std::{fs, path::PathBuf, sync::Arc, time::Duration};
// Starts an isolated credential-free protocol peer.
use tokio::process::Command;
// Calls production routes without network listeners.
use tower::ServiceExt;
// Gives disposable directories and chats unique identities.
use uuid::Uuid;

const LIMIT: usize = 2 * 1024 * 1024;

/// Checks encoded replay bytes, including escaped JSON and UTF-8 text.
#[test]
fn bounds_encoded_replay() {
    for text in ["x".repeat(16384), "\0\n\"\\🦀".repeat(4096)] {
        let mut chat = Chat::new(json!({}));
        for _ in 0..200 { chat.emit("message.delta", json!({ "delta": text })); }
        let bytes: usize = chat.events.iter()
            .map(|event| event.id.len() + event.name.len() + event.data.to_string().len())
            .sum();
        assert!(bytes <= LIMIT, "retained {bytes} bytes");
        assert_eq!(chat.sequence, 200);
        assert!(!chat.events.is_empty());
    }
}

/// An event above the replay budget keeps the full reducer and creates a replay gap.
#[test]
fn oversize_keeps_transcript() {
    let mut chat = Chat::new(json!({}));
    chat.emit("turn.started", json!({}));
    let text = "complete".repeat(LIMIT / 8 + 1);
    chat.messages.push(json!({ "id": "large", "text": text, "complete": true }));
    chat.emit("snapshot", chat.snapshot());
    assert!(chat.events.is_empty());
    assert_eq!(chat.sequence, 2);
    assert_eq!(chat.snapshot()["messages"][0]["text"], text);
    chat.emit("turn.completed", json!({}));
    assert_eq!(chat.events.len(), 1);
}

struct Fixture { backend: Arc<Backend>, dir: PathBuf, id: String }

impl Fixture {
    /// Creates a saved cache and synthetic peer without inference or real credentials.
    async fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("mc-replay-{}", Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let mut command = Command::new("node");
        command.args(["--input-type=module", "-e", r#"
            import { createInterface } from 'node:readline';
            let signedIn = true;
            createInterface({ input: process.stdin }).on('line', line => {
                const msg = JSON.parse(line);
                if (msg.id === undefined) return;
                if (!['initialize', 'account/read', 'turn/interrupt', 'fixture/signout'].includes(msg.method)) {
                    console.log(JSON.stringify({ id: msg.id,
                        error: { code: -32601, message: 'Unsupported fixture method' } }));
                    return;
                }
                if (msg.method === 'fixture/signout') signedIn = false;
                const result = msg.method === 'account/read'
                    ? { account: signedIn ? { type: 'chatgpt', email: 'replay@example.test' } : null } : {};
                console.log(JSON.stringify({ id: msg.id, result }));
            });
        "#]);
        let client = Client::spawn(command).await.unwrap();
        let store = Store::open(&dir.join("agents.sqlite3")).unwrap();
        let id = Uuid::new_v4().to_string();
        // Task routes need a pinned machine; this chat runs on the Mac.
        store.assign_existing(id.clone(), miniclaw_server::environment::Assignment::Host {})
            .await.unwrap();
        let backend = Backend::new(Some(client), dir.to_string_lossy().into(),
            "http://127.0.0.1:8787".into(), dir.join("recovery"), store, None, false);
        let mut chat = Chat::new(json!({ "id": id }));
        chat.history_loaded = true;
        backend.data.lock().await.chats.insert(id.clone(), chat);
        Self { backend, dir, id }
    }

    /// Opens an unpolled production SSE response with an optional reconnect cursor.
    async fn response(&self, cursor: Option<&str>) -> Response {
        let mut request = Request::builder().uri(format!("/api/threads/{}/events", self.id))
            .header("host", "127.0.0.1:8787");
        if let Some(cursor) = cursor { request = request.header("last-event-id", cursor); }
        api::router(self.backend.clone()).oneshot(request.body(Body::empty()).unwrap())
            .await.unwrap()
    }

    /// Stops the fixture transport and drains handlers before removing temporary files.
    async fn close(self) {
        let client = self.backend.client.as_ref().unwrap();
        client.stop().await;
        client.drain_events().await;
        drop(self.backend);
        fs::remove_dir_all(self.dir).unwrap();
    }
}

/// A paused consumer reselects current replay rather than retaining an old batch.
#[tokio::test]
async fn slow_body_resyncs() {
    let fixture = Fixture::new().await;
    let cursor = {
        let mut data = fixture.backend.data.lock().await;
        let chat = data.chats.get_mut(&fixture.id).unwrap();
        let cursor = chat.cursor();
        for index in 0..3 { chat.emit("turn.started", json!({ "index": index })); }
        cursor
    };
    let response = fixture.response(Some(&cursor)).await;
    assert_eq!(response.status(), 200);
    let mut body = response.into_body().into_data_stream();
    let first = body.next().await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&first).contains("\"index\":0"));
    {
        let mut data = fixture.backend.data.lock().await;
        let chat = data.chats.get_mut(&fixture.id).unwrap();
        for index in 3..600 { chat.emit("turn.started", json!({ "index": index })); }
    }
    let next = body.next().await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&next).contains("event: resync.required"));
    drop(body);
    fixture.close().await;
}

/// A resync marker cannot acknowledge a snapshot the browser has not received.
#[tokio::test]
async fn resync_cursor_waits_for_snapshot() {
    let fixture = Fixture::new().await;
    let cursor = "previous-process:42";
    let mut body = fixture.response(Some(cursor)).await.into_body().into_data_stream();
    let marker = body.next().await.unwrap().unwrap();
    let marker = String::from_utf8_lossy(&marker);
    assert!(marker.contains("event: resync.required"));
    assert!(!marker.lines().any(|line| line.starts_with("id:")));
    drop(body);
    let mut body = fixture.response(Some(cursor)).await.into_body().into_data_stream();
    assert!(String::from_utf8_lossy(&body.next().await.unwrap().unwrap())
        .contains("event: resync.required"));
    {
        let mut data = fixture.backend.data.lock().await;
        let chat = data.chats.get_mut(&fixture.id).unwrap();
        chat.messages.push(json!({ "id": "new", "text": "Latest text" }));
        chat.emit("turn.completed", json!({}));
    }
    let snapshot = body.next().await.unwrap().unwrap();
    let snapshot = String::from_utf8_lossy(&snapshot);
    assert!(snapshot.contains("event: snapshot"));
    assert!(snapshot.contains("Latest text"));
    let cursor = fixture.backend.data.lock().await.chats[&fixture.id].cursor();
    assert!(snapshot.contains(&format!("id: {cursor}\n")));
    drop(body);
    fixture.close().await;
}

/// Revalidates account generation before every snapshot or pending replay event.
#[tokio::test]
async fn account_change_closes_unpolled_body() {
    let fixture = Fixture::new().await;
    for cursor in [None, Some("old:12")] {
        let response = fixture.response(cursor).await;
        let mut body = response.into_body().into_data_stream();
        fixture.backend.data.lock().await.generation += 1;
        assert!(body.next().await.is_none(), "published stale account data");
    }
    fixture.close().await;
}

/// Bounds live streams, releases unpolled bodies, and leaves RPC calls independent.
#[tokio::test]
async fn stream_permits_release() {
    let mut fixture = Fixture::new().await;
    let valid_id = fixture.id.clone();
    fixture.id = "invalid".into();
    for _ in 0..64 { assert_eq!(fixture.response(None).await.status(), 404); }
    fixture.id = valid_id;
    let mut responses = Vec::new();
    for _ in 0..64 {
        let response = fixture.response(None).await;
        assert_eq!(response.status(), 200);
        responses.push(response);
    }
    let rejected = fixture.response(None).await;
    assert_eq!(rejected.status(), 503);
    let bytes = axum::body::to_bytes(rejected.into_body(), 4096).await.unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap()["error"]["code"],
        "stream_limit");
    fixture.backend.data.lock().await.chats.get_mut(&fixture.id).unwrap()
        .active = Some("running-turn".into());
    let stop = Request::builder().method("POST")
        .uri(format!("/api/threads/{}/interrupt", fixture.id))
        .header("host", "127.0.0.1:8787")
        .header("content-type", "application/json")
        .header("x-miniclaw-request", "1")
        .body(Body::from(json!({ "turnId": "running-turn" }).to_string())).unwrap();
    let stopped = tokio::time::timeout(Duration::from_secs(1),
        api::router(fixture.backend.clone()).oneshot(stop)).await.unwrap().unwrap();
    assert_eq!(stopped.status(), 200);
    assert_eq!(fixture.backend.data.lock().await.chats[&fixture.id].active.as_deref(),
        Some("running-turn"));
    drop(responses.pop());
    assert_eq!(fixture.response(None).await.status(), 200);
    drop(responses);
    fixture.close().await;
}

/// Account loss between marker and replacement snapshot ends delivery without data.
#[tokio::test]
async fn resync_rechecks_account() {
    let fixture = Fixture::new().await;
    let mut body = fixture.response(Some("stale:42")).await.into_body().into_data_stream();
    assert!(String::from_utf8_lossy(&body.next().await.unwrap().unwrap())
        .contains("event: resync.required"));
    fixture.backend.data.lock().await.generation += 1;
    assert!(body.next().await.is_none());
    drop(body);
    // Completion releases permits as well as dropping an unpolled response.
    assert_eq!(fixture.backend.streams.available_permits(), 64);
    fixture.close().await;
}

/// Runtime loss between marker and replacement snapshot cannot publish stale work.
#[tokio::test]
async fn resync_rechecks_runtime() {
    let fixture = Fixture::new().await;
    let mut body = fixture.response(Some("stale:42")).await.into_body().into_data_stream();
    assert!(String::from_utf8_lossy(&body.next().await.unwrap().unwrap())
        .contains("event: resync.required"));
    fixture.backend.client.as_ref().unwrap().stop().await;
    assert!(body.next().await.is_none());
    drop(body);
    fixture.close().await;
}

/// A delayed first poll refreshes authorization measured from the handler's read.
#[tokio::test]
async fn delayed_body_refreshes_auth() {
    let fixture = Fixture::new().await;
    let mut body = fixture.response(None).await.into_body().into_data_stream();
    fixture.backend.client.as_ref().unwrap().call("fixture/signout", json!({}))
        .await.unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(16)).await;
    tokio::time::resume();
    // An elapsed keepalive may arrive while the fresh authorization RPC is pending.
    while let Some(frame) = body.next().await {
        assert!(String::from_utf8_lossy(&frame.unwrap()).lines()
            .all(|line| line.is_empty() || line.starts_with(':')),
            "published account data before refreshing expired authorization");
    }
    drop(body);
    fixture.close().await;
}
