// Runs real sandboxed commands against granted paths: Seatbelt on macOS, bubblewrap on Linux.
#![cfg(any(target_os = "macos", target_os = "linux"))]
use miniclaw_server::{scoped::Operation, executor::execute_host, seatbelt::Grant};
// Builds tool arguments and reads results.
use serde_json::{Value, json};
// Creates fixture files, links, and listeners.
use std::{fs, net::TcpListener, os::unix::fs::symlink, path::PathBuf};
// Names each disposable fixture directory.
use uuid::Uuid;

/// Creates a canonical fixture root with a granted folder and an ungranted secret.
fn fixture() -> PathBuf {
    let root = std::env::temp_dir().canonicalize().unwrap()
        .join(format!("mc-seatbelt-{}", Uuid::new_v4()));
    fs::create_dir_all(root.join("granted/sub")).unwrap();
    fs::create_dir_all(root.join("secret")).unwrap();
    fs::write(root.join("granted/ok.txt"), "granted-content").unwrap();
    fs::write(root.join("secret/no.txt"), "secret-content").unwrap();
    symlink(root.join("secret/no.txt"), root.join("granted/link")).unwrap();
    root
}

/// Runs one shell command from the granted folder with the given grants.
async fn shell(root: &PathBuf, text: &str, grants: &[Grant]) -> Value {
    let operation = Operation::parse(&json!({ "action": "run_shell", "text": text,
        "cwd": root.join("granted").to_str().unwrap() })).unwrap();
    execute_host(operation, grants).await
}

/// Grants the fixture's `granted` folder, optionally for writing.
fn folder(root: &PathBuf, write: bool) -> Vec<Grant> {
    vec![Grant { path: root.join("granted").to_str().unwrap().into(), write, folder: true, blocked: false }]
}

