#![cfg(windows)]
// Verifies the excluded VM worker cannot launch.
use std::process::Command;

/// A Windows worker executable fails explicitly without spawning a VM or provider.
#[test]
fn worker_is_unavailable() {
    let result = Command::new(env!("CARGO_BIN_EXE_miniclaw-worker")).output().unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("not supported on Windows"));
    assert!(result.stdout.is_empty());
}
