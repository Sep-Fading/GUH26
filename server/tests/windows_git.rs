// Checks trusted embedded Git paths without consulting user credentials.
use miniclaw_server::windows_git::Runtime;
use std::path::Path;

#[test]
fn runtime_stays_inside_owned_package() {
    let data_dir = std::env::temp_dir().join("mc-git-layout");
    let data = data_dir.as_path();
    let root_path = data.join("windows-git-runtime/00000000-0000-4000-8000-000000000001");
    let root = root_path.to_str().unwrap();
    let config = |root: &str, program: &str, exec: &str| Runtime {
        root: root.into(), program: program.into(), exec_path: exec.into(),
    };
    assert!(config(root, "clangarm64/bin/git.exe", "clangarm64/libexec/git-core").validate(data).is_ok());
    // The bundled MiniClaw build uses the `git/` prefix for both architectures.
    assert!(config(root, "git/bin/git.exe", "git/libexec/git-core").validate(data).is_ok());
    let invalid = data.join("windows-git-runtime/not-an-installation");
    for outside in [data.to_str().unwrap(), "/other/secrets", invalid.to_str().unwrap()] {
        assert!(config(outside, "clangarm64/bin/git.exe", "clangarm64/libexec/git-core").validate(data).is_err());
    }
    for program in ["../git.exe", "/git.exe", "C:\\git.exe", "clangarm64/bin/cmd.exe", "a/../git.exe"] {
        assert!(config(root, program, "clangarm64/libexec/git-core").validate(data).is_err());
    }
    assert!(config(root, "clangarm64/bin/git.exe", "../secret").validate(data).is_err());
}

#[cfg(windows)]
#[test]
fn native_git_runtime_grants_are_readonly() {
    use miniclaw_server::platform_fs;
    use std::{fs, io::Write, process::Command};
    let root = std::env::temp_dir().join(format!("mc-git-{}", uuid::Uuid::new_v4()));
    let data = root.join("data");
    let work = root.join("work");
    fs::create_dir_all(&work).unwrap();
    platform_fs::private_dir(&data, false).unwrap();
    let runtime = data.join("windows-git-runtime").join(uuid::Uuid::new_v4().to_string());
    platform_fs::private_dir(&runtime, true).unwrap();
    fs::create_dir_all(runtime.join("git/bin")).unwrap();
    fs::create_dir_all(runtime.join("git/libexec/git-core")).unwrap();
    fs::write(runtime.join("git/bin/git.exe"), b"EMBEDDED_GIT_FIXTURE").unwrap();
    fs::write(data.join("credential-fixture"), b"PRIVATE_NEIGHBOR").unwrap();
    let config = serde_json::json!({"root":runtime,"program":"git/bin/git.exe","execPath":"git/libexec/git-core"});
    let mut file = platform_fs::private_file(&data.join("windows-git.json"), true).unwrap();
    file.write_all(config.to_string().as_bytes()).unwrap();
    drop(file);
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "native_git_child", "--nocapture"])
        .env("MC_GIT_CHILD", &root).env("MINICLAW_DATA_DIR", &data)
        .output().unwrap();
    assert!(output.status.success(), "{} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert_eq!(fs::read(runtime.join("git/bin/git.exe")).unwrap(), b"EMBEDDED_GIT_FIXTURE");
    fs::remove_dir_all(root).unwrap();
}

#[cfg(windows)]
#[tokio::test]
async fn native_git_child() {
    use miniclaw_server::{executor, scoped::Operation, seatbelt::Grant, terminal::{Terminals, Spec}};
    let Some(root) = std::env::var_os("MC_GIT_CHILD").map(std::path::PathBuf::from) else { return };
    let data = root.join("data");
    let runtime = miniclaw_server::windows_git::load().unwrap().unwrap();
    let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\\', "/").replace('\'', "'\"'\"'"));
    let script = format!(
        "set -eu; printf working > write-probe; test \"$(cat write-probe)\" = working; rm write-probe; cat {}; if printf tampered > {}; then exit 31; else printf '\\nWRITE_DENIED\\n'; fi; if cat {}; then exit 32; else printf '\\nSECRET_DENIED\\n'; fi; test -f \"$GIT_CONFIG_GLOBAL\"; test ! -s \"$GIT_CONFIG_GLOBAL\"; cat \"$GIT_CONFIG_GLOBAL\"; if printf tampered > \"$GIT_CONFIG_GLOBAL\"; then exit 33; else printf 'CONFIG_READONLY\\n'; fi; printf 'GIT_GLOBAL_EMPTY\\n'",
        quote(&runtime.root.join(&runtime.program)), quote(&runtime.root.join(&runtime.program)),
        quote(&data.join("credential-fixture")));
    let cwd = root.join("work").to_string_lossy().into_owned();
    let grants = vec![Grant { path: cwd.clone(), folder:true, write:true, blocked:false }];
    let result = executor::execute_host(Operation::RunShell { text:script.clone(), cwd:cwd.clone(), hosts:vec![] }, &grants).await;
    assert_eq!(result["exitCode"], 0, "{result}");
    let text = result["text"].as_str().unwrap();
    for expected in ["EMBEDDED_GIT_FIXTURE", "WRITE_DENIED", "SECRET_DENIED", "GIT_GLOBAL_EMPTY", "CONFIG_READONLY"] { assert!(text.contains(expected), "{result}"); }
    assert!(!text.contains("PRIVATE_NEIGHBOR"));
    let terminals = Terminals::default();
    let terminal = terminals.ensure("git", Spec { cwd:cwd.clone(), grants, hosts:vec![], cols:100, rows:30 }).await.unwrap();
    let result = terminal.run("git-probe", &script, &cwd, vec![], |_| {}).await.unwrap();
    assert_eq!(result.code, Some(0));
    let text = String::from_utf8_lossy(&result.output);
    for expected in ["EMBEDDED_GIT_FIXTURE", "WRITE_DENIED", "SECRET_DENIED", "GIT_GLOBAL_EMPTY", "CONFIG_READONLY"] { assert!(text.contains(expected), "{text}"); }
    assert!(!text.contains("PRIVATE_NEIGHBOR"));
    terminals.close("git").await;
    for _ in 0..100 {
        if std::fs::read_dir(data.join("sandbox-journals")).unwrap().next().is_none() { return; }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("Git sandbox journal cleanup did not complete");
}
