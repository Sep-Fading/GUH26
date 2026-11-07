// Checks how the backend treats its client's lifetime.
use miniclaw_server::rpc::Client;
// Builds fixture requests.
use serde_json::json;
// Creates isolated stores and bounds each wait.
use std::{fs, time::Duration};
// Runs the fake protocol peer.
use tokio::process::Command;
// Names private disposable fixture directories.
use uuid::Uuid;

/// Creates an isolated protocol peer with no credential or file access.
async fn peer() -> Client {
    let mut command = Command::new("node");
    command.args(["--input-type=module", "-e", r#"
        import { createInterface } from 'node:readline';
        createInterface({ input: process.stdin }).on('line', line => {
            const msg = JSON.parse(line);
            if (msg.method === 'fixture/exit') process.exit(0);
            if (msg.id !== undefined) console.log(JSON.stringify({ id: msg.id, result: {} }));
        });
    "#]);
    Client::spawn(command).await.unwrap()
}

/// A late backend listener drains an already-closed transport without hanging.
#[tokio::test]
async fn drains_late_listener() {
    let dir = std::env::temp_dir().join(format!("mc-notice-late-{}", Uuid::new_v4()));
    fs::create_dir(&dir).unwrap();
    let store = miniclaw_server::agents::Store::open(&dir.join("agents.sqlite3")).unwrap();
    let client = peer().await;
    client.call("fixture/exit", json!({})).await.unwrap_err();
    client.closed().await;
    let backend = miniclaw_server::chat::Backend::new(Some(client.clone()),
        dir.to_string_lossy().into(), "http://127.0.0.1:8787".into(),
        dir.join("recovery"), store, None, true);
    tokio::time::timeout(Duration::from_secs(1), client.drain_events()).await.unwrap();
    drop(backend);
    fs::remove_dir_all(dir).unwrap();
}

/// Dropping the final backend and client still shuts down its owned idle child.
#[tokio::test]
async fn dropping_backend_stops_child() {
    let dir = std::env::temp_dir().join(format!("mc-notice-drop-{}", Uuid::new_v4()));
    fs::create_dir(&dir).unwrap();
    let store = miniclaw_server::agents::Store::open(&dir.join("agents.sqlite3")).unwrap();
    let client = peer().await;
    let mut events = client.subscribe();
    let backend = miniclaw_server::chat::Backend::new(Some(client.clone()),
        dir.to_string_lossy().into(), "http://127.0.0.1:8787".into(),
        dir.join("recovery"), store, None, true);
    drop(backend);
    drop(client);
    let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
        .await.expect("notification observer retained the idle child").unwrap();
    assert_eq!(event.value()["method"], "miniclaw/disconnected");
    fs::remove_dir_all(dir).unwrap();
}
