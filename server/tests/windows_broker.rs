#![cfg(windows)]
// Exercises native tools against disposable scoped files.
use miniclaw_server::{executor, platform_fs, scoped::Operation, seatbelt::Grant};
// Builds fixture paths without real user data.
use std::{fs, path::PathBuf};

#[path = "support/etw_diagnostic.rs"]
mod etw_diagnostic;

struct Fixture(PathBuf);
impl Fixture {
    /// Creates an isolated Unicode workspace and one nested denied folder.
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("mc broker 雪 {}", uuid::Uuid::new_v4()));
        fs::create_dir_all(path.join("blocked")).unwrap();
        fs::write(path.join("public.txt"), "public needle").unwrap();
        fs::write(path.join("blocked/secret.txt"), "secret needle").unwrap();
        Self(path)
    }
    /// Resolves fixture paths through the real Windows namespace.
    fn path(&self, suffix: &str) -> String {
        self.0.join(suffix).to_string_lossy().into_owned()
    }
    /// Grants the workspace while denying a nested folder.
    fn grants(&self) -> Vec<Grant> {
        vec![
            Grant {
                path: self.path(""),
                write: true,
                folder: true,
                blocked: false,
            },
            Grant {
                path: self.path("blocked"),
                write: false,
                folder: true,
                blocked: true,
            },
        ]
    }
}
impl Drop for Fixture {
    /// Removes only disposable test files after native handles close.
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
/// Reads, writes, lists and searches all honor a nested block.
async fn broker_nested_policy() {
    let fixture = Fixture::new();
    let grants = fixture.grants();
    let read = executor::execute_host(
        Operation::ReadFile {
            path: fixture.path("public.txt"),
        },
        &grants,
    )
    .await;
    assert_eq!(read["status"], "ok", "{read}");
    let blocked = executor::execute_host(
        Operation::ReadFile {
            path: fixture.path("blocked/secret.txt"),
        },
        &grants,
    )
    .await;
    assert_eq!(blocked["status"], "denied", "{blocked}");
    let overwritten = executor::execute_host(
        Operation::WriteFile {
            path: fixture.path("blocked/secret.txt"),
            text: "changed".into(),
        },
        &grants,
    )
    .await;
    assert_eq!(overwritten["status"], "denied", "{overwritten}");
    assert_eq!(
        fs::read_to_string(fixture.0.join("blocked/secret.txt")).unwrap(),
        "secret needle"
    );
    let created = executor::execute_host(
        Operation::WriteFile {
            path: fixture.path("blocked/new.txt"),
            text: "changed".into(),
        },
        &grants,
    )
    .await;
    assert_eq!(created["status"], "denied", "{created}");
    assert!(!fixture.0.join("blocked/new.txt").exists());

    let search = executor::execute_host(
        Operation::Search {
            path: fixture.path(""),
            text: "needle".into(),
        },
        &grants,
    )
    .await;
    assert_eq!(search["status"], "ok", "{search}");
    assert!(search["text"].as_str().unwrap().contains("public needle"));
    assert!(!search["text"].as_str().unwrap().contains("secret"));
    let listing = executor::execute_host(
        Operation::ListFolder {
            path: fixture.path(""),
        },
        &grants,
    )
    .await;
    assert!(!listing["text"].as_str().unwrap().contains("blocked"));
    let write = executor::execute_host(
        Operation::WriteFile {
            path: fixture.path("new.txt"),
            text: "new".into(),
        },
        &grants,
    )
    .await;
    assert_eq!(write["status"], "ok", "{write}");
    assert_eq!(
        fs::read_to_string(fixture.0.join("new.txt")).unwrap(),
        "new"
    );
    // A file that did not exist is reported as created, and a second write as an update, as on the Mac.
    assert_eq!(write["text"], "File created.", "{write}");
    let again = executor::execute_host(
        Operation::WriteFile { path: fixture.path("new.txt"), text: "newer".into() },
        &grants,
    )
    .await;
    assert_eq!(again["text"], "File updated.", "{again}");
}

#[test]
/// Directory guards prevent the ancestor rename needed for a path-swap attack.
fn pinned_ancestor() {
    let fixture = Fixture::new();
    let guard = platform_fs::guarded_path(&fixture.0.join("public.txt"), false, false).unwrap();
    assert!(fs::rename(&fixture.0, fixture.0.with_extension("moved")).is_err());
    drop(guard);
}

#[tokio::test]
/// A real AppContainer command can use granted files but cannot read adjacent files.
async fn shell_appcontainer_boundary() {
    let fixture = Fixture::new();
    let project = fixture.0.join("sandbox");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("allowed.txt"), "allowed").unwrap();
    let grants = vec![Grant { path: project.to_string_lossy().into_owned(), write: true, folder: true, blocked: false }];
    let result = executor::execute_host(Operation::RunShell { text: SHELL_BOUNDARY.into(), cwd: project.to_string_lossy().into_owned(), hosts: vec![] }, &grants).await;
    assert_eq!(result["exitCode"], 0, "{result}");
    assert_eq!(result["text"].as_str().unwrap().trim(), "sandbox-ok 雪");
    assert!(!String::from_utf8_lossy(&fs::read(project.join("denied.txt")).unwrap()).contains("public needle"));
    assert_eq!(fs::read_to_string(fixture.0.join("public.txt")).unwrap(), "public needle");
    assert!(!fixture.0.join("escaped.txt").exists());
    miniclaw_server::windows_sandbox::drain_cleanup(std::time::Duration::from_secs(35)).unwrap();
}

