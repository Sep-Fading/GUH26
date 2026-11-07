#![cfg(windows)]
// Native acceptance of npm and npx in the production sandbox: MiniClaw's npm beside the private
// Node, `npm`/`npx` in the sandboxed shell, and MCP servers saved as `npx -y <package>`, which
// MiniClaw installs with its own npm and runs with its own Node, never through npx.cmd.
// Needs the installed Windows network service, the private Node with npm staged beside the test
// binary, MINICLAW_DATA_DIR, and internet access to registry.npmjs.org for the networked cases.
use miniclaw_server::{mcp_client::Link, mcp_servers::Server, npm_launch, platform_fs, scoped::Operation, secrets::Secrets, seatbelt::Grant,
    windows_runtimes, windows_sandbox::Sandbox};
// Builds servers and reads results.
use serde_json::{Value, json};
// Builds disposable folders and measures starts.
use std::{fs, path::{Path, PathBuf}, time::{Duration, Instant}};

/// The registry every networked case approves.
const REGISTRY: &str = "registry.npmjs.org";

/// A private working folder for one test.
fn workspace(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("mc-npm-{label}-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&path, false).unwrap();
    path
}

/// Runs one command in the production sandboxed shell with read-write access to `work`.
async fn shell(text: &str, work: &Path, hosts: &[&str]) -> Value {
    let grants = vec![Grant { path: work.to_string_lossy().into_owned(), write: true, folder: true, blocked: false }];
    miniclaw_server::executor::execute_host(Operation::RunShell { text: text.to_owned(), cwd: work.to_string_lossy().into_owned(),
        hosts: hosts.iter().map(|host| (*host).to_owned()).collect() }, &grants).await
}

/// Lets finished runs' cleanup release their files.
fn drain() { miniclaw_server::windows_sandbox::drain_cleanup(Duration::from_secs(60)).unwrap(); }

/// A fake `.npmrc` in the real profile of the test account, removed again when dropped. It names
/// an unreachable registry and a made-up token: if npm in the sandbox read it, installs would fail.
struct UserNpmrc { path: PathBuf, created: bool }

impl UserNpmrc {
    /// Writes the fake file only when the account has none, so a real one is never touched.
    fn plant() -> Self {
        let path = PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(".npmrc");
        let created = !path.exists();
        if created {
            fs::write(&path, "registry=https://registry.miniclaw-planted.invalid/\n//registry.npmjs.org/:_authToken=MINICLAW-FAKE-TOKEN-0000\n").unwrap();
        }
        println!("USER NPMRC {} planted={created}", path.display());
        Self { path, created }
    }
}

impl Drop for UserNpmrc {
    /// Removes only the file this test wrote.
    fn drop(&mut self) { if self.created { let _ = fs::remove_file(&self.path); } }
}

/// What exists of npm's default per-user folders outside the sandbox, to show nothing wrote there.
fn user_npm_state() -> Vec<(PathBuf, Option<std::time::SystemTime>)> {
    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap());
    let roaming = PathBuf::from(std::env::var_os("APPDATA").unwrap());
    [local.join("npm-cache"), roaming.join("npm"), roaming.join("npm-cache")].into_iter()
        .map(|path| { let modified = fs::metadata(&path).and_then(|meta| meta.modified()).ok(); (path, modified) }).collect()
}

