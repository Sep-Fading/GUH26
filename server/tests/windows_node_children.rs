#![cfg(windows)]
// Native checks of child processes started by Node inside the production sandbox. The bundled
// official Node (24.21.0, libuv 1.52.1) names the pipes of a child's standard handles
// `\\?\pipe\uv\<n>-<pid>`, outside the AppContainer's own namespace, and retries forever when that
// is refused (docs/windows/sandbox-review.md, "Child processes of sandboxed Node").
// Needs the installed Windows network service and the private Node staged beside the test binary.
use miniclaw_server::{mcp_client::Link, mcp_servers::Server, platform_fs, scoped::Operation, secrets::Secrets, seatbelt::Grant, windows_runtimes};
// Builds servers and parses results.
use serde_json::{Value, json};
// Builds disposable folders and bounds every wait.
use std::{fs, path::{Path, PathBuf}, time::{Duration, Instant}};

/// A private working folder for one test.
fn workspace(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("mc-children-{label}-{}", uuid::Uuid::new_v4()));
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

/// One probe per case, each run in its own Node process under a deadline the shell enforces.
const PROBE: &str = r#"'use strict';
const cp = require('node:child_process');
const net = require('node:net');
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const which = process.argv[2];
const say = (...words) => console.log('PROBE', which, ...words);
const unique = form => form.replace('%r', crypto.randomBytes(8).readBigUInt64LE().toString()).replace('%p', String(process.pid));
// Binds a pipe server under a name, connects to it from this process and reads one answer.
function bind(form) {
  const name = unique(form);
  return new Promise(resolve => {
    const server = net.createServer(socket => socket.end('pong'));
    server.on('error', error => resolve(`BIND ${error.code} ${name}`));
    server.listen(name, () => {
      const client = net.connect(name);
      let data = '';
      client.on('data', chunk => { data += chunk; });
      client.on('error', error => { server.close(); resolve(`BIND ok CONNECT ${error.code} ${name}`); });
      client.on('end', () => { server.close(); resolve(`BIND ok CONNECT ok DATA ${data} ${name}`); });
    });
  });
}
// A child Node that echoes its standard input in upper case, then writes `size` bytes and a line to stderr.
const child = size => [process.execPath, ['-e', `let t='';process.stdin.on('data',d=>t+=d).on('end',()=>{process.stdout.write(t.toUpperCase());process.stdout.write('x'.repeat(${size}));console.error('ERR-OK');});`]];
// Opens a duplex pipe in this run's own namespace: the server end for this process, the client end as a file descriptor for a child.
function localPipe() {
  const name = unique('\\\\.\\pipe\\LOCAL\\miniclaw-probe-%r-%p');
  const server = net.createServer();
  server.listen(name);
  const fd = fs.openSync(name, 'r+');
  // Collects from the moment of the connection, so nothing is missed before the child exits.
  const accepted = new Promise(resolve => server.once('connection', socket => {
    server.close();
    const record = { socket, text: '' };
    socket.on('data', chunk => { record.text += chunk; });
    record.closed = new Promise(done => socket.on('close', done));
    resolve(record);
  }));
  return { fd, accepted };
}
const cases = {
  versions: async () => say(JSON.stringify({ node: process.versions.node, uv: process.versions.uv, openssl: process.versions.openssl })),
  'bind-uv1.52': async () => say(await bind('\\\\?\\pipe\\uv\\%r-%p')),
  'bind-uv1.53': async () => say(await bind('\\\\?\\pipe\\LOCAL\\uv\\%r-%p')),
  'bind-local': async () => say(await bind('\\\\.\\pipe\\LOCAL\\miniclaw-%r-%p')),
  'spawn-inherit': async () => { const r = cp.spawnSync(process.execPath, ['-e', 'console.log("CHILD-INHERIT ok")'], { stdio: 'inherit' }); say('status', r.status, r.error?.code ?? ''); },
  'spawnsync-pipe': async () => { say('start'); const r = cp.spawnSync(process.execPath, ['-e', 'console.log("hi")'], { encoding: 'utf8', timeout: 5000 }); say('status', r.status, JSON.stringify(r.stdout), r.error?.code ?? ''); },
  'exec-pipe': async () => { say('start'); cp.exec(`"${process.execPath}" -e "console.log('hi')"`, { timeout: 5000 }, (error, stdout) => say('callback', error?.code ?? 'ok', JSON.stringify(stdout))); },
  // Building blocks of a workaround without libuv's pipes: files, and a pipe this program names in LOCAL.
  'file-fds': async () => {
    const out = path.join(process.cwd(), `out-${process.pid}.txt`), err = path.join(process.cwd(), `err-${process.pid}.txt`), inp = path.join(process.cwd(), `in-${process.pid}.txt`);
    fs.writeFileSync(inp, 'ping');
    const fds = [fs.openSync(inp, 'r'), fs.openSync(out, 'w'), fs.openSync(err, 'w')];
    const [file, args] = child(1 << 20);
    const r = cp.spawnSync(file, args, { stdio: fds, timeout: 10000 });
    fds.forEach(fd => fs.closeSync(fd));
    const text = fs.readFileSync(out, 'utf8');
    say('status', r.status, r.error?.code ?? '', 'stdout', text.slice(0, 4), text.length, 'stderr', fs.readFileSync(err, 'utf8').trim());
  },
  'local-pipe': async () => {
    const pipes = [localPipe(), localPipe(), localPipe()];
    const [file, args] = child(1 << 20);
    const started = cp.spawn(file, args, { stdio: pipes.map(pipe => pipe.fd) });
    pipes.forEach(pipe => fs.closeSync(pipe.fd));
    const exited = new Promise(resolve => started.on('exit', resolve));
    const [input, output, error] = await Promise.all(pipes.map(pipe => pipe.accepted));
    input.socket.end('ping');
    const code = await exited;
    await Promise.all([output.closed, error.closed]);
    say('exit', code, 'stdout', output.text.slice(0, 4), output.text.length, 'stderr', error.text.trim());
  },
  // A child that itself starts a grandchild with file descriptors, reporting through its own inherited handles.
  grandchild: async () => {
    const script = path.join(process.cwd(), `grand-${process.pid}.cjs`);
    fs.writeFileSync(script, `const cp=require('child_process'),fs=require('fs');const o='g-out-'+process.pid+'.txt';const fd=fs.openSync(o,'w');` +
      `const r=cp.spawnSync(process.execPath,['-e','console.log("GRANDCHILD ok")'],{stdio:['ignore',fd,'ignore']});fs.closeSync(fd);console.log('CHILD saw',r.status,fs.readFileSync(o,'utf8').trim());`);
    const r = cp.spawnSync(process.execPath, [script], { stdio: 'inherit', timeout: 10000 });
    say('status', r.status, r.error?.code ?? '');
  },
};
Promise.resolve(cases[which]()).catch(error => say('THREW', error.code ?? '', error.message));
"#;