#[tokio::test]
async fn granted_folder_is_readable_and_the_rest_is_not() {
    let root = fixture();
    let secret = root.join("secret/no.txt");
    let result = shell(&root, &format!("cat ok.txt; cat {}; cat link; cat ~/.ssh/config; ls {}",
        secret.display(), root.join("secret").display()), &folder(&root, false)).await;
    assert_eq!(result["status"], "ok", "{result}");
    let text = result["text"].as_str().unwrap();
    assert!(text.contains("granted-content"), "{result}");
    assert!(!text.contains("secret-content") && !text.contains("no.txt"), "{result}");
    // Seatbelt refuses the open; on Linux the path is not in the sandbox's view at all.
    let refusal = if cfg!(target_os = "linux") { "No such file or directory" } else { "Operation not permitted" };
    assert!(result["stderr"].as_str().unwrap().contains(refusal), "{result}");
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn writes_need_a_write_grant_and_stay_inside_it() {
    let root = fixture();
    let outside = root.join("secret/new.txt");
    let script = format!("echo inside > sub/new.txt; echo outside > {}", outside.display());
    shell(&root, &script, &folder(&root, false)).await;
    assert!(!root.join("granted/sub/new.txt").exists(), "a read grant allowed a write");
    shell(&root, &script, &folder(&root, true)).await;
    assert_eq!(fs::read_to_string(root.join("granted/sub/new.txt")).unwrap(), "inside\n");
    assert!(!outside.exists(), "a write escaped the granted folder");
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn an_exact_file_grant_does_not_open_its_siblings() {
    let root = fixture();
    fs::write(root.join("granted/other.txt"), "sibling-content").unwrap();
    let grants = vec![Grant { path: root.join("granted/ok.txt").to_str().unwrap().into(),
        write: false, folder: false, blocked: false }];
    let result = shell(&root, "cat ok.txt; cat other.txt", &grants).await;
    let text = result["text"].as_str().unwrap();
    assert!(text.contains("granted-content") && !text.contains("sibling-content"), "{result}");
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn a_blocked_file_inside_a_granted_folder_stays_closed() {
    let root = fixture();
    let mut grants = folder(&root, true);
    grants.push(Grant { path: root.join("granted/ok.txt").to_str().unwrap().into(),
        write: false, folder: false, blocked: true });
    let result = shell(&root, "cat ok.txt; echo changed > ok.txt", &grants).await;
    assert!(!result["text"].as_str().unwrap().contains("granted-content"), "{result}");
    assert_eq!(fs::read_to_string(root.join("granted/ok.txt")).unwrap(), "granted-content");
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn the_network_is_unreachable() {
    let root = fixture();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let result = shell(&root, &format!("/usr/bin/nc -z -w 2 127.0.0.1 {port}; echo code=$?"),
        &folder(&root, true)).await;
    let text = result["text"].as_str().unwrap();
    assert!(text.contains("code=") && !text.contains("code=0"), "{result}");
    assert!(listener.accept().is_err(), "the sandboxed command reached a local port");
    fs::remove_dir_all(root).unwrap();
}

/// Deletes the throwaway Keychain item even when an assertion fails.
#[cfg(target_os = "macos")]
struct KeychainItem(String);
#[cfg(target_os = "macos")]
impl Drop for KeychainItem {
    /// Removes the test's own item and nothing else.
    fn drop(&mut self) {
        let _ = std::process::Command::new("/usr/bin/security")
            .args(["delete-generic-password", "-s", &self.0]).output();
    }
}

/// The profile names no security service, so a command cannot ask the Keychain for a
/// provider's sign-in token. A profile wide enough for a provider CLI to sign in fails
/// this, which is why the tool runs in its own process and not inside the CLI's sandbox.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn the_keychain_is_unreachable() {
    let root = fixture();
    let item = KeychainItem(format!("mc-seatbelt-test-{}", Uuid::new_v4().simple()));
    let secret = format!("secret-{}", Uuid::new_v4().simple());
    let added = std::process::Command::new("/usr/bin/security")
        .args(["add-generic-password", "-s", &item.0, "-a", "miniclaw-test", "-w", &secret])
        .output().unwrap();
    assert!(added.status.success(), "could not create the throwaway item");
    let read = ["find-generic-password", "-s", &item.0, "-w"];
    let outside = std::process::Command::new("/usr/bin/security").args(read).output().unwrap();
    assert!(String::from_utf8_lossy(&outside.stdout).contains(&secret),
        "the item must be readable outside the sandbox for this test to mean anything");
    let result = shell(&root, &format!("/usr/bin/security {}", read.join(" ")),
        &folder(&root, true)).await;
    assert!(!result.to_string().contains(&secret), "the sandboxed command read a Keychain secret");
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn host_execution_refuses_apple_events_and_gui_operations() {
    for value in [
        json!({ "action": "run_applescript", "text": "return 1", "cwd": "/private/tmp" }),
        json!({ "action": "textedit_open", "path": "/private/tmp/a.txt" }),
    ] {
        let result = execute_host(Operation::parse(&value).unwrap(), &[]).await;
        assert_eq!(result["status"], "denied", "{value}");
    }
}

/// Software installs and administrator-level changes need places and privileges the
/// sandbox never grants. Every attempt uses a unique name, and the test removes anything a
/// broken sandbox would have let through.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn software_installs_and_administrator_changes_are_refused() {
    let root = fixture();
    let tag = format!("mc-sandbox-check-{}", Uuid::new_v4().simple());
    let home = std::env::var("HOME").unwrap();
    let targets = [format!("/Applications/{tag}"), format!("/usr/local/{tag}"),
        format!("/Library/LaunchDaemons/{tag}.plist"),
        format!("{home}/Library/LaunchAgents/{tag}.plist"),
        format!("{home}/Library/Preferences/{tag}.plist"), format!("{home}/.zshrc-{tag}")];
    let mut script = String::new();
    for (index, target) in targets.iter().enumerate() {
        script.push_str(&format!("touch '{target}' 2>/dev/null; echo t{index}=$?\n"));
    }
    script.push_str("sudo -n true 2>/dev/null; echo sudo=$?\n");
    script.push_str(&format!("defaults write {tag} key value 2>/dev/null; echo defaults=$?\n"));
    script.push_str("launchctl bootstrap gui/$(id -u) /dev/null 2>/dev/null; echo launchctl=$?\n");
    let result = shell(&root, &script, &folder(&root, true)).await;
    let leaked: Vec<&String> = targets.iter().filter(|path| std::path::Path::new(path).exists()).collect();
    for path in &leaked { let _ = fs::remove_file(path); }
    let _ = std::process::Command::new("/usr/bin/defaults").args(["delete", &tag]).output();
    assert!(leaked.is_empty(), "the sandbox let these be created: {leaked:?}");
    assert_eq!(result["status"], "ok", "{result}");
    let text = result["text"].as_str().unwrap();
    for line in text.lines() {
        let (name, code) = line.split_once('=').unwrap();
        assert_ne!(code, "0", "{name} succeeded inside the sandbox: {result}");
    }
    assert_eq!(text.lines().count(), targets.len() + 3, "{result}");
    fs::remove_dir_all(root).unwrap();
}

/// MiniClaw's own state and the providers' sign-in files stay closed even when the user
/// granted a folder that contains them.
#[tokio::test]
async fn miniclaw_state_and_provider_sign_ins_stay_closed_inside_a_granted_folder() {
    let root = fixture();
    let state = root.join("granted/state");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("agents.sqlite3"), "backend-state").unwrap();
    // The executor learns the data folder the way the backend does.
    unsafe { std::env::set_var("MINICLAW_DATA_DIR", &state); }
    let grants = folder(&root, true);
    let result = shell(&root, "cat state/agents.sqlite3; echo changed > state/agents.sqlite3; \
        echo new > state/planted; echo done", &grants).await;
    assert!(!result["text"].as_str().unwrap().contains("backend-state"), "{result}");
    assert_eq!(fs::read_to_string(state.join("agents.sqlite3")).unwrap(), "backend-state");
    assert!(!state.join("planted").exists());
    let file = state.join("agents.sqlite3").to_str().unwrap().to_owned();
    let read = execute_host(Operation::parse(&json!({ "action": "read_file", "path": file })).unwrap(),
        &grants).await;
    assert_eq!(read["status"], "denied", "{read}");
    let write = execute_host(Operation::parse(&json!({ "action": "write_file", "path": file,
        "text": "changed" })).unwrap(), &grants).await;
    assert_eq!(write["status"], "denied", "{write}");
    assert_eq!(fs::read_to_string(state.join("agents.sqlite3")).unwrap(), "backend-state");
    // A provider's sign-in folder under a granted home folder: only a yes or no leaves the
    // sandbox, never the file's content.
    let home = std::env::var("HOME").unwrap();
    let whole_home = vec![Grant { path: home.clone(), write: false, folder: true, blocked: false }];
    let probe = shell(&root, "for f in ~/.codex/auth.json ~/.claude.json ~/.claude/settings.json; do \
        [ -e \"$f\" ] && head -c1 \"$f\" >/dev/null 2>&1 && echo readable; done; echo checked",
        &whole_home).await;
    assert_eq!(probe["text"].as_str().unwrap().trim(), "checked", "{probe}");
    fs::remove_dir_all(root).unwrap();
}

/// With approved hosts a command gets one way out: a local proxy that opens HTTPS
/// connections to those hosts only. Everything else, including a direct connection to the
/// same server, stays refused.
#[tokio::test]
async fn a_command_reaches_only_its_approved_hosts_and_only_through_the_proxy() {
    use std::io::{Read, Write};
    let root = fixture();
    let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = upstream.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in upstream.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut head = [0u8; 1024];
            let _ = stream.read(&mut head);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello");
        }
    });
    // The test stands in for DNS: the approved name leads to the local server.
    let resolve: miniclaw_server::egress::Resolver = std::sync::Arc::new(move |host: String|
        Box::pin(async move { if host == "allowed.test" { vec![address] } else { Vec::new() } }));
    let script = format!("curl -s -p -x \"$HTTPS_PROXY\" http://allowed.test:443/; echo; \
        curl -s -o /dev/null -w 'other=%{{http_connect}}\\n' -p -x \"$HTTPS_PROXY\" http://other.test:443/; \
        curl -s -o /dev/null -w 'port=%{{http_connect}}\\n' -p -x \"$HTTPS_PROXY\" http://allowed.test:8443/; \
        curl -s -m 2 --noproxy '*' -o /dev/null http://127.0.0.1:{}/ || echo direct-refused", address.port());
    let operation = Operation::parse(&json!({ "action": "run_shell", "text": script,
        "cwd": root.join("granted").to_str().unwrap(), "hosts": ["allowed.test"] })).unwrap();
    let result = miniclaw_server::executor::execute_host_resolving(operation, &folder(&root, false),
        resolve).await;
    assert_eq!(result["status"], "ok", "{result}");
    let text = result["text"].as_str().unwrap();
    assert!(text.contains("hello"), "{result}");
    assert!(text.contains("other=403") && text.contains("port=403"), "{result}");
    assert!(text.contains("direct-refused"), "{result}");
    assert_eq!(result["refusedHosts"], json!(["other.test", "allowed.test:8443"]), "{result}");
    fs::remove_dir_all(root).unwrap();
}

