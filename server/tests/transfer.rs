#![cfg(unix)]
// Exercises the VM side of file transfer on disposable folders.
use miniclaw_server::transfer::{Guest, Refusal};
// Builds the JSON requests the backend sends.
use serde_json::json;
// Creates fixtures, links and permissions.
use std::{fs, os::unix::fs::symlink, path::PathBuf};
// Names each disposable root.
use uuid::Uuid;

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("mc-transfer-{}", Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
}
impl Drop for Directory { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

/// Builds a guest with the two folders a VM offers, under one disposable root.
fn guest(root: &Directory) -> Guest {
    fs::create_dir_all(root.0.join("Workspace/src")).unwrap();
    fs::write(root.0.join("Workspace/src/main.rs"), "fn main() {}").unwrap();
    fs::write(root.0.join("Workspace/notes.txt"), "hello").unwrap();
    Guest::new(vec![("workspace".into(), root.0.join("Workspace")),
        ("transfers".into(), root.0.join("Transfers"))])
}

#[test]
fn lists_only_the_offered_folders_and_never_follows_links() {
    let root = Directory::new();
    let guest = guest(&root);
    fs::write(root.0.join("secret.txt"), "token").unwrap();
    symlink(root.0.join("secret.txt"), root.0.join("Workspace/link.txt")).unwrap();
    symlink(&root.0, root.0.join("Workspace/up")).unwrap();
    let listing = guest.list("workspace", "").unwrap();
    assert_eq!(listing, json!([{ "name": "notes.txt", "kind": "file", "size": 5 },
        { "name": "src", "kind": "folder", "size": 0 }]), "links are left out");
    assert_eq!(guest.list("workspace", "src").unwrap()[0]["name"], "main.rs");
    // The Transfers folder is created on first use.
    assert_eq!(guest.list("transfers", "").unwrap(), json!([]));
    for (folder, path) in [("workspace", "../secret.txt"), ("workspace", "/etc"), ("workspace", "up"),
        ("workspace", "src/../../secret.txt"), ("home", ""), ("workspace", "a\0b"), ("workspace", "./src")] {
        assert_eq!(guest.list(folder, path), Err(Refusal::Path), "{folder} {path:?}");
    }
    assert_eq!(guest.read("workspace", "link.txt", 0, 10).unwrap_err(), Refusal::Path);
    assert_eq!(guest.read("workspace", "up/secret.txt", 0, 10).unwrap_err(), Refusal::Path);
}

#[test]
fn reads_in_pieces_and_reports_what_the_file_looked_like() {
    let root = Directory::new();
    let guest = guest(&root);
    let first = guest.read("workspace", "notes.txt", 0, 3).unwrap();
    assert_eq!((first.bytes.as_slice(), first.size, first.end), (&b"hel"[..], 5, false));
    let rest = guest.read("workspace", "notes.txt", 3, 100).unwrap();
    assert_eq!((rest.bytes.as_slice(), rest.end), (&b"lo"[..], true));
    assert_eq!(first.modified, rest.modified);
    fs::write(root.0.join("Workspace/notes.txt"), "changed while copying").unwrap();
    assert_ne!(guest.read("workspace", "notes.txt", 0, 1).unwrap().size, first.size);
    assert_eq!(guest.read("workspace", "src", 0, 1).unwrap_err(), Refusal::Path, "a folder is not a file");
}

#[test]
fn a_file_appears_only_when_its_last_piece_arrived() {
    let root = Directory::new();
    let guest = guest(&root);
    guest.write("transfers", "2026-09-21/report.txt", 0, b"hel", false, false).unwrap();
    assert!(!root.0.join("Transfers/2026-09-21/report.txt").exists(), "a partial copy is not a file");
    // A piece at the wrong place means pieces were lost; nothing is patched together.
    assert_eq!(guest.write("transfers", "2026-09-21/report.txt", 9, b"x", true, false), Err(Refusal::Sequence));
    guest.write("transfers", "2026-09-21/report.txt", 3, b"lo", true, false).unwrap();
    assert_eq!(fs::read_to_string(root.0.join("Transfers/2026-09-21/report.txt")).unwrap(), "hello");
    assert_eq!(fs::read_dir(root.0.join("Transfers/2026-09-21")).unwrap().count(), 1, "no partial file is left");

    // An existing file is kept unless the user chose Replace.
    assert_eq!(guest.write("transfers", "2026-09-21/report.txt", 0, b"new", true, false), Err(Refusal::Exists));
    assert_eq!(fs::read_to_string(root.0.join("Transfers/2026-09-21/report.txt")).unwrap(), "hello");
    guest.write("transfers", "2026-09-21/report.txt", 0, b"new", true, true).unwrap();
    assert_eq!(fs::read_to_string(root.0.join("Transfers/2026-09-21/report.txt")).unwrap(), "new");
    assert_eq!(guest.exists("transfers", "2026-09-21/report.txt").unwrap(), true);
    assert_eq!(guest.exists("transfers", "2026-09-21/other.txt").unwrap(), false);
}

#[test]
fn writes_stay_inside_the_transfers_folder() {
    let root = Directory::new();
    let guest = guest(&root);
    assert_eq!(guest.write("workspace", "x.txt", 0, b"x", true, false), Err(Refusal::Path),
        "only the Transfers folder receives files");
    assert_eq!(guest.write("transfers", "../escape.txt", 0, b"x", true, false), Err(Refusal::Path));
    fs::create_dir_all(root.0.join("Transfers")).unwrap();
    symlink(root.0.join("Workspace"), root.0.join("Transfers/out")).unwrap();
    assert_eq!(guest.write("transfers", "out/x.txt", 0, b"x", true, false), Err(Refusal::Path));
    assert!(!root.0.join("Workspace/x.txt").exists());
    // A cancelled copy removes its partial file and nothing else.
    guest.write("transfers", "big.bin", 0, b"part", false, false).unwrap();
    guest.discard("transfers", "big.bin").unwrap();
    assert_eq!(fs::read_dir(root.0.join("Transfers")).unwrap()
        .filter(|entry| entry.as_ref().unwrap().file_name() != "out").count(), 0);
    assert!(guest.write("transfers", "huge.bin", 0, b"x", false, false).is_ok());
    assert_eq!(guest.room("transfers", u64::MAX / 2), Err(Refusal::Space));
    assert!(guest.room("transfers", 1).is_ok());
}