#[test]
#[ignore = "needs the private Node and npm staged beside the test binary"]
/// MiniClaw's npm is the pinned tree with the two launchers and BusyBox as sh.exe; any change is refused.
fn private_npm_is_verified_and_a_changed_one_is_refused() {
    let started = Instant::now();
    let npm = windows_runtimes::private_npm().expect("the private npm beside the test binary is verified");
    println!("PRIVATE NPM {} cli={} shell={} checked in {:?}", npm.root.display(), npm.cli.display(), npm.shell.display(), started.elapsed());
    let node = windows_runtimes::private_node().unwrap();
    let copy = workspace("tamper");
    for name in ["npm", "npx", "npm-sandbox.cjs", windows_runtimes::SCRIPT_SHELL] { fs::copy(node.folder.join(name), copy.join(name)).unwrap(); }
    let tree = |from: &Path, to: &Path| {
        let mut stack = vec![(from.to_path_buf(), to.to_path_buf())];
        while let Some((from, to)) = stack.pop() {
            fs::create_dir_all(&to).unwrap();
            for entry in fs::read_dir(&from).unwrap().flatten() {
                if entry.file_type().unwrap().is_dir() { stack.push((entry.path(), to.join(entry.file_name()))); }
                else { fs::copy(entry.path(), to.join(entry.file_name())).unwrap(); }
            }
        }
    };
    tree(&npm.root, &copy.join(r"node_modules\npm"));
    assert!(windows_runtimes::load_private_npm(&copy).is_ok(), "an identical copy is accepted");
    let changed = copy.join(r"node_modules\npm\lib\cli.js");
    let original = fs::read(&changed).unwrap();
    fs::write(&changed, [original.as_slice(), b"\n// changed"].concat()).unwrap();
    assert!(windows_runtimes::load_private_npm(&copy).is_err(), "a changed npm file is refused");
    fs::write(&changed, &original).unwrap();
    fs::write(copy.join(r"node_modules\npm\extra.js"), "module.exports = 1;\n").unwrap();
    assert!(windows_runtimes::load_private_npm(&copy).is_err(), "an added npm file is refused");
    fs::remove_file(copy.join(r"node_modules\npm\extra.js")).unwrap();
    fs::write(copy.join("npx"), "#!/bin/sh\nexec cmd.exe /c npx.cmd \"$@\"\n").unwrap();
    assert!(windows_runtimes::load_private_npm(&copy).is_err(), "a changed launcher is refused");
    fs::copy(node.folder.join("npx"), copy.join("npx")).unwrap();
    fs::copy(node.program.clone(), copy.join(windows_runtimes::SCRIPT_SHELL)).unwrap();
    assert!(windows_runtimes::load_private_npm(&copy).is_err(), "sh.exe must be the pinned shell");
    fs::remove_dir_all(copy).unwrap();
}

#[test]
#[ignore = "needs the installed Windows network service and the private npm"]
/// What a sandboxed shell pays for npm: the extra grant of npm's tree on top of a shell run.
fn npm_grant_cost() {
    let work = workspace("cost");
    let grants = vec![Grant { path: work.to_string_lossy().into_owned(), write: true, folder: true, blocked: false }];
    for round in 0..3 {
        let started = Instant::now();
        let mut sandbox = Sandbox::new_shell(&grants, &work).unwrap();
        let shell = started.elapsed();
        let started = Instant::now();
        assert!(sandbox.add_private_npm().unwrap().is_some());
        let npm = started.elapsed();
        drop(sandbox);
        let started = Instant::now();
        drain();
        println!("GRANT COST round={round} shell={shell:?} npm_extra={npm:?} cleanup_drain={:?}", started.elapsed());
    }
    fs::remove_dir_all(work).unwrap();
}

