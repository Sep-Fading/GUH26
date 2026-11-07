#![cfg(windows)]
// Exercises actual Git without shell parsing.
use miniclaw_server::{executor, platform_fs, scoped::Operation, seatbelt::Grant, windows_sandbox};
// Keeps disposable repository and sibling evidence.
use std::{fs, time::Duration};

/// Git handles literal Windows filenames while outside files remain inaccessible.
#[tokio::test]
#[ignore = "requires installed private Git, bundled shell and authenticated Windows broker"]
async fn git_arguments_and_boundary() {
    assert!(miniclaw_server::windows_git::load().unwrap().is_some());
    let root = std::env::temp_dir().join(format!("mc-git-command-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&root, false).unwrap();
    let work = root.join("work '雪");
    fs::create_dir(&work).unwrap();
    own_as_user(&work);
    let secret = root.join("outside.txt");
    fs::write(&secret, b"OUTSIDE_GIT_SENTINEL").unwrap();
    let cwd = work.to_string_lossy().into_owned();
    let grants = vec![Grant { path: cwd.clone(), folder: true, write: true, blocked: false }];
    cwd_probe(&cwd, &grants).await;
    let filename = "literal '$x; &(雪).txt";
    fs::write(work.join(filename), "literal-content\n").unwrap();
    for arguments in [vec!["--version"], vec!["init", "--quiet"], vec!["add", "--", filename]] {
        println!("Git arguments: {arguments:?}");
        let result = executor::execute_git(&arguments, &cwd, &grants).await;
        assert_eq!(result["exitCode"], 0, "{result}");
        windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
        if arguments[0] == "init" { own_as_user(&work.join(".git")); }
    }
    let status = executor::execute_git(&["status", "--porcelain=v1", "-z"], &cwd, &grants).await;
    assert_eq!(status["exitCode"], 0, "{status}");
    assert_eq!(status["text"], format!("A  {filename}\0"));
    windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    let result = executor::execute_git(&["diff", "--cached", "--name-only", "-z"], &cwd, &grants).await;
    assert_eq!(result["exitCode"], 0, "{result}");
    assert_eq!(result["text"], format!("{filename}\0"));
    windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    let message = "first '雪 $x; &(commit)";
    let commit = executor::execute_git(&["-c", "user.name=MiniClaw Test", "-c", "user.email=test@example.invalid",
        "commit", "--quiet", "-m", message], &cwd, &grants).await;
    assert_eq!(commit["exitCode"], 0, "{commit}");
    windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    let log = executor::execute_git(&["log", "-1", "--format=%s", "--name-only"], &cwd, &grants).await;
    assert_eq!(log["exitCode"], 0, "{log}");
    assert_eq!(log["text"], format!("{message}\n\n{filename}\n"));
    windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    let clean = executor::execute_git(&["status", "--porcelain=v1", "-z"], &cwd, &grants).await;
    assert_eq!(clean["exitCode"], 0, "{clean}");
    assert_eq!(clean["text"], "");
    windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    let grep = executor::execute_git(&["grep", "-P", "literal-\\w+(?=$)", "HEAD"], &cwd, &grants).await;
    assert_eq!(grep["exitCode"], 0, "{grep}");
    assert_eq!(grep["text"], format!("HEAD:{filename}:literal-content\n"));
    windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    fs::write(work.join(filename), "literal-content\nchanged\n").unwrap();
    let changed = executor::execute_git(&["diff", "--name-only", "-z"], &cwd, &grants).await;
    assert_eq!(changed["exitCode"], 0, "{changed}");
    assert_eq!(changed["text"], format!("{filename}\0"));
    windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    let inside = work.join(filename).to_string_lossy().into_owned();
    let allowed = executor::execute_git(&["hash-object", "--", &inside], &cwd, &grants).await;
    assert_eq!(allowed["exitCode"], 0, "{allowed}");
    let hash = allowed["text"].as_str().unwrap().trim();
    assert!(matches!(hash.len(), 40 | 64) && hash.bytes().all(|byte| byte.is_ascii_hexdigit()), "{allowed}");
    windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    let outside = secret.to_string_lossy().into_owned();
    let denied = executor::execute_git(&["hash-object", "--", &outside], &cwd, &grants).await;
    assert!(denied["exitCode"].as_i64().is_some_and(|code| code != 0), "{denied}");
    assert_eq!(denied["text"], "", "{denied}");
    windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    assert_eq!(fs::read(secret).unwrap(), b"OUTSIDE_GIT_SENTINEL");
    fs::remove_dir_all(root).unwrap();
}

/// Gives a fixture directory the ordinary owner of an unelevated installation.
/// The acceptance runner is elevated, so new objects default to the Administrators group,
/// which Git's repository ownership check rejects for the sandboxed, non-administrator token.
fn own_as_user(path: &std::path::Path) {
    use windows_sys::Win32::{Foundation::*, Security::*, System::Threading::*};
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        assert_ne!(OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token), 0);
        let mut buffer = [0u64; 32];
        let mut length = 0;
        let found = GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), std::mem::size_of_val(&buffer) as u32, &mut length);
        CloseHandle(token);
        assert_ne!(found, 0);
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);
        set_owner(path, user.User.Sid);
    }
}