#[tokio::test]
#[ignore = "diagnostic: needs the installed Windows network service and the private Node"]
/// Measures, in the offline sandbox token, which pipe names Node's libuv can create and what a
/// child with piped standard handles does. Each probe runs under BusyBox `timeout`, so a hang is
/// cut off and reported as exit 124. The same probes then run outside the sandbox as the control.
async fn node_child_pipes_diagnostic() {
    let node = windows_runtimes::private_node().unwrap();
    let work = workspace("diagnostic");
    fs::write(work.join("probe.cjs"), PROBE).unwrap();
    let cases = ["versions", "bind-uv1.52", "bind-uv1.53", "bind-local", "spawn-inherit", "spawnsync-pipe", "exec-pipe", "file-fds", "local-pipe", "grandchild"];
    let mut sandboxed = String::new();
    for case in cases {
        let started = Instant::now();
        // The first Node start of a run has been seen to take over 20 seconds (VM1), so it gets more time.
        let limit = if case == "versions" { 120 } else { 20 };
        let result = shell(&format!("timeout {limit} node probe.cjs {case} 2>&1; echo \"EXIT {case} $?\""), &work, &[]).await;
        let text = result["text"].as_str().unwrap_or_default();
        println!("SANDBOX {case} ({:?}): {}", started.elapsed(), text.trim_end().replace('\n', " | "));
        sandboxed.push_str(text);
    }
    drain();
    // Control: the private node.exe outside the sandbox, bounded the same way.
    for case in cases {
        let started = Instant::now();
        let mut child = std::process::Command::new(&node.program).arg(work.join("probe.cjs")).arg(case).current_dir(&work)
            .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() { break Some(status); }
            if Instant::now() > deadline { let _ = child.kill(); let _ = child.wait(); break None; }
            std::thread::sleep(Duration::from_millis(50));
        };
        let output = child.wait_with_output().unwrap();
        println!("CONTROL {case} ({:?}) status={status:?}: {}{}", started.elapsed(), String::from_utf8_lossy(&output.stdout).trim_end().replace('\n', " | "),
            String::from_utf8_lossy(&output.stderr).trim_end().replace('\n', " | "));
    }
    let _ = fs::remove_dir_all(&work);
    // The premise: libuv 1.52.1's name is refused, both LOCAL forms work, and the piped child hangs.
    // libuv's bind reports ERROR_ACCESS_DENIED from CreateNamedPipe as EADDRINUSE (src/win/pipe.c,
    // uv_pipe_bind2); outside the sandbox the same name binds.
    assert!(sandboxed.contains("PROBE bind-uv1.52 BIND EADDRINUSE"), "{sandboxed}");
    assert!(sandboxed.contains("PROBE bind-uv1.53 BIND ok CONNECT ok DATA pong"), "{sandboxed}");
    assert!(sandboxed.contains("PROBE bind-local BIND ok CONNECT ok DATA pong"), "{sandboxed}");
    // The building blocks of a workaround that avoids libuv's own pipes work in the sandbox.
    assert!(sandboxed.contains("PROBE file-fds status 0  stdout PING 1048580 stderr ERR-OK"), "{sandboxed}");
    assert!(sandboxed.contains("PROBE local-pipe exit 0 stdout PING 1048580 stderr ERR-OK"), "{sandboxed}");
    assert!(sandboxed.contains("CHILD saw 0 GRANDCHILD ok"), "{sandboxed}");
}

