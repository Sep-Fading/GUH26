//! The tool a project conversation uses for what the project shares: its memory now, its earlier
//! conversations later. It is one tool with actions, so a conversation carries one schema, and
//! every result is bounded. It grants nothing: it reads and writes MiniClaw's own notes only.

// The backend, its store and the running conversations.
use crate::chat::Backend;
// Notes, their limits and the refusals of a save.
use crate::project_memory::{self, Draft, Refusal};
// Shapes the tool, its results and Activity entries.
use serde_json::{Value, json};
// Shares the backend with the spawned call.
use std::sync::Arc;
// Hashes the index a conversation was told.
use std::hash::{Hash, Hasher};

pub const TOOL: &str = "miniclaw_project";

/// Describes the tool to a provider. `type` is required by the real Codex.
pub fn tool_definition() -> Value {
    json!({ "type": "function", "name": TOOL,
        "description": "Reads and writes what this project's conversations share. Memory is a set of short notes every conversation of the project can read; you are told their names and descriptions once, and read a note's text with memory_read only when it matters for the task. Save a note with memory_save when you learn something a later conversation will need and could not find out quickly by itself: a decision and its reason, a preference of the user, a convention, where something lives, a pitfall. One subject per note, a name of a few words, a description of one sentence that says when the note is worth reading, and text of a few lines. Saving under an existing name replaces that note, so update a note instead of adding a near-duplicate, and delete one that turned out wrong. Never save secrets, things the project's files already say, or what only matters to this conversation. Earlier conversations: when you finish a piece of work a later conversation may build on, call summary_save once with what was done, what was decided and what is left, in at most 600 characters; not after every message. conversations_list names the project's other conversations, conversations_search finds the messages that hold all of your words, in any order, and returns short passages, and conversation_read returns one 8 KB page of one. Look one up only when the task builds on it. Issues are the project's known bugs and features. A bug you will not fix now: issue_list, then issue_create. issue_claim before working (moves it to dev); one agent owns an issue, and only it may issue_stage it (dev, qa, agent_review), issue_done it (a summary; closes it or leaves it for the user's review by severity) or issue_release it (a handover: found, done, left). issue_severity only raises; issue_comment adds to the history issue_read shows. A swarm's Lead also has issue_assign (a story to an agent, by name) and issue_review_level, used when the user asks.",
        "inputSchema": { "type": "object", "additionalProperties": false, "required": ["action"], "properties": {
            "action": { "type": "string", "enum": ["memory_read", "memory_save", "memory_delete", "summary_save", "conversations_list", "conversations_search", "conversation_read",
                "issue_list", "issue_read", "issue_create", "issue_comment", "issue_claim", "issue_release", "issue_done", "issue_severity", "issue_stage",
                "issue_assign", "issue_review_level"] },
            "agent": { "type": "string", "description": "issue_assign: the agent, e.g. Dev-2." },
            "level": { "type": "string", "enum": ["none", "low", "medium", "high"], "description": "issue_review_level: highest severity agents close alone." },
            "stage": { "type": "string", "enum": ["dev", "qa", "agent_review"] },
            "issue": { "type": "integer", "description": "The issue's number." },
            "kind": { "type": "string", "enum": ["bug", "feature"] },
            "title": { "type": "string", "description": "issue_create: one line, at most 120 characters." },
            "severity": { "type": "string", "enum": ["low", "medium", "high"] },
            "tags": { "type": "array", "maxItems": 8, "items": { "type": "string" } },
            "status": { "type": "string", "enum": ["backlog", "dev", "qa", "agent_review", "review", "closed"], "description": "issue_list: only this stage; review is the user's." },
            "name": { "type": "string", "description": "The note's name, at most 80 characters." },
            "description": { "type": "string", "description": "memory_save: one sentence, at most 200 characters, saying when the note is worth reading." },
            "text": { "type": "string", "description": "memory_save: the note itself, at most 8 KB. summary_save: the summary, at most 600 characters." },
            "query": { "type": "string", "description": "conversations_search: words to find, 3 to 200 characters. conversations_list: optional words to filter by." },
            "id": { "type": "string", "description": "conversation_read: the id of a conversation of this project." },
            "page": { "type": "integer", "minimum": 1, "description": "conversation_read: which 8 KB page; starts at 1." } } } })
}

/// An answer for the model: success or not, and words.
fn result(success: bool, text: &str) -> Value {
    json!({ "success": success, "contentItems": [{ "type": "inputText", "text": text }] })
}

/// Hashes an index, which is how a conversation's view of the memory is compared with the present one.
pub fn version(index: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    index.hash(&mut hasher);
    hasher.finish()
}

