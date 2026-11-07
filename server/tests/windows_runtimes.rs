#![cfg(windows)]
// Native acceptance of language runtimes in the production sandbox: MiniClaw's private Node in
// the shell and for MCP servers, offline and networked, and Python after activation priming.
// Inputs come from the runner: MINICLAW_TEST_NODE (an official node.exe standing in for the
// user's own Node), MINICLAW_TEST_MCP_PROJECT (a folder with the MCP SDK installed from its
// lockfile outside the sandbox), MINICLAW_TEST_PYTHON (a CPython 3.13 python.exe) and, optionally,
// MINICLAW_TEST_PY_VENV and MINICLAW_TEST_PY_MCP (a virtual environment with the `mcp` package).
use miniclaw_server::{scoped::Operation, mcp_client::Link, mcp_servers::Server, platform_fs, secrets::Secrets, seatbelt::Grant,
    windows_runtimes, windows_sandbox::Sandbox};
// Serves fixtures and parses results.
use serde_json::{Value, json};
// Builds disposable folders.
use std::{fs, path::{Path, PathBuf}, time::Duration};

/// A private working folder for one test.
fn workspace(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("mc-runtimes-{label}-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&path, false).unwrap();
    path
}

/// Runs one command in the production sandboxed shell with read-write access to `work`.
async fn shell(text: &str, work: &Path, hosts: &[&str]) -> Value {
    let grants = vec![Grant { path: work.to_string_lossy().into_owned(), write: true, folder: true, blocked: false }];
    miniclaw_server::executor::execute_host(Operation::RunShell { text: text.to_owned(), cwd: work.to_string_lossy().into_owned(),
        hosts: hosts.iter().map(|host| (*host).to_owned()).collect() }, &grants).await
}

/// The last line of a command's output that parses as JSON.
fn report(result: &Value) -> Value {
    let text = result["text"].as_str().unwrap_or_default();
    text.lines().rev().find_map(|line| serde_json::from_str::<Value>(line.trim()).ok().filter(Value::is_object))
        .unwrap_or_else(|| panic!("no JSON report in {result}"))
}

/// A script that exercises what Node programs need: OpenSSL randomness, files, ES modules and a
/// package, and reports whether the network was reachable.
const NODE_CHECK: &str = r#"
import crypto from 'node:crypto';
import fs from 'node:fs';
import net from 'node:net';
import dns from 'node:dns/promises';
import { createRequire } from 'node:module';
import { local } from './local.mjs';
const require = createRequire(import.meta.url);
const pkg = require('fixture-pkg');
fs.writeFileSync('written.txt', 'hello');
const reach = port => new Promise(resolve => {
  const socket = net.connect({ host: '127.0.0.1', port, timeout: 2000 });
  socket.on('connect', () => { socket.destroy(); resolve('CONNECTED'); });
  socket.on('timeout', () => { socket.destroy(); resolve('timeout'); });
  socket.on('error', error => resolve(error.code));
});
const lookup = await dns.lookup('localhost').then(() => 'RESOLVED', error => error.code);
console.log(JSON.stringify({ execPath: process.execPath, version: process.version, openssl: process.versions.openssl,
  random: crypto.randomBytes(16).length, uuid: crypto.randomUUID().length, key: crypto.generateKeyPairSync('ed25519').publicKey.asymmetricKeyType,
  file: fs.readFileSync('written.txt', 'utf8'), local, pkg: pkg.name, options: process.env.NODE_OPTIONS,
  connect: await reach(Number(process.env.MC_LIVE_PORT || '1')), lookup }));
"#;

/// Writes the Node check and its local module and package into a folder.
fn write_node_check(work: &Path, port: u16) {
    fs::write(work.join("check.mjs"), NODE_CHECK.replace("process.env.MC_LIVE_PORT || '1'", &format!("'{port}'"))).unwrap();
    fs::write(work.join("local.mjs"), "export const local = 42;\n").unwrap();
    fs::create_dir_all(work.join("node_modules/fixture-pkg")).unwrap();
    fs::write(work.join("node_modules/fixture-pkg/package.json"), r#"{"name":"fixture-pkg","main":"index.js"}"#).unwrap();
    fs::write(work.join("node_modules/fixture-pkg/index.js"), "module.exports = { name: 'fixture-pkg' };\n").unwrap();
}

#[test]
#[ignore = "needs the private Node staged beside the test binary"]
/// The private Node is the pinned official build with MiniClaw's seed library and its preload; a changed file is refused.
fn private_node_is_verified_and_a_changed_one_is_refused() {
    let node = windows_runtimes::private_node().expect("the private Node beside the test binary is verified");
    println!("PRIVATE NODE {} seed={:?}", node.program.display(), windows_runtimes::SEED_DIGEST);
    assert!(node.program.ends_with("node.exe"));
    let copy = workspace("tamper");
    for name in ["node.exe", windows_runtimes::SEED_LIBRARY, "openssl.cnf", windows_runtimes::NODE_PRELOAD_FILE] { fs::copy(node.folder.join(name), copy.join(name)).unwrap(); }
    assert!(windows_runtimes::load_private_node(&copy, windows_runtimes::SEED_DIGEST).is_ok(), "an identical copy is accepted");
    assert!(windows_runtimes::load_private_node(&copy, None).is_err(), "a backend built without the seed hash has no private Node");
    fs::write(copy.join("openssl.cnf"), windows_runtimes::OPENSSL_CONFIG.replace("miniclaw_openssl_seed.dll", r"C:\Temp\other.dll")).unwrap();
    assert!(windows_runtimes::load_private_node(&copy, windows_runtimes::SEED_DIGEST).is_err(), "a changed configuration is refused");
    fs::write(copy.join("openssl.cnf"), windows_runtimes::OPENSSL_CONFIG).unwrap();
    // The child-process preload is checked the same way, and a missing one is refused too.
    fs::write(copy.join(windows_runtimes::NODE_PRELOAD_FILE), windows_runtimes::NODE_PRELOAD.replace("install();", "void 0;")).unwrap();
    assert!(windows_runtimes::load_private_node(&copy, windows_runtimes::SEED_DIGEST).is_err(), "a changed preload is refused");
    fs::remove_file(copy.join(windows_runtimes::NODE_PRELOAD_FILE)).unwrap();
    assert!(windows_runtimes::load_private_node(&copy, windows_runtimes::SEED_DIGEST).is_err(), "a missing preload is refused");
    fs::write(copy.join(windows_runtimes::NODE_PRELOAD_FILE), windows_runtimes::NODE_PRELOAD).unwrap();
    assert!(windows_runtimes::load_private_node(&copy, windows_runtimes::SEED_DIGEST).is_ok());
    let mut library = fs::read(copy.join(windows_runtimes::SEED_LIBRARY)).unwrap();
    let last = library.len() - 1;
    library[last] ^= 1;
    fs::write(copy.join(windows_runtimes::SEED_LIBRARY), library).unwrap();
    assert!(windows_runtimes::load_private_node(&copy, windows_runtimes::SEED_DIGEST).is_err(), "a changed seed library is refused");
    fs::remove_dir_all(copy).unwrap();
}

#[tokio::test]
#[ignore = "needs the installed Windows network service and the private Node"]
/// `node` in a sandboxed command without approved hosts is the private Node: it seeds OpenSSL,
/// runs script files, ES modules and packages, and reaches no socket and no name.
async fn shell_node_runs_offline_with_the_private_node() {
    let node = windows_runtimes::private_node().unwrap();
    let work = workspace("shell-offline");
    let live = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    live.set_nonblocking(true).unwrap();
    write_node_check(&work, live.local_addr().unwrap().port());
    let result = shell("node check.mjs", &work, &[]).await;
    println!("{result}");
    assert_eq!(result["exitCode"], 0, "{result}");
    let report = report(&result);
    assert_eq!(report["execPath"].as_str().unwrap().to_ascii_lowercase(), node.program.to_string_lossy().to_ascii_lowercase(), "the user's Node never runs in the sandbox");
    assert_eq!((report["random"].as_u64(), report["uuid"].as_u64(), report["key"].as_str()), (Some(16), Some(36), Some("ed25519")));
    assert_eq!((report["file"].as_str(), report["local"].as_u64(), report["pkg"].as_str()), (Some("hello"), Some(42), Some("fixture-pkg")));
    // Node's flags, then the private Node's child-process preload (escaped for NODE_OPTIONS).
    assert_eq!(report["options"], format!("--preserve-symlinks --preserve-symlinks-main --require \"{}\"", node.preload.to_string_lossy().replace('\\', "\\\\")));
    assert_ne!(report["connect"], "CONNECTED", "an offline run reached a socket");
    assert_ne!(report["lookup"], "RESOLVED", "an offline run resolved a name");
    std::thread::sleep(Duration::from_millis(200));
    assert!(live.accept().is_err(), "the live listener saw a connection");
    miniclaw_server::windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    let _ = fs::remove_dir_all(work);
}

#[tokio::test]
#[ignore = "needs the installed Windows network service, the private Node and internet access"]
/// With an approved host, the private Node's `fetch` goes through MiniClaw's proxy with Node's own
/// CA store: the approved host answers, another host is refused by the proxy.
async fn shell_node_https_through_the_proxy() {
    let work = workspace("shell-https");
    fs::write(work.join("fetch.mjs"), r#"
const get = url => fetch(url, { signal: AbortSignal.timeout(20000) }).then(response => response.status, error => String(error.cause?.code ?? error.message));
console.log(JSON.stringify({ approved: await get('https://example.com/'), other: await get('https://www.iana.org/') }));
"#).unwrap();
    let result = shell("node fetch.mjs", &work, &["example.com"]).await;
    println!("{result}");
    assert_eq!(result["exitCode"], 0, "{result}");
    let report = report(&result);
    assert_eq!(report["approved"], 200, "{report}");
    assert_ne!(report["other"], 200, "{report}");
    assert!(result["refusedHosts"].to_string().contains("www.iana.org"), "{result}");
    // The run's cleanup may still hold the folder for a moment.
    miniclaw_server::windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    let _ = fs::remove_dir_all(work);
}

/// A saved stdio server whose command is the user's own `node.exe` and whose folder is the
/// prepared MCP SDK project.
fn sdk_server(project: &Path, extra: Value) -> (Server, Secrets) {
    let node = std::env::var("MINICLAW_TEST_NODE").expect("MINICLAW_TEST_NODE names a node.exe outside the private Node");
    let mut entry = json!({"id": format!("sdk-{}", uuid::Uuid::new_v4().simple()), "name": "SDK fixture", "kind": "stdio", "command": node,
        "args": ["server.mjs"], "folder": project, "startupSeconds": 90, "toolSeconds": 60});
    entry.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    let secrets = Secrets::File(std::env::temp_dir().join(format!("mc-sdk-secrets-{}", uuid::Uuid::new_v4())));
    (serde_json::from_value(entry).unwrap(), secrets)
}

/// A small stdio server built with the official MCP SDK.
const SDK_SERVER: &str = r#"
import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { z } from 'zod';
import crypto from 'node:crypto';
import fs from 'node:fs';
const server = new McpServer({ name: 'miniclaw-sandbox-fixture', version: '1.0.0' });
const text = value => ({ content: [{ type: 'text', text: JSON.stringify(value) }] });
server.registerTool('inspect', { description: 'Reports the runtime', inputSchema: {} }, async () => {
  fs.writeFileSync('sdk-written.txt', 'sdk');
  return text({ execPath: process.execPath, version: process.version, random: crypto.randomBytes(16).length, file: fs.readFileSync('sdk-written.txt', 'utf8') });
});
server.registerTool('fetch', { description: 'Fetches a URL', inputSchema: { url: z.string() } }, async ({ url }) =>
  text({ status: await fetch(url, { signal: AbortSignal.timeout(20000) }).then(response => response.status, error => String(error.cause?.code ?? error.message)) }));
await server.connect(new StdioServerTransport());
"#;

/// Reads the JSON a fixture tool returned as its text content.
fn tool_text(result: &Value) -> Value {
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap_or_else(|| panic!("no text in {result}"))).unwrap()
}

#[tokio::test]
#[ignore = "needs the installed Windows network service, the private Node and the prepared MCP SDK project"]
/// A real MCP server built with the official SDK, configured with the user's `node.exe`, starts
/// confined with the private Node and answers tool calls.
async fn sdk_server_runs_confined_with_the_private_node() {
    let node = windows_runtimes::private_node().unwrap();
    let project = PathBuf::from(std::env::var("MINICLAW_TEST_MCP_PROJECT").expect("MINICLAW_TEST_MCP_PROJECT"));
    fs::write(project.join("server.mjs"), SDK_SERVER).unwrap();
    let (server, secrets) = sdk_server(&project, json!({}));
    let mut link = Link::connect(&server, &secrets).await.unwrap_or_else(|fault| panic!("the SDK server starts confined: {}", fault.0));
    println!("SDK server {}", link.info);
    let tools = link.tools().await.unwrap();
    assert!(tools.iter().any(|tool| tool["name"] == "inspect") && tools.iter().any(|tool| tool["name"] == "fetch"), "{tools:?}");
    let inspect = tool_text(&link.call("inspect", json!({}), Duration::from_secs(30)).await.unwrap());
    println!("SDK inspect {inspect}");
    assert_eq!(inspect["execPath"].as_str().unwrap().to_ascii_lowercase(), node.program.to_string_lossy().to_ascii_lowercase());
    assert_eq!((inspect["random"].as_u64(), inspect["file"].as_str()), (Some(16), Some("sdk")));
    // Without approved hosts the server has no network at all.
    let offline = tool_text(&link.call("fetch", json!({"url": "https://example.com/"}), Duration::from_secs(40)).await.unwrap());
    println!("SDK offline fetch {offline}");
    assert_ne!(offline["status"], 200);
}

#[tokio::test]
#[ignore = "needs the installed Windows network service, the private Node, the prepared MCP SDK project and internet access"]
/// The same SDK server with an approved host fetches it over HTTPS through its proxy, and nothing else.
async fn sdk_server_fetches_only_its_approved_host() {
    let project = PathBuf::from(std::env::var("MINICLAW_TEST_MCP_PROJECT").expect("MINICLAW_TEST_MCP_PROJECT"));
    fs::write(project.join("server.mjs"), SDK_SERVER).unwrap();
    let (server, secrets) = sdk_server(&project, json!({"hosts": ["example.com"]}));
    let mut link = Link::connect(&server, &secrets).await.unwrap_or_else(|fault| panic!("the networked SDK server starts: {}", fault.0));
    let approved = tool_text(&link.call("fetch", json!({"url": "https://example.com/"}), Duration::from_secs(40)).await.unwrap());
    let other = tool_text(&link.call("fetch", json!({"url": "https://www.iana.org/"}), Duration::from_secs(40)).await.unwrap());
    println!("SDK networked fetch approved={approved} other={other}");
    assert_eq!(approved["status"], 200);
    assert_ne!(other["status"], 200);
}

/// Copies a Python program and its libraries into a fresh folder under a given name: a path
/// Windows has never built an activation context for.
fn fresh_python(work: &Path, folder: &str, name: &str) -> PathBuf {
    let base = PathBuf::from(std::env::var("MINICLAW_TEST_PYTHON").expect("MINICLAW_TEST_PYTHON"));
    let target = work.join(folder);
    fs::create_dir(&target).unwrap();
    fs::copy(&base, target.join(name)).unwrap();
    for entry in fs::read_dir(base.parent().unwrap()).unwrap().flatten() {
        if entry.path().extension().is_some_and(|extension| extension.eq_ignore_ascii_case("dll")) { fs::copy(entry.path(), target.join(entry.file_name())).unwrap(); }
    }
    target.join(name)
}

/// Starts a program in the production offline sandbox and returns its exit code and output.
async fn sandboxed(program: &Path, args: &[&str], work: &Path, extra: &[PathBuf]) -> (u32, String) {
    let mut grants = vec![Grant { path: work.to_string_lossy().into_owned(), write: true, folder: true, blocked: false }];
    grants.extend(extra.iter().map(|path| Grant { path: path.to_string_lossy().into_owned(), write: false, folder: true, blocked: false }));
    let mut sandbox = Sandbox::new(&grants, work).unwrap();
    let env: Vec<(String, String)> = sandbox.environment().into_iter().map(|(name, value)| (name.into_string().unwrap(), value.into_string().unwrap())).collect();
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let pending = miniclaw_server::windows_process::pending_stdio_restricted(program, &args, work, &env, &sandbox.security()).unwrap();
    sandbox.bind_process(&pending.server.process).unwrap();
    let (output, errors, code) = tokio::time::timeout(Duration::from_secs(60), miniclaw_server::windows_process::capture_stdio(pending.resume().unwrap()))
        .await.unwrap().unwrap();
    drop(sandbox);
    miniclaw_server::windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    (code, format!("{}{}", String::from_utf8_lossy(&output), String::from_utf8_lossy(&errors)))
}

#[tokio::test]
#[ignore = "needs CPython 3.13 (MINICLAW_TEST_PYTHON); relies on an undocumented Windows cache"]
/// The launcher primes a Python image before starting it, and Python then starts in the sandbox.
/// The same bytes under a name the launcher does not recognise show why: unprimed, Windows cannot
/// build the side-by-side context inside the sandbox. If the primed start fails, Windows no longer
/// keeps contexts built outside the sandbox, and P1 (docs/windows/sandbox-review.md) needs a new remedy.
async fn python_starts_after_activation_priming() {
    let work = workspace("python-prime");
    let control = fresh_python(&work, "control", "pyfresh.exe");
    let (code, text) = sandboxed(&control, &["--version"], &work, &[]).await;
    println!("UNPRIMED control exit={code:#x} {text}");
    if code == 0 { println!("INFO this Windows starts an unprimed Python in the sandbox; priming is no longer needed here"); }
    else { assert_eq!(code, 0xC015_0002, "the unprimed control failed for another reason: {text}"); }
    let primed = fresh_python(&work, "primed", "python.exe");
    let (code, text) = sandboxed(&primed, &["--version"], &work, &[]).await;
    println!("PRIMED exit={code:#x} {text}");
    assert_eq!(code, 0, "Python did not start after priming ({code:#x}): Windows may no longer reuse activation contexts built outside the sandbox. {text}");
    assert!(text.contains("Python 3.13"), "{text}");
    fs::remove_dir_all(work).unwrap();
}

/// Copies a folder tree; links are not expected in a Python installation and are refused.
fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let kind = entry.file_type().unwrap();
        assert!(!kind.is_symlink(), "{}", entry.path().display());
        if kind.is_dir() { copy_tree(&entry.path(), &to.join(entry.file_name())); } else { fs::copy(entry.path(), to.join(entry.file_name())).unwrap(); }
    }
}

