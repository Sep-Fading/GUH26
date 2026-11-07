#![cfg(unix)]
// Runs whole transfers between disposable "Mac" folders and a local stand-in for the VM.
use miniclaw_server::transfer::Guest;
use miniclaw_server::transfer_jobs::{Choice, Remote, Source, Transfers};
// Builds fixtures, links and checks results.
use std::{fs, os::unix::fs::symlink, path::PathBuf, sync::Arc, time::Duration};
// Names each disposable root.
use uuid::Uuid;

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("mc-jobs-{}", Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
}
impl Drop for Directory { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

/// Builds the VM stand-in: the same code the worker runs, on local folders.
fn vm(root: &Directory) -> Remote {
    fs::create_dir_all(root.0.join("vm/Workspace/out")).unwrap();
    Remote::Local(Arc::new(Guest::new(vec![("workspace".into(), root.0.join("vm/Workspace")),
        ("transfers".into(), root.0.join("vm/Transfers"))])))
}

/// Waits until the job leaves `running`, and returns its view.
async fn settle(transfers: &Transfers, job: Uuid) -> serde_json::Value {
    for _ in 0..500 {
        let view = transfers.view(job).unwrap();
        if view["state"] != "running" { return view; }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the transfer did not settle");
}

#[tokio::test]
async fn files_and_folders_reach_the_vm_and_links_are_left_behind() {
    let root = Directory::new();
    let mac = root.0.join("mac");
    fs::create_dir_all(mac.join("site/css")).unwrap();
    fs::write(mac.join("site/index.html"), "<h1>hi</h1>").unwrap();
    fs::write(mac.join("site/css/app.css"), vec![b'x'; 1_300_000]).unwrap();
    fs::write(mac.join("notes.txt"), "notes").unwrap();
    fs::write(root.0.join("secret"), "token").unwrap();
    symlink(root.0.join("secret"), mac.join("site/leak")).unwrap();
    let transfers = Transfers::default();
    let job = transfers.to_vm(vm(&root), vec![mac.join("site"), mac.join("notes.txt")], "2026-09-21".into());
    let view = settle(&transfers, job).await;
    assert_eq!(view["state"], "done", "{view}");
    assert_eq!((view["doneFiles"].as_u64(), view["totalFiles"].as_u64(), view["skipped"].as_u64()),
        (Some(3), Some(3), Some(1)), "{view}");
    assert_eq!(view["doneBytes"], view["totalBytes"]);
    let landed = root.0.join("vm/Transfers/2026-09-21");
    assert_eq!(fs::read_to_string(landed.join("site/index.html")).unwrap(), "<h1>hi</h1>");
    assert_eq!(fs::read(landed.join("site/css/app.css")).unwrap().len(), 1_300_000);
    assert_eq!(fs::read_to_string(landed.join("notes.txt")).unwrap(), "notes");
    assert!(!landed.join("site/leak").exists(), "a link is never copied");
}

#[tokio::test]
async fn a_name_conflict_waits_for_the_user_and_each_answer_does_what_it_says() {
    let root = Directory::new();
    let mac = root.0.join("mac");
    fs::create_dir_all(&mac).unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] { fs::write(mac.join(name), format!("new {name}")).unwrap(); }
    let remote = vm(&root);
    let landed = root.0.join("vm/Transfers/2026-09-21");
    fs::create_dir_all(&landed).unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] { fs::write(landed.join(name), "old").unwrap(); }
    let transfers = Transfers::default();
    let job = transfers.to_vm(remote, vec![mac.join("a.txt"), mac.join("b.txt"), mac.join("c.txt")],
        "2026-09-21".into());
    for (name, choice) in [("a.txt", Choice::Replace), ("b.txt", Choice::KeepBoth), ("c.txt", Choice::Skip)] {
        let view = settle(&transfers, job).await;
        assert_eq!((view["state"].as_str(), view["conflict"].as_str()), (Some("conflict"), Some(name)), "{view}");
        assert!(transfers.resolve(job, choice, false));
    }
    let view = settle(&transfers, job).await;
    assert_eq!(view["state"], "done", "{view}");
    assert_eq!(fs::read_to_string(landed.join("a.txt")).unwrap(), "new a.txt");
    assert_eq!(fs::read_to_string(landed.join("b.txt")).unwrap(), "old");
    assert_eq!(fs::read_to_string(landed.join("b 2.txt")).unwrap(), "new b.txt");
    assert_eq!(fs::read_to_string(landed.join("c.txt")).unwrap(), "old");
    assert_eq!(view["skipped"], 1);

    // "Apply to all" answers the rest without asking again.
    let again = transfers.to_vm(vm(&root), vec![mac.join("a.txt"), mac.join("c.txt")], "2026-09-21".into());
    assert_eq!(settle(&transfers, again).await["state"], "conflict");
    assert!(transfers.resolve(again, Choice::Replace, true));
    assert_eq!(settle(&transfers, again).await["state"], "done");
    assert_eq!(fs::read_to_string(landed.join("c.txt")).unwrap(), "new c.txt");
}

#[tokio::test]
async fn files_come_back_to_the_mac_whole_or_not_at_all() {
    let root = Directory::new();
    let remote = vm(&root);
    fs::write(root.0.join("vm/Workspace/out/result.bin"), vec![7u8; 900_000]).unwrap();
    fs::write(root.0.join("vm/Workspace/readme.md"), "# done").unwrap();
    let destination = root.0.join("mac-downloads");
    fs::create_dir(&destination).unwrap();
    let transfers = Transfers::default();
    let job = transfers.to_mac(remote, vec![Source { folder: "workspace".into(), path: "out".into() },
        Source { folder: "workspace".into(), path: "readme.md".into() }], destination.clone());
    let view = settle(&transfers, job).await;
    assert_eq!(view["state"], "done", "{view}");
    assert_eq!(fs::read(destination.join("out/result.bin")).unwrap(), vec![7u8; 900_000]);
    assert_eq!(fs::read_to_string(destination.join("readme.md")).unwrap(), "# done");

    // A path outside the offered folders fails the job and leaves nothing behind.
    let bad = transfers.to_mac(vm(&root), vec![Source { folder: "workspace".into(), path: "../../mac-downloads".into() }],
        destination.clone());
    let view = settle(&transfers, bad).await;
    assert_eq!(view["state"], "failed", "{view}");
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 2);
}

#[tokio::test]
async fn a_cancelled_transfer_leaves_no_partial_file_and_is_not_called_done() {
    let root = Directory::new();
    let mac = root.0.join("mac");
    fs::create_dir_all(&mac).unwrap();
    fs::write(mac.join("big.bin"), vec![1u8; 6_000_000]).unwrap();
    fs::write(mac.join("z-last.txt"), "never sent").unwrap();
    let transfers = Transfers::default();
    let job = transfers.to_vm(vm(&root), vec![mac.join("big.bin"), mac.join("z-last.txt")], "2026-09-21".into());
    assert!(transfers.cancel(job));
    let view = settle(&transfers, job).await;
    assert_eq!(view["state"], "cancelled", "{view}");
    let landed = root.0.join("vm/Transfers/2026-09-21");
    let left: Vec<_> = fs::read_dir(&landed).map(|entries| entries.flatten()
        .map(|entry| entry.file_name().into_string().unwrap()).collect()).unwrap_or_default();
    assert!(left.iter().all(|name| !name.contains("partial") && name != "z-last.txt"), "{left:?}");
    assert!(view["doneFiles"].as_u64().unwrap() < 2);
}
