#![cfg(unix)]

// Exercises the real authenticated worker transport.
use miniclaw_server::worker;
// Builds requests without exposing credential payloads.
use serde_json::{Value, json};
// Owns private disposable runtime test storage.
use std::{os::unix::fs::PermissionsExt, path::PathBuf, time::Duration};
// Uses kernel sockets and owned processes.
use tokio::{net::UnixStream, process::Command};
// Names disposable directories without sharing state.
use uuid::Uuid;

struct Home(PathBuf);

impl Drop for Home {
    /// Removes only the temporary home owned by this test.
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Verifies Codex over kernel sockets without importing credentials or requesting inference.
#[tokio::test]
#[ignore = "requires MINICLAW_TEST_CODEX pointing to an installed Codex binary"]
async fn real_app_server() {
    let binary = std::env::var("MINICLAW_TEST_CODEX").expect("MINICLAW_TEST_CODEX");
    assert!(PathBuf::from(&binary).is_absolute());
    let home = Home(std::env::temp_dir().join(format!("mc-worker-{}", Uuid::new_v4())));
    std::fs::create_dir(&home.0).unwrap();
    std::fs::set_permissions(&home.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = home.0.clone();
    let (controller, guest) = UnixStream::pair().unwrap();
    let serving = tokio::spawn(worker::serve(guest, [83; 32], move || {
        let mut command = Command::new(binary);
        command
            .env_clear()
            .env("HOME", &path)
            .env("CODEX_HOME", &path)
            .env("PATH", "/usr/bin:/bin")
            .env("TMPDIR", &path)
            .current_dir(&path)
            .args([
                "app-server",
                "--listen",
                "stdio://",
                "-c",
                "cli_auth_credentials_store=\"file\"",
                "-c",
                "forced_login_method=\"chatgpt\"",
            ]);
        let policy: serde_json::Map<String, Value> =
            serde_json::from_str(include_str!("../chat-policy.json")).unwrap();
        for (key, value) in policy {
            command.arg("-c").arg(format!("{key}={value}"));
        }
        command
    }));
    let client = worker::connect(controller, [83; 32]).await.unwrap();
    let account = client
        .call("account/read", json!({ "refreshToken": false }))
        .await;
    let config = client
        .call("config/read", json!({ "includeLayers": false }))
        .await;
    client.stop().await;
    tokio::time::timeout(Duration::from_secs(5), serving)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        account.unwrap()["account"].is_null(),
        "temporary home must be signed out"
    );
    assert!(config.unwrap()["config"].is_object());
    assert!(!home.0.join("auth.json").exists());
}