/// Imports every standard-library extension module and reports each failure.
const PY_EXTENSIONS: &str = r#"
import importlib, json, os, sys
failures = {}
for name in sorted(os.listdir(os.path.join(sys.base_prefix, "DLLs"))):
    if name.endswith(".pyd") and name[:-4] not in ("_tkinter", "winsound", "_wmi"):
        try:
            importlib.import_module(name[:-4])
        except BaseException as error:
            failures[name[:-4]] = "%s: %s" % (type(error).__name__, str(error)[:120])
print(json.dumps(failures))
"#;

#[tokio::test]
#[ignore = "needs CPython 3.13 (MINICLAW_TEST_PYTHON); relies on an undocumented Windows cache"]
/// Every standard-library extension module carries its own manifest, and Windows builds its
/// activation context when the module loads. The launcher primes them too: a full copy of the
/// installation at a path Windows has never seen imports all of them in the sandbox (offline,
/// `_overlapped` then stops at its socket, which shows it loaded). The same copy started with only
/// the interpreter and its library primed shows why.
async fn python_extension_modules_load_after_priming() {
    let base = PathBuf::from(std::env::var("MINICLAW_TEST_PYTHON").expect("MINICLAW_TEST_PYTHON")).parent().unwrap().to_owned();
    let work = workspace("python-extensions");
    fs::write(work.join("extensions.py"), PY_EXTENSIONS).unwrap();
    let control = work.join("control");
    copy_tree(&base, &control);
    fs::rename(control.join("python.exe"), control.join("pyfresh.exe")).unwrap();
    // Only what P1 primed before extension modules were added: the program and its python3*.dll.
    println!("CONTROL primed exe={} dll={}", windows_runtimes::activation_context(&control.join("pyfresh.exe"), 1),
        fs::read_dir(&control).unwrap().flatten().filter(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase().starts_with("python3"))
            .map(|entry| windows_runtimes::activation_context(&entry.path(), 2)).sum::<u32>());
    let (code, text) = sandboxed(&control.join("pyfresh.exe"), &["extensions.py"], &work, &[]).await;
    println!("UNPRIMED MODULES exit={code:#x} {text}");
    if text.contains("side-by-side") { assert!(text.contains("_overlapped"), "{text}"); }
    else { println!("INFO this Windows loads unprimed extension modules in the sandbox"); }
    let primed = work.join("primed");
    copy_tree(&base, &primed);
    let started = std::time::Instant::now();
    let (code, text) = sandboxed(&primed.join("python.exe"), &["extensions.py"], &work, &[]).await;
    println!("PRIMED MODULES exit={code:#x} seconds={:.1} {text}", started.elapsed().as_secs_f32());
    assert_eq!(code, 0, "{text}");
    let failures: Value = serde_json::from_str(text.lines().rev().find(|line| line.starts_with('{')).unwrap()).unwrap();
    for (name, failure) in failures.as_object().unwrap() {
        assert!(!failure.as_str().unwrap().contains("side-by-side"), "{name}: {failure}");
    }
    assert!(failures["_overlapped"].as_str().is_none_or(|failure| failure.contains("10047")), "{failures}");
    let _ = fs::remove_dir_all(work);
}