#[tokio::test]
#[ignore = "Requires an independently verified portable PowerShell ZIP"]
async fn portable_powershell_appcontainer_boundary() {
    let runtime = PathBuf::from(std::env::var_os("MINICLAW_TEST_POWERSHELL_RUNTIME")
        .expect("Set MINICLAW_TEST_POWERSHELL_RUNTIME to the verified extracted ZIP"));
    assert!(runtime.is_absolute());
    assert!(runtime.join("pwsh.exe").is_file());
    shell_boundary(Some(runtime)).await;
    etw_diagnostic::restore().expect("PowerShell ETW permissions must be restored");
}

#[test]
#[ignore = "Recovery step for the portable PowerShell diagnostic"]
fn restore_portable_powershell_etw() {
    etw_diagnostic::restore().unwrap();
}

/// Uses the same filesystem boundary assertions for inbox and candidate runtimes.
async fn shell_boundary(runtime: Option<PathBuf>) {
    let fixture = Fixture::new();
    let project = fixture.0.join("sandbox");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("allowed.txt"), "allowed").unwrap();
    let grants = vec![Grant {
        path: project.to_string_lossy().into_owned(),
        write: true,
        folder: true,
        blocked: false,
    }];
    let mut grants = grants;
    if let Some(runtime) = &runtime {
        grants.push(Grant { path: runtime.to_string_lossy().into_owned(),
            write: false, folder: true, blocked: false });
    }
    let sandbox = miniclaw_server::windows_sandbox::Sandbox::new(&grants, &project).unwrap();
    let _etw = if runtime.is_some() {
        Some(etw_diagnostic::Grant::new(sandbox.security().capabilities.AppContainerSid).unwrap())
    } else { None };
    let portable = runtime.is_some();
    let program = runtime.map(|root| root.join("pwsh.exe")).unwrap_or_else(||
        PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe"));
    let outside = fixture.path("public.txt").replace('\'', "''");
    let script = format!(
        "$ErrorActionPreference='Stop'; if ((Get-Content -LiteralPath './allowed.txt') -ne 'allowed') {{ exit 31 }}; try {{ [IO.File]::ReadAllText('{outside}'); exit 32 }} catch {{ }}; [IO.File]::WriteAllText('./allowed.txt','updated'); if ([IO.File]::ReadAllText('./allowed.txt') -ne 'updated') {{ exit 33 }}; [IO.File]::Move('./allowed.txt','./renamed.txt'); [IO.File]::Delete('./renamed.txt'); if ([IO.File]::Exists('./renamed.txt')) {{ exit 35 }}; [IO.Directory]::CreateDirectory('./new-dir') | Out-Null; [IO.File]::WriteAllText('./new-dir/new.txt','created'); [IO.File]::AppendAllText('./new-dir/new.txt','-modified'); if ([IO.File]::ReadAllText('./new-dir/new.txt') -ne 'created-modified') {{ exit 34 }}; [Console]::Out.Write('sandbox-ok')"
    );
    let args = if portable && std::env::var_os("MINICLAW_TEST_POWERSHELL_FILE").is_some() {
        let path = project.join("boundary.ps1");
        fs::write(&path, format!("[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false);\n{script}")).unwrap();
        vec!["-NoLogo".into(), "-NoProfile".into(), "-NonInteractive".into(),
            "-ExecutionPolicy".into(), "Bypass".into(), "-File".into(), path.to_string_lossy().into_owned()]
    } else { miniclaw_server::powershell::arguments(&script).unwrap() };
    let captured = miniclaw_server::windows_process::capture_restricted(
        &program,
        &args,
        &project,
        &sandbox
            .environment()
            .into_iter()
            .map(|(key, value)| (key.into_string().unwrap(), value.into_string().unwrap()))
            .collect::<Vec<_>>(),
        &sandbox.security(),
    );
    let (output, errors, status) = tokio::time::timeout(std::time::Duration::from_secs(45), captured)
        .await.expect("Sandboxed shell startup or execution exceeded 45 seconds").unwrap();
    assert_eq!(status, 0, "{}", String::from_utf8_lossy(&errors));
    assert_eq!(String::from_utf8_lossy(&output), "sandbox-ok");
}

