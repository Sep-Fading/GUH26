//! What MiniClaw's own tool did in each conversation, for the user to follow live and read
//! later. Providers do not return tool calls with their history, so the Mac keeps this log
//! itself. The user sees the full log; delegation status exposes only bounded operation
//! names, states and timestamps to the parent model. Outputs and diffs stay in the UI.

// Describes operations and shapes entries.
use crate::scoped::Operation;
use serde_json::{Value, json};
// Keeps recent logs in memory and private copies on disk.
use std::{collections::HashMap, fs, io::Write, path::PathBuf, sync::{Mutex, OnceLock}};

/// The newest entries kept for one conversation, and the most output one entry holds.
const KEPT: usize = 300;
const OUTPUT: usize = 16 * 1024;

#[derive(Default)]
pub struct Log { pub dir: OnceLock<PathBuf>, threads: Mutex<HashMap<String, Vec<Value>>> }

/// Cuts text at a character boundary and says how much is missing.
fn bounded(text: &str) -> String {
    if text.len() <= OUTPUT { return text.to_owned(); }
    let mut end = OUTPUT;
    while !text.is_char_boundary(end) { end -= 1; }
    format!("{}\n... {} more bytes not shown", &text[..end], text.len() - end)
}

/// Starts an entry for one tool call: what kind it is and what it names.
pub fn started(id: &str, turn: &str, operation: &Operation, agent: Option<&str>) -> Value {
    let (kind, title, detail) = match operation {
        Operation::ListFolder { path } => ("list", path.clone(), None),
        Operation::Search { path, text } => ("search", text.clone(), Some(path.clone())),
        Operation::ReadFile { path } => ("read", path.clone(), None),
        Operation::WriteFile { path, .. } => ("write", path.clone(), None),
        Operation::ReplaceInFile { path, .. } => ("edit", path.clone(), None),
        Operation::RunShell { text, cwd, .. } => ("command", text.clone(), Some(cwd.clone())),
        _ => ("other", String::new(), None),
    };
    json!({ "id": id, "turnId": turn, "kind": kind, "title": bounded(&title), "detail": detail,
        "agent": agent, "status": "running", "startedAt": chrono::Utc::now().timestamp_millis() })
}

/// Starts an entry for a call whose arguments MiniClaw could not read. What can be read of it is
/// shown the way a readable call is, so a command missing its folder reads as that command; only
/// arguments with no recognizable action or subject show as sent.
pub fn malformed(id: &str, turn: &str, arguments: &Value) -> Value {
    let action = arguments["action"].as_str().unwrap_or_default();
    let field = |name: &str| arguments[name].as_str().map(str::to_owned);
    let (kind, title) = match action {
        "run_shell" => ("command", field("text").or_else(|| field("command"))),
        "read_file" => ("read", field("path")),
        "write_file" => ("write", field("path")),
        "replace_in_file" => ("edit", field("path")),
        "list_folder" => ("list", field("path")),
        "search" => ("search", field("text")),
        _ => ("other", None),
    };
    let (kind, title) = match title { Some(title) => (kind, title), None => ("other", arguments.to_string()) };
    json!({ "id": id, "turnId": turn, "kind": kind, "title": bounded(&title), "detail": null,
        "agent": null, "status": "running", "malformed": true, "startedAt": chrono::Utc::now().timestamp_millis() })
}

/// Says what a call MiniClaw could not read lacks, for the model and the activity row.
pub fn malformed_reason(arguments: &Value) -> String {
    let missing = |name: &str| !arguments[name].is_string();
    let lacking = match arguments["action"].as_str() {
        Some("run_shell") if missing("cwd") => " It had no cwd: give the absolute path of the folder to run in.",
        Some("run_shell") if missing("text") && missing("command") => " It had no text: give the command to run.",
        Some("read_file" | "write_file" | "replace_in_file" | "list_folder" | "search") if missing("path") => " It had no path: give an absolute path.",
        _ => "",
    };
    format!("MiniClaw could not read this request, so nothing was done.{lacking} Use exactly the fields of the tool's schema: action, and for run_shell the fields text (the command) and cwd.")
}

/// Finishes an entry from what the executor reported; `None` means the call was refused or
/// never ran. File contents that were read are not repeated here.
pub fn finished(mut entry: Value, executed: Option<&Value>) -> Value {
    entry["endedAt"] = json!(chrono::Utc::now().timestamp_millis());
    let Some(executed) = executed else {
        entry["status"] = json!("refused");
        if entry["malformed"] == true && entry["output"].is_null() { entry["output"] = json!("MiniClaw could not read this request, so nothing was done."); }
        return entry
    };
    entry["status"] = json!(match executed["status"].as_str() { Some("ok") => "ok", Some("failed") => "failed", _ => "refused" });
    match entry["kind"].as_str() {
        Some("command") => {
            let (stdout, stderr) = (executed["text"].as_str().unwrap_or_default(), executed["stderr"].as_str().unwrap_or_default());
            entry["output"] = json!(bounded(&if stderr.is_empty() { stdout.to_owned() } else { format!("{stdout}\n{stderr}") }));
            entry["exitCode"] = executed["exitCode"].clone();
            if executed["refusedHosts"].as_array().is_some_and(|hosts| !hosts.is_empty()) {
                entry["refusedHosts"] = executed["refusedHosts"].clone();
            }
        }
        Some("write" | "edit") => {
            if let Some(diff) = executed["diff"].as_str() { entry["diff"] = json!(diff); }
            if executed["status"] == "failed" { entry["output"] = executed["text"].clone(); }
        }
        Some("list" | "search") if executed["status"] == "ok" =>
            entry["output"] = json!(bounded(executed["text"].as_str().unwrap_or_default())),
        _ => {}
    }
    entry
}