#[tokio::test]
#[ignore = "needs a virtual environment (MINICLAW_TEST_PY_VENV) made outside the sandbox from MINICLAW_TEST_PYTHON"]
/// A virtual environment's launcher starts its base interpreter inside the sandbox: both are primed.
/// Offline, `asyncio` cannot be imported (it creates a socket at import), the rest of Python works.
async fn python_venv_runs_offline_without_asyncio() {
    let venv = PathBuf::from(std::env::var("MINICLAW_TEST_PY_VENV").expect("MINICLAW_TEST_PY_VENV"));
    let base = PathBuf::from(std::env::var("MINICLAW_TEST_PYTHON").unwrap()).parent().unwrap().to_owned();
    let work = workspace("python-venv");
    let script = "import os, sys, json, hashlib, ssl\ntry:\n    import asyncio; aio = 'ok'\nexcept BaseException as error:\n    aio = type(error).__name__ + ': ' + str(error)\nprint(json.dumps({'prefix': sys.prefix, 'urandom': len(os.urandom(8)), 'ssl': len(ssl.RAND_bytes(8)), 'asyncio': aio}))\n";
    fs::write(work.join("check.py"), script).unwrap();
    let (code, text) = sandboxed(&venv.join(r"Scripts\python.exe"), &["check.py"], &work, &[venv.clone(), base]).await;
    println!("VENV exit={code:#x} {text}");
    assert_eq!(code, 0, "{text}");
    let report: Value = serde_json::from_str(text.lines().rev().find(|line| line.starts_with('{')).unwrap()).unwrap();
    assert_eq!((report["urandom"].as_u64(), report["ssl"].as_u64()), (Some(8), Some(8)));
    assert!(report["prefix"].as_str().unwrap().to_ascii_lowercase().starts_with(&venv.to_string_lossy().to_ascii_lowercase()), "{report}");
    println!("VENV offline asyncio: {}", report["asyncio"]);
    fs::remove_dir_all(work).unwrap();
}