#[test]
fn credential_child() {
    let Ok(target) = std::env::var("MINICLAW_CREDENTIAL_ISOLATION_TARGET") else {
        return;
    };
    assert!(target.starts_with("MiniClaw.IsolationFixture."));
    let target: Vec<u16> = target.encode_utf16().chain(Some(0)).collect();
    let mut credential = std::ptr::null_mut();
    let readable = unsafe {
        windows_sys::Win32::Security::Credentials::CredReadW(
            target.as_ptr(),
            windows_sys::Win32::Security::Credentials::CRED_TYPE_GENERIC,
            0,
            &mut credential,
        )
    };
    if readable != 0 {
        unsafe {
            windows_sys::Win32::Security::Credentials::CredFree(credential.cast());
        }
    }
    assert_eq!(
        readable, 0,
        "AppContainer must not read the host user's synthetic credential"
    );
    println!("synthetic-credential-denied");
}

#[tokio::test]
async fn appcontainer_cannot_read_host_credential() {
    use windows_sys::Win32::Security::Credentials::*;
    struct Credential(Vec<u16>);
    impl Drop for Credential {
        fn drop(&mut self) {
            unsafe {
                CredDeleteW(self.0.as_ptr(), CRED_TYPE_GENERIC, 0);
            }
        }
    }
    let target = format!("MiniClaw.IsolationFixture.{}", uuid::Uuid::new_v4());
    let mut guard = Credential(target.encode_utf16().chain(Some(0)).collect());
    let value = b"disposable-native-test-value";
    let mut credential: CREDENTIALW = unsafe { std::mem::zeroed() };
    credential.Type = CRED_TYPE_GENERIC;
    credential.TargetName = guard.0.as_mut_ptr();
    credential.CredentialBlobSize = value.len() as u32;
    credential.CredentialBlob = value.as_ptr().cast_mut();
    credential.Persist = CRED_PERSIST_SESSION;
    assert_ne!(unsafe { CredWriteW(&credential, 0) }, 0);
    let mut visible = std::ptr::null_mut();
    assert_ne!(
        unsafe { CredReadW(guard.0.as_ptr(), CRED_TYPE_GENERIC, 0, &mut visible) },
        0
    );
    unsafe {
        CredFree(visible.cast());
    }
    let fixture = Fixture::new();
    let source = std::env::current_exe().unwrap();
    let executable = fixture.0.join("credential-fixture.exe");
    fs::copy(source, &executable).unwrap();
    let grants = [Grant {
        path: fixture.path(""),
        folder: true,
        write: false,
        blocked: false,
    }];
    let mut sandbox = miniclaw_server::windows_sandbox::Sandbox::new(&grants, &fixture.0).unwrap();
    let mut environment = sandbox
        .environment()
        .into_iter()
        .map(|(k, v)| (k.into_string().unwrap(), v.into_string().unwrap()))
        .collect::<Vec<_>>();
    environment.push(("MINICLAW_CREDENTIAL_ISOLATION_TARGET".into(), target));
    let args = ["--exact", "credential_child", "--nocapture"].map(str::to_owned);
    let pending = miniclaw_server::windows_process::pending_stdio_restricted(
        &executable,
        &args,
        &fixture.0,
        &environment,
        &sandbox.security(),
    )
    .unwrap();
    sandbox.bind_process(&pending.server.process).unwrap();
    let tree = pending.server.process.tree_guard();
    let (out, err, status) =
        miniclaw_server::windows_process::capture_stdio(pending.resume().unwrap())
            .await
            .unwrap();
    tree.terminate_and_wait(std::time::Duration::from_secs(10))
        .unwrap();
    assert!(String::from_utf8_lossy(&out).contains("synthetic-credential-denied"));
    assert_eq!(
        status,
        0,
        "{} {}",
        String::from_utf8_lossy(&out),
        String::from_utf8_lossy(&err)
    );
    let profile = sandbox.profile_name();
    drop(sandbox);
    miniclaw_server::windows_sandbox::drain_cleanup(std::time::Duration::from_secs(35)).unwrap();
    let data = PathBuf::from(std::env::var_os("MINICLAW_DATA_DIR").unwrap());
    assert!(!data.join("sandbox-journals").join(format!("{profile}.jsonl")).exists());
    assert!(!data.join("sandbox-scratch").join(profile).exists());
}