impl Log {
    fn file(&self, thread: &str) -> Option<PathBuf> {
        (uuid::Uuid::parse_str(thread).ok()?.to_string() == thread)
            .then(|| self.dir.get().map(|dir| dir.join(format!("{thread}.json")))).flatten()
    }

    /// Adds an entry or replaces the one with the same id, then saves a finished one.
    pub fn record(&self, thread: &str, entry: &Value) {
        let mut threads = self.threads.lock().unwrap_or_else(|error| error.into_inner());
        if !threads.contains_key(thread) {
            if threads.len() >= 64 { threads.clear(); }
            threads.insert(thread.to_owned(), self.read(thread));
        }
        let entries = threads.get_mut(thread).expect("inserted above");
        match entries.iter_mut().find(|existing| existing["id"] == entry["id"]) {
            Some(existing) => *existing = entry.clone(),
            None => entries.push(entry.clone()),
        }
        if entries.len() > KEPT { entries.drain(..entries.len() - KEPT); }
        if entry["status"] == "running" { return; }
        let (Some(path), Some(dir)) = (self.file(thread), self.dir.get()) else { return };
        if crate::platform_fs::private_dir(dir, true).is_err() { return; }
        let staged = dir.join(format!(".{thread}.{}", uuid::Uuid::new_v4()));
        let saved = crate::platform_fs::private_file(&staged, true)
            .and_then(|mut output| { output.write_all(json!(entries).to_string().as_bytes())?; output.sync_all() })
            .and_then(|()| fs::rename(&staged, path));
        if saved.is_err() { let _ = fs::remove_file(staged); }
    }

    fn read(&self, thread: &str) -> Vec<Value> {
        self.file(thread).and_then(|path| fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str::<Vec<Value>>(&text).ok()).unwrap_or_default()
    }

    /// Finishes an entry again from a later result, for a command that outlived the model's wait
    /// in the shared terminal. Returns the entry to publish, or nothing when the entry is gone.
    pub fn finish_late(&self, thread: &str, id: &str, executed: &Value) -> Option<Value> {
        let entry = self.list(thread).into_iter().find(|entry| entry["id"] == id)?;
        let done = finished(entry, Some(executed));
        self.record(thread, &done);
        Some(done)
    }

    /// Lists a conversation's entries. One still `running` after a restart can never finish.
    pub fn list(&self, thread: &str) -> Vec<Value> {
        let threads = self.threads.lock().unwrap_or_else(|error| error.into_inner());
        threads.get(thread).cloned().unwrap_or_else(|| self.read(thread))
    }

    /// Returns a conversation's newest entry without copying the rest of its log.
    pub fn last(&self, thread: &str) -> Option<Value> {
        let threads = self.threads.lock().unwrap_or_else(|error| error.into_inner());
        match threads.get(thread) { Some(entries) => entries.last().cloned(), None => self.read(thread).pop() }
    }

    /// Deletes the log of a removed conversation.
    pub fn remove(&self, thread: &str) {
        self.threads.lock().unwrap_or_else(|error| error.into_inner()).remove(thread);
        if let Some(path) = self.file(thread) { let _ = fs::remove_file(path); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A command the model sent without its folder reads as that command, not as raw JSON, and
    /// the reason names what was missing.
    #[test]
    fn a_call_missing_a_field_reads_as_what_it_asked_for() {
        let arguments = json!({ "action": "run_shell", "text": "pwd; echo hi > hi.txt" });
        let entry = malformed("call", "turn", &arguments);
        assert_eq!((entry["kind"].as_str(), entry["title"].as_str()), (Some("command"), Some("pwd; echo hi > hi.txt")));
        assert!(malformed_reason(&arguments).contains("It had no cwd"));
        let read = malformed("call", "turn", &json!({ "action": "read_file", "path": "C:\\x.txt", "extra": 1 }));
        assert_eq!((read["kind"].as_str(), read["title"].as_str()), (Some("read"), Some("C:\\x.txt")));
        // Nothing recognizable: shown as sent, so the user still sees what was attempted.
        let odd = malformed("call", "turn", &json!({ "what": 1 }));
        assert_eq!((odd["kind"].as_str(), odd["title"].as_str()), (Some("other"), Some("{\"what\":1}")));
    }
}
