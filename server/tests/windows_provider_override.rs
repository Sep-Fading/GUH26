#![cfg(windows)]
// Starts the real backend with provider overrides that name batch launchers.
use std::{fs, process::{Command, Stdio}, time::{Duration, Instant}};

/// A `.cmd` or `.bat` provider override stops startup with words that name the variable, and
/// the batch file never runs.
#[test]
fn batch_provider_overrides_stop_startup() {
    let root = std::env::temp_dir().join(format!("mc-override-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let marker = root.join("ran.txt");
    for (variable, name) in [("CODEX_BIN", "codex.cmd"), ("CLAUDE_BIN", "claude.bat"), ("CODEX_BIN", "codex.cmd.")] {
        let shim = root.join(name.trim_end_matches('.'));
        fs::write(&shim, "@echo off\r\necho ran>\"%~dp0ran.txt\"\r\n").unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_miniclaw-server"))
            .env("MINICLAW_DATA_DIR", root.join("data")).env("MINICLAW_PORT", "0")
            .env_remove("CODEX_BIN").env_remove("CLAUDE_BIN").env(variable, root.join(name))
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() { break status; }
            if started.elapsed() > Duration::from_secs(20) {
                child.kill().unwrap();
                panic!("{variable}={name}: the backend kept running");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let output = child.wait_with_output().unwrap();
        let (printed, errors) = (String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(!status.success(), "{variable}={name}");
        assert!(errors.contains(variable) && errors.contains(".exe"), "{variable}={name}: {errors}");
        assert!(!printed.contains("listening"), "{variable}={name}: no port was opened");
        assert!(!root.join("data").exists(), "{variable}={name}: no state was created");
    }
    assert!(!marker.exists(), "a batch launcher ran");
    fs::remove_dir_all(root).unwrap();
}