/// Without approved hosts nothing listens, and no proxy variable is set.
#[tokio::test]
async fn a_command_without_hosts_has_no_proxy() {
    let root = fixture();
    let result = shell(&root, "echo \"proxy=[$HTTPS_PROXY]\"", &folder(&root, false)).await;
    assert_eq!(result["text"].as_str().unwrap().trim(), "proxy=[]", "{result}");
    assert!(result.get("refusedHosts").is_none(), "{result}");
    fs::remove_dir_all(root).unwrap();
}

/// The real thing, against the internet: an approved host is reached over HTTPS through the
/// proxy with the system resolver, and a host that was not approved is refused. Opt-in,
/// because it needs a network connection: MINICLAW_REAL_NETWORK=1.
#[tokio::test]
async fn a_real_download_works_for_an_approved_host_only() {
    if std::env::var("MINICLAW_REAL_NETWORK").as_deref() != Ok("1") { return; }
    let root = fixture();
    let script = "curl -s -o npm.json -w 'npm=%{http_code}\\n' https://registry.npmjs.org/left-pad/latest; \
        wc -c < npm.json | tr -d ' '; \
        curl -s -o /dev/null -w 'other=%{http_code} connect=%{http_connect}\\n' https://example.com/; \
        curl -s -m 5 --noproxy '*' -o /dev/null -w 'direct=%{http_code}\\n' https://registry.npmjs.org/ || true";
    let operation = Operation::parse(&json!({ "action": "run_shell", "text": script,
        "cwd": root.join("granted").to_str().unwrap(), "hosts": ["registry.npmjs.org"] })).unwrap();
    let result = execute_host(operation, &folder(&root, true)).await;
    assert_eq!(result["status"], "ok", "{result}");
    let text = result["text"].as_str().unwrap();
    assert!(text.contains("npm=200"), "{result}");
    assert!(text.contains("connect=403"), "{result}");
    assert!(text.contains("direct=000"), "a direct connection must fail: {result}");
    assert_eq!(result["refusedHosts"], json!(["example.com"]), "{result}");
    let saved = fs::read_to_string(root.join("granted/npm.json")).unwrap();
    assert!(saved.contains("\"name\":\"left-pad\""), "the download is real package metadata");
    fs::remove_dir_all(root).unwrap();
}

