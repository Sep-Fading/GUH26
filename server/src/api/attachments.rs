//! Uploads of a conversation: the route that receives a file, the one that shows it again, and
//! the one that takes back an upload no message carries yet. The send route ties uploads to a
//! message and hands each provider what it can read.

// Reuses the shared request checks.
use super::*;
// The store, its records and its limits.
use crate::attachments::{self, Issue, Kind, Record};
// Reads an upload as it arrives.
use futures_util::StreamExt;
// Writes the arriving pieces to disk.
use tokio::io::AsyncWriteExt;

#[derive(Deserialize)]
pub(super) struct Named { name: String }

/// Turns a store refusal into the answer the page shows.
fn refuse(issue: Issue) -> Error {
    match issue {
        Issue::Name => Error(StatusCode::BAD_REQUEST, "attachment_name", "That file name cannot be used.", false),
        Issue::Empty => Error(StatusCode::BAD_REQUEST, "attachment_empty", "That file is empty.", false),
        Issue::TooLarge => Error(StatusCode::PAYLOAD_TOO_LARGE, "attachment_too_large",
            "Pictures can be up to 20 MB and other files up to 500 MB.", false),
        Issue::Missing | Issue::Count => invalid(),
        Issue::Sent => Error(StatusCode::CONFLICT, "attachment_sent", "That file already belongs to a message.", false),
        Issue::Storage => unavailable(),
    }
}

/// Receives one file into the conversation's store and returns what MiniClaw made of it. The
/// body is written to disk as it arrives and the upload stops at the size limit.
pub(super) async fn upload(State(backend): State<Shared>, Path(id): Path<String>, Query(named): Query<Named>,
    request: Request) -> Result<(StatusCode, Json<Value>)> {
    load(backend.clone(), id.clone()).await?;
    authorized_thread(&backend, &id).await?;
    if !attachments::valid_name(&named.name) { return Err(refuse(Issue::Name)); }
    let received = backend.attachments.incoming(&id).map_err(refuse)?;
    let limit = attachments::file_bytes();
    let written = async {
        let mut file = tokio::fs::File::from_std(crate::platform_fs::private_file(&received, true)
            .map_err(|_| Issue::Storage)?);
        let mut pieces = request.into_body().into_data_stream();
        let mut size = 0u64;
        while let Some(piece) = pieces.next().await {
            let piece = piece.map_err(|_| Issue::Storage)?;
            size += piece.len() as u64;
            if size > limit { return Err(Issue::TooLarge); }
            file.write_all(&piece).await.map_err(|_| Issue::Storage)?;
        }
        file.flush().await.map_err(|_| Issue::Storage)?;
        Ok(size)
    }.await;
    let size = match written {
        Ok(size) => size,
        Err(issue) => { let _ = tokio::fs::remove_file(&received).await; return Err(refuse(issue)); }
    };
    let (store, thread) = (backend.clone(), id.clone());
    let record = tokio::task::spawn_blocking(move || store.attachments.accept(&thread, &named.name, &received, size))
        .await.map_err(|_| unavailable())?.map_err(refuse)?;
    Ok((StatusCode::CREATED, Json(json!({ "attachment": record.view() }))))
}

/// Returns an upload's bytes. Only a picture is served as what it is; every other file is a
/// download, so the page never renders or runs something the user uploaded.
pub(super) async fn content(State(backend): State<Shared>, Path((id, attachment)): Path<(String, Uuid)>) -> Result<Response> {
    authorized_thread(&backend, &id).await?;
    let record = backend.attachments.get(&id, attachment).map_err(|_| missing())?;
    let file = tokio::fs::File::open(backend.attachments.content(&id, attachment).map_err(|_| missing())?).await.map_err(|_| missing())?;
    let body = HttpBody::from_stream(tokio_util::io::ReaderStream::new(file));
    let mut response = Response::builder().header("content-length", record.size);
    response = match record.kind {
        Kind::Image => response.header("content-type", record.media_type.as_str()),
        _ => response.header("content-type", "application/octet-stream").header("content-disposition", "attachment"),
    };
    response.body(body).map_err(|_| unavailable())
}

/// Removes an upload that was never sent.
pub(super) async fn discard(State(backend): State<Shared>, Path((id, attachment)): Path<(String, Uuid)>,
    input: Body<Activate>) -> Result<Json<Value>> {
    body(input)?;
    authorized_thread(&backend, &id).await?;
    backend.attachments.discard(&id, attachment).map_err(|issue| match issue { Issue::Missing => missing(), other => refuse(other) })?;
    Ok(Json(json!({})))
}

/// Checks the uploads a message names and returns their records.
pub(super) fn named(backend: &Backend, thread: &str, ids: &[Uuid], message: Uuid) -> Result<Vec<Record>> {
    backend.attachments.check(thread, ids, message).map_err(|issue| match issue { Issue::Missing => invalid(), other => refuse(other) })
}

/// Copies each upload into the working folder when the conversation has one on this Mac, and
/// returns the provider's picture inputs and the note for the model. A copy is recorded as soon
/// as it is made, so a send that has to be repeated never makes a second one.
pub(super) async fn deliver(backend: &Shared, thread: &str, records: &mut [Record]) -> Result<(Vec<Value>, Option<String>)> {
    if records.is_empty() { return Ok((Vec::new(), None)); }
    let root = working_folder(backend, thread).await;
    // A VM's worker accepts words only, so a picture cannot travel there yet.
    let on_mac = matches!(backend.agents.assignment(thread.to_owned()).await, Ok(Some(Assignment::Host {})));
    let mut pictures = Vec::new();
    for record in records.iter_mut() {
        let content = backend.attachments.content(thread, record.id).map_err(refuse)?;
        if record.saved.is_none() && let Some(root) = root.clone() {
            let (record_copy, content_copy) = (record.clone(), content.clone());
            record.saved = tokio::task::spawn_blocking(move || attachments::place(std::path::Path::new(&root), &record_copy, &content_copy))
                .await.ok().flatten();
            if record.saved.is_some() { backend.attachments.put(thread, record).map_err(refuse)?; }
        }
        if record.kind == Kind::Image && on_mac {
            let path = attachments::for_model(&content, record).await;
            pictures.push(json!({ "type": "localImage", "path": path }));
        }
    }
    Ok((pictures, attachments::note(&backend.attachments, thread, records, on_mac)))
}

/// Returns the working folder of a project conversation on this Mac, where the agent's file
/// tool can read. A plain conversation and a VM conversation have none here.
async fn working_folder(backend: &Backend, thread: &str) -> Option<String> {
    let assignment = backend.agents.assignment(thread.to_owned()).await.ok()??;
    if !matches!(assignment, Assignment::Host {}) { return None; }
    let project = backend.agents.project_context(thread.to_owned(), None, false).await.ok()??;
    project.project.definition.workspace
}
