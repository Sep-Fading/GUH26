#![cfg(windows)]
// Verifies recovery against preserved native records.
use miniclaw_server::{platform_fs, windows_network_broker, windows_sandbox};
// Reads explicit disposable test state only.
use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
};

/// Requires a real later boot before revoking a preserved interrupted test run.
#[test]
#[ignore = "Requires preserved disposable sandbox journals and an actual Windows restart"]
fn recovers_after_windows_restart() {
    let _ = tracing_subscriber::fmt().with_max_level(tracing::Level::WARN)
        .with_test_writer().try_init();
    let data =
        PathBuf::from(std::env::var_os("MINICLAW_DATA_DIR").expect("Explicit test data required"));
    let expected = std::env::var("MINICLAW_TEST_RECOVERY_PROFILE")
        .expect("Explicit interrupted profile required");
    assert!(
        data.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("MiniClaw-Native-CI-")
    );
    assert!(expected.starts_with("MiniClaw.Run."));
    assert_eq!(expected.len(), "MiniClaw.Run.".len() + 32);
    assert!(
        expected["MiniClaw.Run.".len()..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    );
    let journal = data
        .join("sandbox-journals")
        .join(format!("{expected}.jsonl"));
    let file = platform_fs::open_read_nofollow(&journal).unwrap();
    assert!(platform_fs::private_regular_file(&file).unwrap());
    let mut lines = BufReader::new(file).lines();
    let first = lines.next().unwrap().unwrap();
    let saved: serde_json::Value = serde_json::from_str(&first).unwrap();
    let saved: windows_network_broker::BootEpoch =
        serde_json::from_value(saved["boot"].clone()).unwrap();
    let records: Vec<serde_json::Value> = lines.map(|line|
        serde_json::from_str(&line.unwrap()).unwrap()).collect();
    assert!(records.iter().any(|record| record["job"].as_str()
        == Some("Global\\MiniClaw.Job.92e61803cc4c48a197d5726e00a8e5b5")),
        "Preserved fixture must contain its actual interrupted job binding");
    assert!(records.iter().any(|record| record["identity"].is_array()),
        "Preserved fixture must include ACL changes");
    let current = windows_network_broker::verified_boot_epoch().unwrap();
    assert!(
        current.supersedes(&saved),
        "A real later Windows boot is required; preserve the journal"
    );
    windows_sandbox::recover().expect("Native interrupted cleanup must succeed");
    assert!(
        !journal.exists(),
        "Completed recovery must remove its active record"
    );
    let archive = data.join("sandbox-journals").join("retired")
        .join(format!("{expected}.retired.jsonl"));
    let file = platform_fs::open_read_nofollow(&archive)
        .expect("The preserved error-87 fixture must retain its uncertain identities");
    assert!(platform_fs::private_regular_file(&file).unwrap());
    let archived: Vec<serde_json::Value> = BufReader::new(file).lines().map(|line|
        serde_json::from_str(&line.unwrap()).unwrap()).collect();
    assert_eq!(archived.first().unwrap()["boot"], serde_json::to_value(&saved).unwrap());
    assert!(archived[1..].starts_with(&records), "Original evidence must remain intact");
    let certificate = &archived.last().unwrap()["retirement"];
    assert_eq!(certificate["profile"], expected);
    assert_eq!(certificate["profile_deleted"], true);
    assert_eq!(certificate["scratch_removed"], true);
    assert_eq!(certificate["proof"]["kind"], "LaterBoot");
    let unknown = certificate["unknown"].as_array().unwrap();
    assert!(!unknown.is_empty());
    for item in unknown {
        assert_eq!(item["kind"], "UnknownIdentity");
        assert_eq!(item["error"], 87);
        assert!(records.contains(&item["object"]));
    }
    assert!(
        !data.join("sandbox-scratch").join(&expected).exists(),
        "Recovered private scratch must be removed"
    );
    windows_sandbox::recover().expect("A second recovery must be harmless");
    let unchanged: Vec<serde_json::Value> = BufReader::new(
        platform_fs::open_read_nofollow(&archive).unwrap()).lines().map(|line|
        serde_json::from_str(&line.unwrap()).unwrap()).collect();
    assert_eq!(unchanged, archived, "Idempotent recovery must preserve the archive");
}
