#![cfg(unix)]

// Exercises the guest's descriptor-based file boundary.
use miniclaw_server::scoped::Operation;
// Runs a reviewed action without bypassing the controller approval test.
use miniclaw_server::executor::{execute_host, read_file, write_file};
// Creates disposable regular files and hostile shortcuts.
use std::{fs, path::PathBuf};
// Keeps test names isolated from user data.
use uuid::Uuid;

struct Fixture(PathBuf);

impl Fixture {
    /// Creates one private test directory and returns its cleanup owner.
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("mc-scoped-{}", Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
}

impl Drop for Fixture {
    /// Removes only this UUID-named test fixture.
    fn drop(&mut self) { fs::remove_dir_all(&self.0).unwrap(); }
}

#[test]
fn reads_and_edits_only_exact_regular_file() {
    let fixture = Fixture::new();
    let approved = fixture.0.join("approved.txt");
    let denied = fixture.0.join("denied.txt");
    fs::write(&approved, "before").unwrap();
    fs::write(&denied, "private").unwrap();
    assert_eq!(read_file(approved.to_str().unwrap()).unwrap(), "before");
    write_file(approved.to_str().unwrap(), "after").unwrap();
    assert_eq!(fs::read_to_string(&approved).unwrap(), "after");
    assert_eq!(fs::read_to_string(&denied).unwrap(), "private");
}

#[cfg(unix)]
#[test]
fn refuses_symlink_and_directory_shortcuts() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let approved = fixture.0.join("approved.txt");
    fs::write(&approved, "before").unwrap();
    symlink(&approved, fixture.0.join("link.txt")).unwrap();
    symlink(&fixture.0, fixture.0.join("linked-dir")).unwrap();
    assert!(read_file(fixture.0.join("link.txt").to_str().unwrap()).is_err());
    assert!(write_file(fixture.0.join("link.txt").to_str().unwrap(), "bad").is_err());
    assert!(read_file(fixture.0.join("linked-dir/approved.txt").to_str().unwrap()).is_err());
    assert!(read_file(&format!("{}/./approved.txt", fixture.0.display())).is_err());
    assert!(read_file(&format!("{}//approved.txt", fixture.0.display())).is_err());
    assert_eq!(fs::read_to_string(&approved).unwrap(), "before");
}

#[tokio::test]
async fn exact_command_uses_reviewed_working_directory() {
    let fixture = Fixture::new();
    let cwd = fixture.0.canonicalize().unwrap();
    let operation = Operation::RunShell { text: "pwd".into(), cwd: cwd.to_str().unwrap().into(), hosts: Vec::new() };
    // The working directory is readable only when granted, like everything else.
    let grant = miniclaw_server::seatbelt::Grant { path: cwd.to_str().unwrap().into(),
        write: false, folder: true, blocked: false };
    let result = execute_host(operation, &[grant]).await;
    assert_eq!(result["status"], "ok");
    assert_eq!(result["text"].as_str().unwrap().trim(), cwd.to_str().unwrap());
    // Apple Events make an unsandboxed app act, so they are refused everywhere.
    let script = Operation::RunApplescript {
        text: "return \"approved\"".into(), cwd: cwd.to_str().unwrap().into(),
    };
    assert_eq!(execute_host(script, &[]).await["status"], "denied");
}

/// Runs one tool operation with no shell grants; file operations need none.
async fn run(arguments: serde_json::Value) -> serde_json::Value {
    execute_host(Operation::parse(&arguments).unwrap(), &[]).await
}

