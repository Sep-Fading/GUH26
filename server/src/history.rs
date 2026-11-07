// Tracks duplicate items and completed turns.
use std::collections::{HashSet, VecDeque};

// Reads saved history and builds public summaries.
use serde_json::{Value, json};
// Checks saved task identifiers before loading.
use uuid::Uuid;

// Fetches Codex history and reports transport failures.
use crate::rpc::{Client, Failure};
// Reads several turns' items at once.
use futures_util::future::try_join_all;

pub struct Page {
    pub messages: Vec<Value>,
    pub active: Option<String>,
    pub finished: VecDeque<String>,
    pub cursor: Option<String>,
}

#[derive(Default)]
pub struct Loading {
    pub frames: Vec<Value>,
    pub overflow: bool,
    bytes: usize,
}

impl Loading {
    /// Buffers a notification during history loading; clears and marks overflow
    /// once the byte or frame limit is reached.
    pub fn push(&mut self, frame: &Value) {
        self.bytes += frame.to_string().len();
        if self.bytes > 2 * 1024 * 1024 || self.frames.len() >= 512 {
            self.overflow = true;
            self.frames.clear();
        } else if !self.overflow {
            self.frames.push(frame.clone());
        }
    }
}

/// Returns a public task summary only for a valid saved OpenAI task in this workspace.
pub fn summary(value: &Value, cwd: &str) -> Option<Value> {
    summary_for(value, cwd, "openai")
}

/// The spellings of this workspace a saved task may carry. Before 2026-10-04 the Windows
/// backend's folder was the verbatim `\\?\C:\...` form `canonicalize` answers; it is now the
/// plain drive form, so a task saved earlier still belongs to it. A macOS path has one spelling.
pub fn workspace_spellings(cwd: &str) -> Vec<String> {
    let plain = crate::platform_fs::plain_disk_form(cwd);
    if crate::platform_fs::plain_disk_form(&format!(r"\\?\{plain}")) == plain {
        vec![plain.to_owned(), format!(r"\\?\{plain}")]
    } else {
        vec![cwd.to_owned()]
    }
}

/// Reports whether a saved task's folder is this workspace in one of its spellings.
fn same_workspace(saved: &Value, cwd: &str) -> bool {
    saved.as_str().is_some_and(|saved| workspace_spellings(cwd).iter().any(|spelling| spelling == saved))
}

