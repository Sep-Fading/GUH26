// Reuses ownership checks and provider history routing.
use super::*;
// Holds the index independently of chat eviction.
use crate::search::{Index, Status};
// Detects nonadvancing provider pagination without looping.
use std::collections::HashSet;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SearchQuery { #[serde(default)] q: String }

/// Starts indexing once per account generation, returning available matches and progress.
pub(super) async fn search(State(backend): State<Shared>, Query(query): Query<SearchQuery>) -> Result<Json<Value>> {
    if query.q.len() > 200 { return Err(invalid()); }
    let generation = authorized(&backend).await?;
    let claude = claude_account(&backend).await;
    let account = backend.data.lock().await.account.clone();
    let key = json!([generation, account["email"], account["status"],
        claude.as_ref().map(|a| (&a["email"], &a["status"]))]).to_string();
    let codex_allowed = account["status"] == "signed_in";
    let claude_allowed = claude.as_ref().is_some_and(|a| a["status"] == "signed_in");
    let mut status = backend.search.status.lock().await;
    if status.generation.as_ref() != Some(&key) {
        let index = tokio::task::spawn_blocking(Index::new).await.map_err(|_| unavailable())?
            .map_err(|_| unavailable())?;
        *status = Status { generation: Some(key.clone()), index: Some(index.clone()), indexing: true,
            ..Default::default() };
        let shared = backend.clone();
        let job = key.clone();
        tokio::spawn(async move {
            let result = build(shared.clone(), generation, &job, index).await;
            let mut status = shared.search.status.lock().await;
            if status.generation.as_ref() == Some(&job) {
                status.indexing = false;
                status.incomplete |= result.is_err();
            }
        });
    }
    let mut updates = Vec::new();
    {
        let data = backend.data.lock().await;
        for (id, chat) in &data.chats {
            if chat.active.is_some() || chat.sending { continue; }
            let cursor = chat.cursor();
            if status.live.get(id) == Some(&cursor) { continue; }
            updates.push((chat.summary.clone(), chat.messages.clone()));
            if !status.indexing { status.live.insert(id.clone(), cursor); }
        }
    }
    let index = status.index.clone().ok_or_else(unavailable)?;
    let (indexing, indexed, incomplete) = (status.indexing, status.indexed, status.incomplete);
    drop(status);
    let (matches, limited) = tokio::task::spawn_blocking(move || {
        for (summary, messages) in updates { index.put(&summary, &messages)?; }
        index.query(query.q.trim())
    }).await
        .map_err(|_| unavailable())?.map_err(|_| unavailable())?;
    let hidden = backend.agents.hidden_run_threads().await?;
    let mut results = Vec::new();
    for mut found in matches {
        let id = found["thread"]["id"].as_str().ok_or_else(unavailable)?.to_owned();
        if hidden.contains(&id) || backend.agents.chat_removed(id.clone()).await?
            || if backend.is_claude(&id) { !claude_allowed } else { !codex_allowed } { continue; }
        tag_context(&backend, &id, &mut found["thread"]).await?;
        results.push(found);
    }
    if backend.data.lock().await.generation != generation
        || backend.search.status.lock().await.generation.as_ref() != Some(&key) { return Err(unavailable()); }
    Ok(Json(json!({ "results": results, "indexing": indexing, "indexed": indexed,
        "incomplete": incomplete, "limited": limited })))
}

/// Rebuilds on request, including conversations created since the initial index.
pub(super) async fn refresh(State(backend): State<Shared>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    let mut status = backend.search.status.lock().await;
    if !status.indexing { status.generation = None; }
    drop(status);
    search(State(backend), Query(SearchQuery { q: String::new() })).await
}

/// Reads all owned conversation pages sequentially so indexing does not flood the provider.
async fn build(backend: Shared, generation: u64, key: &String, index: Index) -> Result<()> {
    let mut cursor = None;
    let mut cursors = HashSet::new();
    let mut threads = HashSet::new();
    loop {
        if backend.data.lock().await.generation != generation { return Err(unavailable()); }
        let page = tasks(State(backend.clone()), Query(HistoryQuery { cursor: cursor.clone() })).await?.0;
        for summary in page["threads"].as_array().ok_or_else(unavailable)? {
            let id = summary["id"].as_str().ok_or_else(unavailable)?;
            if !threads.insert(id.to_owned()) { continue; }
            let result = index_thread(&backend, generation, &index, summary).await;
            let mut status = backend.search.status.lock().await;
            if status.generation.as_ref() != Some(key) { return Err(unavailable()); }
            status.indexed += usize::from(result.is_ok());
            status.incomplete |= result.is_err();
        }
        cursor = history::cursor(&page["nextCursor"])?;
        match &cursor {
            None => break,
            Some(next) if !cursors.insert(next.clone()) => return Err(unavailable()),
            _ => {},
        }
    }
    Ok(())
}

/// Indexes full provider history without activating a chat or starting a model turn.
async fn index_thread(backend: &Shared, generation: u64, index: &Index, summary: &Value) -> Result<()> {
    let id = summary["id"].as_str().ok_or_else(unavailable)?;
    if summary["vmOffline"] == true { return Err(unavailable()); }
    authorized_thread(backend, id).await?;
    let assignment = backend.agents.assignment(id.to_owned()).await?;
    let (client, _) = thread_client(backend, id, assignment.as_ref())?;
    let mut cursor = None;
    let mut seen = HashSet::new();
    loop {
        let page = history::page(&client, id, cursor.as_deref(), &backend.attachments).await?;
        if backend.data.lock().await.generation != generation { return Err(unavailable()); }
        let (copy, summary) = (index.clone(), summary.clone());
        tokio::task::spawn_blocking(move || copy.put(&summary, &page.messages)).await
            .map_err(|_| unavailable())?.map_err(|_| unavailable())?;
        cursor = page.cursor;
        match &cursor {
            None => break,
            Some(next) if !seen.insert(next.clone()) => return Err(unavailable()),
            _ => {},
        }
    }
    Ok(())
}