/// A stdio server built with the `mcp` package's FastMCP, run by the virtual environment's Python.
/// Its tools report the runtime and try what a confined server must not do.
const PY_SERVER: &str = r#"
import asyncio, ctypes, json, os, socket, sys, traceback, winreg, _winapi
from ctypes import wintypes
try:
    from mcp.server.fastmcp import FastMCP
    server = FastMCP("miniclaw-python-fixture")
    pipes = []

    def package_sid():
        advapi, kernel = ctypes.WinDLL("advapi32", use_last_error=True), ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.GetCurrentProcess.restype = wintypes.HANDLE
        advapi.OpenProcessToken.argtypes = [wintypes.HANDLE, wintypes.DWORD, ctypes.POINTER(wintypes.HANDLE)]
        advapi.GetTokenInformation.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD, ctypes.POINTER(wintypes.DWORD)]
        advapi.ConvertSidToStringSidW.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_wchar_p)]
        token, size = wintypes.HANDLE(), wintypes.DWORD()
        advapi.OpenProcessToken(kernel.GetCurrentProcess(), 0x8, ctypes.byref(token))
        advapi.GetTokenInformation(token, 31, None, 0, ctypes.byref(size))
        buffer = ctypes.create_string_buffer(size.value)
        advapi.GetTokenInformation(token, 31, buffer, size, ctypes.byref(size))
        text = ctypes.c_wchar_p()
        advapi.ConvertSidToStringSidW(ctypes.cast(buffer, ctypes.POINTER(ctypes.c_void_p))[0], ctypes.byref(text))
        session = wintypes.DWORD()
        kernel.ProcessIdToSessionId(os.getpid(), ctypes.byref(session))
        return text.value, session.value

    def failure(error):
        return "%s %s" % (type(error).__name__, getattr(error, "winerror", None) or getattr(error, "errno", None))

    @server.tool()
    async def inspect() -> dict:
        loop = asyncio.get_running_loop()
        hook = sys.modules.get("miniclaw_sitecustomize")
        sid, session = package_sid()
        return {"executable": sys.executable, "pid": os.getpid(), "urandom": len(os.urandom(8)), "hook": getattr(hook, "__file__", None),
            "marker": os.environ.get("MINICLAW_PYTHON_SANDBOX"), "pythonpath": os.environ.get("PYTHONPATH"), "loop": type(loop).__name__,
            "wake": type(loop._ssock).__name__, "sid": sid, "session": session, "proxy": os.environ.get("HTTPS_PROXY")}

    @server.tool()
    async def probe(targets: list[str], name: str) -> dict:
        report = {}
        for target in targets:
            host, port = target.rsplit(":", 1)
            try:
                reader, writer = await asyncio.wait_for(asyncio.open_connection(host.strip("[]"), int(port)), 3)
                writer.close()
                report[target] = "CONNECTED"
            except BaseException as error:
                report[target] = failure(error)
        try:
            listener = socket.socket()
            listener.bind(("127.0.0.1", 0))
            listener.listen()
            report["listen"] = "LISTENING %d" % listener.getsockname()[1]
            listener.close()
        except BaseException as error:
            report["listen"] = failure(error)
        try:
            report["lookup"] = repr(await asyncio.get_running_loop().getaddrinfo(name, 443))[:80]
        except BaseException as error:
            report["lookup"] = failure(error)
        return report

    @server.tool()
    async def outside(path: str) -> dict:
        report = {}
        try:
            with open(path) as handle:
                report["file"] = "READ " + handle.read(20)
        except BaseException as error:
            report["file"] = failure(error)
        for label, key in (("hkcu", "Environment"), ("hkcu_software", "Software")):
            try:
                winreg.CloseKey(winreg.OpenKey(winreg.HKEY_CURRENT_USER, key))
                report[label] = "OPENED"
            except BaseException as error:
                report[label] = failure(error)
        return report

    @server.tool()
    async def pipe_listen() -> dict:
        name = "\\\\.\\pipe\\LOCAL\\miniclaw-fixture-" + os.urandom(8).hex()
        # Two waiting instances: one for the unsandboxed control, one for this run's own open.
        for _ in range(2):
            pipes.append(_winapi.CreateNamedPipe(name, _winapi.PIPE_ACCESS_INBOUND, _winapi.PIPE_WAIT, 2, 0, 4096, 0, _winapi.NULL))
        return {"name": name}

    @server.tool()
    async def pipe_open(names: list[str]) -> dict:
        report = {}
        for name in names:
            try:
                _winapi.CloseHandle(_winapi.CreateFile(name, _winapi.GENERIC_WRITE, 0, _winapi.NULL, _winapi.OPEN_EXISTING, 0, _winapi.NULL))
                report[name] = "OPENED"
            except BaseException as error:
                report[name] = failure(error)
        return report

    @server.tool()
    async def fetch(url: str) -> dict:
        import httpx
        try:
            async with httpx.AsyncClient(timeout=20) as client:
                return {"status": (await client.get(url)).status_code}
        except BaseException as error:
            return {"status": failure(error), "detail": str(error)[:200]}

    server.run()