#[test]
#[ignore = "native large-workspace ACL preparation and cleanup acceptance"]
fn sandbox_handles_workspace_above_previous_limit() {
    let fixture = Fixture::new();
    let project = fixture.0.join("large-workspace");
    fs::create_dir(&project).unwrap();
    for index in 0..6000 {
        fs::write(project.join(format!("module-{index}.py")), b"# fixture\n").unwrap();
    }
    let grants = [Grant {
        path: project.to_string_lossy().into_owned(),
        write: false,
        folder: true,
        blocked: false,
    }];
    let sandbox = miniclaw_server::windows_sandbox::Sandbox::new(&grants, &project).unwrap();
    drop(sandbox);
    assert_eq!(fs::read_dir(project).unwrap().count(), 6000);
}

#[test]
fn recoverable_discard_preserves_name_content_and_siblings() {
    let fixture = Fixture::new();
    let recovery = fixture.0.join("recovery");
    platform_fs::private_dir(&recovery, false).unwrap();
    let target = executor::recover_workspace_file(&fixture.0, "public.txt", &recovery).unwrap();
    assert_eq!(target.file_name().unwrap(), "public.txt");
    assert_eq!(fs::read(&target).unwrap(), b"public needle");
    assert!(!fixture.0.join("public.txt").exists());
    assert!(fixture.0.join("blocked/secret.txt").exists());
    assert!(executor::recover_workspace_file(&fixture.0, "../outside", &recovery).is_err());
}

