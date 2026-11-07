//! Routes of the message queue, and the step that sends the next queued message. A queued
//! message leaves through the same `send` the page uses, with the id it was queued under, so
//! everything that guards a send guards it too, and a repeated attempt starts no second turn.
//! It is removed from the queue only after that send was accepted.

// Reuses sending, interrupting and the shared request checks.
use super::*;
// The store's view of one conversation's queue.
use crate::queue::View;
// Remembers which conversations were interrupted in order to send a queued message now.
use std::{collections::HashSet, sync::{Mutex as SyncMutex, OnceLock as SyncOnceLock}};

/// Conversations whose running response was interrupted by "send now". Their interrupted turn
/// must not pause the queue, since the user asked for the next message to go at once.
fn sending_now() -> &'static SyncMutex<HashSet<String>> {
    static SET: SyncOnceLock<SyncMutex<HashSet<String>>> = SyncOnceLock::new();
    SET.get_or_init(Default::default)
}

/// One drain at a time, so two ended turns or a resume beside one cannot send the same message twice.
fn gate() -> &'static tokio::sync::Mutex<()> {
    static GATE: SyncOnceLock<tokio::sync::Mutex<()>> = SyncOnceLock::new();
    GATE.get_or_init(Default::default)
}

/// Says the queue is full.
fn full() -> Error {
    Error(StatusCode::CONFLICT, "queue_full", "This conversation's queue holds 20 messages. Remove one or wait.", false)
}

/// Maps the store's failures to this feature's words.
fn queue_error(issue: Issue) -> Error {
    match issue {
        Issue::Limit => full(),
        Issue::Conflict => Error(StatusCode::CONFLICT, "queue_conflict", "This message id was queued with other words.", false),
        other => other.into(),
    }
}

/// Checks a queued message's words with the rules of a direct send.
fn checked(text: &str, uploads: usize) -> Result<()> {
    if text.trim().is_empty() && uploads == 0 { return Err(invalid()); }
    if text.len() > 32768 {
        return Err(Error(StatusCode::PAYLOAD_TOO_LARGE, "message_too_long", "Messages must be at most 32 KiB.", false));
    }
    Ok(())
}

/// Sends the queue's current state to the conversation's open pages.
async fn publish(backend: &Shared, thread: &str) -> View {
    let view = backend.agents.queue(thread.to_owned()).await.unwrap_or_default();
    if let Some(chat) = backend.data.lock().await.chats.get_mut(thread) { chat.emit("queue", json!(view)); }
    backend.changed();
    view
}

/// Registers what happens when a turn ends: the next message leaves, or the queue pauses.
pub(super) fn install(backend: &Shared) {
    let _ = backend.turn_ended.set(Box::new(|backend, thread, status, stopping| {
        tokio::spawn(async move {
            let asked = sending_now().lock().is_ok_and(|mut set| set.remove(&thread));
            let reason = match status.as_str() {
                _ if asked => None,
                "completed" if !stopping => None,
                "failed" => Some("failed"),
                "interrupted" | "completed" => Some("stopped"),
                _ => Some("unknown"),
            };
            if let Some(reason) = reason {
                let _ = backend.agents.pause_queue(thread.clone(), Some(reason)).await;
                publish(&backend, &thread).await;
            } else {
                drain(&backend, &thread).await;
            }
        });
    }));
}

/// Sends the first queued message if the conversation is idle and its queue is not paused.
pub(super) async fn drain(backend: &Shared, thread: &str) {
    let _one = gate().lock().await;
    let Ok(view) = backend.agents.queue(thread.to_owned()).await else { return };
    // With nothing queued, finished background results carry the parent on; a paused queue or a
    // stopped response means the user stopped things, so the results wait for them.
    if view.items.is_empty() && !view.paused { super::delegation::carry_on(backend, thread).await; return; }
    let Some(next) = view.items.first().filter(|_| !view.paused).cloned() else { return };
    {
        let data = backend.data.lock().await;
        let Some(chat) = data.chats.get(thread) else { return };
        // `stopping` outlives an interrupted turn until the next send, so only a running or
        // starting turn holds the queue back.
        if chat.active.is_some() || chat.sending { return; }
        if chat.uncertain {
            drop(data);
            let _ = backend.agents.pause_queue(thread.to_owned(), Some("unknown")).await;
            publish(backend, thread).await;
            return;
        }
    }
    let sent = send(State(backend.clone()), Path(thread.to_owned()), Ok(Json(Send { text: next.text.clone(),
        client_message_id: next.client_message_id, model: next.model.clone(), effort: next.effort.clone(), speed: next.speed.clone(),
        attachments: uploads(&next) }))).await;
    match sent {
        Ok(_) => { let _ = backend.agents.remove_queued(thread.to_owned(), Some(next.id)).await; }
        // Another send got there first; the message stays queued for the turn after it.
        Err(error) if error.1 == "busy" => return,
        // A refused send would be refused again, so the queue waits for the user.
        Err(_) => { let _ = backend.agents.pause_queue(thread.to_owned(), Some("refused")).await; }
    }
    publish(backend, thread).await;
}

/// Names the uploads a queued message carries.
fn uploads(item: &crate::queue::Item) -> Vec<Uuid> {
    item.attachments.iter().filter_map(|upload| upload["id"].as_str().and_then(|id| Uuid::parse_str(id).ok())).collect()
}

