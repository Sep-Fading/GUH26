#![cfg(windows)]
// Starts only native initialization modes, never providers or authenticated services.
use miniclaw_server::{agents::Store, platform_fs};
// Owns disposable Windows paths and native child outputs.
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Directory(PathBuf);
impl Directory {
    /// Uses spaces and non-ASCII characters to exercise argument and path handling.
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("MiniClaw smoke 雪 {}", uuid::Uuid::new_v4()));
        Self(path)
    }
}
impl Drop for Directory {
    /// Removes only the temporary state tree created by this test.
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs a setup-only binary mode with argument boundaries preserved.
fn setup(mode: &str, path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_miniclaw-server"))
        .arg(mode)
        .arg(path)
        .output()
        .expect("native setup mode starts")
}

/// Initialization creates private state, can reopen SQLite, and refuses a second init.
#[test]
fn initializes_private_state_once() {
    let root = Directory::new();
    let output = setup("--initialize-data", &root.0);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for name in ["agents.sqlite3", "agents.sqlite3.identity", "instance.lock"] {
        let file = platform_fs::open_read_nofollow(&root.0.join(name)).unwrap();
        assert!(platform_fs::private_regular_file(&file).unwrap(), "{name}");
    }
    let marker = std::fs::read(root.0.join("agents.sqlite3.identity")).unwrap();
    let store = Store::open(&root.0.join("agents.sqlite3")).unwrap();
    drop(store);
    let output = setup("--initialize-data", &root.0);
    assert!(!output.status.success());
    assert_eq!(
        std::fs::read(root.0.join("agents.sqlite3.identity")).unwrap(),
        marker
    );
    assert!(Store::open(&root.0.join("agents.sqlite3")).is_ok());
}

/// Securing a fresh data directory never silently initializes policy storage.
#[test]
fn secure_mode_does_not_initialize_database() {
    let root = Directory::new();
    let output = setup("--secure-data-dir", &root.0);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!root.0.join("agents.sqlite3").exists());
    assert!(!root.0.join("agents.sqlite3.identity").exists());
    assert!(setup("--secure-data-dir", &root.0).status.success());
    assert!(setup("--initialize-data", &root.0).status.success());
    assert!(Store::open(&root.0.join("agents.sqlite3")).is_ok());
}

/// Runs the backend until it exits or `ready` holds, then returns its captured output.
fn start_until(root: &Path, ready: impl Fn() -> bool) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_miniclaw-server"))
        .env("MINICLAW_DATA_DIR", root).env("MINICLAW_PORT", "0")
        .env("CODEX_BIN", "").env("CLAUDE_BIN", "")
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
    let started = std::time::Instant::now();
    while child.try_wait().unwrap().is_none() && !ready()
        && started.elapsed() < std::time::Duration::from_secs(10) {
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    if child.try_wait().unwrap().is_none() { child.kill().unwrap(); }
    child.wait_with_output().unwrap()
}

/// Starts the backend on a free port and waits until it answers, or until it exits.
fn serve(root: &Path) -> (std::process::Child, u16) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut child = Command::new(env!("CARGO_BIN_EXE_miniclaw-server"))
        .env("MINICLAW_DATA_DIR", root).env("MINICLAW_PORT", port.to_string())
        .env("CODEX_BIN", "").env("CLAUDE_BIN", "")
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped()).spawn().unwrap();
    let started = std::time::Instant::now();
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        if let Some(status) = child.try_wait().unwrap() {
            let mut stderr = String::new();
            std::io::Read::read_to_string(child.stderr.as_mut().unwrap(), &mut stderr).unwrap();
            panic!("the backend exited ({status}) instead of serving: {stderr}");
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(30), "the backend did not listen");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    (child, port)
}