/// Returns a public summary for one provider's saved task; Claude tasks are labelled.
pub fn summary_for(value: &Value, cwd: &str, provider: &str) -> Option<Value> {
    let id = value["id"].as_str()?;
    if Uuid::parse_str(id).is_err()
        || !same_workspace(&value["cwd"], cwd)
        || value["modelProvider"] != provider
        || value["ephemeral"] == true
    {
        return None;
    }
    let name = value["name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| value["preview"].as_str().filter(|s| !s.is_empty()))
        .unwrap_or("New chat");
    let mut summary = json!({
        "id": id, "name": name.chars().take(200).collect::<String>(),
        "model": value["model"].as_str(), "effort": value["reasoningEffort"].as_str(),
        "createdAt": value["createdAt"].as_i64().and_then(|t| t.checked_mul(1000)),
        "updatedAt": value["updatedAt"].as_i64().and_then(|t| t.checked_mul(1000)),
        "computerUse": false,
    });
    if provider == crate::claude::PROVIDER { summary["provider"] = json!("claude"); }
    Some(summary)
}

/// Returns an optional bounded upstream cursor, rejecting malformed values.
pub fn cursor(value: &Value) -> Result<Option<String>, Failure> {
    match value {
        Value::Null => Ok(None),
        Value::String(value) if !value.is_empty() && value.len() <= 4096 => {
            Ok(Some(value.clone()))
        }
        _ => Err(Failure::Unavailable),
    }
}

/// Reads one saved-task page and returns workspace-filtered summaries and the next cursor.
pub async fn list(
    client: &Client,
    cwd: &str,
    after: Option<&str>,
) -> Result<Value, Failure> {
    list_for(client, cwd, after, "openai").await
}

/// The `thread/list` folder filter: the one spelling as before, or all of a Windows folder's.
fn workspace_filter(cwd: &str) -> Value {
    match workspace_spellings(cwd).as_slice() {
        [only] => json!(only),
        spellings => json!(spellings),
    }
}

/// Reads one saved-task page for a single provider, checking ownership locally too.
pub async fn list_for(
    client: &Client,
    cwd: &str,
    after: Option<&str>,
    provider: &str,
) -> Result<Value, Failure> {
    let result = client
        .call(
            "thread/list",
            json!({
                // Codex filters on any of several exact folders (App Server, `thread/list`).
                "cwd": workspace_filter(cwd), "modelProviders": [provider],
                // Installed Codex labels App Server tasks as vscode.
                "sourceKinds": ["appServer", "vscode"],
                "archived": false, "limit": 50, "cursor": after,
                "sortKey": "created_at", "sortDirection": "desc",
            }),
        )
        .await?;
    let data = result["data"].as_array().ok_or(Failure::Unavailable)?;
    if data.len() > 50 {
        return Err(Failure::Unavailable);
    }
    // Check ownership locally too; an upstream filter is not an access check.
    let threads: Vec<_> = data.iter()
        .filter_map(|item| summary_for(item, cwd, provider)).collect();
    Ok(json!({ "threads": threads, "nextCursor": cursor(&result["nextCursor"])? }))
}

/// Returns a bounded history page in conversation order with text-only messages,
/// turn status, and the next cursor. Malformed or duplicate message data fails the read.
/// Items asked for per `thread/items/list` call. A saved tool result can hold a few megabytes
/// (a screenshot, a long command output), so a page of this many stays far below the
/// transport's frame limit however large the conversation grew.
const ITEMS_PER_CALL: usize = 6;

/// The most items one turn may hold before its history is refused as unreadable.
const MOST_ITEMS: usize = 5000;

/// Turns whose items are read at the same time. A long conversation has a hundred or more
/// small pages to read; a few in flight at once keep the wait short without flooding Codex.
const TURNS_AT_ONCE: usize = 4;

/// Reads one turn's items in small pages, oldest first. Codex sends each item inside an entry
/// (`{turnId, item, ...}`); a bare item is read as given.
async fn items(client: &Client, thread: &str, turn: &str) -> Result<Vec<Value>, Failure> {
    let mut items = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let result = client
            .call(
                "thread/items/list",
                json!({
                    "threadId": thread, "turnId": turn, "limit": ITEMS_PER_CALL,
                    "cursor": cursor, "sortDirection": "asc",
                }),
            )
            .await?;
        let data = result["data"].as_array().ok_or(Failure::Unavailable)?;
        if data.len() > ITEMS_PER_CALL || items.len() + data.len() > MOST_ITEMS {
            return Err(Failure::Unavailable);
        }
        for entry in data {
            if entry["turnId"].as_str().is_some_and(|owner| owner != turn) {
                return Err(Failure::Unavailable);
            }
            items.push(if entry.get("item").is_some() { entry["item"].clone() } else { entry.clone() });
        }
        // An opaque continuation is passed back as given; an empty page ends the read.
        cursor = match &result["nextCursor"] {
            Value::Null => break,
            value @ (Value::String(_) | Value::Object(_)) if value.to_string().len() <= 4096 => value.clone(),
            _ => return Err(Failure::Unavailable),
        };
        if data.is_empty() { break; }
    }
    Ok(items)
}

/// Returns one turn's items. Codex answers `notLoaded` with no items, and they are read page
/// by page; a turn that arrives with its items (the Claude bridge, an older peer) is read as
/// given.
async fn turn_items(client: &Client, thread: &str, turn: &Value, id: &str)
    -> Result<Vec<Value>, Failure>
{
    match turn["itemsView"].as_str() {
        Some("notLoaded" | "summary") => items(client, thread, id).await,
        _ => turn["items"].as_array().cloned().ok_or(Failure::Unavailable),
    }
}