/// Replaces only the owner of one fixture object; its DACL is unchanged.
fn set_owner(path: &std::path::Path, owner: windows_sys::Win32::Security::PSID) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::{Authorization::*, *};
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let code = unsafe { SetNamedSecurityInfoW(wide.as_ptr(), SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION,
        owner, std::ptr::null_mut(), std::ptr::null(), std::ptr::null()) };
    assert_eq!(code, 0, "owner update failed for {}", path.display());
}

/// Sets a fixture owner from a string SID. Owners other than the caller or one of its owner
/// groups need SeRestorePrivilege, enabled only around this call in this test process.
fn own_as_sid(path: &std::path::Path, sid: &str, restore: bool) {
    use windows_sys::Win32::{Foundation::*, Security::{Authorization::*, *}, System::Threading::*};
    let text: Vec<u16> = sid.encode_utf16().chain([0]).collect();
    unsafe {
        let mut owner: PSID = std::ptr::null_mut();
        assert_ne!(ConvertStringSidToSidW(text.as_ptr(), &mut owner), 0);
        let mut token: HANDLE = std::ptr::null_mut();
        assert_ne!(OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token), 0);
        let mut privilege = TOKEN_PRIVILEGES { PrivilegeCount: 1, Privileges: [LUID_AND_ATTRIBUTES::default()] };
        assert_ne!(LookupPrivilegeValueW(std::ptr::null(), SE_RESTORE_NAME, &mut privilege.Privileges[0].Luid), 0);
        if restore {
            privilege.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;
            assert_ne!(AdjustTokenPrivileges(token, 0, &privilege, 0, std::ptr::null_mut(), std::ptr::null_mut()), 0);
            assert_eq!(GetLastError(), 0, "SeRestorePrivilege is not available to the acceptance runner");
        }
        set_owner(path, owner);
        if restore {
            privilege.Privileges[0].Attributes = 0;
            assert_ne!(AdjustTokenPrivileges(token, 0, &privilege, 0, std::ptr::null_mut(), std::ptr::null_mut()), 0);
        }
        CloseHandle(token);
        LocalFree(owner as HLOCAL);
    }
}