/// Listing names a folder's direct entries and follows no link.
#[tokio::test]
async fn list_folder_names_direct_entries_only() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("src")).unwrap();
    fs::write(fixture.0.join("src/deep.rs"), "fn deep() {}").unwrap();
    fs::write(fixture.0.join("notes.md"), "hello").unwrap();
    std::os::unix::fs::symlink("/etc", fixture.0.join("shortcut")).unwrap();
    let result = run(serde_json::json!({ "action": "list_folder",
        "path": fixture.0.to_str().unwrap() })).await;
    assert_eq!(result["status"], "ok", "{result}");
    let text = result["text"].as_str().unwrap();
    assert_eq!(text, "notes.md\tfile\t5\nshortcut\tlink\nsrc\tfolder");
    let file = run(serde_json::json!({ "action": "list_folder",
        "path": fixture.0.join("notes.md").to_str().unwrap() })).await;
    assert_eq!(file["status"], "denied");
    let through_link = run(serde_json::json!({ "action": "list_folder",
        "path": fixture.0.join("shortcut").to_str().unwrap() })).await;
    assert_eq!(through_link["status"], "denied", "a linked folder is not listed");
}

/// Search is a literal text search inside one folder; it follows no link and is bounded.
#[tokio::test]
async fn search_finds_literal_text_below_one_folder() {
    let fixture = Fixture::new();
    let outside = Fixture::new();
    fs::write(outside.0.join("secret.txt"), "needle outside").unwrap();
    fs::create_dir_all(fixture.0.join("src/inner")).unwrap();
    fs::write(fixture.0.join("src/a.rs"), "one\nlet needle = 1;\nthree").unwrap();
    fs::write(fixture.0.join("src/inner/b.rs"), "a (needle).* b").unwrap();
    fs::write(fixture.0.join("binary.bin"), [0u8, 159, 146, 150, b'n', b'e']).unwrap();
    std::os::unix::fs::symlink(&outside.0, fixture.0.join("escape")).unwrap();
    let root = fixture.0.to_str().unwrap();
    let result = run(serde_json::json!({ "action": "search", "path": root, "text": "needle" })).await;
    assert_eq!(result["status"], "ok", "{result}");
    assert_eq!(result["text"].as_str().unwrap(),
        format!("{root}/src/a.rs:2: let needle = 1;\n{root}/src/inner/b.rs:1: a (needle).* b"));
    // The text is literal, never a pattern.
    let literal = run(serde_json::json!({ "action": "search", "path": root, "text": "(needle).*" })).await;
    assert_eq!(literal["text"].as_str().unwrap().lines().count(), 1);
    let none = run(serde_json::json!({ "action": "search", "path": root, "text": "absent" })).await;
    assert_eq!(none["text"], "No matches.");
    for index in 0..250 { fs::write(fixture.0.join(format!("many-{index}.txt")), "needle").unwrap(); }
    let capped = run(serde_json::json!({ "action": "search", "path": root, "text": "needle" })).await;
    let text = capped["text"].as_str().unwrap();
    assert_eq!(text.lines().count(), 201, "200 matches and one note that more exist");
    assert!(text.ends_with("More matches exist; narrow the search."), "{text}");
}

/// An edit replaces one exact passage; anything ambiguous changes nothing.
#[tokio::test]
async fn replace_in_file_changes_exactly_one_passage() {
    let fixture = Fixture::new();
    let file = fixture.0.join("main.rs");
    fs::write(&file, "fn a() {}\nfn b() {}\nfn a_again() {}\n").unwrap();
    let path = file.to_str().unwrap();
    let done = run(serde_json::json!({ "action": "replace_in_file", "path": path,
        "find": "fn b() {}", "text": "fn b() { a() }" })).await;
    assert_eq!(done["status"], "ok", "{done}");
    assert_eq!(fs::read_to_string(&file).unwrap(), "fn a() {}\nfn b() { a() }\nfn a_again() {}\n");
    let twice = run(serde_json::json!({ "action": "replace_in_file", "path": path,
        "find": "fn a", "text": "fn z" })).await;
    assert_eq!(twice["status"], "failed", "{twice}");
    assert!(twice["text"].as_str().unwrap().contains("2 times"), "{twice}");
    let absent = run(serde_json::json!({ "action": "replace_in_file", "path": path,
        "find": "fn missing", "text": "x" })).await;
    assert_eq!(absent["status"], "failed");
    assert_eq!(fs::read_to_string(&file).unwrap(), "fn a() {}\nfn b() { a() }\nfn a_again() {}\n");
    assert!(Operation::parse(&serde_json::json!({ "action": "replace_in_file", "path": path,
        "find": "", "text": "x" })).is_err(), "an empty passage would match everywhere");
}