#[tokio::test]
#[ignore = "Candidate inbox cmd runtime under unchanged LPAC"]
async fn cmd_appcontainer_boundary() {
    let fixture = Fixture::new();
    let project = fixture.0.join("sandbox");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("allowed.txt"), "allowed").unwrap();
    fs::write(project.join("boundary.cmd"), concat!(
        "@echo off\r\n",
        "chcp 65001 >nul\r\n",
        "if errorlevel 1 exit /b 38\r\n",
        "set /p first=<allowed.txt\r\n",
        "if not \"%first%\"==\"allowed\" exit /b 31\r\n",
        "type ..\\public.txt >denied.txt 2>nul\r\n",
        "if not errorlevel 1 exit /b 32\r\n",
        "echo changed >..\\public.txt 2>nul\r\n",
        "if not errorlevel 1 exit /b 37\r\n",
        ">allowed.txt echo updated\r\n",
        "set /p changed=<allowed.txt\r\n",
        "if not \"%changed%\"==\"updated\" exit /b 33\r\n",
        "move /y allowed.txt renamed.txt >nul\r\n",
        "if errorlevel 1 exit /b 34\r\n",
        "del renamed.txt\r\n",
        "if exist renamed.txt exit /b 35\r\n",
        "mkdir new-dir\r\n",
        ">new-dir\\new.txt echo created\r\n",
        "set /p created=<new-dir\\new.txt\r\n",
        "if not \"%created%\"==\"created\" exit /b 36\r\n",
        "echo sandbox-ok 雪\r\n",
        "exit /b 0\r\n",
    )).unwrap();
    fs::write(project.join("plain.cmd"), "@echo off\r\necho plain-ok\r\nexit /b 0\r\n").unwrap();
    let grants = vec![Grant { path: project.to_string_lossy().into_owned(), write: true, folder: true, blocked: false }];
    let sandbox = miniclaw_server::windows_sandbox::Sandbox::new(&grants, &project).unwrap();
    let program = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32").join("cmd.exe");
    let probe = ["/d", "/q", "/e:on", "/v:off", "/c", "ver"].map(str::to_owned);
    let baseline = miniclaw_server::windows_process::capture(&program, &probe, &project).await.unwrap();
    assert_eq!(baseline.2, 0, "Unrestricted cmd startup: stdout={:?} stderr={:?}", baseline.0, baseline.1);
    let env = sandbox.environment().into_iter().map(|(key, value)| (key.into_string().unwrap(), value.into_string().unwrap())).collect::<Vec<_>>();
    let isolated = miniclaw_server::windows_process::capture_restricted(&program, &probe, &project, &env, &sandbox.security()).await.unwrap();
    assert_eq!(isolated.2, 0, "LPAC cmd startup: stdout={:?} stderr={:?}", isolated.0, isolated.1);
    let read_probe = ["/d", "/q", "/e:on", "/v:off", "/c", "type", "boundary.cmd"].map(str::to_owned);
    let read = miniclaw_server::windows_process::capture_restricted(&program, &read_probe, &project, &env, &sandbox.security()).await.unwrap();
    assert_eq!(read.2, 0, "LPAC batch read: stdout={:?} stderr={:?}", read.0, read.1);
    assert!(String::from_utf8_lossy(&read.0).contains("sandbox-ok"));
    for command in ["echo plain-ok", "plain.cmd", "echo null-ok >nul", "chcp 65001", "call .\\boundary.cmd"] {
        let probe_args = ["/d", "/q", "/e:on", "/v:off", "/c", command].map(str::to_owned);
        let result = miniclaw_server::windows_process::capture_restricted(&program, &probe_args, &project, &env, &sandbox.security()).await.unwrap();
        eprintln!("Command {command}: status={} stdout={:?} stderr={:?}", result.2, String::from_utf8_lossy(&result.0), String::from_utf8_lossy(&result.1));
    }
    let args = ["/d", "/q", "/e:on", "/v:off", "/c", ".\\boundary.cmd"].map(str::to_owned);
    let capture = miniclaw_server::windows_process::capture_restricted(&program, &args, &project,
        &sandbox.environment().into_iter().map(|(key, value)| (key.into_string().unwrap(), value.into_string().unwrap())).collect::<Vec<_>>(),
        &sandbox.security());
    let (output, errors, status) = tokio::time::timeout(std::time::Duration::from_secs(30), capture).await.unwrap().unwrap();
    assert_eq!(status, 0, "stdout={:?}; stderr={:?}", String::from_utf8_lossy(&output), String::from_utf8_lossy(&errors));
    assert_eq!(String::from_utf8_lossy(&output).trim(), "sandbox-ok 雪");
    assert!(!String::from_utf8_lossy(&fs::read(project.join("denied.txt")).unwrap()).contains("public needle"));
    assert_eq!(fs::read_to_string(fixture.0.join("public.txt")).unwrap(), "public needle");
}