/// The private Node's libuv version, asked outside the sandbox. With 1.53.0 or later the preload
/// does nothing and piped children must simply work.
fn libuv_fixed() -> (String, bool) {
    let node = windows_runtimes::private_node().unwrap();
    let output = std::process::Command::new(&node.program).args(["-p", "process.versions.uv"]).output().unwrap();
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let mut parts = version.split('.').map(|part| part.parse::<u32>().unwrap_or(0));
    let (major, minor) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    let fixed = major > 1 || major == 1 && minor >= 53;
    println!("PRIVATE NODE libuv {version} fixed={fixed}");
    (version, fixed)
}

/// Every way Node starts a child, in one sandboxed Node program: each piped one must end at once
/// (refused, or working once libuv is fixed); inherited and file handles, a child of a child and
/// large output through a file must work. Prints one JSON line.
const CHILDREN: &str = r#"'use strict';
const cp = require('node:child_process');
const fs = require('node:fs');
const out = { options: process.env.NODE_OPTIONS };
const started = Date.now();
const child = ['-e', 'process.stdout.write("hi"); console.error("err-hi")'];
const code = error => error ? (error.code ?? String(error.status)) : 'ok';
const sync = (name, call) => { try { out[name] = ['ok', String(call())]; } catch (error) { out[name] = [code(error), String(error.stdout ?? '')]; } };
sync('execSync', () => cp.execSync(`"${process.execPath}" -e "process.stdout.write('hi')"`, { windowsHide: true }));
sync('execFileSync', () => cp.execFileSync(process.execPath, child, { encoding: 'utf8' }));
const spawned = cp.spawnSync(process.execPath, child, { encoding: 'utf8' });
out.spawnSync = [code(spawned.error), spawned.stdout ?? null, spawned.stderr ?? null];
const big = cp.spawnSync(process.execPath, ['-e', 'process.stdout.write("x".repeat(1 << 20))'], { encoding: 'utf8', maxBuffer: 4 << 20 });
out.bigSpawnSync = [code(big.error), (big.stdout ?? '').length];
out.inherit = cp.spawnSync(process.execPath, ['-e', 'process.exitCode = 7'], { stdio: 'inherit' }).status;
const fd = fs.openSync('big.txt', 'w');
out.file = cp.spawnSync(process.execPath, ['-e', 'process.stdout.write("x".repeat(1 << 20)); require("child_process").spawnSync(process.execPath, ["-e", "process.stdout.write(\\"+grandchild\\")"], { stdio: "inherit" })'], { stdio: ['ignore', fd, 'inherit'] }).status;
fs.closeSync(fd);
out.fileBytes = fs.statSync('big.txt').size;
const pending = [
  new Promise(done => cp.exec(`"${process.execPath}" -e "process.stdout.write('hi')"`, (error, stdout) => { out.exec = [code(error), stdout]; done(); })),
  new Promise(done => cp.execFile(process.execPath, child, (error, stdout, stderr) => { out.execFile = [code(error), stdout, stderr.trim()]; done(); })),
  new Promise(done => { const c = cp.spawn(process.execPath, ['-e', 'process.stdin.pipe(process.stdout)']); let text = '';
    c.stdout.on('data', d => { text += d; }); c.on('error', error => { out.spawn = [code(error), text]; done(); });
    c.on('close', status => { out.spawn ??= [status === 0 ? 'ok' : String(status), text]; done(); }); c.stdin.end('echo'); }),
  new Promise(done => { const c = cp.fork('-e', [], { execArgv: ['-e', 'process.send("forked"); process.disconnect()'] });
    c.on('message', m => { out.fork = ['ok', m]; }); c.on('error', error => { out.fork = [code(error), '']; done(); }); c.on('close', () => { out.fork ??= ['closed', '']; done(); }); }),
];
Promise.all(pending).then(() => { out.ms = Date.now() - started; console.log(JSON.stringify(out)); });
"#;