except BaseException:
    with open("server-error.txt", "a") as log: log.write(traceback.format_exc())
    raise
"#;

/// A saved stdio server: the virtual environment's `python.exe` running [`PY_SERVER`] in `folder`.
fn py_server(venv: &Path, base: &Path, folder: &Path, extra: Value) -> Server {
    let mut entry = json!({"id": format!("py-{}", uuid::Uuid::new_v4().simple()), "name": "Python fixture", "kind": "stdio",
        "command": venv.join(r"Scripts\python.exe"), "args": ["server.py"], "folder": folder, "runtimePaths": [venv, base],
        "startupSeconds": 90, "toolSeconds": 60});
    entry.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    serde_json::from_value::<Server>(entry).unwrap()
}

/// The JSON a fixture tool returned, from a server whose stdout carries MCP messages.
async fn py_call(link: &mut Link, tool: &str, arguments: Value) -> Value {
    tool_text(&link.call(tool, arguments, Duration::from_secs(60)).await.unwrap_or_else(|fault| panic!("{tool}: {}", fault.0)))
}

/// TCP ports a process listens on, from `netstat`, which needs no privilege.
fn listening_ports(pid: u64) -> Vec<String> {
    let output = std::process::Command::new("netstat").args(["-ano", "-p", "TCP"]).output().unwrap();
    let output6 = std::process::Command::new("netstat").args(["-ano", "-p", "TCPv6"]).output().unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output6.stdout));
    text.lines().filter(|line| { let words: Vec<&str> = line.split_whitespace().collect();
        words.len() == 5 && words[3] == "LISTENING" && words[4] == pid.to_string() }).map(str::to_owned).collect()
}