/// Removes the uploads of queued messages the user took back; one that was sent stays.
async fn forget(backend: &Backend, thread: &str, only: Option<&str>) {
    let Ok(view) = backend.agents.queue(thread.to_owned()).await else { return; };
    for item in view.items.iter().filter(|item| only.is_none_or(|id| item.id == id)) {
        for upload in uploads(item) { let _ = backend.attachments.discard(thread, upload); }
    }
}

/// Returns one conversation's queue.
pub(super) async fn list(State(backend): State<Shared>, Path(id): Path<String>) -> Result<Json<Value>> {
    authorized_thread(&backend, &id).await?;
    known_conversation(&backend, &id).await?;
    Ok(Json(json!(backend.agents.queue(id).await?)))
}

/// Queues one message. In an idle conversation with a running queue it is sent at once.
pub(super) async fn add(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Send>)
    -> Result<(StatusCode, Json<Value>)> {
    let input = body(input)?;
    authorized_thread(&backend, &id).await?;
    known_conversation(&backend, &id).await?;
    checked(&input.text, input.attachments.len())?;
    let uploads = super::attachments::named(&backend, &id, &input.attachments, input.client_message_id)?
        .iter().map(|record| record.view()).collect();
    let item = backend.agents.enqueue(id.clone(), input.text, input.client_message_id, input.model, input.effort, input.speed, uploads)
        .await.map_err(queue_error)?;
    publish(&backend, &id).await;
    let (backend, thread) = (backend.clone(), id);
    tokio::spawn(async move { drain(&backend, &thread).await; });
    Ok((StatusCode::CREATED, Json(json!({ "item": item }))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Words { text: String }

/// Changes the words of a message that has not left the queue yet.
pub(super) async fn edit(State(backend): State<Shared>, Path((id, item)): Path<(String, String)>, input: Body<Words>)
    -> Result<Json<Value>> {
    let words = body(input)?;
    authorized_thread(&backend, &id).await?;
    if words.text.trim().is_empty() || words.text.len() > 32768 { return Err(invalid()); }
    let item = backend.agents.edit_queued(id.clone(), item, words.text).await.map_err(queue_error)?;
    publish(&backend, &id).await;
    Ok(Json(json!({ "item": item })))
}

/// Removes one queued message.
pub(super) async fn remove(State(backend): State<Shared>, Path((id, item)): Path<(String, String)>, input: Body<Value>)
    -> Result<Json<Value>> {
    let _ = input.map_err(|_| invalid())?;
    authorized_thread(&backend, &id).await?;
    forget(&backend, &id, Some(&item)).await;
    backend.agents.remove_queued(id.clone(), Some(item)).await.map_err(queue_error)?;
    Ok(Json(json!(publish(&backend, &id).await)))
}

/// Empties a conversation's queue.
pub(super) async fn clear(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Value>) -> Result<Json<Value>> {
    let _ = input.map_err(|_| invalid())?;
    authorized_thread(&backend, &id).await?;
    forget(&backend, &id, None).await;
    backend.agents.remove_queued(id.clone(), None).await.map_err(queue_error)?;
    Ok(Json(json!(publish(&backend, &id).await)))
}

/// Pauses the queue by the user's own choice.
pub(super) async fn pause(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Value>) -> Result<Json<Value>> {
    let _ = input.map_err(|_| invalid())?;
    authorized_thread(&backend, &id).await?;
    backend.agents.pause_queue(id.clone(), Some("user")).await?;
    Ok(Json(json!(publish(&backend, &id).await)))
}

/// Resumes the queue and sends its first message if the conversation is idle.
pub(super) async fn resume(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Value>) -> Result<Json<Value>> {
    let _ = input.map_err(|_| invalid())?;
    authorized_thread(&backend, &id).await?;
    known_conversation(&backend, &id).await?;
    backend.agents.pause_queue(id.clone(), None).await?;
    let view = publish(&backend, &id).await;
    let (backend, thread) = (backend.clone(), id);
    tokio::spawn(async move { drain(&backend, &thread).await; });
    Ok(Json(json!(view)))
}

/// Makes one queued message the next to leave, interrupting the running response if there is
/// one. The message is sent when that turn has really ended, never beside it.
pub(super) async fn send_now(State(backend): State<Shared>, Path((id, item)): Path<(String, String)>, input: Body<Value>)
    -> Result<(StatusCode, Json<Value>)> {
    let _ = input.map_err(|_| invalid())?;
    authorized_thread(&backend, &id).await?;
    known_conversation(&backend, &id).await?;
    backend.agents.queue_first(id.clone(), item).await.map_err(queue_error)?;
    backend.agents.pause_queue(id.clone(), None).await?;
    publish(&backend, &id).await;
    let active = backend.data.lock().await.chats.get(&id).and_then(|chat| chat.active.clone());
    match active {
        Some(turn) => {
            if let Ok(mut set) = sending_now().lock() { set.insert(id.clone()); }
            if let Err(error) = interrupt(State(backend.clone()), Path(id.clone()), Ok(Json(Interrupt { turn_id: turn }))).await {
                if let Ok(mut set) = sending_now().lock() { set.remove(&id); }
                return Err(error);
            }
        }
        None => {
            let (backend, thread) = (backend.clone(), id);
            tokio::spawn(async move { drain(&backend, &thread).await; });
        }
    }
    Ok((StatusCode::ACCEPTED, Json(json!({}))))
}
