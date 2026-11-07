#![cfg(windows)]
// Verifies temporary inheritance and exact SID revocation.
use miniclaw_server::{platform_fs, seatbelt::Grant, windows_sandbox::Sandbox};
use std::{
    fs,
    fs::File,
    io,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Path, PathBuf},
};
use windows_sys::Win32::{
    Foundation::{INVALID_HANDLE_VALUE, LocalFree},
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::IO::DeviceIoControl,
};

/// Reads a stable ACL representation without exposing user credentials.
fn acl(path: &Path, directory: bool) -> String {
    let pin = platform_fs::guarded_acl_path(path, directory).unwrap();
    unsafe {
        let mut descriptor = std::ptr::null_mut();
        assert_eq!(
            GetSecurityInfo(
                pin.file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut descriptor
            ),
            0
        );
        let mut text = std::ptr::null_mut();
        assert_ne!(
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                std::ptr::null_mut()
            ),
            0
        );
        let mut length = 0;
        while *text.add(length) != 0 {
            length += 1;
        }
        let value = String::from_utf16(std::slice::from_raw_parts(text, length)).unwrap();
        LocalFree(text.cast());
        LocalFree(descriptor);
        value
    }
}

/// Captures the unique run trustee before sandbox destruction frees its SID storage.
fn sid(sandbox: &Sandbox) -> String {
    unsafe {
        let mut text = std::ptr::null_mut();
        assert_ne!(
            ConvertSidToStringSidW(sandbox.security().capabilities.AppContainerSid, &mut text),
            0
        );
        let mut length = 0;
        while *text.add(length) != 0 {
            length += 1;
        }
        let value = String::from_utf16(std::slice::from_raw_parts(text, length)).unwrap();
        LocalFree(text.cast());
        value
    }
}