#[tokio::test]
#[ignore = "needs the installed Windows network service, the private npm and access to registry.npmjs.org"]
/// `npm` and `npx` in the sandboxed shell are MiniClaw's: npm's version, `npm init`, an install
/// from the registry through the proxy, a package script run by BusyBox `sh`, and `npx`. The
/// user's `.npmrc` is not read and npm writes nothing to the user's own npm folders.
async fn shell_npm_and_npx_work_in_a_workspace() {
    let npmrc = UserNpmrc::plant();
    let before = user_npm_state();
    let work = workspace("shell");
    // A local package with an install script that only a POSIX shell can run (`test`, `$VAR`).
    let local = work.join("localpkg");
    fs::create_dir(&local).unwrap();
    fs::write(local.join("package.json"), r#"{"name":"localpkg","version":"1.0.0","scripts":{"postinstall":"test -n \"$npm_lifecycle_event\" && echo \"sh:$npm_lifecycle_event\" > ../../postinstall-ran.txt"}}"#).unwrap();
    fs::write(local.join("index.js"), "module.exports = 'local';\n").unwrap();
    let user_npmrc = npmrc.path.to_string_lossy().replace('\\', "/");
    // One shell run per step, each bounded below the shell's own 120 seconds, so a step that
    // fails or hangs is named in the log with its own output.
    let steps = [
        ("which", "echo \"WHICH $(command -v npm) $(command -v npx)\"; echo \"NPM $(npm --version)\"".to_owned(), false),
        ("config", format!("echo \"USERCONFIG $(npm config get userconfig)\"; echo \"REGISTRY $(npm config get registry)\"; \
            npm config list | grep -c MINICLAW-FAKE; cat '{user_npmrc}' >/dev/null 2>&1 && echo 'USER NPMRC READABLE' || echo 'USER NPMRC DENIED'"), false),
        ("init", "npm init -y >/dev/null && echo INIT ok".to_owned(), false),
        ("ping", "timeout 100 npm ping --loglevel=http 2>&1 | tail -n 5; echo PING $?".to_owned(), true),
        ("install", "timeout 100 npm install is-number@7.0.0 --no-audit --no-fund --loglevel=http 2>&1 | tail -n 8; \
            echo \"REQUIRE $(node -e \"console.log(require('is-number')(5))\")\"".to_owned(), true),
        // How Node starts a child here. With its own standard handles it works (the script shell
        // primed outside first); with pipes, Node 24.21's libuv 1.52.1 would name the pipe outside
        // the AppContainer's namespace and retry forever, so MiniClaw's preload refuses it at once
        // (cut off after 15 seconds should that ever hang again).
        ("spawn", "node -e \"const r=require('child_process').spawnSync(process.env.npm_config_script_shell,['-c','echo SPAWN-INHERIT ok'],{stdio:'inherit'});if(r.error)console.log('SPAWN-INHERIT',r.error.code)\"; \
            timeout 15 node -e \"const r=require('child_process').spawnSync(process.env.npm_config_script_shell,['-c','echo hi'],{encoding:'utf8',timeout:5000});console.log('PIPED',r.status,r.stdout,r.error?.code)\"; \
            echo \"SPAWN-PIPED exit $?\"".to_owned(), false),
        ("script", "cd localpkg && npm pack --silent >/dev/null && cd .. && timeout 60 npm install ./localpkg/localpkg-1.0.0.tgz --no-audit --no-fund --loglevel=silly 2>&1 | tail -n 25; \
            echo \"POSTINSTALL $(cat postinstall-ran.txt)\"".to_owned(), false),
        // A real package with a native binary. Its install script runs `esbuild --version` through a
        // pipe, which is refused (above), so scripts are skipped; its command starts the binary with
        // inherited handles. Informational: printed, not asserted.
        ("esbuild", "timeout 100 npm install esbuild@0.25 --ignore-scripts --no-audit --no-fund 2>&1 | tail -n 3; echo \"ESBUILD $(timeout 30 npx esbuild --version 2>&1 | tail -n 2)\"".to_owned(), true),
        ("npx", "echo \"NPX $(timeout 60 npx --yes semver@7.6.3 1.2.3 2>&1 | tail -n 5)\"; for log in \"$LOCALAPPDATA\"/npm-cache/_logs/*.log; do tail -n 30 \"$log\"; done".to_owned(), true),
    ];
    let mut text = String::new();
    for (label, command, networked) in steps {
        let started = Instant::now();
        let result = shell(&command, &work, if networked { &[REGISTRY] } else { &[] }).await;
        println!("STEP {label} ({:?}) {result}", started.elapsed());
        text.push_str(result["text"].as_str().unwrap_or_default());
        text.push_str(result["stderr"].as_str().unwrap_or_default());
    }
    let node = windows_runtimes::private_node().unwrap();
    let folder = node.folder.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
    assert!(text.lines().any(|line| line.starts_with("WHICH ") && line.to_ascii_lowercase().matches(&folder).count() == 2), "{text}");
    assert!(text.contains(&format!("NPM {}", windows_runtimes::NPM_VERSION)), "{text}");
    assert!(text.contains("REGISTRY https://registry.npmjs.org/"), "the user's .npmrc changed the registry: {text}");
    assert!(text.contains("USER NPMRC DENIED") && !text.contains("MINICLAW-FAKE-TOKEN"), "{text}");
    assert!(text.lines().any(|line| line.starts_with("USERCONFIG ") && line.contains("sandbox-scratch")), "{text}");
    assert!(text.contains("INIT ok") && text.contains("REQUIRE true"), "{text}");
    assert!(text.contains("SPAWN-INHERIT ok"), "{text}");
    if text.contains("PIPED 0 hi") { println!("INFO a piped child now starts in the sandbox: the libuv limitation is gone"); }
    else {
        assert!(text.contains(&format!("PIPED null null {}", windows_runtimes::CHILD_PIPES_CODE)) && text.contains("SPAWN-PIPED exit 0"),
            "the piped probe was neither refused at once nor worked: {text}");
    }
    println!("INFO esbuild: {}", text.lines().find(|line| line.starts_with("ESBUILD")).unwrap_or("no line"));
    assert!(text.contains("POSTINSTALL sh:postinstall"), "{text}");
    assert!(text.contains("NPX 1.2.3"), "{text}");
    // Another host stays closed while the registry is approved.
    let other = shell("node -e \"fetch('https://example.com/',{signal:AbortSignal.timeout(15000)}).then(r=>console.log('STATUS',r.status),e=>console.log('REFUSED',e.cause?.code??e.message))\"", &work, &[REGISTRY]).await;
    println!("OTHER HOST {other}");
    assert!(other["text"].as_str().unwrap_or_default().contains("REFUSED"), "{other}");
    // Offline, npm reports the missing network quickly instead of hanging.
    let started = Instant::now();
    let offline = shell("npm view is-number version --fetch-retries=0 --fetch-timeout=10000; echo EXIT $?", &work, &[]).await;
    println!("SHELL NPM OFFLINE ({:?}) {offline}", started.elapsed());
    assert!(offline["text"].as_str().unwrap_or_default().contains("EXIT 1"), "{offline}");
    assert!(started.elapsed() < Duration::from_secs(60));
    drain();
    assert_eq!(user_npm_state(), before, "npm wrote to the user's own npm folders");
    drop(npmrc);
    let _ = fs::remove_dir_all(work);
}

/// A saved server started as `npx <args>` with these approved hosts.
fn npx_server(id: &str, args: &[&str], hosts: &[&str]) -> (Server, Secrets) {
    let server: Server = serde_json::from_value(json!({"id": id, "name": "npx fixture", "kind": "stdio", "command": "npx",
        "args": args, "hosts": hosts, "startupSeconds": 120, "toolSeconds": 60})).unwrap();
    (server, Secrets::File(std::env::temp_dir().join(format!("mc-npx-secrets-{}", uuid::Uuid::new_v4()))))
}

/// The server's private npm folder.
fn server_folder(id: &str) -> PathBuf {
    npm_launch::server_folder(&PathBuf::from(std::env::var_os("MINICLAW_DATA_DIR").expect("MINICLAW_DATA_DIR")), id).unwrap()
}

/// Lists the files under a folder, for showing that nothing else was written.
fn files_under(root: &Path) -> usize {
    fs::read_dir(root).into_iter().flatten().flatten().map(|entry| if entry.path().is_dir() { files_under(&entry.path()) } else { 1 }).sum()
}

#[tokio::test]
#[ignore = "needs the installed Windows network service, the private npm and access to registry.npmjs.org"]
/// The official sequential-thinking server, saved as `npx -y @modelcontextprotocol/server-sequential-thinking`,
/// is installed from the real registry through the server's proxy, lists its tools and answers a
/// call. A second start reuses the install, also with no approved host. A fresh server with no
/// approved host, or with a registry that cannot be reached, fails quickly and says why.
async fn npx_server_installs_from_the_registry_and_starts_offline_afterwards() {
    let npmrc = UserNpmrc::plant();
    let before = user_npm_state();
    let id = format!("npx-{}", uuid::Uuid::new_v4().simple());
    let package = "@modelcontextprotocol/server-sequential-thinking";
    let (server, secrets) = npx_server(&id, &["-y", package], &[REGISTRY]);
    println!("RUNS {}", npm_launch::describe("npx", &server.args).unwrap());
    let started = Instant::now();
    let mut link = Link::connect(&server, &secrets).await.unwrap_or_else(|fault| panic!("the npx server installs and starts: {}", fault.0));
    let cold = started.elapsed();
    let installed: Value = serde_json::from_str(&fs::read_to_string(server_folder(&id).join(r"pkg\node_modules\@modelcontextprotocol\server-sequential-thinking\package.json")).unwrap()).unwrap();
    println!("COLD START {cold:?} server={} installed={} files={}", link.info, installed["version"], files_under(&server_folder(&id)));
    let tools = link.tools().await.unwrap();
    assert!(tools.iter().any(|tool| tool["name"] == "sequentialthinking"), "{tools:?}");
    let answer = link.call("sequentialthinking", json!({"thought": "a & b | %PATH% ^ \"q\"", "nextThoughtNeeded": false, "thoughtNumber": 1, "totalThoughts": 1}),
        Duration::from_secs(30)).await.unwrap();
    println!("CALL {answer}");
    assert_ne!(answer["isError"], true, "{answer}");
    assert!(answer["content"][0]["text"].as_str().is_some_and(|text| text.contains("thoughtNumber")), "{answer}");
    drop(link);
    drain();
    let started = Instant::now();
    let mut warm = Link::connect(&server, &secrets).await.unwrap_or_else(|fault| panic!("the installed server starts again: {}", fault.0));
    println!("WARM START {:?}", started.elapsed());
    assert!(!warm.tools().await.unwrap().is_empty());
    drop(warm);
    drain();
    let (offline, _) = npx_server(&id, &["-y", package], &[]);
    let started = Instant::now();
    let mut link = Link::connect(&offline, &secrets).await.unwrap_or_else(|fault| panic!("the installed server starts without network: {}", fault.0));
    println!("OFFLINE WARM START {:?}", started.elapsed());
    assert!(!link.tools().await.unwrap().is_empty());
    drop(link);
    drain();
    // A fresh server without the registry approved is refused at once, naming the host to approve.
    let fresh = format!("npx-{}", uuid::Uuid::new_v4().simple());
    let (cold_offline, _) = npx_server(&fresh, &["-y", package], &[]);
    let started = Instant::now();
    let fault = Link::connect(&cold_offline, &secrets).await.err().expect("a cold server without network does not start");
    println!("COLD OFFLINE ({:?}) {}", started.elapsed(), fault.0);
    assert!(fault.0.contains(REGISTRY) && started.elapsed() < Duration::from_secs(5), "{}", fault.0);
    // A registry that cannot be reached fails within npm's short retries, with npm's reason.
    let dead = format!("npx-{}", uuid::Uuid::new_v4().simple());
    let (unreachable, _) = npx_server(&dead, &["--registry=https://registry.miniclaw-offline.invalid/", "-y", package], &["registry.miniclaw-offline.invalid"]);
    let started = Instant::now();
    let fault = Link::connect(&unreachable, &secrets).await.err().expect("an unreachable registry does not install");
    println!("DEAD REGISTRY ({:?}) {}", started.elapsed(), fault.0);
    assert!(fault.0.contains("could not install") && started.elapsed() < Duration::from_secs(90), "{}", fault.0);
    drain();
    assert_eq!(user_npm_state(), before, "npm wrote to the user's own npm folders");
    drop(npmrc);
    for id in [&id, &fresh, &dead] { let _ = fs::remove_dir_all(server_folder(id)); }
}

/// A stdio MCP server in plain Node that reports how it was started: its arguments, its Node,
/// and whether it can change its own installed files.
const ARGV_SERVER: &str = r#"#!/usr/bin/env node
const fs = require('node:fs');
const readline = require('node:readline');
const send = message => process.stdout.write(JSON.stringify(message) + '\n');
readline.createInterface({ input: process.stdin }).on('line', line => {
  const message = JSON.parse(line);
  if (message.id === undefined) return;
  if (message.method === 'initialize') return send({ jsonrpc: '2.0', id: message.id, result: { protocolVersion: '2025-06-18', capabilities: { tools: {} }, serverInfo: { name: 'argv-fixture', version: '1.0.0' } } });
  if (message.method === 'tools/list') return send({ jsonrpc: '2.0', id: message.id, result: { tools: [{ name: 'argv', inputSchema: { type: 'object' } }] } });
  let writable;
  try { fs.appendFileSync(__filename, '\n'); writable = true; } catch (error) { writable = error.code; }
  send({ jsonrpc: '2.0', id: message.id, result: { content: [{ type: 'text', text: JSON.stringify({ argv: process.argv.slice(2), execPath: process.execPath, writable, flags: process.execArgv }) }] } });
});
"#;

#[tokio::test]
#[ignore = "needs the installed Windows network service and the private npm"]
/// The run step passes the server's arguments as a real argument list: batch and shell
/// metacharacters, quotes, spaces, empty and Unicode arguments reach the server unchanged, its
/// Node is the private one with the options of its `#!` line, and its own install is read-only.
/// The install is prepared on disk as an install would leave it, so no network is needed.
async fn npx_arguments_reach_the_server_unchanged() {
    let id = format!("npx-{}", uuid::Uuid::new_v4().simple());
    let hostile = ["a&b", "|", "%PATH%", "^caret", "\"quoted\"", "two words", "", "tail\\", "\\\"", "ü€😀", "$(x)", "'single'", "-y", "--registry=https://evil.example/"];
    let mut args = vec!["-y", "argv-fixture@1.0.0"];
    args.extend(hostile);
    let (server, secrets) = npx_server(&id, &args, &[]);
    let plan = npm_launch::plan(npm_launch::Shim::Npx, &server.args).unwrap();
    let folder = server_folder(&id);
    let package = folder.join(r"pkg\node_modules\argv-fixture");
    fs::create_dir_all(package.join("bin")).unwrap();
    fs::write(package.join("package.json"), r#"{"name":"argv-fixture","version":"1.0.0","bin":{"argv-fixture":"./bin/server.js"}}"#).unwrap();
    fs::write(package.join(r"bin\server.js"), ARGV_SERVER.replace("#!/usr/bin/env node", "#!/usr/bin/env -S node --no-warnings")).unwrap();
    fs::write(folder.join("installed.json"), npm_launch::install_key(&plan)).unwrap();
    let mut link = Link::connect(&server, &secrets).await.unwrap_or_else(|fault| panic!("the prepared server starts: {}", fault.0));
    let answer = link.call("argv", json!({}), Duration::from_secs(20)).await.unwrap();
    let report: Value = serde_json::from_str(answer["content"][0]["text"].as_str().unwrap()).unwrap();
    println!("ARGV {report}");
    assert_eq!(report["argv"], json!(hostile));
    assert_eq!(report["flags"], json!(["--no-warnings"]));
    let node = windows_runtimes::private_node().unwrap();
    assert_eq!(report["execPath"].as_str().unwrap().to_ascii_lowercase(), node.program.to_string_lossy().to_ascii_lowercase());
    assert_ne!(report["writable"], true, "the server changed its own install");
    drop(link);
    drain();
    let _ = fs::remove_dir_all(folder);
}