#[tokio::test]
#[ignore = "needs the installed Windows network service and the private Node"]
/// A sandboxed Node program never hangs on a child process: with the bundled libuv a piped child,
/// in every form Node offers, is refused at once with MiniClaw's code and one line on standard
/// error; children with inherited or file handles, a child of a child and 1 MB through a file work.
/// Once the bundled Node has libuv 1.53.0 or later, the same program must get every output instead.
async fn node_children_in_the_shell_never_hang() {
    let (_, fixed) = libuv_fixed();
    let node = windows_runtimes::private_node().unwrap();
    let work = workspace("shell");
    fs::write(work.join("children.cjs"), CHILDREN).unwrap();
    let started = Instant::now();
    let result = shell("timeout 60 node children.cjs 2>notice.txt; echo \"EXIT $?\"; cat notice.txt", &work, &[]).await;
    println!("CHILDREN ({:?}) {result}", started.elapsed());
    let text = result["text"].as_str().unwrap_or_default().to_owned();
    assert!(text.contains("EXIT 0"), "the program hung or failed: {text}");
    let report: Value = text.lines().find_map(|line| serde_json::from_str::<Value>(line).ok().filter(Value::is_object)).unwrap_or_else(|| panic!("no report: {text}"));
    let preload = node.preload.to_string_lossy().replace('\\', "\\\\");
    assert!(report["options"].as_str().unwrap_or_default().ends_with(&format!("--require \"{preload}\"")), "{report}");
    assert_eq!((report["inherit"].as_i64(), report["file"].as_i64(), report["fileBytes"].as_u64()), (Some(7), Some(0), Some((1 << 20) + "+grandchild".len() as u64)), "{report}");
    let code = windows_runtimes::CHILD_PIPES_CODE;
    if fixed {
        for name in ["execSync", "exec"] { assert_eq!(report[name], json!(["ok", "hi"]), "{name}: {report}"); }
        assert_eq!(report["execFileSync"], json!(["ok", "hi"]), "{report}");
        assert_eq!(report["execFile"], json!(["ok", "hi", "err-hi"]), "{report}");
        assert_eq!(report["spawnSync"], json!(["ok", "hi", "err-hi\n"]), "{report}");
        assert_eq!(report["bigSpawnSync"], json!(["ok", 1 << 20]), "{report}");
        assert_eq!(report["spawn"], json!(["ok", "echo"]), "{report}");
        assert_eq!(report["fork"], json!(["ok", "forked"]), "{report}");
    } else {
        for name in ["execSync", "execFileSync", "spawnSync", "bigSpawnSync", "exec", "execFile", "spawn", "fork"] {
            assert_eq!(report[name][0], code, "{name}: {report}");
        }
        assert!(report["ms"].as_u64().is_some_and(|ms| ms < 10_000), "{report}");
        assert_eq!(text.matches(&format!("MiniClaw {code}: ")).count(), 1, "one notice on standard error: {text}");
    }
    drain();
    let _ = fs::remove_dir_all(work);
}