pub async fn page(
    client: &Client,
    thread: &str,
    after: Option<&str>,
    attachments: &crate::attachments::Store,
) -> Result<Page, Failure> {
    // Turns come without their items. One turn's saved tool results can run to tens of
    // megabytes, more than one frame may carry; `items` reads them in small pages instead.
    let result = client
        .call(
            "thread/turns/list",
            json!({
                "threadId": thread, "limit": 50, "cursor": after,
                "sortDirection": "desc", "itemsView": "notLoaded",
            }),
        )
        .await?;
    let turns = result["data"].as_array().ok_or(Failure::Unavailable)?;
    if turns.len() > 50 {
        return Err(Failure::Unavailable);
    }
    let mut page = Page {
        messages: Vec::new(),
        active: None,
        finished: VecDeque::new(),
        cursor: cursor(&result["nextCursor"])?,
    };
    let mut ids = HashSet::new();
    let mut bytes = 0;
    // Codex pages newest first; the conversation reads oldest first.
    let mut ordered = Vec::with_capacity(turns.len());
    for turn in turns.iter().rev() {
        let id = turn["id"].as_str().ok_or(Failure::Unavailable)?;
        let status = turn["status"].as_str().ok_or(Failure::Unavailable)?;
        if !matches!(
            status,
            "inProgress" | "completed" | "interrupted" | "failed"
        ) {
            return Err(Failure::Unavailable);
        }
        ordered.push((turn, id, status));
    }
    // A few turns' items are read at a time; each batch keeps the turns' order.
    let mut fetched = Vec::with_capacity(ordered.len());
    for batch in ordered.chunks(TURNS_AT_ONCE) {
        fetched.extend(try_join_all(batch.iter()
            .map(|(turn, id, _)| turn_items(client, thread, turn, id))).await?);
    }
    for ((_, id, status), items) in ordered.iter().zip(fetched) {
        let (id, status) = (*id, *status);
        let complete = status != "inProgress";
        // Each message names its turn, so the page can place that turn's activity beside it.
        let turn_id = id;
        if complete {
            page.finished.push_back(id.into());
        } else if page.active.replace(id.into()).is_some() {
            return Err(Failure::Unavailable);
        }
        for item in &items {
            if item["type"] == "imageGeneration" {
                let image_id = item["id"].as_str().ok_or(Failure::Unavailable)?;
                if image_id.is_empty() || !ids.insert(image_id.to_owned()) { return Err(Failure::Unavailable); }
                let status = item["status"].as_str().filter(|status| !status.is_empty()).unwrap_or("generating");
                let text = if item["failure"]["type"] == "usageLimitExceeded" { "Image generation limit reached." }
                    else if status == "failed" { "Image generation failed." } else { "" };
                let mut message = json!({ "id": image_id, "role": "assistant", "text": text,
                    "turnId": turn_id, "complete": status != "generating", "imageStatus": status });
                if status == "completed" {
                    if let Some(record) = item["result"].as_str().and_then(|result|
                        attachments.generated(thread, image_id, result).ok()) {
                            message["attachments"] = json!([record.view()]);
                    } else {
                        message["imageStatus"] = json!("failed");
                        message["text"] = json!("Could not save generated image.");
                    }
                }
                page.messages.push(message);
                continue;
            }
            let (role, text) = match item["type"].as_str() {
                Some("agentMessage") => (
                    "assistant",
                    item["text"]
                        .as_str()
                        .ok_or(Failure::Unavailable)?
                        .to_owned(),
                ),
                Some("userMessage") => {
                    let mut parts = Vec::new();
                    for part in item["content"].as_array().ok_or(Failure::Unavailable)? {
                        if part["type"] == "text" {
                            parts
                                .push(part["text"].as_str().ok_or(Failure::Unavailable)?);
                        }
                    }
                    ("user", parts.join("\n"))
                }
                _ => continue,
            };
            let id = item["id"].as_str().ok_or(Failure::Unavailable)?;
            if id.is_empty() || !ids.insert(id.to_owned()) {
                return Err(Failure::Unavailable);
            }
            bytes += text.len();
            if bytes > 2 * 1024 * 1024 || page.messages.len() >= 10000 {
                return Err(Failure::Unavailable);
            }
            page.messages.push(json!({
                "id": id, "role": role, "text": text, "turnId": turn_id,
                "complete": role == "user" || complete,
            }));
        }
    }
    Ok(page)
}

#[cfg(test)]
mod workspace_tests {
    use super::*;

    fn saved(cwd: &str) -> Value {
        json!({ "id": "019a1111-1111-7111-8111-111111111111", "cwd": cwd, "modelProvider": "openai", "name": "Saved" })
    }

    /// A Windows task saved under the old verbatim folder still belongs to the plain one, and the
    /// reverse; a different folder, another drive or a macOS look-alike does not.
    #[test]
    fn a_windows_task_matches_either_spelling_of_its_folder() {
        let plain = r"C:\Users\S\AppData\Local\MiniClaw\chat-workspace";
        let verbatim = r"\\?\C:\Users\S\AppData\Local\MiniClaw\chat-workspace";
        assert!(summary(&saved(verbatim), plain).is_some());
        assert!(summary(&saved(plain), plain).is_some());
        assert!(summary(&saved(plain), verbatim).is_some());
        assert!(summary(&saved(r"C:\Users\S\AppData\Local\MiniClaw\other"), plain).is_none());
        assert!(summary(&saved(r"\\?\D:\Users\S\AppData\Local\MiniClaw\chat-workspace"), plain).is_none());
        assert!(summary(&saved(r"\\?\UNC\C:\Users\S\AppData\Local\MiniClaw\chat-workspace"), plain).is_none());
        assert!(summary(&saved(r"c:\users\s\appdata\local\miniclaw\chat-workspace"), plain).is_none());
    }

    /// The upstream filter names both Windows spellings; a macOS folder is sent exactly as before.
    #[test]
    fn the_list_filter_names_every_spelling() {
        let plain = r"C:\Users\S\chat";
        assert_eq!(workspace_filter(plain), json!([plain, r"\\?\C:\Users\S\chat"]));
        assert_eq!(workspace_filter("/Users/sepehr/chat"), json!("/Users/sepehr/chat"));
        assert!(summary(&saved("/Users/sepehr/chat"), "/Users/sepehr/chat").is_some());
        assert!(summary(&saved("/Users/sepehr/chat/"), "/Users/sepehr/chat").is_none());
    }
}