/// Sends one local GET as the page does and returns the status and body.
fn get(port: u16, path: &str) -> (u16, String) {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let origin = format!("http://127.0.0.1:{port}");
    write!(stream, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: {origin}\r\nSec-Fetch-Site: same-origin\r\nConnection: close\r\n\r\n").unwrap();
    let mut text = String::new();
    stream.read_to_string(&mut text).unwrap();
    let status = text.split(' ').nth(1).and_then(|code| code.parse().ok()).unwrap_or(0);
    let body = text.split_once("\r\n\r\n").map(|(_, body)| body.to_owned()).unwrap_or_default();
    (status, body)
}

/// Unresolved records of earlier runs pause sandboxed work, not the backend. The first test kept
/// "startup stops" for damage in the middle of a record (it codified the fail-closed rule when a
/// stuck record bricked MiniClaw on the second guest, see sandbox-review "Startup with unresolved
/// records"); the requirement is now that the backend serves, `/api/sandbox` says why the sandbox
/// is unavailable, and the record stays for recovery.
#[test]
fn startup_with_corrupt_records_serves_and_pauses_the_sandbox() {
    use std::io::Write;
    let root = Directory::new();
    assert!(setup("--initialize-data", &root.0).status.success());
    let journals = root.0.join("sandbox-journals");
    platform_fs::private_dir(&journals, false).unwrap();
    let record = journals.join(format!("MiniClaw.Run.{}.jsonl", uuid::Uuid::new_v4().simple()));
    let mut file = platform_fs::private_file(&record, true).unwrap();
    file.write_all(b"{\"path\":\n{\"job\":\"later\"}\n").unwrap();
    file.sync_all().unwrap();
    drop(file);
    let (mut child, port) = serve(&root.0);
    let health = get(port, "/api/health");
    let sandbox = get(port, "/api/sandbox");
    child.kill().unwrap();
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(health.0, 200, "{health:?}");
    assert_eq!(sandbox.0, 200, "{sandbox:?}");
    let report: serde_json::Value = serde_json::from_str(&sandbox.1).unwrap();
    assert_eq!(report["available"], false, "{report}");
    assert_eq!(report["recovery"], true, "{report}");
    let reason = report["reason"].as_str().unwrap();
    assert!(reason.contains("Windows sandbox recovery retained 1 unresolved") && reason.contains("damaged") && reason.contains("Chat still works"), "{reason}");
    assert_eq!(report["git"]["available"], false);
    assert!(stderr.contains("Windows sandbox recovery"), "{stderr}");
    assert!(record.exists(), "Uncertain evidence must be preserved");
}

/// Deletes an object through `DELETE` alone, the right its folder's `FILE_DELETE_CHILD` (and the
/// fixtures' `OWNER RIGHTS` entry) gives. `std::fs` asks for `SYNCHRONIZE` too, which these
/// fixtures withhold as the real Administrators-only folders do.
fn remove_locked(path: &Path) {
    use windows_sys::{Wdk::{Foundation::OBJECT_ATTRIBUTES, Storage::FileSystem as nt}, Win32::{Foundation::{CloseHandle, UNICODE_STRING}, System::IO::IO_STATUS_BLOCK}};
    let mut text: Vec<u16> = format!(r"\??\{}", path.display()).encode_utf16().collect();
    let length = (text.len() * 2) as u16;
    let mut name = UNICODE_STRING { Length: length, MaximumLength: length, Buffer: text.as_mut_ptr() };
    let attributes = OBJECT_ATTRIBUTES { Length: size_of::<OBJECT_ATTRIBUTES>() as u32, RootDirectory: std::ptr::null_mut(),
        ObjectName: &mut name, Attributes: 0, SecurityDescriptor: std::ptr::null(), SecurityQualityOfService: std::ptr::null_mut() };
    let mut status: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
    let mut handle = std::ptr::null_mut();
    let result = unsafe { nt::NtCreateFile(&mut handle, 0x0001_0000, &attributes, &mut status, std::ptr::null(), 0,
        7, nt::FILE_OPEN, nt::FILE_DELETE_ON_CLOSE | nt::FILE_OPEN_REPARSE_POINT, std::ptr::null(), 0) };
    assert!(result >= 0, "delete {}: {result:#x}", path.display());
    unsafe { CloseHandle(handle) };
    assert!(std::fs::symlink_metadata(path).is_err(), "{} is gone", path.display());
}

/// Reads an object's full identity as the journal records it.
fn identity(path: &Path) -> (u64, u128) {
    let file = std::fs::File::open(path).unwrap();
    platform_fs::file_identity(&file).unwrap()
}

/// A record of a granted object this user can no longer open is genuinely uncertain: it stays,
/// and the reason the sandbox gives names the object, while the backend serves everything else.
#[test]
fn startup_with_an_unreadable_grant_names_it_and_serves() {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{Foundation::LocalFree, Security::{Authorization::*, *}};
    let root = Directory::new();
    assert!(setup("--initialize-data", &root.0).status.success());
    let journals = root.0.join("sandbox-journals");
    platform_fs::private_dir(&journals, false).unwrap();
    let object = root.0.join("granted object.txt");
    std::fs::write(&object, "stamped by an earlier run").unwrap();
    let saved = serde_json::json!({ "path": object, "identity": identity(&object), "directory": false });
    // SYSTEM only, and OWNER RIGHTS with DELETE alone: this user cannot open it (no READ_CONTROL, WRITE_DAC or SYNCHRONIZE).
    let sddl: Vec<u16> = "D:P(A;;FA;;;SY)(A;;SD;;;OW)\0".encode_utf16().collect();
    let wide: Vec<u16> = object.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let mut descriptor = std::ptr::null_mut();
        assert_ne!(ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), 1, &mut descriptor, std::ptr::null_mut()), 0);
        assert_ne!(SetFileSecurityW(wide.as_ptr(), DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, descriptor), 0);
        LocalFree(descriptor);
    }
    let record = journals.join(format!("MiniClaw.Run.{}.jsonl", uuid::Uuid::new_v4().simple()));
    let boot = serde_json::json!({ "boot": { "installation": format!("MiniClawNetwork-{}", "a".repeat(32)), "epoch": "b".repeat(32) } });
    let mut file = platform_fs::private_file(&record, true).unwrap();
    std::io::Write::write_all(&mut file, format!("{boot}\n{saved}\n").as_bytes()).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let (mut child, port) = serve(&root.0);
    let sandbox = get(port, "/api/sandbox");
    let health = get(port, "/api/health");
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(health.0, 200, "{health:?}");
    let report: serde_json::Value = serde_json::from_str(&sandbox.1).unwrap();
    assert_eq!(report["available"], false, "{report}");
    let reason = report["reason"].as_str().unwrap();
    println!("STARTUP unreadable grant reason: {reason}");
    assert!(reason.contains("retained 1 unresolved") && reason.contains("granted object.txt") && reason.contains("Chat still works"), "{reason}");
    assert!(record.exists(), "Uncertain evidence must be preserved");
    // The folder's owner may still delete it; the test leaves nothing behind.
    remove_locked(&object);
}

