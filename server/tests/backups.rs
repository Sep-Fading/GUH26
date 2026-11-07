// Runs folder backups between a local stand-in for the VM and disposable Mac folders.
use miniclaw_server::backups::{Backups, Schedule, Settings};
use miniclaw_server::transfer::Guest;
use miniclaw_server::transfer_jobs::{Remote, Transfers};
// Builds fixtures and inspects results.
use std::{fs, path::PathBuf, sync::Arc, time::Duration};
// Names each disposable root.
use uuid::Uuid;

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("mc-backups-{}", Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
}
impl Drop for Directory { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

/// Builds the VM stand-in with a small workspace, and a backup store beside it.
fn fixture(root: &Directory) -> (Remote, Backups, Transfers) {
    fs::create_dir_all(root.0.join("vm/Workspace/src")).unwrap();
    fs::write(root.0.join("vm/Workspace/src/main.rs"), "fn main() {}").unwrap();
    fs::write(root.0.join("vm/Workspace/notes.txt"), "first").unwrap();
    let remote = Remote::Local(Arc::new(Guest::new(vec![("workspace".into(), root.0.join("vm/Workspace")),
        ("transfers".into(), root.0.join("vm/Transfers"))])));
    let backups = Backups::new(root.0.join("vm-backups.json"), root.0.join("Backups"), root.0.join("Trash"));
    (remote, backups, Transfers::default())
}

#[tokio::test]
async fn backups_are_off_until_the_user_switches_them_on() {
    let root = Directory::new();
    let (_remote, backups, _transfers) = fixture(&root);
    let settings = backups.settings("master");
    assert_eq!((settings.schedule, settings.keep), (Schedule::Off, 3));
    assert!(!backups.due("master", 10_000_000), "no schedule, nothing is ever due");
    assert!(!root.0.join("Backups").exists(), "nothing is written before the first backup");
    assert!(backups.list("master").is_empty());
    // Only offered values are saved.
    assert!(backups.save("master", Settings { keep: 0, ..backups.settings("master") }).is_err());
    assert!(backups.save("master", Settings { folders: vec!["../etc".into()], ..backups.settings("master") }).is_err());
    backups.save("master", Settings { schedule: Schedule::Daily, ..backups.settings("master") }).unwrap();
    assert!(backups.due("master", 1_000));
    assert!(!backups.due("other-vm", 1_000), "each VM has its own switch");
    let reopened = Backups::new(root.0.join("vm-backups.json"), root.0.join("Backups"), root.0.join("Trash"));
    assert_eq!(reopened.settings("master").schedule, Schedule::Daily);
}

#[tokio::test]
async fn a_backup_is_whole_with_checksums_and_old_ones_go_to_the_trash() {
    let root = Directory::new();
    let (remote, backups, transfers) = fixture(&root);
    backups.save("master", Settings { keep: 2, ..backups.settings("master") }).unwrap();
    for (second, text) in [(100, "first"), (200, "second"), (300, "third")] {
        fs::write(root.0.join("vm/Workspace/notes.txt"), text).unwrap();
        let done = backups.run(&transfers, remote.clone(), "master", second).await.unwrap();
        assert_eq!(done.files, 2);
    }
    let kept = backups.list("master");
    assert_eq!(kept.len(), 2, "the newest two stay");
    assert!(kept[0].created > kept[1].created);
    let newest = root.0.join("Backups/master").join(&kept[0].id);
    assert_eq!(fs::read_to_string(newest.join("files/Workspace/notes.txt")).unwrap(), "third");
    let manifest: serde_json::Value = serde_json::from_slice(&fs::read(newest.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["files"].as_array().unwrap().len(), 2);
    assert_eq!(manifest["files"][0]["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(fs::read_dir(root.0.join("Trash")).unwrap().count(), 1, "the oldest was moved, not deleted");
    assert!(!backups.due("master", 301));
    assert_eq!(backups.settings("master").last_error, None);
}

#[tokio::test]
async fn a_failed_backup_leaves_nothing_behind_and_keeps_the_old_ones() {
    let root = Directory::new();
    let (remote, backups, transfers) = fixture(&root);
    backups.run(&transfers, remote.clone(), "master", 100).await.unwrap();
    backups.save("master", Settings { folders: vec!["missing-folder".into()], keep: 1, ..backups.settings("master") }).unwrap();
    let failed = backups.run_with_retries(&transfers, remote.clone(), "master", 200, Duration::from_millis(1)).await;
    assert_eq!(failed.unwrap_err().attempts, 4, "one attempt and three retries");
    assert_eq!(backups.list("master").len(), 1, "the earlier backup is untouched");
    assert_eq!(fs::read_dir(root.0.join("Backups/master")).unwrap().count(), 1, "no incomplete folder is left");
    assert!(backups.settings("master").last_error.is_some());
}

#[tokio::test]
async fn restore_checks_every_file_and_never_overwrites_the_workspace() {
    let root = Directory::new();
    let (remote, backups, transfers) = fixture(&root);
    let done = backups.run(&transfers, remote.clone(), "master", 100).await.unwrap();
    fs::write(root.0.join("vm/Workspace/notes.txt"), "changed after the backup").unwrap();
    let job = backups.restore(&transfers, remote.clone(), "master", &done.id).await.unwrap();
    for _ in 0..500 {
        if transfers.view(job).unwrap()["state"] != "running" { break; }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(transfers.view(job).unwrap()["state"], "done");
    let restored = root.0.join("vm/Transfers").join(format!("restore-{}", done.id));
    assert_eq!(fs::read_to_string(restored.join("Workspace/notes.txt")).unwrap(), "first");
    assert_eq!(fs::read_to_string(root.0.join("vm/Workspace/notes.txt")).unwrap(), "changed after the backup");

    // A backup that no longer matches its manifest is refused before anything is sent.
    fs::write(root.0.join("Backups/master").join(&done.id).join("files/Workspace/notes.txt"), "tampered").unwrap();
    assert!(backups.restore(&transfers, remote.clone(), "master", &done.id).await.is_err());
    assert!(backups.restore(&transfers, remote, "master", "../../etc").await.is_err());
}