#[tokio::test]
#[ignore = "needs the installed Windows network service and the private npm"]
/// npm keeps running install scripts with its own handles (`foreground-scripts`), and an install
/// script that captures a child's output ends at once with MiniClaw's code instead of hanging npm.
async fn npm_install_script_with_a_piped_child_never_hangs() {
    let (_, fixed) = libuv_fixed();
    let work = workspace("npm");
    let local = work.join("localpkg");
    fs::create_dir(&local).unwrap();
    fs::write(local.join("package.json"), r#"{"name":"localpkg","version":"1.0.0","scripts":{"postinstall":"node -e \"console.log('CAPTURED ' + require('child_process').execFileSync(process.execPath, ['-e', 'process.stdout.write(String(6 * 7))']))\""}}"#).unwrap();
    let started = Instant::now();
    let result = shell("cd localpkg && npm pack --silent >/dev/null && cd .. && npm init -y >/dev/null && \
        timeout 100 npm install ./localpkg/localpkg-1.0.0.tgz --no-audit --no-fund --foreground-scripts >npm.txt 2>&1; echo \"NPM-EXIT $?\"; cat npm.txt", &work, &[]).await;
    println!("NPM ({:?}) {result}", started.elapsed());
    let text = format!("{}{}", result["text"].as_str().unwrap_or_default(), result["stderr"].as_str().unwrap_or_default());
    assert!(!text.contains("NPM-EXIT 124") && started.elapsed() < Duration::from_secs(100), "npm hung: {text}");
    if fixed {
        assert!(text.contains("CAPTURED 42") && text.contains("NPM-EXIT 0"), "{text}");
    } else {
        // The script fails at once with MiniClaw's code and its line, and npm reports the failed command.
        assert!(text.contains(&format!("MiniClaw {}: ", windows_runtimes::CHILD_PIPES_CODE)) && !text.contains("CAPTURED 42"), "{text}");
        assert!(text.contains("NPM-EXIT 1") && text.contains("npm error command failed"), "{text}");
    }
    drain();
    let _ = fs::remove_dir_all(work);
}

/// A plain-Node stdio MCP server that starts a helper child when it starts, the way servers that
/// wrap a command-line tool ask for its version: `piped` captures its output (uncaught), `caught`
/// the same inside try/catch, `file` with its output in a file.
const HELPER_SERVER: &str = r#"'use strict';
const cp = require('node:child_process');
const fs = require('node:fs');
const readline = require('node:readline');
const mode = process.argv[2];
const helperArgs = ['-e', 'process.stdout.write("helper 1.0")'];
let helper;
if (mode === 'piped') helper = cp.execFileSync(process.execPath, helperArgs, { encoding: 'utf8' });
if (mode === 'caught') { try { helper = cp.execFileSync(process.execPath, helperArgs, { encoding: 'utf8' }); } catch (error) { helper = `refused ${error.code}`; } }
if (mode === 'file') {
  const name = `helper-${process.pid}.txt`, fd = fs.openSync(name, 'w');
  cp.spawnSync(process.execPath, helperArgs, { stdio: ['ignore', fd, 'ignore'] });
  fs.closeSync(fd);
  helper = fs.readFileSync(name, 'utf8');
  fs.unlinkSync(name);
}
const send = message => process.stdout.write(JSON.stringify(message) + '\n');
readline.createInterface({ input: process.stdin }).on('line', line => {
  const message = JSON.parse(line);
  if (message.id === undefined) return;
  if (message.method === 'initialize') return send({ jsonrpc: '2.0', id: message.id, result: { protocolVersion: '2025-06-18', capabilities: { tools: {} }, serverInfo: { name: 'helper-fixture', version: '1.0.0' } } });
  if (message.method === 'tools/list') return send({ jsonrpc: '2.0', id: message.id, result: { tools: [{ name: 'helper', inputSchema: { type: 'object' } }] } });
  send({ jsonrpc: '2.0', id: message.id, result: { content: [{ type: 'text', text: JSON.stringify({ helper, uv: process.versions.uv }) }] } });
});
"#;

/// Calls the fixture's one tool and reads its report.
async fn helper(link: &mut Link) -> Value {
    let answer = link.call("helper", json!({}), Duration::from_secs(20)).await.unwrap();
    serde_json::from_str::<Value>(answer["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[tokio::test]
#[ignore = "needs the installed Windows network service and the private Node"]
/// A confined MCP server whose start needs a piped helper child fails within seconds, well before
/// its 60-second start limit, and MiniClaw says why. The same server that catches the refusal, or
/// that gives its helper a file instead of a pipe, starts and answers.
async fn mcp_server_with_a_helper_child_fails_fast_or_works() {
    let (_, fixed) = libuv_fixed();
    let work = workspace("mcp");
    fs::write(work.join("server.cjs"), HELPER_SERVER).unwrap();
    let node = std::env::var("MINICLAW_TEST_NODE").expect("MINICLAW_TEST_NODE names a node.exe outside the private Node");
    let server = |mode: &str| -> (Server, Secrets) {
        let entry = json!({"id": format!("helper-{mode}-{}", uuid::Uuid::new_v4().simple()), "name": "helper fixture", "kind": "stdio", "command": node,
            "args": ["server.cjs", mode], "folder": work, "startupSeconds": 60, "toolSeconds": 30});
        (serde_json::from_value(entry).unwrap(), Secrets::File(std::env::temp_dir().join(format!("mc-helper-secrets-{}", uuid::Uuid::new_v4()))))
    };
    // Piped and uncaught: the server stops at its start.
    let (piped, secrets) = server("piped");
    let started = Instant::now();
    let outcome = Link::connect(&piped, &secrets).await;
    let elapsed = started.elapsed();
    match outcome {
        Ok(mut link) => {
            assert!(fixed, "a piped helper started with a libuv that cannot name its pipe");
            let report = helper(&mut link).await;
            println!("PIPED ({elapsed:?}) {report}");
            assert_eq!(report["helper"], "helper 1.0");
        }
        Err(fault) => {
            println!("PIPED ({elapsed:?}) {}", fault.0);
            assert!(!fixed, "with a fixed libuv the server must start: {}", fault.0);
            assert!(fault.0.starts_with("This server starts a child process, which the Windows sandbox cannot support with the bundled Node yet."), "{}", fault.0);
            assert!(elapsed < Duration::from_secs(15), "the failure took {elapsed:?}");
        }
    }
    drain();
    // Once more, warm: how long the refusal itself takes after the first sandboxed start.
    if !fixed {
        let started = Instant::now();
        let fault = Link::connect(&piped, &secrets).await.err().expect("the piped server stops again");
        println!("PIPED again ({:?}) {}", started.elapsed(), fault.0);
        assert!(fault.0.starts_with("This server starts a child process"), "{}", fault.0);
        drain();
    }
    // Caught: the server starts without its helper and says so.
    let (caught, secrets) = server("caught");
    let started = Instant::now();
    let mut link = Link::connect(&caught, &secrets).await.unwrap_or_else(|fault| panic!("the server that catches the refusal starts: {}", fault.0));
    let report = helper(&mut link).await;
    println!("CAUGHT ({:?}) {report}", started.elapsed());
    assert_eq!(report["helper"], if fixed { "helper 1.0".to_owned() } else { format!("refused {}", windows_runtimes::CHILD_PIPES_CODE) });
    drop(link);
    drain();
    // A file instead of a pipe: works with the bundled Node.
    let (file, secrets) = server("file");
    let started = Instant::now();
    let mut link = Link::connect(&file, &secrets).await.unwrap_or_else(|fault| panic!("the server with a file for its helper starts: {}", fault.0));
    let report = helper(&mut link).await;
    println!("FILE ({:?}) {report}", started.elapsed());
    assert_eq!(report["helper"], "helper 1.0");
    drop(link);
    drain();
    let _ = fs::remove_dir_all(work);
}