/// Checks the pinned Git shell without adding registry or network capabilities.
#[tokio::test]
#[ignore = "Candidate pinned Git shell under unchanged LPAC"]
async fn git_shell_appcontainer_boundary() {
    let fixture = Fixture::new();
    let project = fixture.0.join("sandbox");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("allowed.txt"), "allowed").unwrap();
    let runtime = miniclaw_server::windows_git::load().unwrap().unwrap();
    let grants = vec![
        Grant { path: project.to_string_lossy().into_owned(), write: true, folder: true, blocked: false },
    ];
    let sandbox = miniclaw_server::windows_sandbox::Sandbox::new_shell(&grants, &project).unwrap();
    let mut env = sandbox.environment().into_iter().map(|(key, value)| (key.into_string().unwrap(), value.into_string().unwrap())).collect::<Vec<_>>();
    env.push(("PATH".into(), runtime.root.join("usr").join("bin").to_string_lossy().into_owned()));
    let script = "set -eu; IFS= read -r first < allowed.txt || test \"$first\" = allowed; test \"$first\" = allowed; if cat ../public.txt > denied.txt 2> denied-error.txt; then exit 32; fi; if printf changed > ../public.txt; then exit 37; fi; printf updated > allowed.txt; mv allowed.txt renamed.txt; rm renamed.txt; mkdir new-dir; printf created > new-dir/new.txt; test \"$(cat new-dir/new.txt)\" = created; printf 'sandbox-ok 雪\\n'";
    let args = ["-c".to_owned(), script.to_owned()];
    let capture = miniclaw_server::windows_process::capture_restricted(&runtime.root.join("usr").join("bin").join("sh.exe"), &args, &project, &env, &sandbox.security());
    let (output, errors, status) = tokio::time::timeout(std::time::Duration::from_secs(45), capture).await.unwrap().unwrap();
    assert_eq!(status, 0, "stdout={:?}; stderr={:?}", String::from_utf8_lossy(&output), String::from_utf8_lossy(&errors));
    assert_eq!(String::from_utf8_lossy(&output).trim(), "sandbox-ok 雪");
    assert!(!String::from_utf8_lossy(&fs::read(project.join("denied.txt")).unwrap()).contains("public needle"));
    assert_eq!(fs::read_to_string(fixture.0.join("public.txt")).unwrap(), "public needle");
}

/// Checks the pinned standalone shell without registry or network capabilities.
#[tokio::test]
#[ignore = "Candidate pinned standalone shell under unchanged LPAC"]
async fn busybox_appcontainer_boundary() {
    let fixture = Fixture::new();
    let project = fixture.0.join("sandbox");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("allowed.txt"), "allowed").unwrap();
    let program = PathBuf::from(std::env::var_os("MINICLAW_TEST_BUSYBOX").unwrap());
    let baseline = miniclaw_server::windows_process::capture(&program, &["sh".into(), "-c".into(), "printf baseline-ok".into()], &project).await.unwrap();
    assert_eq!(baseline.2, 0, "Unrestricted shell: {:?} {:?}", baseline.0, baseline.1);
    assert_eq!(baseline.0, b"baseline-ok");
    let grants = vec![
        Grant { path: project.to_string_lossy().into_owned(), write: true, folder: true, blocked: false },
        Grant { path: program.to_string_lossy().into_owned(), write: false, folder: false, blocked: false },
    ];
    let sandbox = miniclaw_server::windows_sandbox::Sandbox::new(&grants, &project).unwrap();
    let mut env = sandbox.environment().into_iter().map(|(key, value)| (key.into_string().unwrap(), value.into_string().unwrap())).collect::<Vec<_>>();
    env.push(("HOSTNAME".into(), "miniclaw".into()));
    let script = SHELL_BOUNDARY;
    let args = ["sh".to_owned(), "-c".to_owned(), script.to_owned()];
    let capture = miniclaw_server::windows_process::capture_restricted(&program, &args, &project, &env, &sandbox.security());
    let (output, errors, status) = tokio::time::timeout(std::time::Duration::from_secs(45), capture).await.unwrap().unwrap();
    assert_eq!(status, 0, "stdout={:?}; stderr={:?}", String::from_utf8_lossy(&output), String::from_utf8_lossy(&errors));
    assert_eq!(String::from_utf8_lossy(&output).trim(), "sandbox-ok 雪");
    assert!(!String::from_utf8_lossy(&fs::read(project.join("denied.txt")).unwrap()).contains("public needle"));
    assert_eq!(fs::read_to_string(fixture.0.join("public.txt")).unwrap(), "public needle");
    assert!(!fixture.0.join("escaped.txt").exists());
}

// Exercises filesystem mutations and denials through the real sandboxed shell.
const SHELL_BOUNDARY: &str = "set -eu; IFS= read -r first < allowed.txt || test \"$first\" = allowed; test \"$first\" = allowed; if cat ../public.txt > denied.txt 2> denied-error.txt; then exit 32; fi; if printf changed > ../public.txt; then exit 37; fi; printf updated > allowed.txt; mv allowed.txt renamed.txt; if mv renamed.txt ../escaped.txt; then exit 39; fi; test -f renamed.txt; rm renamed.txt; mkdir new-dir; printf created > new-dir/new.txt; test \"$(cat new-dir/new.txt)\" = created; printf 'sandbox-ok 雪\\n'";