/// Writes and edits report a unified diff for the page. It is never part of `text`, which is
/// all the model sees.
#[tokio::test]
async fn writes_and_edits_report_a_diff_for_the_user_only() {
    let fixture = Fixture::new();
    let file = fixture.0.join("lib.rs");
    fs::write(&file, "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();
    let path = file.to_str().unwrap();
    let edited = run(serde_json::json!({ "action": "replace_in_file", "path": path,
        "find": "fn b() {}", "text": "fn b() { a() }" })).await;
    let diff = edited["diff"].as_str().expect("an edit reports its diff");
    assert!(diff.contains("-fn b() {}") && diff.contains("+fn b() { a() }"), "{diff}");
    assert!(diff.contains(" fn a() {}"), "context lines are kept: {diff}");
    assert_eq!(edited["text"], "File updated.");
    let written = run(serde_json::json!({ "action": "write_file", "path": path, "text": "fn only() {}\n" })).await;
    let diff = written["diff"].as_str().expect("a write reports its diff");
    assert!(diff.contains("-fn c() {}") && diff.contains("+fn only() {}"), "{diff}");
    // Writing the same content changes nothing and says so with an empty diff.
    let same = run(serde_json::json!({ "action": "write_file", "path": path, "text": "fn only() {}\n" })).await;
    assert_eq!(same["diff"], "");
    // A refused edit reports no diff.
    let absent = run(serde_json::json!({ "action": "replace_in_file", "path": path, "find": "nope", "text": "x" })).await;
    assert!(absent.get("diff").is_none());
    // A very large change is summarised, not sent whole.
    fs::write(&file, "a long line of existing text\n".repeat(8_000)).unwrap();
    let huge = run(serde_json::json!({ "action": "write_file", "path": path, "text": "short\n" })).await;
    let diff = huge["diff"].as_str().unwrap();
    assert!(diff.len() <= 66_000 && diff.contains("is not shown"), "the diff is bounded: {}", diff.len());
}

/// A write may create a new file in an existing folder, and nowhere a link leads.
#[tokio::test]
async fn write_file_creates_a_new_file_but_never_through_a_link_or_a_missing_folder() {
    let fixture = Fixture::new();
    let fresh = fixture.0.join("fresh.txt");
    let made = run(serde_json::json!({ "action": "write_file", "path": fresh.to_str().unwrap(), "text": "one\ntwo\n" })).await;
    assert_eq!(made["status"], "ok", "{made}");
    assert_eq!(made["text"], "File created.");
    assert_eq!(fs::read_to_string(&fresh).unwrap(), "one\ntwo\n");
    assert!(made["diff"].as_str().unwrap().contains("+one"), "a new file is all additions: {made}");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(fs::metadata(&fresh).unwrap().permissions().mode() & 0o777, 0o644);

    let missing = fixture.0.join("no-such-folder/file.txt");
    assert_eq!(run(serde_json::json!({ "action": "write_file", "path": missing.to_str().unwrap(), "text": "x" })).await["status"], "denied");
    let outside = Fixture::new();
    std::os::unix::fs::symlink(&outside.0, fixture.0.join("link")).unwrap();
    let through = fixture.0.join("link/escaped.txt");
    assert_eq!(run(serde_json::json!({ "action": "write_file", "path": through.to_str().unwrap(), "text": "x" })).await["status"], "denied");
    assert!(!outside.0.join("escaped.txt").exists());
}
