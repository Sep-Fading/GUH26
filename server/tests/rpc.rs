use miniclaw_server::rpc::Client;
use serde_json::json;
use tokio::process::Command;

fn peer() -> Command {
    let mut command = Command::new("node");
    command.args([
        "--input-type=module",
        "-e",
        r#"
        import { createInterface } from 'node:readline';
        let initialized = false;
        let first;
        const send = value => console.log(JSON.stringify(value));
        createInterface({ input: process.stdin }).on('line', line => {
            const msg = JSON.parse(line);
            if (msg.method === 'initialize') {
                send({ id: msg.id, result: {} });
            } else if (msg.method === 'initialized') {
                initialized = true;
            } else if (msg.method === 'exit') {
                process.exit(0);
            } else if (msg.method === 'hold') {
                // Deliberately no reply.
            } else if (msg.method === 'first') {
                first = msg;
            } else if (msg.method === 'second') {
                send({ id: msg.id, result: msg.params });
                send({ id: first.id, result: first.params });
            } else {
                send({ id: msg.id, result: { initialized } });
            }
        });
    "#,
    ]);
    command
}

#[tokio::test]
async fn initializes_once() {
    let client = Client::spawn(peer()).await.unwrap();
    let reply = client.call("check", json!({})).await.unwrap();
    assert_eq!(reply, json!({ "initialized": true }));
    client.stop().await;
}

#[tokio::test]
async fn correlates_replies() {
    let client = Client::spawn(peer()).await.unwrap();
    let (first, second) = tokio::join!(
        client.call("first", json!({ "value": 1 })),
        client.call("second", json!({ "value": 2 })),
    );
    assert_eq!(first.unwrap(), json!({ "value": 1 }));
    assert_eq!(second.unwrap(), json!({ "value": 2 }));
    client.stop().await;
}

#[tokio::test]
async fn fails_on_exit() {
    let client = Client::spawn(peer()).await.unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.call("exit", json!({})),
    )
    .await
    .unwrap();
    assert!(result.is_err());
    client.stop().await;
}