/// Whether Windows' DNS client cached an answer for a name: a name the sandbox asked for and that
/// left the machine would be there, positive or negative.
fn dns_cached(name: &str) -> String {
    let output = std::process::Command::new("powershell.exe").args(["-NoProfile", "-Command",
        &format!("@(Get-DnsClientCache -ErrorAction SilentlyContinue | Where-Object {{ $_.Entry -eq '{name}' }}).Count")]).output().unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// The fixtures every Python MCP test needs: the virtual environment, its base installation and a
/// private folder holding the server.
fn py_fixture(label: &str) -> (PathBuf, PathBuf, PathBuf) {
    let venv = PathBuf::from(std::env::var("MINICLAW_TEST_PY_MCP").expect("MINICLAW_TEST_PY_MCP"));
    let base = PathBuf::from(std::env::var("MINICLAW_TEST_PYTHON").unwrap()).parent().unwrap().to_owned();
    let folder = workspace(label);
    fs::write(folder.join("server.py"), PY_SERVER).unwrap();
    (venv, base, folder)
}

#[test]
#[ignore = "needs MiniClaw's sitecustomize staged beside the test binary"]
/// The hook is the reviewed file; a changed copy is refused.
fn python_hook_is_verified_and_a_changed_one_is_refused() {
    let hook = windows_runtimes::python_hook().expect("the hook beside the test binary is verified");
    println!("PYTHON HOOK {}", hook.file.display());
    let copy = workspace("hook-tamper");
    fs::write(copy.join(windows_runtimes::PYTHON_HOOK_FILE), fs::read(&hook.file).unwrap()).unwrap();
    assert!(windows_runtimes::load_python_hook(&copy).is_ok(), "an identical copy is accepted");
    fs::write(copy.join(windows_runtimes::PYTHON_HOOK_FILE), windows_runtimes::PYTHON_HOOK.replacen("return -1", "return 3", 1)).unwrap();
    assert!(windows_runtimes::load_python_hook(&copy).is_err(), "a changed hook is refused");
    fs::remove_dir_all(copy).unwrap();
}

#[tokio::test]
#[ignore = "needs a virtual environment with the mcp package (MINICLAW_TEST_PY_MCP), MiniClaw's sitecustomize and the installed network service"]
/// A FastMCP server without approved hosts starts, lists its tools and answers, in the networked
/// token under a lease whose proxy allows no host. It listens on no port, reaches no receiver and no
/// name, reads neither the user's registry nor a file outside its folder, and another run cannot
/// open its pipes.
async fn python_mcp_server_runs_without_approved_hosts() {
    let hook = windows_runtimes::python_hook().unwrap();
    let (venv, base, folder) = py_fixture("python-mcp");
    let outside = workspace("python-outside");
    fs::write(outside.join("secret.txt"), "outside-secret").unwrap();
    let secrets = Secrets::File(std::env::temp_dir().join(format!("mc-py-secrets-{}", uuid::Uuid::new_v4())));
    let mut link = Link::connect(&py_server(&venv, &base, &folder, json!({})), &secrets).await
        .unwrap_or_else(|fault| panic!("the asyncio server starts: {} | {}", fault.0, fs::read_to_string(folder.join("server-error.txt")).unwrap_or_default()));
    let tools = link.tools().await.unwrap();
    for name in ["inspect", "probe", "outside", "pipe_listen", "pipe_open", "fetch"] { assert!(tools.iter().any(|tool| tool["name"] == name), "{name}: {tools:?}"); }
    let inspect = py_call(&mut link, "inspect", json!({})).await;
    println!("PY MCP inspect {inspect}");
    assert_eq!(inspect["hook"].as_str().unwrap().to_ascii_lowercase(), hook.file.to_string_lossy().to_ascii_lowercase());
    assert_eq!((inspect["marker"].as_str(), inspect["loop"].as_str(), inspect["wake"].as_str()), (Some("1"), Some("ProactorEventLoop"), Some("PipeHandle")));
    assert!(inspect["pythonpath"].as_str().unwrap().to_ascii_lowercase().starts_with(&hook.folder.to_string_lossy().to_ascii_lowercase()), "{inspect}");
    assert_eq!(inspect["urandom"], 8);
    let pid = inspect["pid"].as_u64().unwrap();
    // Live receivers the server must not reach, besides its own proxy.
    let receivers: Vec<std::net::TcpListener> = ["127.0.0.1:0", "127.0.0.2:0", "[::1]:0"].iter().map(|address| {
        let listener = std::net::TcpListener::bind(address).unwrap();
        listener.set_nonblocking(true).unwrap();
        listener
    }).collect();
    let targets: Vec<String> = receivers.iter().map(|listener| listener.local_addr().unwrap().to_string()).collect();
    let name = format!("mc-{}.example.org", uuid::Uuid::new_v4().simple());
    let probe = py_call(&mut link, "probe", json!({"targets": targets, "name": name})).await;
    println!("PY MCP probe {probe}");
    for target in &targets { assert_ne!(probe[target], "CONNECTED", "{target}: {probe}"); }
    assert!(!probe["listen"].as_str().unwrap().starts_with("LISTENING"), "{probe}");
    assert!(probe["lookup"].as_str().unwrap().starts_with("gaierror"), "{probe}");
    std::thread::sleep(Duration::from_millis(300));
    for receiver in &receivers { assert!(receiver.accept().is_err(), "a receiver saw a connection: {}", receiver.local_addr().unwrap()); }
    let listening = listening_ports(pid);
    println!("PY MCP listening ports of {pid}: {listening:?}");
    assert!(listening.is_empty(), "{listening:?}");
    let cached = dns_cached(&name);
    println!("PY MCP DNS cache entries for {name}: {cached}");
    assert_eq!(cached, "0");
    let offline = py_call(&mut link, "fetch", json!({"url": "https://example.com/"})).await;
    println!("PY MCP fetch without hosts {offline}");
    assert_ne!(offline["status"], 200);
    let denied = py_call(&mut link, "outside", json!({"path": outside.join("secret.txt")})).await;
    println!("PY MCP outside {denied}");
    for key in ["file", "hkcu", "hkcu_software"] { assert!(denied[key].as_str().unwrap().starts_with("PermissionError"), "{key}: {denied}"); }
    // A pipe in this run's namespace: another run, and the namespace path of this one, are closed to it.
    let pipe = py_call(&mut link, "pipe_listen", json!({})).await;
    let local = pipe["name"].as_str().unwrap().to_owned();
    let leaf = local.rsplit('\\').next().unwrap().to_owned();
    let namespaced = format!(r"\\.\pipe\Sessions\{}\AppContainerNamedObjects\{}\{leaf}", inspect["session"], inspect["sid"].as_str().unwrap());
    let (other_venv, other_base, other_folder) = py_fixture("python-mcp-other");
    let _ = (other_venv, other_base);
    let mut other = Link::connect(&py_server(&venv, &base, &other_folder, json!({})), &secrets).await.unwrap_or_else(|fault| panic!("second run: {}", fault.0));
    let other_inspect = py_call(&mut other, "inspect", json!({})).await;
    assert_ne!(other_inspect["sid"], inspect["sid"], "each run has its own package");
    let crossing = py_call(&mut other, "pipe_open", json!({"names": [local.clone(), namespaced.clone()]})).await;
    println!("PY MCP another run opening the first run's pipe {crossing}");
    assert_ne!(crossing[&local], "OPENED", "{crossing}");
    assert_ne!(crossing[&namespaced], "OPENED", "{crossing}");
    // Controls: the LOCAL name is not in the global namespace, the namespace path does name the pipe
    // (the unsandboxed owner opens it), and the run itself opens its pipe.
    let open = |path: &str| std::fs::OpenOptions::new().write(true).open(path).map(|_| "OPENED".to_owned()).unwrap_or_else(|error| error.to_string());
    let (global, owner) = (open(&local), open(&namespaced));
    println!("PY MCP unsandboxed open: LOCAL name {global}; namespace path {owner}");
    assert_ne!(global, "OPENED", "the run's pipe is not in the global namespace");
    assert_eq!(owner, "OPENED", "the namespace path names the run's pipe");
    let own = py_call(&mut link, "pipe_open", json!({"names": [local.clone()]})).await;
    println!("PY MCP own pipe {own}");
    assert_eq!(own[&local], "OPENED", "{own}");
    drop(other);
    drop(link);
    miniclaw_server::windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    for path in [folder, other_folder, outside] { let _ = fs::remove_dir_all(path); }
}

#[tokio::test]
#[ignore = "needs a virtual environment with the mcp package (MINICLAW_TEST_PY_MCP), MiniClaw's sitecustomize, the installed network service and internet access"]
/// The same server with one approved host reaches it through its proxy, and nothing else.
async fn python_mcp_server_fetches_only_its_approved_host() {
    let (venv, base, folder) = py_fixture("python-mcp-host");
    let secrets = Secrets::File(std::env::temp_dir().join(format!("mc-py-secrets-{}", uuid::Uuid::new_v4())));
    let mut link = Link::connect(&py_server(&venv, &base, &folder, json!({"hosts": ["example.com"]})), &secrets).await
        .unwrap_or_else(|fault| panic!("the networked asyncio server starts: {}", fault.0));
    let approved = py_call(&mut link, "fetch", json!({"url": "https://example.com/"})).await;
    let other = py_call(&mut link, "fetch", json!({"url": "https://www.iana.org/"})).await;
    println!("PY MCP networked fetch approved={approved} other={other}");
    assert_eq!(approved["status"], 200);
    assert_ne!(other["status"], 200);
    drop(link);
    miniclaw_server::windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    let _ = fs::remove_dir_all(folder);
}

/// A plain asyncio program: a subprocess with pipes, and CONNECT requests through the proxy.
const ASYNCIO_SCRIPT: &str = r#"
import asyncio, json, os, subprocess, sys
async def connect(host):
    proxy = os.environ["HTTPS_PROXY"].rsplit(":", 1)
    reader, writer = await asyncio.wait_for(asyncio.open_connection("127.0.0.1", int(proxy[1])), 10)
    writer.write(("CONNECT %s:443 HTTP/1.1\r\nHost: %s:443\r\n\r\n" % (host, host)).encode())
    await writer.drain()
    line = (await asyncio.wait_for(reader.readline(), 20)).decode().strip()
    writer.close()
    return line
async def main():
    child = await asyncio.create_subprocess_exec(sys.executable, "-c", "import sys; print(sys.stdin.read().upper())",
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    output, _ = await asyncio.wait_for(child.communicate(b"hello"), 30)
    return {"child": output.decode().strip(), "approved": await connect("example.com"), "other": await connect("www.iana.org"),
        "wake": type(asyncio.get_running_loop()._ssock).__name__}
print(json.dumps(asyncio.run(main())))
"#;

#[tokio::test]
#[ignore = "needs CPython (MINICLAW_TEST_PYTHON), MiniClaw's sitecustomize, the installed network service and internet access"]
/// A plain asyncio script in the sandboxed shell with one approved host: its subprocess pipes work,
/// and the proxy opens a tunnel to the approved host only.
async fn python_asyncio_script_uses_subprocess_and_the_proxy() {
    let python = PathBuf::from(std::env::var("MINICLAW_TEST_PYTHON").expect("MINICLAW_TEST_PYTHON"));
    windows_runtimes::prime(&python);
    let work = workspace("python-asyncio");
    fs::write(work.join("check.py"), ASYNCIO_SCRIPT).unwrap();
    let grants = vec![Grant { path: work.to_string_lossy().into_owned(), write: true, folder: true, blocked: false },
        Grant { path: python.parent().unwrap().to_string_lossy().into_owned(), write: false, folder: true, blocked: false }];
    let text = format!("'{}' check.py", python.to_string_lossy().replace('\\', "/"));
    let result = miniclaw_server::executor::execute_host(Operation::RunShell { text, cwd: work.to_string_lossy().into_owned(),
        hosts: vec!["example.com".into()] }, &grants).await;
    println!("{result}");
    assert_eq!(result["exitCode"], 0, "{result}");
    let report = report(&result);
    assert_eq!((report["child"].as_str(), report["wake"].as_str()), (Some("HELLO"), Some("PipeHandle")), "{report}");
    assert!(report["approved"].as_str().unwrap().contains(" 200"), "{report}");
    assert!(!report["other"].as_str().unwrap().contains(" 200"), "{report}");
    miniclaw_server::windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    let _ = fs::remove_dir_all(work);
}