/// Reports whether project memory is switched off for this conversation. It is on unless the switch is set.
pub async fn switched_off(backend: &Backend, thread: &str) -> bool {
    use crate::permissions::{Decision, Target};
    let Ok(context) = backend.agents.permission_context(thread.to_owned()).await else { return false };
    matches!(backend.agents.permission(&context, &Target::Feature { name: "no_project_memory".into() }, chrono::Utc::now().timestamp()).await,
        Ok(Decision::Allow(_)))
}

/// Returns the project a conversation belongs to.
pub async fn project_of(backend: &Backend, thread: &str) -> Option<uuid::Uuid> {
    backend.agents.project_context(thread.to_owned(), None, false).await.ok().flatten().map(|context| context.project.id)
}

/// Returns what a conversation is told about its project with this message, or nothing when it
/// already knows: the index is sent with the first message and again only after someone else
/// changed the memory. An unchanged key costs nothing to repeat, but a changed one is added to
/// the conversation in full, so it changes as rarely as it can.
pub async fn note(backend: &Backend, thread: &str) -> Option<String> {
    let project = project_of(backend, thread).await?;
    if switched_off(backend, thread).await { return None; }
    let index = project_memory::index(&backend.agents.notes(project).await.ok()?);
    let now = version(&index);
    let first = {
        let mut seen = backend.project_seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if seen.get(thread) == Some(&now) { return None; }
        let first = !seen.contains_key(thread);
        seen.insert(thread.to_owned(), now);
        first
    };
    let memory = if !index.is_empty() {
        Some(format!("## Project memory\nNotes shared by every conversation of this project, as name: description. Read one with miniclaw_project, action memory_read, when it matters for the task.\n{index}"))
    } else if first { None } else {
        Some("## Project memory\nThe project's memory is empty now; the notes you were told of earlier were deleted.".to_owned())
    };
    // The earlier conversations are a snapshot for the first message; the tool gives the current list.
    let digest = if first { crate::project_history::digest(backend, project, thread).await } else { None };
    let parts: Vec<String> = [memory, digest].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

/// Keeps a conversation's own write from coming back to it: if it knew the memory as it was before
/// the write, it now knows it as it is after.
async fn advance(backend: &Backend, thread: &str, project: uuid::Uuid, before: u64) {
    let Ok(notes) = backend.agents.notes(project).await else { return; };
    let after = version(&project_memory::index(&notes));
    let mut seen = backend.project_seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if seen.get(thread) == Some(&before) { seen.insert(thread.to_owned(), after); }
}

/// Runs one call and answers the Activity status, the entry's title and output, and the result.
async fn run(backend: &Backend, thread: &str, arguments: &Value) -> (&'static str, String, String, Value) {
    let refused = |title: String, text: &str| ("refused", title, text.to_owned(), result(false, text));
    let action = arguments["action"].as_str().unwrap_or_default();
    let name = arguments["name"].as_str().unwrap_or_default().to_owned();
    let Some(project) = project_of(backend, thread).await else {
        return refused("Project memory".into(), "This conversation is not in a project, so it has no shared memory.");
    };
    // Issues are the project's record of work, so the memory switch leaves them on.
    if action.starts_with("issue_") { return crate::issue_tool::run(backend, thread, project, action, arguments).await; }
    if switched_off(backend, thread).await {
        return refused("Project memory".into(), "Project memory is switched off for this conversation. The user can switch it on from Tools beside the message box.");
    }
    match action {
        "memory_read" => match backend.agents.note(project, name.clone()).await {
            Ok(Some(note)) => ("ok", format!("Recalled: {}", note.name), note.text.clone(), result(true, &format!("{}\n{}\n\n{}", note.name, note.description, note.text))),
            Ok(None) => ("failed", format!("Recalled: {name}"), "No such note.".into(), result(false, "No note has that name. The names are in the project memory list you were given.")),
            Err(_) => ("failed", format!("Recalled: {name}"), String::new(), result(false, "MiniClaw could not read the project's memory.")),
        },
        "memory_save" => {
            let draft = Draft { name: name.clone(), description: arguments["description"].as_str().unwrap_or_default().to_owned(),
                text: arguments["text"].as_str().unwrap_or_default().to_owned() };
            let before = version(&project_memory::index(&backend.agents.notes(project).await.unwrap_or_default()));
            let shown = format!("{}\n\n{}", draft.description, draft.text);
            match backend.agents.save_note(project, draft, None, Some(thread.to_owned())).await {
                Ok(note) => {
                    advance(backend, thread, project, before).await;
                    ("ok", format!("Remembered: {}", note.name), shown, result(true, &format!("Saved the note \"{}\".", note.name)))
                }
                Err(refusal) => {
                    let why = match refusal {
                        Refusal::Shape => "A note needs a one-line name of at most 80 characters, a one-line description of at most 200, and text of at most 8 KB.",
                        Refusal::Secret => "That looks like it contains a secret, such as a password, a token or a key. Secrets are never saved to memory. Save the note without it.",
                        Refusal::Full => "The project already has 64 notes. Update or delete one instead of adding another.",
                        Refusal::Missing | Refusal::Storage => "MiniClaw could not save the note.",
                    };
                    ("failed", format!("Remembered: {name}"), why.to_owned(), result(false, why))
                }
            }
        }
        "memory_delete" => {
            let before = version(&project_memory::index(&backend.agents.notes(project).await.unwrap_or_default()));
            match backend.agents.remove_note(project, name.clone()).await {
                Ok(true) => { advance(backend, thread, project, before).await; ("ok", format!("Forgot: {name}"), String::new(), result(true, "Deleted the note.")) }
                Ok(false) => ("failed", format!("Forgot: {name}"), "No such note.".into(), result(false, "No note has that name.")),
                Err(_) => ("failed", format!("Forgot: {name}"), String::new(), result(false, "MiniClaw could not delete the note.")),
            }
        }
        "summary_save" => {
            let text = arguments["text"].as_str().unwrap_or_default().trim().to_owned();
            let title = "Saved this conversation's summary".to_owned();
            if text.is_empty() || text.chars().count() > crate::project_history::SUMMARY || text.chars().any(|c| c.is_control() && c != '\n') {
                return ("failed", title, String::new(), result(false, "A summary is 1 to 600 characters of plain text."));
            }
            if project_memory::looks_secret(&text) {
                return ("failed", title, String::new(), result(false, "That looks like it contains a secret. Save the summary without it."));
            }
            match backend.agents.save_summary(thread.to_owned(), text.replace('\n', " ")).await {
                Ok(()) => ("ok", title, text, result(true, "Saved. Later conversations of this project are told it once.")),
                Err(_) => ("failed", title, String::new(), result(false, "MiniClaw could not save the summary.")),
            }
        }
        "conversations_list" => {
            let listed = crate::project_history::list(backend, project, thread, arguments["query"].as_str().filter(|query| !query.trim().is_empty())).await;
            ("ok", "Listed the project's conversations".into(), listed.clone(), result(true, &listed))
        }
        "conversations_search" => {
            let query = arguments["query"].as_str().unwrap_or_default().trim();
            if query.chars().count() < 3 || query.chars().count() > 200 {
                return ("failed", "Searched conversations".into(), String::new(), result(false, "Search for 3 to 200 characters."));
            }
            let found = crate::project_history::search(backend, project, thread, query).await;
            ("ok", format!("Searched conversations for: {query}"), found.clone(), result(true, &found))
        }
        "conversation_read" => {
            let page = arguments["page"].as_u64().unwrap_or(1) as usize;
            match crate::project_history::read(backend, project, thread, arguments["id"].as_str().unwrap_or_default(), page).await {
                Ok((name, text)) => ("ok", format!("Read conversation: {name}"), format!("Page {page}"), result(true, &text)),
                Err(why) => ("failed", "Read conversation".into(), why.to_owned(), result(false, why)),
            }
        }
        _ => refused("Project memory".into(), "Unknown action."),
    }
}

/// Handles one call from a provider: runs it, shows it to the user in Activity, and answers.
pub async fn handle(backend: Arc<Backend>, request: crate::rpc::ServerRequest) {
    let params = request.params.clone();
    let thread = params["threadId"].as_str().unwrap_or_default().to_owned();
    let started = chrono::Utc::now().timestamp_millis();
    let (status, title, output, answer) = run(&backend, &thread, &params["arguments"]).await;
    if let (Some(id), Some(turn)) = (params["callId"].as_str(), params["turnId"].as_str()) {
        let agent = backend.data.lock().await.chats.get(&thread).and_then(|chat| chat.summary["agentName"].as_str().map(str::to_owned));
        // Issue calls get their own row kind; everything else reads as project memory.
        let kind = if params["arguments"]["action"].as_str().is_some_and(|action| action.starts_with("issue_")) { "issue" } else { "memory" };
        backend.publish_activity(&thread, &json!({ "id": id, "turnId": turn, "kind": kind, "title": title, "detail": null, "agent": agent,
            "status": status, "startedAt": started, "endedAt": chrono::Utc::now().timestamp_millis().max(started + 1), "output": output })).await;
    }
    let _ = request.answer.send(answer);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The schema travels with every request of a project conversation, so its size is a budget.
    /// Raised from 3200 to 3600 bytes on 2026-11-07, when the tool gained the issue actions, and to 4000
    /// the same day for a swarm Lead's issue_assign and issue_review_level.
    #[test]
    fn the_tool_stays_small() {
        let size = tool_definition().to_string().len();
        println!("miniclaw_project definition: {size} bytes");
        assert!(size <= 4000, "{size} bytes");
    }

    #[test]
    fn versions_differ_only_when_the_index_does() {
        assert_eq!(version("- A: b\n"), version("- A: b\n"));
        assert_ne!(version("- A: b\n"), version("- A: c\n"));
    }
}
