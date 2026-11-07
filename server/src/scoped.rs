// Defines bounded model operations and their exact permission targets.
use crate::permissions::Target;
// Parses closed input and builds Codex's dynamic tool schema.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    ReadFile { path: String },
    WriteFile { path: String, text: String },
    /// `hosts` are the DNS names the command may reach over HTTPS; none means no network.
    RunShell { text: String, cwd: String, #[serde(default)] hosts: Vec<String> },
    /// Names the direct entries of one folder.
    ListFolder { path: String },
    /// Finds literal text in the files below one folder.
    Search { path: String, text: String },
    /// Replaces one exact passage, which must occur exactly once.
    ReplaceInFile { path: String, find: String, text: String },
    RunApplescript { text: String, cwd: String },
    TexteditOpen { path: String },
    TexteditObserve { path: String },
    TexteditReplace { path: String, text: String },
    TexteditSave { path: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidOperation;

impl Operation {
    /// Parses one closed model request and rejects malformed or oversized targets.
    pub fn parse(value: &Value) -> Result<Self, InvalidOperation> {
        // Measured with the real Codex and Claude: both name a shell command `command`, although
        // the schema calls it `text`. One exact alias is accepted; it changes no check below.
        let mut value = value.clone();
        if value["action"] == "run_shell" && value.get("text").is_none()
            && let Some(command) = value.as_object_mut().and_then(|fields| fields.remove("command")) {
            value["text"] = command;
        }
        let operation: Self = serde_json::from_value(value).map_err(|_| InvalidOperation)?;
        if operation.targets().iter().any(|target| target.validate().is_err()) {
            return Err(InvalidOperation);
        }
        let text = match &operation {
            Self::WriteFile { text, .. } | Self::TexteditReplace { text, .. }
                | Self::ReplaceInFile { text, .. } => Some(text),
            _ => None,
        };
        // A command names at most eight hosts, each once.
        if let Self::RunShell { hosts, .. } = &operation {
            let mut unique = hosts.clone();
            unique.sort();
            unique.dedup();
            if hosts.len() > 8 || unique.len() != hosts.len() { return Err(InvalidOperation); }
        }
        // An empty passage matches everywhere, and neither may hide a terminator.
        let needle = match &operation {
            Self::Search { text, .. } => Some((text, 512)),
            Self::ReplaceInFile { find, .. } => Some((find, 32_768)),
            _ => None,
        };
        if needle.is_some_and(|(text, limit)| text.is_empty() || text.len() > limit
            || text.contains('\0')) {
            return Err(InvalidOperation);
        }
        if text.is_some_and(|text| text.len() > 32_768 || text.contains('\0')) {
            return Err(InvalidOperation);
        }
        Ok(operation)
    }

    /// Returns every exact app, file, or command grant this operation needs.
    pub fn targets(&self) -> Vec<Target> {
        match self {
            Self::ReadFile { path } => vec![Target::File {
                path: path.clone(), action: "read".into(),
            }],
            Self::WriteFile { path, .. } | Self::ReplaceInFile { path, .. } => vec![Target::File {
                path: path.clone(), action: "write".into(),
            }],
            Self::ListFolder { path } | Self::Search { path, .. } => vec![Target::Folder {
                path: path.clone(), action: "read".into(),
            }],
            Self::RunShell { text, cwd, hosts } => std::iter::once(Target::Command {
                kind: "shell".into(), text: text.clone(), cwd: cwd.clone(),
            }).chain(hosts.iter().map(|host| Target::Host { host: host.clone() })).collect(),
            Self::RunApplescript { text, cwd } => vec![Target::Command {
                kind: "applescript".into(), text: text.clone(), cwd: cwd.clone(),
            }],
            Self::TexteditOpen { path } => vec![
                Self::app("control"), Self::file(path, "read"),
            ],
            Self::TexteditObserve { path } => vec![
                Self::app("observe"), Self::file(path, "read"),
            ],
            Self::TexteditReplace { path, .. } | Self::TexteditSave { path } => vec![
                Self::app("control"), Self::file(path, "write"),
            ],
        }
    }

    /// Names the fixed reviewed TextEdit identity and requested access mode.
    fn app(action: &str) -> Target {
        Target::Application { id: "com.apple.TextEdit".into(), action: action.into() }
    }

    /// Names an exact document without granting adjacent paths.
    fn file(path: &str, action: &str) -> Target {
        Target::File { path: path.into(), action: action.into() }
    }
}

/// Keeps unsupported isolation closed while permitting a separately authorized timed shell.
pub(crate) fn host_execution_available(sandbox: bool, operation: &Operation, timed_shell: bool) -> bool {
    sandbox || (timed_shell && matches!(operation, Operation::RunShell { .. }))
}

/// The file and command tool's name, as both providers see it.
pub const TOOL: &str = "miniclaw_scoped_native";

/// Defines one MiniClaw-owned tool with no open-ended app or process surface.
pub fn tool_definition() -> Value { tool_definition_for(cfg!(windows)) }

/// Applies platform execution guidance without removing permission-checked operations.
fn tool_definition_for(windows: bool) -> Value {
    let mut definition = json!({
        "type": "function", "name": "miniclaw_scoped_native",
        "description": "Ask MiniClaw to list a folder, search a folder for literal text, read a file, write a file (write_file replaces an existing file or creates a new one in an existing folder), replace one exact passage in a file, or run an exact reviewed shell command. With an explicit live timed Unsandboxed shell grant, run_shell instead runs as the signed-in user outside the sandbox, with unrestricted file and network access in a fresh shell, and is stopped when access expires or is revoked. It never shares the persistent terminal in that mode. Otherwise run_shell has no network unless hosts lists the exact DNS names it needs over HTTPS (for example registry.npmjs.org); the user approves them with the command, and any other host is refused. In a conversation with a working folder, run_shell runs in the conversation's shared terminal, which the user can see and type into: the shell keeps its state between commands (cd, exported variables, a server left running), a command that prints more than 60 KB is shown to you cut with the rest in the terminal, and one that runs past 120 s is reported as still running. list_folder and search take path (search also text, a literal string, not a pattern). replace_in_file takes path, find (must occur exactly once) and text (the replacement). Every operation is checked against this conversation's permissions and may require user approval. No apps or GUI controls are available.",
        "inputSchema": { "type": "object", "additionalProperties": false,
            "required": ["action"],
            "properties": {
                "action": { "type": "string", "enum": ["list_folder", "search", "read_file",
                    "write_file", "replace_in_file", "run_shell", "run_applescript", "textedit_open", "textedit_observe",
                    "textedit_replace", "textedit_save"] },
                "path": { "type": "string" },
                "text": { "type": "string", "description": "For run_shell: the exact command line. For write_file and replace_in_file: the new text. For search: the literal text to find." },
                "find": { "type": "string" },
                "hosts": { "type": "array", "maxItems": 8, "items": { "type": "string" } },
                "cwd": { "type": "string", "description": "For run_shell: the absolute folder to run in. In a conversation with a working folder, leave it out to use that folder; a folder this conversation may not use is refused." }
            }
        }
    });
    if windows {
        let common = definition["description"].as_str().unwrap_or_default();
        definition["description"] = json!(format!("{common} On Windows, sandboxed run_shell uses the bundled POSIX shell with an absolute Windows cwd. Use POSIX syntax and forward slashes in shell paths. File operations use the native permission broker; sandboxed commands and the shared terminal use AppContainer isolation. Approved network access requires the installed Windows network service and goes through the host-filtering proxy. A live timed Unsandboxed shell grant remains an explicit exception: it runs in a fresh PowerShell with the signed-in user's unrestricted files and network, and expiry or revocation terminates its process tree. Never substitute unrestricted execution when sandbox preparation fails."));
    }
    definition
}

#[cfg(test)]
mod platform_tests {
    use super::*;

    /// Windows exposes the same permission-checked operations through its native broker.
    #[test]
    fn windows_schema_preserves_file_tools_and_sandboxed_shell() {
        let definition = tool_definition_for(true);
        let actions = definition["inputSchema"]["properties"]["action"]["enum"].as_array().unwrap();
        for action in ["read_file", "write_file", "replace_in_file", "list_folder", "search", "run_shell"] {
            assert!(actions.iter().any(|value| value == action), "missing {action}");
        }
        let description = definition["description"].as_str().unwrap();
        assert!(description.contains("AppContainer") && description.contains("POSIX shell"));
        assert!(description.contains("fresh PowerShell"));
        assert!(description.contains("timed Unsandboxed shell grant"));
        assert!(!description.contains("unavailable"));
    }

    #[test]
    fn missing_sandbox_only_allows_explicit_timed_shell() {
        let shell = Operation::RunShell { text: "echo test".into(), cwd: "/".into(), hosts: vec![] };
        let file = Operation::ReadFile { path: "/test".into() };
        assert!(!host_execution_available(false, &shell, false));
        assert!(host_execution_available(false, &shell, true));
        assert!(!host_execution_available(false, &file, true));
        assert!(host_execution_available(true, &file, false));
    }
}
