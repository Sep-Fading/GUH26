// Checks the owned tool's exact request contract.
use miniclaw_server::scoped::{Operation, tool_definition};
// Compares hostile and valid model arguments.
use serde_json::json;

#[test]
fn scoped_tool_rejects_unreviewable_arguments() {
    for value in [
        json!({"action":"read_file","path":"/tmp/../private"}),
        json!({"action":"read_file","path":"/tmp/link","extra":true}),
        json!({"action":"run_shell","text":"id","cwd":"."}),
        json!({"action":"textedit_replace","path":"/tmp/a.txt"}),
        json!({"action":"unknown","path":"/tmp/a.txt"}),
    ] {
        assert!(Operation::parse(&value).is_err(), "accepted {value}");
    }
}

#[test]
fn scoped_tool_maps_exact_authorization_targets() {
    let read = Operation::parse(&json!({"action":"read_file","path":"/tmp/a.txt"}))
        .unwrap();
    assert_eq!(read.targets().iter().map(|target| serde_json::to_value(target).unwrap())
        .collect::<Vec<_>>(), vec![json!({"target_type":"file",
        "path":"/tmp/a.txt","action":"read"})]);
    let shell = Operation::parse(&json!({"action":"run_shell","text":"printf yes",
        "cwd":"/tmp"})).unwrap();
    assert_eq!(shell.targets().iter().map(|target| serde_json::to_value(target).unwrap())
        .collect::<Vec<_>>(), vec![json!({"target_type":"command",
        "kind":"shell","text":"printf yes","cwd":"/tmp"})]);
    let native = Operation::parse(&json!({"action":"textedit_replace",
        "path":"/tmp/a.txt","text":"new"})).unwrap();
    assert_eq!(native.targets().iter().map(|target| serde_json::to_value(target).unwrap())
        .collect::<Vec<_>>(), vec![
        json!({"target_type":"application","id":"com.apple.TextEdit","action":"control"}),
        json!({"target_type":"file","path":"/tmp/a.txt","action":"write"}),
    ]);
}

#[test]
fn scoped_definition_has_one_closed_input_surface() {
    let definition = tool_definition();
    assert_eq!(definition["name"], "miniclaw_scoped_native");
    assert_eq!(definition["inputSchema"]["additionalProperties"], false);
}

/// The folder operations ask for the folder, and an edit asks for the file, to write.
#[test]
fn folder_and_edit_operations_map_to_exact_targets() {
    use miniclaw_server::permissions::Target;
    let list = Operation::parse(&json!({ "action": "list_folder", "path": "/work/project" })).unwrap();
    assert_eq!(list.targets(), vec![Target::Folder { path: "/work/project".into(), action: "read".into() }]);
    let search = Operation::parse(&json!({ "action": "search", "path": "/work/project",
        "text": "needle" })).unwrap();
    assert_eq!(search.targets(), vec![Target::Folder { path: "/work/project".into(), action: "read".into() }]);
    let edit = Operation::parse(&json!({ "action": "replace_in_file", "path": "/work/project/a.rs",
        "find": "old", "text": "new" })).unwrap();
    assert_eq!(edit.targets(), vec![Target::File { path: "/work/project/a.rs".into(), action: "write".into() }]);
    assert!(Operation::parse(&json!({ "action": "search", "path": "/work/../etc", "text": "x" })).is_err());
    assert!(Operation::parse(&json!({ "action": "search", "path": "/work", "text": "" })).is_err());
    let names: Vec<String> = miniclaw_server::scoped::tool_definition()["inputSchema"]["properties"]
        ["action"]["enum"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
    for action in ["list_folder", "search", "replace_in_file"] { assert!(names.contains(&action.to_owned())); }
}

/// Real models name a shell command `command`; that one alias is read, nothing else is loosened.
#[test]
fn run_shell_accepts_command_as_the_command_text() {
    let parsed = Operation::parse(&serde_json::json!({ "action": "run_shell", "command": "wc -l a.txt", "cwd": "/tmp" })).unwrap();
    assert!(matches!(parsed, Operation::RunShell { ref text, .. } if text == "wc -l a.txt"));
    assert!(Operation::parse(&serde_json::json!({ "action": "read_file", "path": "/tmp/a", "command": "x" })).is_err());
    assert!(Operation::parse(&serde_json::json!({ "action": "run_shell", "cwd": "/tmp", "cmd": "ls" })).is_err());
}