/// A record torn before its first flushed line changed nothing, so startup clears it.
#[test]
fn startup_clears_prelaunch_sandbox_journals() {
    use std::io::Write;
    let root = Directory::new();
    assert!(setup("--initialize-data", &root.0).status.success());
    let journals = root.0.join("sandbox-journals");
    platform_fs::private_dir(&journals, false).unwrap();
    let records: Vec<_> = [&b""[..], b"{\"path\":", b"{\"boot\":{\"installation\":\"MiniClawNetwork-"]
        .into_iter().map(|bytes| {
            let record = journals.join(format!("MiniClaw.Run.{}.jsonl", uuid::Uuid::new_v4().simple()));
            let mut file = platform_fs::private_file(&record, true).unwrap();
            file.write_all(bytes).unwrap();
            file.sync_all().unwrap();
            record
        }).collect();
    let output = start_until(&root.0, || records.iter().all(|record| !record.exists()));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Windows sandbox recovery"), "{}", String::from_utf8_lossy(&output.stderr));
    for record in &records {
        assert!(!record.exists(), "Pre-launch record must be cleared: {}", record.display());
    }
}

/// Losing the launcher's private stdin pipe drains the server and releases its state lock.
#[test]
fn launcher_pipe_closure_stops_backend() {
    use std::{net::TcpListener, process::Stdio, thread, time::{Duration, Instant}};
    let root = Directory::new();
    assert!(setup("--initialize-data", &root.0).status.success());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut child = Command::new(env!("CARGO_BIN_EXE_miniclaw-server"))
        .env("MINICLAW_DATA_DIR", &root.0)
        .env("MINICLAW_PORT", port.to_string())
        .env("MINICLAW_LAUNCHER_STDIN", "1")
        .env("CODEX_BIN", "").env("CLAUDE_BIN", "")
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::inherit())
        .spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(child.try_wait().unwrap().is_none(), "Backend exited before listening");
        if Instant::now() >= deadline { let _ = child.kill(); panic!("Backend did not listen"); }
        thread::sleep(Duration::from_millis(30));
    }
    drop(child.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().unwrap() { assert!(status.success()); break; }
        if Instant::now() >= deadline { let _ = child.kill(); panic!("Launcher EOF did not stop backend"); }
        thread::sleep(Duration::from_millis(30));
    }
    assert!(Store::open(&root.0.join("agents.sqlite3")).is_ok());
}