/// Lists one directory level for "nothing was created" checks.
fn names(path: &std::path::Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(path).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

/// The leading-directories patch accepts "exists, attributes denied" without proving a directory.
/// A denied FILE in that position must still make Git fail cleanly and create nothing, both as the
/// last leading component and as a middle component (review L2, docs/windows/git-patch-review.md).
#[tokio::test]
#[ignore = "requires installed private Git, bundled shell and authenticated Windows broker"]
async fn git_leading_directories_reject_denied_file() {
    assert!(miniclaw_server::windows_git::load().unwrap().is_some());
    let root = std::env::temp_dir().join(format!("mc-git-leading-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&root, false).unwrap();
    let work = root.join("work");
    fs::create_dir(&work).unwrap();
    own_as_user(&work);
    // Outside the grant: the sandbox can neither read this file's attributes nor list its folder.
    let blocker = root.join("blocker.txt");
    fs::write(&blocker, b"LEADING_DIRECTORY_SENTINEL").unwrap();
    let cwd = work.to_string_lossy().into_owned();
    let grants = vec![Grant { path: cwd.clone(), folder: true, write: true, blocked: false }];
    let before = names(&root);
    for target in [blocker.join("repo"), blocker.join("middle").join("repo")] {
        let target = target.to_string_lossy().into_owned();
        let result = executor::execute_git(&["init", "--quiet", &target], &cwd, &grants).await;
        println!("Git init {target}: {result}");
        assert_eq!(result["status"], "ok", "{result}");
        assert!(result["exitCode"].as_i64().is_some_and(|code| code != 0), "{result}");
        assert!(result["stderr"].as_str().unwrap().contains("cannot mkdir"), "{result}");
        windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
        assert_eq!(names(&root), before, "nothing may be created beside the blocker");
        assert!(fs::metadata(&blocker).unwrap().is_file());
        assert_eq!(fs::read(&blocker).unwrap(), b"LEADING_DIRECTORY_SENTINEL");
        assert!(names(&work).is_empty(), "nothing may be created in the workspace");
    }
    fs::remove_dir_all(root).unwrap();
}

/// Git's own repository ownership check stays in force: a repository owned by BUILTIN\Administrators
/// or by another principal is refused before its configuration is read. No safe.directory is added.
#[tokio::test]
#[ignore = "requires installed private Git, bundled shell and authenticated Windows broker"]
async fn git_refuses_administrators_and_foreign_owned_repositories() {
    assert!(miniclaw_server::windows_git::load().unwrap().is_some());
    // BUILTIN\Administrators and NT AUTHORITY\LOCAL SERVICE; the second needs SeRestorePrivilege.
    for (owner, restore) in [("S-1-5-32-544", false), ("S-1-5-19", true)] {
        let root = std::env::temp_dir().join(format!("mc-git-owner-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&root, false).unwrap();
        let work = root.join("work");
        fs::create_dir(&work).unwrap();
        own_as_user(&work);
        let cwd = work.to_string_lossy().into_owned();
        let grants = vec![Grant { path: cwd.clone(), folder: true, write: true, blocked: false }];
        fs::write(work.join("tracked.txt"), "tracked\n").unwrap();
        for arguments in [vec!["init", "--quiet"], vec!["add", "--", "tracked.txt"]] {
            let result = executor::execute_git(&arguments, &cwd, &grants).await;
            assert_eq!(result["exitCode"], 0, "{result}");
            windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
            if arguments[0] == "init" { own_as_user(&work.join(".git")); }
        }
        let owned = executor::execute_git(&["status", "--porcelain=v1"], &cwd, &grants).await;
        assert_eq!(owned["exitCode"], 0, "user-owned control: {owned}");
        windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
        own_as_sid(&work.join(".git"), owner, restore);
        own_as_sid(&work, owner, restore);
        let refused = executor::execute_git(&["status", "--porcelain=v1"], &cwd, &grants).await;
        println!("Owner {owner}: {refused}");
        assert_eq!(refused["status"], "ok", "{refused}");
        assert_eq!(refused["exitCode"], 128, "{refused}");
        assert_eq!(refused["text"], "", "{refused}");
        let stderr = refused["stderr"].as_str().unwrap();
        assert!(stderr.contains("detected dubious ownership"), "{refused}");
        assert!(stderr.contains(owner), "{refused}");
        windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}

/// Outside a repository, Git's discovery stops at the parent of the granted folder (GIT_CEILING_DIRECTORIES)
/// instead of dying with "failed to stat" at the first parent the sandbox may not read, for direct Git and
/// for Git in the sandboxed shell. Inside the grant it still walks up, also from a nested grant.
#[tokio::test]
#[ignore = "requires installed private Git, bundled shell and authenticated Windows broker"]
async fn git_discovery_stops_at_the_grant() {
    assert!(miniclaw_server::windows_git::load().unwrap().is_some());
    // The private root is outside the grant: the sandbox can neither list nor stat it.
    let root = std::env::temp_dir().join(format!("mc-git-ceiling-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&root, false).unwrap();
    let work = root.join("work '雪");
    let deeper = work.join("sub").join("deeper");
    fs::create_dir_all(&deeper).unwrap();
    own_as_user(&work);
    let cwd = work.to_string_lossy().into_owned();
    let grants = vec![Grant { path: cwd.clone(), folder: true, write: true, blocked: false }];
    let canonical = platform_fs::permission_path(&work).unwrap();
    let direct = executor::execute_git(&["status", "--porcelain=v1"], &cwd, &grants).await;
    windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    println!("Direct Git outside a repository: {direct}");
    assert_eq!(direct["exitCode"], 128, "{direct}");
    let stderr = direct["stderr"].as_str().unwrap();
    assert!(stderr.contains("not a git repository") && !stderr.contains("failed to stat"), "{direct}");
    let shell = executor::execute_host(Operation::RunShell { text: "git status --porcelain=v1".into(), cwd: deeper.to_string_lossy().into_owned(), hosts: vec![] }, &grants).await;
    windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    println!("Shell Git outside a repository: {shell}");
    assert_eq!(shell["exitCode"], 128, "{shell}");
    let stderr = shell["stderr"].as_str().unwrap();
    assert!(stderr.contains("not a git repository") && !stderr.contains("failed to stat"), "{shell}");
    // The value is the grant's parent in Git's spelling, after an empty entry, with both drive-letter cases.
    let parent = canonical.rsplit_once('\\').unwrap().0.replace('\\', "/");
    let expected = format!(";{}{};{}{}", parent[..1].to_ascii_uppercase(), &parent[1..], parent[..1].to_ascii_lowercase(), &parent[1..]);
    let shown = executor::execute_host(Operation::RunShell { text: "printf '%s' \"$GIT_CEILING_DIRECTORIES\"".into(), cwd: cwd.clone(), hosts: vec![] }, &grants).await;
    windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    println!("Shell ceiling: {shown}");
    assert_eq!(shown["exitCode"], 0, "{shown}");
    assert_eq!(shown["text"], expected.as_str(), "{shown}");
    let created = executor::execute_git(&["init", "--quiet"], &cwd, &grants).await;
    assert_eq!(created["exitCode"], 0, "{created}");
    windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    own_as_user(&work.join(".git"));
    let nested = vec![grants[0].clone(), Grant { path: deeper.to_string_lossy().into_owned(), folder: true, write: true, blocked: false }];
    for grants in [&grants, &nested] {
        let top = executor::execute_git(&["rev-parse", "--show-toplevel"], &deeper.to_string_lossy(), grants).await;
        windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
        println!("Top level from a subfolder with {} grant(s): {top}", grants.len());
        assert_eq!(top["exitCode"], 0, "{top}");
        assert_eq!(top["text"].as_str().unwrap().trim_end(), canonical.replace('\\', "/"), "{top}");
    }
    fs::remove_dir_all(root).unwrap();
}

/// Git whose stdout is the sandboxed terminal (ConPTY) exits with its own status. Git closes stdout before it exits
/// and then restores the console mode; lpac4 Git did that through the closed standard handle, which the sandbox's
/// strict handle checks turn into STATUS_INVALID_HANDLE, so the shell saw 8 after Git had done its work and `&&`
/// chains stopped after the first Git command (git-lpac-console-handle.patch, docs/windows/git-patch-review.md).
#[tokio::test]
#[ignore = "requires installed private Git, bundled shell and authenticated Windows broker"]
async fn git_in_the_terminal_exits_with_its_own_status() {
    use miniclaw_server::terminal::{Spec, Terminals};
    assert!(miniclaw_server::windows_git::load().unwrap().is_some());
    let root = std::env::temp_dir().join(format!("mc-git-terminal-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&root, false).unwrap();
    let work = root.join("work '雪");
    fs::create_dir(&work).unwrap();
    own_as_user(&work);
    fs::write(work.join("tracked.txt"), "tracked\n").unwrap();
    let cwd = work.to_string_lossy().into_owned();
    let grants = vec![Grant { path: cwd.clone(), folder: true, write: true, blocked: false }];
    // The repository gets the ordinary user owner first (see own_as_user); `git init` in the chain then reinitializes it.
    let created = executor::execute_git(&["init", "--quiet"], &cwd, &grants).await;
    assert_eq!(created["exitCode"], 0, "{created}");
    windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    own_as_user(&work.join(".git"));
    let terminals = Terminals::default();
    let terminal = terminals.ensure("git-exit", Spec { cwd: cwd.clone(), grants: grants.clone(), hosts: vec![], cols: 100, rows: 30 }).await.unwrap();
    // Each line reports the shell's view of the status; a chain reports the first failing command's status.
    // Results are checked after the terminal is closed: a failed assertion with the shell still open can leave the
    // test process waiting on the pseudo-terminal (seen with the lpac4 control, git-console-native-vm1-2.log).
    let mut results = Vec::new();
    for (call, script, expected) in [
        ("git-version", "git --version; echo \"GIT_RC=$?\"", "git version 2.56.0"),
        ("git-chain", "git init -q && git add . && git -c user.name='MiniClaw Test' -c user.email=test@example.invalid commit -q -m first; echo \"GIT_RC=$?\"", ""),
        ("git-status", "git status; echo \"GIT_RC=$?\"", "nothing to commit"),
        ("git-fresh", "git init -q fresh && test -d fresh/.git; echo \"GIT_RC=$?\"", ""),
    ] {
        let ran = terminal.run(call, script, &cwd, vec![], |_| {}).await;
        let (text, code) = match ran {
            Ok(ran) => (String::from_utf8_lossy(&ran.output).into_owned(), ran.code),
            Err(error) => (format!("run failed: {error}"), None),
        };
        println!("Terminal {call}: code={code:?}\n{text}");
        results.push((call, expected, text, code));
    }
    terminals.close("git-exit").await;
    windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    for (call, expected, text, code) in &results {
        let statuses: Vec<&str> = text.match_indices("GIT_RC=").map(|(at, _)| {
            let digits = &text[at + 7..];
            &digits[..digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len())]
        }).collect();
        assert_eq!(statuses, ["0"], "{call}: {text}");
        assert!(text.contains(expected), "{call}: {text}");
        assert_eq!(*code, Some(0), "{call}: {text}");
    }
    // The chain did all its work: one commit with the tracked file.
    let log = executor::execute_git(&["log", "--format=%s", "--name-only"], &cwd, &grants).await;
    assert_eq!(log["exitCode"], 0, "{log}");
    assert_eq!(log["text"], "first\n\ntracked.txt\n", "{log}");
    windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    fs::remove_dir_all(root).unwrap();
}

/// Git in the sandbox never waits for a person it cannot reach: paging commands print straight to the terminal
/// (the bundled Git has no pager program; it failed with "cannot spawn less", exit 128), a commit without a
/// message fails at once with Git's own advice instead of waiting for an editor, and a credential request fails
/// instead of prompting, in the terminal and in a model's shell command alike (git(1): GIT_PAGER, GIT_EDITOR,
/// GIT_TERMINAL_PROMPT). Every command must finish well inside the terminal's run limit.
#[tokio::test]
#[ignore = "requires installed private Git, bundled shell and authenticated Windows broker"]
async fn git_never_waits_for_a_pager_an_editor_or_a_password() {
    use miniclaw_server::terminal::{Spec, Terminals};
    assert!(miniclaw_server::windows_git::load().unwrap().is_some());
    let root = std::env::temp_dir().join(format!("mc-git-pager-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&root, false).unwrap();
    let work = root.join("work '雪");
    fs::create_dir(&work).unwrap();
    own_as_user(&work);
    fs::write(work.join("tracked.txt"), "tracked\n").unwrap();
    let cwd = work.to_string_lossy().into_owned();
    let grants = vec![Grant { path: cwd.clone(), folder: true, write: true, blocked: false }];
    let created = executor::execute_git(&["init", "--quiet"], &cwd, &grants).await;
    assert_eq!(created["exitCode"], 0, "{created}");
    windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    own_as_user(&work.join(".git"));
    for arguments in [vec!["add", "--", "tracked.txt"], vec!["-c", "user.name=MiniClaw Test", "-c", "user.email=test@example.invalid", "commit", "--quiet", "-m", "first"]] {
        let result = executor::execute_git(&arguments, &cwd, &grants).await;
        assert_eq!(result["exitCode"], 0, "{result}");
        windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    }
    fs::write(work.join("tracked.txt"), "tracked\nchanged line\n").unwrap();
    let terminals = Terminals::default();
    let terminal = terminals.ensure("git-pager", Spec { cwd: cwd.clone(), grants: grants.clone(), hosts: vec![], cols: 100, rows: 30 }).await.unwrap();
    let identity = "-c user.name='MiniClaw Test' -c user.email=test@example.invalid";
    let mut results = Vec::new();
    for (call, script, expected) in [
        ("git-log", "git log; echo \"GIT_RC=$?\"".to_owned(), "first"),
        ("git-log-oneline", "git log --oneline -1; echo \"GIT_RC=$?\"".to_owned(), "first"),
        ("git-diff", "git diff; echo \"GIT_RC=$?\"".to_owned(), "+changed line"),
        ("git-show", "git show --stat HEAD; echo \"GIT_RC=$?\"".to_owned(), "tracked.txt"),
        ("git-commit-no-message", format!("git {identity} commit -a; echo \"GIT_RC=$?\""), "-m"),
        ("git-credential", "printf 'protocol=https\\nhost=example.invalid\\n\\n' | git credential fill; echo \"GIT_RC=$?\"".to_owned(), "terminal prompts disabled"),
    ] {
        let started = std::time::Instant::now();
        let ran = tokio::time::timeout(Duration::from_secs(200), terminal.run(call, &script, &cwd, vec![], |_| {})).await;
        let elapsed = started.elapsed();
        let (text, code) = match ran {
            Ok(Ok(ran)) => (String::from_utf8_lossy(&ran.output).into_owned(), ran.code),
            Ok(Err(error)) => (format!("run failed: {error}"), None),
            Err(_) => ("no answer within 200 s".to_owned(), None),
        };
        println!("Terminal {call}: code={code:?} after {elapsed:?}\n{text}");
        // Git colours its output on the terminal; the checks read the plain text.
        results.push((call, expected, plain(&text), code, elapsed));
    }
    terminals.close("git-pager").await;
    windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    // A model's shell command (no terminal) gets the same credential policy.
    let started = std::time::Instant::now();
    let shell = tokio::time::timeout(Duration::from_secs(150), executor::execute_host(Operation::RunShell {
        text: "printf 'protocol=https\\nhost=example.invalid\\n\\n' | git credential fill".into(), cwd: cwd.clone(), hosts: vec![] }, &grants)).await;
    let shell_elapsed = started.elapsed();
    windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    println!("Shell credential after {shell_elapsed:?}: {shell:?}");
    for (call, expected, text, code, elapsed) in &results {
        let statuses: Vec<&str> = text.match_indices("GIT_RC=").map(|(at, _)| {
            let digits = &text[at + 7..];
            &digits[..digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len())]
        }).collect();
        assert_eq!(*code, Some(0), "{call} finished: {text}");
        assert!(*elapsed < Duration::from_secs(30), "{call} took {elapsed:?}: {text}");
        assert!(text.contains(expected), "{call}: {text}");
        match *call {
            "git-commit-no-message" | "git-credential" => assert!(statuses.len() == 1 && statuses[0] != "0", "{call} must fail: {text}"),
            _ => {
                assert_eq!(statuses, ["0"], "{call}: {text}");
                assert!(!text.contains("cannot spawn") && !text.contains("unable to execute pager"), "{call}: {text}");
            }
        }
    }
    let shell = shell.expect("the shell command answered within 150 s");
    assert_ne!(shell["exitCode"], 0, "{shell}");
    assert!(shell["stderr"].as_str().unwrap_or_default().contains("terminal prompts disabled") || shell["text"].as_str().unwrap_or_default().contains("terminal prompts disabled"), "{shell}");
    assert!(shell_elapsed < Duration::from_secs(30), "{shell_elapsed:?}");
    // The commit without a message changed nothing: HEAD is still the first commit.
    let log = executor::execute_git(&["log", "--format=%s"], &cwd, &grants).await;
    assert_eq!(log["text"], "first\n", "{log}");
    windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    fs::remove_dir_all(root).unwrap();
}

/// Removes ANSI escape sequences (Git's colours on a terminal) from captured output.
fn plain(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() { if c.is_ascii_alphabetic() { break; } }
        } else { out.push(c); }
    }
    out
}

/// Records the exact Win32 path calls used by Git's mingw_getcwd before running Git.
async fn cwd_probe(cwd: &str, grants: &[Grant]) {
    let program = std::env::current_exe().unwrap();
    let mut grants = grants.to_vec();
    grants.push(Grant { path: program.to_string_lossy().into_owned(), folder:false, write:false, blocked:false });
    let mut sandbox = windows_sandbox::Sandbox::new_shell(&grants, std::path::Path::new(cwd)).unwrap();
    let mut env: Vec<_> = sandbox.environment().into_iter().map(|(k,v)| (k.into_string().unwrap(),v.into_string().unwrap())).collect();
    env.push(("MC_GIT_CWD_PROBE".into(), "1".into()));
    let args = ["--exact", "git_cwd_child", "--ignored", "--nocapture"].map(str::to_owned);
    let pending = miniclaw_server::windows_process::pending_stdio_restricted(&program, &args,
        std::path::Path::new(cwd), &env, &sandbox.security()).unwrap();
    sandbox.bind_process(&pending.server.process).unwrap();
    let (stdout, stderr, code) = miniclaw_server::windows_process::capture_stdio(pending.resume().unwrap()).await.unwrap();
    println!("CWD diagnostic exit={code}\n{}\n{}", String::from_utf8_lossy(&stdout), String::from_utf8_lossy(&stderr));
    assert_eq!(code,0);
    drop(sandbox);
    windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
}

/// Probes native path lookup without interpreting diagnostic failures as acceptance.
#[test]
#[ignore = "internal LPAC subprocess only"]
fn git_cwd_child() {
    if std::env::var("MC_GIT_CWD_PROBE").as_deref() != Ok("1") { return; }
    use windows_sys::Win32::{Foundation::*, Storage::FileSystem::*};
    #[link(name = "kernel32")]
    unsafe extern "system" { fn GetCurrentDirectoryW(length: u32, buffer: *mut u16) -> u32; }
    unsafe {
        let mut cwd = [0u16; 32768];
        let length = GetCurrentDirectoryW(cwd.len() as u32, cwd.as_mut_ptr());
        println!("GetCurrentDirectoryW={length}, error={}", GetLastError());
        assert!(length > 0 && length < cwd.len() as u32);
        let handle = CreateFileW(cwd.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(), OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, std::ptr::null_mut());
        println!("CreateFileW valid={}, error={}", handle != INVALID_HANDLE_VALUE, GetLastError());
        let mut path = [0u16; 32768];
        if handle != INVALID_HANDLE_VALUE {
            for flags in [FILE_NAME_NORMALIZED, FILE_NAME_OPENED,
                VOLUME_NAME_NT, VOLUME_NAME_NT | FILE_NAME_OPENED,
                VOLUME_NAME_NONE, VOLUME_NAME_NONE | FILE_NAME_OPENED] {
                let result = GetFinalPathNameByHandleW(handle, path.as_mut_ptr(), path.len() as u32, flags);
                println!("GetFinalPathNameByHandleW flags={flags}, result={result}, error={}", GetLastError());
            }
            let mut original: FILE_ID_INFO = std::mem::zeroed();
            let id_ok = GetFileInformationByHandleEx(handle, FileIdInfo,
                (&mut original as *mut FILE_ID_INFO).cast(), std::mem::size_of::<FILE_ID_INFO>() as u32);
            println!("Original FileIdInfo={id_ok}, error={}", GetLastError());
            assert_ne!(id_ok, 0);
            let suffix_length = GetFinalPathNameByHandleW(handle, path.as_mut_ptr(), path.len() as u32, VOLUME_NAME_NONE);
            assert!(suffix_length > 0 && suffix_length < path.len() as u32);
            assert_eq!(cwd[1], b':' as u16);
            assert_eq!(path[0], b'\\' as u16);
            let mut candidate = cwd[..2].to_vec();
            candidate.extend_from_slice(&path[..suffix_length as usize]);
            candidate.push(0);
            let reopened = CreateFileW(candidate.as_ptr(), 0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, std::ptr::null(),
                OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, std::ptr::null_mut());
            println!("Reconstructed DOS open={}, error={}", reopened != INVALID_HANDLE_VALUE, GetLastError());
            assert_ne!(reopened, INVALID_HANDLE_VALUE);
            let mut candidate_id: FILE_ID_INFO = std::mem::zeroed();
            let candidate_ok = GetFileInformationByHandleEx(reopened, FileIdInfo,
                (&mut candidate_id as *mut FILE_ID_INFO).cast(), std::mem::size_of::<FILE_ID_INFO>() as u32);
            let candidate_error = GetLastError();
            CloseHandle(reopened);
            println!("Reconstructed FileIdInfo={candidate_ok}, error={candidate_error}");
            assert_ne!(candidate_ok, 0);
            assert_eq!(original.VolumeSerialNumber, candidate_id.VolumeSerialNumber);
            assert_eq!(original.FileId.Identifier, candidate_id.FileId.Identifier);
            println!("Reconstructed DOS full identity matches");
            CloseHandle(handle);
        }
        // Git's leading-directory creation stats, then tries to create, every existing ancestor.
        let text = String::from_utf16_lossy(&cwd[..length as usize]);
        for (index, _) in text.match_indices('\\').skip(1) {
            let mut ancestor: Vec<u16> = text[..index].encode_utf16().collect();
            ancestor.push(0);
            let attributes = GetFileAttributesW(ancestor.as_ptr());
            let attributes_error = GetLastError();
            let created = CreateDirectoryW(ancestor.as_ptr(), std::ptr::null());
            let create_error = GetLastError();
            assert_eq!(created, 0, "ancestor must already exist");
            println!("Ancestor depth={} attributes={attributes}, error={attributes_error}; CreateDirectoryW error={create_error}",
                text[..index].matches('\\').count());
        }
        let result = GetLongPathNameW(cwd.as_ptr(), path.as_mut_ptr(), path.len() as u32);
        println!("GetLongPathNameW={result}, error={}", GetLastError());
        let attrs = GetFileAttributesW(cwd.as_ptr());
        println!("GetFileAttributesW={attrs}, error={}", GetLastError());
    }
}