/// While one run is live, another run on the same folder (the Changes panel's Git beside an open
/// terminal, or a one-shot command) creates and replaces files. Those files inherit the live
/// run's entry as well as their creator's, and each run's cleanup must remove its own.
#[tokio::test]
#[ignore = "Requires the authenticated native broker, the pinned shell and private test data"]
async fn file_made_by_another_run_is_revoked() {
    use miniclaw_server::{executor, scoped::Operation};
    // MiniClaw's own events go to stderr when the runner asks for them.
    if std::env::var_os("MINICLAW_LOG").is_some() { miniclaw_server::logging::install().unwrap(); }
    let root = std::env::temp_dir().join(format!("mc-overlap-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    fs::write(root.join("existing.txt"), "old").unwrap();
    let cwd = root.to_string_lossy().into_owned();
    let grants = [Grant { path: cwd.clone(), write: true, folder: true, blocked: false }];
    fs::write(root.join("keep.txt"), "never replaced").unwrap();
    let untouched = [raw_acl(&root), raw_acl(&root.join("keep.txt"))];
    let first = Sandbox::new_shell(&grants, &root).unwrap();
    let trustee = sid(&first);
    let ran = executor::execute_host(Operation::RunShell { cwd,
        text: "echo made > made.txt; rm existing.txt; echo again > existing.txt; mkdir sub; echo deep > sub/deep.txt".into(), hosts: vec![] }, &grants).await;
    assert_eq!(ran["exitCode"], 0, "{ran}");
    miniclaw_server::windows_sandbox::drain_cleanup(std::time::Duration::from_secs(35)).unwrap();
    fs::write(root.join("host.txt"), "made by the host meanwhile").unwrap();
    let names = ["made.txt", "existing.txt", "sub", "sub/deep.txt", "host.txt"];
    for name in names {
        let text = raw_acl(&root.join(name));
        println!("OVERLAP live {name}: {text}");
        assert!(text.contains(&trustee), "{name} inherited the live run's entry: {text}");
    }
    drop(first);
    let drained = miniclaw_server::windows_sandbox::drain_cleanup(std::time::Duration::from_secs(35));
    println!("OVERLAP cleanup of the first run: {drained:?}");
    let mut left = Vec::new();
    for name in names {
        let text = raw_acl(&root.join(name));
        println!("OVERLAP after {name}: {text}");
        if text.contains("S-1-15-2-") { left.push(format!("{name}: {text}")); }
    }
    assert!(left.is_empty(), "sandbox entries remain after both runs were cleaned up: {left:#?}");
    assert!(!raw_acl(&root).contains("S-1-15-2-"));
    // The folder and a file nobody replaced have exactly the access lists they started with:
    // the same entries, each still marked inherited where it was.
    assert!(untouched[1].contains(";ID;"), "the fixture file had inherited entries: {}", untouched[1]);
    assert_eq!([raw_acl(&root), raw_acl(&root.join("keep.txt"))], untouched);
    fs::remove_dir_all(root).unwrap();
}

/// New descendants inherit access and every retained object loses it during cleanup.
#[test]
#[ignore = "Requires the authenticated native broker and private test data"]
fn new_descendants_are_revoked() {
    let root = std::env::temp_dir().join(format!("mc-inherit-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let first = root.join("first");
    let second = root.join("second");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    let grants = [first.clone(), second.clone()].map(|path| Grant {
        path: path.to_string_lossy().into_owned(),
        write: true,
        folder: true,
        blocked: false,
    });
    let before = [acl(&first, true), acl(&second, true)];
    let sandbox = Sandbox::new(&grants, &first).unwrap();
    let trustee = sid(&sandbox);
    let child = first.join("new-child");
    fs::create_dir(&child).unwrap();
    fs::write(child.join("created.txt"), "new contents").unwrap();
    assert!(
        acl(&child, true).contains(&trustee),
        "New directory lacks inherited run SID"
    );
    assert!(acl(&child.join("created.txt"), false).contains(&trustee));
    let moved = second.join("moved-child");
    fs::rename(&child, &moved).unwrap();
    drop(sandbox);
    miniclaw_server::windows_sandbox::drain_cleanup(std::time::Duration::from_secs(35)).unwrap();
    for (path, directory) in [
        (&first, true),
        (&second, true),
        (&moved, true),
        (&moved.join("created.txt"), false),
    ] {
        assert!(
            !acl(path, directory).contains(&trustee),
            "Run SID remains at {}",
            path.display()
        );
    }
    assert_eq!(
        [acl(&first, true), acl(&second, true)],
        before,
        "Unrelated root ACEs must remain unchanged"
    );
    fs::remove_dir_all(root).unwrap();
}

/// An overlapping denied subtree prevents all grant mutations, including inheritance.
#[test]
#[ignore = "Requires private native test data"]
fn blocked_subtree_is_unchanged() {
    let root = std::env::temp_dir().join(format!("mc-block-inherit-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let blocked = root.join("blocked");
    fs::create_dir(&blocked).unwrap();
    let before = acl(&blocked, true);
    let grants = [
        Grant {
            path: root.to_string_lossy().into_owned(),
            write: true,
            folder: true,
            blocked: false,
        },
        Grant {
            path: blocked.to_string_lossy().into_owned(),
            write: false,
            folder: true,
            blocked: true,
        },
    ];
    assert!(Sandbox::new(&grants, &root).is_err());
    assert_eq!(acl(&blocked, true), before);
    fs::remove_dir_all(root).unwrap();
}

/// Leaves a pre-launch orphan so recovery must discover children absent from its original journal.
#[test]
#[ignore = "Internal subprocess fixture for inherited ACL recovery"]
fn inheritance_crash_fixture() {
    let Some(root) = std::env::var_os("MINICLAW_INHERITANCE_CRASH_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let grant = Grant {
        path: root.to_string_lossy().into_owned(),
        write: true,
        folder: true,
        blocked: false,
    };
    let sandbox = Sandbox::new(&[grant], &root).unwrap();
    fs::create_dir(root.join("after-grant")).unwrap();
    fs::write(root.join("after-grant/new.txt"), "durable contents").unwrap();
    fs::write(root.join("run-sid.txt"), sid(&sandbox)).unwrap();
    std::mem::forget(sandbox);
    std::process::exit(0);
}

/// Recovery removes new inherited descendants after the original process disappears.
#[test]
#[ignore = "Requires the authenticated native broker and private test data"]
fn inherited_orphan_recovers() {
    let root = std::env::temp_dir().join(format!("mc-inherit-orphan-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "inheritance_crash_fixture",
            "--ignored",
            "--nocapture",
        ])
        .env("MINICLAW_INHERITANCE_CRASH_ROOT", &root)
        .status()
        .unwrap();
    assert!(status.success());
    let trustee = fs::read_to_string(root.join("run-sid.txt")).unwrap();
    assert!(acl(&root.join("after-grant/new.txt"), false).contains(&trustee));
    miniclaw_server::windows_sandbox::recover().unwrap();
    for (path, directory) in [
        (&root, true),
        (&root.join("after-grant"), true),
        (&root.join("after-grant/new.txt"), false),
        (&root.join("run-sid.txt"), false),
    ] {
        assert!(
            !acl(path, directory).contains(&trustee),
            "Recovered run SID remains at {}",
            path.display()
        );
    }
    fs::remove_dir_all(root).unwrap();
}

/// Opens the named object itself, including a link or a multiply linked file.
fn raw(path: &Path, access: u32) -> io::Result<File> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_handle(handle) })
}

/// Reads the DACL of the named object without the grant-time link guards.
fn raw_acl(path: &Path) -> String {
    let file = raw(path, READ_CONTROL).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    unsafe {
        let mut descriptor = std::ptr::null_mut();
        assert_eq!(
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut descriptor
            ),
            0
        );
        let mut text = std::ptr::null_mut();
        assert_ne!(
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                std::ptr::null_mut()
            ),
            0
        );
        let mut length = 0;
        while *text.add(length) != 0 {
            length += 1;
        }
        let value = String::from_utf16(std::slice::from_raw_parts(text, length)).unwrap();
        LocalFree(text.cast());
        LocalFree(descriptor);
        value
    }
}

/// Turns a new empty directory into a junction; no privilege is involved.
fn junction(link: &Path, target: &Path) -> io::Result<()> {
    fs::create_dir(link)?;
    let file = raw(link, FILE_GENERIC_WRITE)?;
    let plain = target.to_str().unwrap().trim_start_matches(r"\\?\");
    let substitute: Vec<u16> = format!(r"\??\{plain}").encode_utf16().collect();
    let print: Vec<u16> = plain.encode_utf16().collect();
    let mut names: Vec<u16> = substitute.clone();
    names.push(0);
    names.extend(&print);
    names.push(0);
    let mut buffer = Vec::new();
    buffer.extend(0xA000_0003u32.to_le_bytes());
    buffer.extend(((8 + names.len() * 2) as u16).to_le_bytes());
    buffer.extend(0u16.to_le_bytes());
    buffer.extend(0u16.to_le_bytes());
    buffer.extend(((substitute.len() * 2) as u16).to_le_bytes());
    buffer.extend(((substitute.len() * 2 + 2) as u16).to_le_bytes());
    buffer.extend(((print.len() * 2) as u16).to_le_bytes());
    buffer.extend(names.iter().flat_map(|unit| unit.to_le_bytes()));
    let mut returned = 0;
    // FSCTL_SET_REPARSE_POINT
    let set = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            0x0009_00A4,
            buffer.as_ptr().cast(),
            buffer.len() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if set == 0 {
        let error = io::Error::last_os_error();
        drop(file);
        let _ = fs::remove_dir(link);
        return Err(error);
    }
    Ok(())
}

/// Lists active recovery records in the private test data directory (none before the first launch).
fn journals() -> Vec<PathBuf> {
    let root = PathBuf::from(std::env::var_os("MINICLAW_DATA_DIR").unwrap()).join("sandbox-journals");
    let Ok(entries) = fs::read_dir(root) else { return Vec::new() };
    entries
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "jsonl"))
        .collect()
}