/// Linux keeps credentials behind the session bus (Secret Service, the keyring daemon) and in
/// ~/.local/share/keyrings. Neither the bus socket nor the keyring folder is in the sandbox,
/// even with the whole home folder granted.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_session_bus_and_keyrings_are_unreachable() {
    let root = fixture();
    let home = std::env::var("HOME").unwrap();
    let uid = rustix::process::getuid().as_raw();
    let whole_home = vec![Grant { path: home.clone(), write: false, folder: true, blocked: false }];
    let probe = shell(&root, &format!("for f in /run/user/{uid}/bus /run/dbus/system_bus_socket {home}/.local/share/keyrings/login.keyring; do \
        [ -e \"$f\" ] && echo \"present $f\"; done; ls {home}/.local/share/keyrings 2>/dev/null; echo checked"), &whole_home).await;
    assert_eq!(probe["text"].as_str().unwrap().trim(), "checked", "{probe}");
    fs::remove_dir_all(root).unwrap();
}

/// The Linux counterpart of the macOS install check: system folders, autostart entries, user
/// services, shell start-up files and privilege escalation are all out of reach.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn software_installs_and_administrator_changes_are_refused() {
    let root = fixture();
    let tag = format!("mc-sandbox-check-{}", Uuid::new_v4().simple());
    let home = std::env::var("HOME").unwrap();
    let targets = [format!("/usr/local/{tag}"), format!("/etc/{tag}"), format!("/opt/{tag}"),
        format!("{home}/.config/autostart/{tag}.desktop"),
        format!("{home}/.config/systemd/user/{tag}.service"), format!("{home}/.bashrc-{tag}")];
    let mut script = String::new();
    for (index, target) in targets.iter().enumerate() {
        script.push_str(&format!("mkdir -p \"$(dirname '{target}')\" 2>/dev/null; touch '{target}' 2>/dev/null; echo t{index}=$?\n"));
    }
    script.push_str("sudo -n true 2>/dev/null; echo sudo=$?\n");
    script.push_str("systemctl --user daemon-reload 2>/dev/null; echo systemctl=$?\n");
    script.push_str("unshare -U true 2>/dev/null; echo unshare=$?\n");
    let whole_home = vec![Grant { path: home.clone(), write: false, folder: true, blocked: false }];
    let result = shell(&root, &script, &whole_home).await;
    let leaked: Vec<&String> = targets.iter().filter(|path| std::path::Path::new(path).exists()).collect();
    for path in &leaked { let _ = fs::remove_file(path); }
    assert!(leaked.is_empty(), "the sandbox let these be created: {leaked:?}");
    assert_eq!(result["status"], "ok", "{result}");
    let text = result["text"].as_str().unwrap();
    for line in text.lines() {
        let (name, code) = line.split_once('=').unwrap();
        assert_ne!(code, "0", "{name} succeeded inside the sandbox: {result}");
    }
    assert_eq!(text.lines().count(), targets.len() + 3, "{result}");
    fs::remove_dir_all(root).unwrap();
}

/// A command that outlives its time limit is stopped with everything it started: nothing of it
/// is left running on the host.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_timed_out_command_leaves_nothing_running() {
    let root = fixture();
    let marker = format!("{}.{}", 600 + (Uuid::new_v4().as_u128() % 300) as u32, 7);
    let result = shell(&root, &format!("sleep {marker} & sleep {marker}"), &folder(&root, false)).await;
    assert_eq!(result["status"], "denied", "{result}");
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let found = std::process::Command::new("/usr/bin/pgrep").args(["-f", &format!("sleep {marker}")])
        .output().unwrap();
    assert!(found.stdout.is_empty(), "left running: {}", String::from_utf8_lossy(&found.stdout));
    fs::remove_dir_all(root).unwrap();
}