/// Grants one writable folder plus read access to this test executable.
fn folder_grants(folder: &Path) -> Vec<Grant> {
    vec![
        Grant {
            path: folder.to_string_lossy().into_owned(),
            write: true,
            folder: true,
            blocked: false,
        },
        Grant {
            path: std::env::current_exe().unwrap().to_string_lossy().into_owned(),
            write: false,
            folder: false,
            blocked: false,
        },
    ]
}

/// Builds the child environment: sandbox redirections plus one fixture variable.
fn child_env(sandbox: &Sandbox, name: &str, value: &Path) -> Vec<(String, String)> {
    let mut env: Vec<_> = sandbox
        .environment()
        .into_iter()
        .map(|(k, v)| (k.into_string().unwrap(), v.into_string().unwrap()))
        .collect();
    env.push((name.into(), value.to_string_lossy().into_owned()));
    env
}

/// Selects one ignored fixture of this binary as the sandboxed program's arguments.
fn fixture_args(name: &str) -> Vec<String> {
    ["--exact", name, "--ignored", "--nocapture"].map(str::to_owned).to_vec()
}

/// Runs inside the real LPAC token and reports which link kinds Windows let it create.
#[test]
#[ignore = "Internal LPAC fixture for link creation"]
fn link_child() {
    let Some(root) = std::env::var_os("MINICLAW_LINK_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let scratch = PathBuf::from(std::env::var_os("TEMP").unwrap());
    let outcome = |result: io::Result<()>| match result {
        Ok(()) => "created".to_owned(),
        Err(error) => format!("refused:{}", error.raw_os_error().unwrap_or(-1)),
    };
    fs::write(root.join("made.txt"), "made inside").unwrap();
    let report = format!(
        "hardlink-existing={}\nhardlink-new={}\njunction={}\nscratch-junction={}\nscratch-hardlink={}\n",
        outcome(fs::hard_link(root.join("existing.txt"), root.join("existing-link.txt"))),
        outcome(fs::hard_link(root.join("made.txt"), root.join("made-link.txt"))),
        outcome(junction(&root.join("junction"), &root.join("target"))),
        outcome(junction(&scratch.join("junction"), &root.join("target"))),
        outcome(fs::hard_link(root.join("made.txt"), scratch.join("made-link.txt"))),
    );
    fs::write(root.join("link-report.txt"), report).unwrap();
}

/// Hard links and junctions made by a sandboxed command must not strand its cleanup.
#[tokio::test]
#[ignore = "Requires the authenticated native broker and private test data"]
async fn sandbox_links_do_not_block_cleanup() {
    let root = std::env::temp_dir().join(format!("mc-links-{}", uuid::Uuid::new_v4()));
    let work = root.join("work");
    fs::create_dir_all(work.join("target")).unwrap();
    fs::write(work.join("existing.txt"), "existing").unwrap();
    fs::write(work.join("target/keep.txt"), "keep").unwrap();
    let sandbox = Sandbox::new(&folder_grants(&work), &work).unwrap();
    let trustee = sid(&sandbox);
    let scratch = sandbox.scratch().to_owned();
    let (out, err, code) = miniclaw_server::windows_process::capture_restricted(
        &std::env::current_exe().unwrap(),
        &fixture_args("link_child"),
        &work,
        &child_env(&sandbox, "MINICLAW_LINK_ROOT", &work),
        &sandbox.security(),
    )
    .await
    .unwrap();
    assert_eq!(code, 0, "{} {}", String::from_utf8_lossy(&out), String::from_utf8_lossy(&err));
    let report = fs::read_to_string(work.join("link-report.txt")).unwrap();
    println!("LPAC link creation report:\n{report}");
    // Whatever Windows refused inside LPAC can still be made by a host tool mid-run.
    if !work.join("existing-link.txt").exists() {
        fs::hard_link(work.join("existing.txt"), work.join("existing-link.txt")).unwrap();
    }
    if !work.join("made-link.txt").exists() {
        fs::hard_link(work.join("made.txt"), work.join("made-link.txt")).unwrap();
    }
    if !work.join("junction").exists() {
        junction(&work.join("junction"), &work.join("target")).unwrap();
    }
    if !scratch.join("junction").exists() {
        junction(&scratch.join("junction"), &work.join("target")).unwrap();
    }
    assert!(raw_acl(&work.join("made-link.txt")).contains(&trustee));
    drop(sandbox);
    miniclaw_server::windows_sandbox::drain_cleanup(std::time::Duration::from_secs(120))
        .expect("Links created during a run must not strand cleanup");
    for name in [
        "",
        "existing.txt",
        "existing-link.txt",
        "made.txt",
        "made-link.txt",
        "junction",
        "target",
        "target/keep.txt",
    ] {
        assert!(
            !raw_acl(&work.join(name)).contains(&trustee),
            "Run SID remains at {name}"
        );
    }
    assert!(
        fs::symlink_metadata(work.join("junction")).unwrap().file_type().is_symlink(),
        "Cleanup must keep the user's junction"
    );
    assert_eq!(fs::read_to_string(work.join("target/keep.txt")).unwrap(), "keep");
    assert!(!scratch.exists(), "Scratch must be removed without following its junction");
    assert!(journals().is_empty(), "{:?}", journals());
    fs::remove_dir(work.join("junction")).unwrap();
    fs::remove_dir_all(root).unwrap();
}

/// Works on a folder through hard links inside the real LPAC token, then adds more links.
#[test]
#[ignore = "Internal LPAC fixture for hard links inside a granted folder"]
fn relink_child() {
    let Some(root) = std::env::var_os("MINICLAW_RELINK_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let read = |name: &str| fs::read_to_string(root.join(name)).unwrap();
    let append = |name: &str, text: &str| {
        use std::io::Write;
        let mut file = fs::OpenOptions::new().append(true).open(root.join(name)).unwrap();
        file.write_all(text.as_bytes()).unwrap();
    };
    if !root.join("made.txt").exists() {
        // First launch: both existing names reach one file; then link as `ln` would.
        assert_eq!(read("a.txt"), "one");
        append("b.txt", "+first");
        assert_eq!(read("a.txt"), "one+first");
        fs::write(root.join("made.txt"), "made").unwrap();
        fs::hard_link(root.join("made.txt"), root.join("made-link.txt")).unwrap();
        fs::hard_link(root.join("a.txt"), root.join("sub/c.txt")).unwrap();
        // A file this run may only read cannot be given a new name in the writable folder.
        let program = std::env::current_exe().unwrap();
        assert!(fs::read(&program).is_ok(), "The read-only grant must be readable");
        let pulled = fs::hard_link(&program, root.join("pulled-in.exe"));
        assert!(pulled.is_err(), "A sandboxed process linked a file it cannot write");
        fs::write(root.join("round.txt"), "first").unwrap();
    } else {
        // Second launch: every name made by the first is readable and writable.
        assert_eq!(read("sub/c.txt"), "one+first");
        append("sub/c.txt", "+second");
        assert_eq!(read("b.txt"), "one+first+second");
        assert_eq!(read("made-link.txt"), "made");
        append("made-link.txt", "+second");
        assert_eq!(read("made.txt"), "made+second");
        fs::write(root.join("round.txt"), "second").unwrap();
    }
}

/// A folder whose files are hard-linked to each other stays usable launch after launch.
#[tokio::test]
#[ignore = "Requires the authenticated native broker and private test data"]
async fn internal_hard_links_are_granted() {
    let root = std::env::temp_dir().join(format!("mc-relink-{}", uuid::Uuid::new_v4()));
    let work = root.join("work");
    fs::create_dir_all(work.join("sub")).unwrap();
    fs::write(work.join("a.txt"), "one").unwrap();
    fs::hard_link(work.join("a.txt"), work.join("b.txt")).unwrap();
    for round in ["first", "second"] {
        let sandbox = Sandbox::new(&folder_grants(&work), &work)
            .unwrap_or_else(|error| panic!("{round} launch on internally linked files: {error}"));
        let trustee = sid(&sandbox);
        for name in ["a.txt", "b.txt"] {
            assert!(raw_acl(&work.join(name)).contains(&trustee), "{round}: no grant at {name}");
        }
        let (out, err, code) = miniclaw_server::windows_process::capture_restricted(
            &std::env::current_exe().unwrap(),
            &fixture_args("relink_child"),
            &work,
            &child_env(&sandbox, "MINICLAW_RELINK_ROOT", &work),
            &sandbox.security(),
        )
        .await
        .unwrap();
        assert_eq!(code, 0, "{} {}", String::from_utf8_lossy(&out), String::from_utf8_lossy(&err));
        assert_eq!(fs::read_to_string(work.join("round.txt")).unwrap(), round);
        drop(sandbox);
        miniclaw_server::windows_sandbox::drain_cleanup(std::time::Duration::from_secs(120))
            .expect("Cleanup after a run on linked files");
        for name in ["", "a.txt", "b.txt", "sub/c.txt", "made.txt", "made-link.txt", "round.txt"] {
            assert!(!raw_acl(&work.join(name)).contains(&trustee), "{round}: run SID remains at {name}");
        }
        assert!(journals().is_empty(), "{:?}", journals());
    }
    assert_eq!(fs::read_to_string(work.join("a.txt")).unwrap(), "one+first+second");
    fs::remove_dir_all(root).unwrap();
}

/// Expects a launch to be refused for a hard link, with nothing granted and nothing left over.
fn refused_for_link(grants: &[Grant], cwd: &Path, inside: &Path, hidden: &str) {
    let before = raw_acl(inside);
    let error = match Sandbox::new(grants, cwd) {
        Ok(_) => panic!("A file linked outside the grant must refuse the launch"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{error}");
    let text = error.to_string();
    assert!(text.contains("hard link"), "The refusal must name its reason: {text}");
    assert!(text.contains(inside.file_name().unwrap().to_str().unwrap()), "{text}");
    assert!(!text.contains(hidden), "The refusal must not name the other path: {text}");
    miniclaw_server::windows_sandbox::drain_cleanup(std::time::Duration::from_secs(60)).unwrap();
    assert_eq!(raw_acl(inside), before, "A refused launch must leave the file's ACL unchanged");
    assert!(journals().is_empty(), "{:?}", journals());
}

/// A granted file that is also reachable under a name outside the grant is still refused.
#[test]
#[ignore = "Requires the authenticated native broker and private test data"]
fn outside_hard_link_is_refused() {
    let root = std::env::temp_dir().join(format!("mc-outlink-{}", uuid::Uuid::new_v4()));
    let work = root.join("work");
    let elsewhere = root.join("elsewhere-7c1d");
    let second = root.join("second");
    for folder in [&work, &elsewhere, &second] {
        fs::create_dir_all(folder).unwrap();
    }
    fs::write(work.join("shared.txt"), "shared").unwrap();
    fs::hard_link(work.join("shared.txt"), elsewhere.join("store.txt")).unwrap();
    let folder = |path: &Path, write: bool| Grant {
        path: path.to_string_lossy().into_owned(),
        write,
        folder: true,
        blocked: false,
    };
    // The other name lies in no granted folder.
    refused_for_link(&folder_grants(&work), &work, &work.join("shared.txt"), "elsewhere-7c1d");
    // The other name lies in a folder granted for reading only: writing here would write there.
    let mut grants = folder_grants(&work);
    grants.push(folder(&elsewhere, false));
    refused_for_link(&grants, &work, &work.join("shared.txt"), "store.txt");
    // Both names lie in folders granted for writing: nothing beyond the approval is reached.
    let mut grants = folder_grants(&work);
    grants.push(folder(&elsewhere, true));
    let sandbox = Sandbox::new(&grants, &work)
        .unwrap_or_else(|error| panic!("Both names are inside writable grants: {error}"));
    let trustee = sid(&sandbox);
    assert!(raw_acl(&elsewhere.join("store.txt")).contains(&trustee));
    drop(sandbox);
    miniclaw_server::windows_sandbox::drain_cleanup(std::time::Duration::from_secs(60)).unwrap();
    assert!(!raw_acl(&work.join("shared.txt")).contains(&trustee));
    // A read-only grant is refused as well when the other name is outside every grant.
    fs::write(second.join("doc.txt"), "doc").unwrap();
    fs::hard_link(second.join("doc.txt"), root.join("doc-outside-91ab.txt")).unwrap();
    let mut grants = folder_grants(&work);
    grants.push(folder(&second, false));
    grants.push(folder(&elsewhere, true));
    refused_for_link(&grants, &work, &second.join("doc.txt"), "doc-outside-91ab");
    assert!(journals().is_empty(), "{:?}", journals());
    fs::remove_dir_all(root).unwrap();
}

/// A link to a file under a protected root never makes that file grantable.
#[test]
#[ignore = "Requires the authenticated native broker and private test data"]
fn protected_hard_link_is_refused() {
    let root = std::env::temp_dir().join(format!("mc-protlink-{}", uuid::Uuid::new_v4()));
    let work = root.join("work");
    fs::create_dir_all(&work).unwrap();
    // The private data folder is a protected root for every run.
    let data = PathBuf::from(std::env::var_os("MINICLAW_DATA_DIR").unwrap());
    let secret = data.join(format!("link-probe-{}.txt", uuid::Uuid::new_v4().simple()));
    fs::write(&secret, "private state").unwrap();
    fs::hard_link(&secret, work.join("innocent.txt")).unwrap();
    let hidden = secret.file_name().unwrap().to_str().unwrap().to_owned();
    refused_for_link(&folder_grants(&work), &work, &work.join("innocent.txt"), &hidden);
    // Granting the protected folder itself is refused before any file is looked at.
    let mut grants = folder_grants(&work);
    grants.push(Grant { path: data.to_string_lossy().into_owned(), write: true, folder: true, blocked: false });
    assert!(Sandbox::new(&grants, &work).is_err(), "A protected root cannot be granted");
    miniclaw_server::windows_sandbox::drain_cleanup(std::time::Duration::from_secs(60)).unwrap();
    fs::remove_file(work.join("innocent.txt")).unwrap();
    fs::remove_file(&secret).unwrap();
    fs::remove_dir_all(root).unwrap();
}

/// A build that creates more objects than a launch may stamp must still be cleaned up.
#[test]
#[ignore = "Requires the authenticated native broker and private test data"]
fn large_tree_is_revoked() {
    let root = std::env::temp_dir().join(format!("mc-large-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let grant = Grant {
        path: root.to_string_lossy().into_owned(),
        write: true,
        folder: true,
        blocked: false,
    };
    let sandbox = Sandbox::new(&[grant], &root).unwrap();
    let trustee = sid(&sandbox);
    let started = std::time::Instant::now();
    for directory in 0..101 {
        let path = root.join(format!("d{directory:03}"));
        fs::create_dir(&path).unwrap();
        for file in 0..1000 {
            File::create(path.join(format!("f{file:04}"))).unwrap();
        }
    }
    println!("Created 101101 objects in {:?}", started.elapsed());
    assert!(raw_acl(&root.join("d100/f0999")).contains(&trustee));
    let started = std::time::Instant::now();
    drop(sandbox);
    miniclaw_server::windows_sandbox::drain_cleanup(std::time::Duration::from_secs(900))
        .expect("A tree above the launch limits must still be revoked");
    println!("Revoked 101101 objects in {:?}", started.elapsed());
    for name in ["", "d000", "d000/f0000", "d050/f0500", "d100", "d100/f0999"] {
        assert!(
            !raw_acl(&root.join(name)).contains(&trustee),
            "Run SID remains at {name}"
        );
    }
    assert!(journals().is_empty(), "{:?}", journals());
    fs::remove_dir_all(root).unwrap();
}

/// Stays alive inside the sandbox until the job is destroyed.
#[test]
#[ignore = "Internal LPAC fixture that outlives its launcher"]
fn sleep_child() {
    let Some(root) = std::env::var_os("MINICLAW_SLEEP_ROOT") else {
        return;
    };
    fs::write(PathBuf::from(root).join("child-started.txt"), std::process::id().to_string()).unwrap();
    std::thread::sleep(std::time::Duration::from_secs(300));
}

/// Plays a backend that dies while its sandboxed command is still running.
#[tokio::test]
#[ignore = "Internal subprocess fixture for backend-only exit"]
async fn backend_exit_fixture() {
    let Some(root) = std::env::var_os("MINICLAW_BACKEND_EXIT_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let work = root.join("work");
    let mut sandbox = Sandbox::new(&folder_grants(&work), &work).unwrap();
    let pending = miniclaw_server::windows_process::pending_stdio_restricted(
        &std::env::current_exe().unwrap(),
        &fixture_args("sleep_child"),
        &work,
        &child_env(&sandbox, "MINICLAW_SLEEP_ROOT", &work),
        &sandbox.security(),
    )
    .unwrap();
    sandbox.bind_process(&pending.server.process).unwrap();
    let server = pending.resume().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !work.join("child-started.txt").exists() {
        assert!(std::time::Instant::now() < deadline, "Sandboxed child did not start");
        assert!(server.process.try_wait().unwrap().is_none(), "Sandboxed child exited early");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    fs::write(root.join("run-sid.txt"), sid(&sandbox)).unwrap();
    // No destructor runs: the job handle closes with the process, as in a crash or End task.
    std::process::exit(0);
}

/// A backend-only exit destroys the job; recovery must not then demand a Windows restart.
#[test]
#[ignore = "Requires the authenticated native broker and private test data"]
fn backend_exit_recovers_without_restart() {
    let root = std::env::temp_dir().join(format!("mc-backend-exit-{}", uuid::Uuid::new_v4()));
    let work = root.join("work");
    fs::create_dir_all(&work).unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(fixture_args("backend_exit_fixture"))
        .env("MINICLAW_BACKEND_EXIT_ROOT", &root)
        .status()
        .unwrap();
    assert!(status.success());
    let trustee = fs::read_to_string(root.join("run-sid.txt")).unwrap();
    let records = journals();
    assert_eq!(records.len(), 1, "{records:?}");
    let pid: u32 = fs::read_to_string(work.join("child-started.txt")).unwrap().parse().unwrap();
    println!("Orphaned sandbox child was pid {pid}");
    assert!(raw_acl(&work).contains(&trustee));
    miniclaw_server::windows_sandbox::recover()
        .expect("A destroyed kill-on-close job with no surviving process needs no restart");
    assert!(journals().is_empty(), "{:?}", journals());
    for name in ["", "child-started.txt"] {
        assert!(
            !raw_acl(&work.join(name)).contains(&trustee),
            "Run SID remains at {name}"
        );
    }
    fs::remove_dir_all(root).unwrap();
}

/// Exits at once so the launcher can prove its job empty.
#[test]
#[ignore = "Internal LPAC fixture that exits immediately"]
fn quick_child() {}

/// A cleanup that fails after the job was proven empty can be retried in the same boot.
#[tokio::test]
#[ignore = "Requires the authenticated native broker and private test data"]
async fn failed_cleanup_retries_without_restart() {
    use std::os::windows::fs::OpenOptionsExt;
    let root = std::env::temp_dir().join(format!("mc-retry-{}", uuid::Uuid::new_v4()));
    let work = root.join("work");
    fs::create_dir_all(&work).unwrap();
    let mut sandbox = Sandbox::new(&folder_grants(&work), &work).unwrap();
    let trustee = sid(&sandbox);
    let pending = miniclaw_server::windows_process::pending_stdio_restricted(
        &std::env::current_exe().unwrap(),
        &fixture_args("quick_child"),
        &work,
        &child_env(&sandbox, "MINICLAW_UNUSED", &work),
        &sandbox.security(),
    )
    .unwrap();
    sandbox.bind_process(&pending.server.process).unwrap();
    let (out, err, code) =
        miniclaw_server::windows_process::capture_stdio(pending.resume().unwrap()).await.unwrap();
    assert_eq!(code, 0, "{} {}", String::from_utf8_lossy(&out), String::from_utf8_lossy(&err));
    // A host program holding a scratch file without delete sharing makes cleanup fail.
    let held = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(sandbox.scratch().join("held.txt"))
        .unwrap();
    drop(sandbox);
    assert!(
        miniclaw_server::windows_sandbox::drain_cleanup(std::time::Duration::from_secs(60)).is_err(),
        "Cleanup cannot claim success while scratch remains"
    );
    assert_eq!(journals().len(), 1, "The unfinished record must be retained");
    drop(held);
    miniclaw_server::windows_sandbox::recover()
        .expect("The job was already proven empty; retrying needs no Windows restart");
    assert!(journals().is_empty(), "{:?}", journals());
    assert!(!raw_acl(&work).contains(&trustee));
    fs::remove_dir_all(root).unwrap();
}

/// Reports what a launch and its cleanup cost on a large existing folder.
#[test]
#[ignore = "Measurement; requires the authenticated native broker and private test data"]
fn stamp_cost_report() {
    let root = std::env::temp_dir().join(format!("mc-stamp-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    for directory in 0..50 {
        let path = root.join(format!("d{directory:02}"));
        fs::create_dir(&path).unwrap();
        for file in 0..1000 {
            File::create(path.join(format!("f{file:04}"))).unwrap();
        }
    }
    let grant = Grant {
        path: root.to_string_lossy().into_owned(),
        write: true,
        folder: true,
        blocked: false,
    };
    for round in 0..2 {
        let started = std::time::Instant::now();
        let sandbox = Sandbox::new(std::slice::from_ref(&grant), &root).unwrap();
        let granted = started.elapsed();
        let trustee = sid(&sandbox);
        assert!(raw_acl(&root.join("d49/f0999")).contains(&trustee));
        let started = std::time::Instant::now();
        drop(sandbox);
        miniclaw_server::windows_sandbox::drain_cleanup(std::time::Duration::from_secs(900)).unwrap();
        println!(
            "STAMP round={round} objects=50051 grant_ms={} cleanup_ms={}",
            granted.as_millis(),
            started.elapsed().as_millis()
        );
        assert!(!raw_acl(&root.join("d49/f0999")).contains(&trustee));
        assert!(!raw_acl(&root).contains(&trustee));
    }
    fs::remove_dir_all(root).unwrap();
}

/// Gives an object a protected access list that leaves this user nothing to manage it with:
/// SYSTEM only, and an OWNER RIGHTS entry that leaves the owner only `DELETE` (no `READ_CONTROL`,
/// `WRITE_DAC` or `SYNCHRONIZE`). It stands for the WebView2 folders an elevated Roblox Studio left
/// in its profile (Administrators and SYSTEM only), and opens for neither whoever created it.
/// Its folder still lists it, and `remove_locked` deletes it, so the tests clean up.
fn lock_out(path: &Path) {
    let sddl: Vec<u16> = "D:P(A;OICI;FA;;;SY)(A;;SD;;;OW)\0".encode_utf16().collect();
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let mut descriptor = std::ptr::null_mut();
        assert_ne!(ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), 1, &mut descriptor, std::ptr::null_mut()), 0);
        let set = SetFileSecurityW(wide.as_ptr(), DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, descriptor);
        LocalFree(descriptor);
        assert_ne!(set, 0, "{}: {}", path.display(), io::Error::last_os_error());
    }
    assert_eq!(raw(path, READ_CONTROL | WRITE_DAC).err().and_then(|error| error.raw_os_error()), Some(5),
        "the fixture must be unmanageable for this user: {}", path.display());
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
    assert!(fs::symlink_metadata(path).is_err(), "{} is gone", path.display());
}

/// The folder of retired records in the private test data directory.
fn retired() -> Vec<PathBuf> {
    let root = PathBuf::from(std::env::var_os("MINICLAW_DATA_DIR").unwrap()).join("sandbox-journals").join("retired");
    fs::read_dir(root).map(|entries| entries.map(|entry| entry.unwrap().path()).collect()).unwrap_or_default()
}

/// A folder that holds an item this user cannot manage is refused before anything exists: no
/// record, no profile, no entry on any object, whether the grant writes or only reads, and
/// whichever way the launch comes (a tool server, the shell and terminal, Git). The words name
/// the item, and the shell's refusal carries them to the conversation.
#[tokio::test]
#[ignore = "Requires the authenticated native broker, the pinned shell and private test data"]
async fn unmanageable_item_refuses_before_any_record() {
    use miniclaw_server::{executor, scoped::Operation, windows_sandbox};
    let root = std::env::temp_dir().join(format!("mc-unmanageable-{}", uuid::Uuid::new_v4()));
    let locked = root.join("RobloxStudio").join("WebView2").join("hyphen-data");
    fs::create_dir_all(&locked).unwrap();
    fs::write(root.join("plain.txt"), "readable").unwrap();
    lock_out(&locked);
    let untouched = [raw_acl(&root), raw_acl(&root.join("plain.txt")), raw_acl(&root.join("RobloxStudio"))];
    let records = journals().len();
    let cwd = root.to_string_lossy().into_owned();
    for write in [true, false] {
        let grants = [Grant { path: cwd.clone(), write, folder: true, blocked: false }];
        for refused in [Sandbox::new(&grants, &root).err(), Sandbox::new_shell(&grants, &root).err(),
            Sandbox::with_private_cwd(&grants).err()] {
            let error = refused.expect("the launch is refused");
            let words = windows_sandbox::refusal(&error).unwrap_or_else(|| panic!("not a refusal for the user: {error:?}"));
            println!("UNMANAGEABLE write={write}: {words}");
            assert!(words.contains("cannot manage") && words.contains(&*locked.to_string_lossy()), "{words}");
        }
        let ran = executor::execute_host(Operation::RunShell { cwd: cwd.clone(), text: "echo never".into(), hosts: vec![] }, &grants).await;
        println!("UNMANAGEABLE shell write={write}: {ran}");
        assert_eq!(ran["status"], "denied", "{ran}");
        let text = ran["text"].as_str().unwrap();
        assert!(text.contains("cannot manage") && text.contains(&*locked.to_string_lossy()), "{text}");
    }
    assert_eq!(journals().len(), records, "a refused launch leaves no record: {:?}", journals());
    assert_eq!([raw_acl(&root), raw_acl(&root.join("plain.txt")), raw_acl(&root.join("RobloxStudio"))], untouched,
        "a refused launch changes no access list");
    remove_locked(&locked);
    fs::remove_dir_all(root).unwrap();
}

/// Leaves a pre-launch record whose writable folder gains, during the run, an item this user
/// cannot open: what another account or an elevated program can do there at any time.
#[test]
#[ignore = "Internal subprocess fixture for unreadable descendants"]
fn unreadable_descendant_fixture() {
    let Some(root) = std::env::var_os("MINICLAW_UNREADABLE_ROOT") else { return };
    let root = PathBuf::from(root);
    let grant = Grant { path: root.to_string_lossy().into_owned(), write: true, folder: true, blocked: false };
    let sandbox = Sandbox::new(&[grant], &root).unwrap();
    fs::write(root.join("run-sid.txt"), sid(&sandbox)).unwrap();
    fs::create_dir(root.join("made-elsewhere")).unwrap();
    lock_out(&root.join("made-elsewhere"));
    std::mem::forget(sandbox);
    std::process::exit(0);
}

/// Recovery does not stay stuck on a descendant it cannot open that MiniClaw never granted:
/// with the run proven stopped and its profile deleted, the record retires and names the item as
/// unreadable (removal not claimed), everything readable is cleaned, the next start validates the
/// archive, and the sandbox stays available.
#[test]
#[ignore = "Requires the authenticated native broker and private test data"]
fn unreadable_descendant_is_retired_not_retained() {
    let root = std::env::temp_dir().join(format!("mc-unreadable-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    fs::write(root.join("plain.txt"), "readable").unwrap();
    let archived = retired().len();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(fixture_args("unreadable_descendant_fixture")).env("MINICLAW_UNREADABLE_ROOT", &root).status().unwrap();
    assert!(status.success());
    let trustee = fs::read_to_string(root.join("run-sid.txt")).unwrap();
    assert_eq!(journals().len(), 1, "{:?}", journals());
    assert!(raw_acl(&root.join("plain.txt")).contains(&trustee));
    miniclaw_server::windows_sandbox::recover().expect("an unreadable descendant never granted does not keep the record");
    assert!(journals().is_empty(), "{:?}", journals());
    assert_eq!(miniclaw_server::windows_sandbox::recovery_problem(), None);
    let archives = retired();
    assert_eq!(archives.len(), archived + 1, "{archives:?}");
    let text = fs::read_to_string(archives.iter().max_by_key(|path| fs::metadata(path).unwrap().modified().unwrap()).unwrap()).unwrap();
    let certificate: serde_json::Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
    println!("UNREADABLE certificate: {certificate}");
    let named = certificate["retirement"]["unknown"].as_array().unwrap();
    assert!(named.iter().any(|item| item["kind"] == "Unreadable"
        && item["object"]["path"].as_str() == Some(&*root.join("made-elsewhere").to_string_lossy())), "{certificate}");
    for name in ["", "plain.txt", "run-sid.txt"] {
        assert!(!raw_acl(&root.join(name)).contains(&trustee), "Run SID remains at {name}");
    }
    miniclaw_server::windows_sandbox::recover().expect("the archive validates at the next start");
    remove_locked(&root.join("made-elsewhere"));
    fs::remove_dir_all(root).unwrap();
}

/// Leaves a pre-launch record that granted an ordinary file.
#[test]
#[ignore = "Internal subprocess fixture for an unreadable grant"]
fn unreadable_grant_fixture() {
    let Some(root) = std::env::var_os("MINICLAW_UNREADABLE_ROOT") else { return };
    let root = PathBuf::from(root);
    let grant = Grant { path: root.to_string_lossy().into_owned(), write: true, folder: true, blocked: false };
    let sandbox = Sandbox::new(&[grant], &root).unwrap();
    fs::write(root.join("run-sid.txt"), sid(&sandbox)).unwrap();
    std::mem::forget(sandbox);
    std::process::exit(0);
}

/// An object MiniClaw did give the run's entry, which this user can no longer open, stays
/// genuinely uncertain: its record is kept and named. The backend does not stop over it: the gate
/// closes for sandboxed work with that reason, a launch is refused with it, and once the object
/// is dealt with a repair (recovery again) opens the gate.
#[test]
#[ignore = "Requires the authenticated native broker and private test data"]
fn unreadable_grant_keeps_its_record_and_pauses_only_the_sandbox() {
    use miniclaw_server::{windows_gate, windows_sandbox};
    let root = std::env::temp_dir().join(format!("mc-unreadable-grant-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    fs::write(root.join("granted.txt"), "stamped, then locked").unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(fixture_args("unreadable_grant_fixture")).env("MINICLAW_UNREADABLE_ROOT", &root).status().unwrap();
    assert!(status.success());
    let trustee = fs::read_to_string(root.join("run-sid.txt")).unwrap();
    assert!(raw_acl(&root.join("granted.txt")).contains(&trustee));
    lock_out(&root.join("granted.txt"));
    let error = windows_sandbox::recover().expect_err("a granted object that cannot be opened stays uncertain");
    let words = windows_sandbox::refusal(&error).unwrap_or_else(|| panic!("not a refusal for the user: {error:?}")).to_owned();
    println!("UNREADABLE GRANT: {words}");
    assert!(words.contains("retained 1 unresolved") && words.contains("granted.txt"), "{words}");
    assert_eq!(journals().len(), 1);
    assert!(windows_sandbox::recovery_problem().is_some_and(|problem| problem.contains("granted.txt")));
    windows_gate::forget();
    let reason = miniclaw_server::seatbelt::unavailable_reason().expect("the gate is closed for sandboxed work");
    println!("UNREADABLE GRANT gate: {reason}");
    assert!(reason.contains("granted.txt") && reason.contains("Chat still works"), "{reason}");
    let other = std::env::temp_dir().join(format!("mc-unreadable-other-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&other).unwrap();
    let grants = [Grant { path: other.to_string_lossy().into_owned(), write: true, folder: true, blocked: false }];
    let refused = Sandbox::new(&grants, &other).err().expect("admission refuses while the record is unresolved");
    assert!(windows_sandbox::refusal(&refused).is_some_and(|words| words.contains("granted.txt")), "{refused}");
    // Dealt with (here: removed by its folder's owner); recovery then resolves it and opens the gate.
    remove_locked(&root.join("granted.txt"));
    windows_sandbox::recover().expect("the record resolves once its object is gone");
    assert!(journals().is_empty(), "{:?}", journals());
    windows_gate::forget();
    assert!(!miniclaw_server::seatbelt::unavailable_reason().is_some_and(|reason| reason.contains("granted.txt")));
    for name in ["", "run-sid.txt"] {
        assert!(!raw_acl(&root.join(name)).contains(&trustee), "Run SID remains at {name}");
    }
    fs::remove_dir_all(other).unwrap();
    fs::remove_dir_all(root).unwrap();
}
