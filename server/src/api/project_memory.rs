//! The routes behind "Memory" in a project's settings: the user reads, adds, changes and deletes
//! the notes the project's conversations share. The same checks apply as to an agent's save,
//! including the refusal of anything that looks like a secret.

// Reuses the shared request checks.
use super::*;
// The notes and the reasons a save is refused.
use crate::project_memory::{Draft, Refusal};

/// Turns a refused save into the answer the page shows.
fn refuse(refusal: Refusal) -> Error {
    match refusal {
        Refusal::Shape => invalid(),
        Refusal::Secret => Error(StatusCode::BAD_REQUEST, "memory_secret",
            "That looks like it contains a password, token or key. Secrets are never saved to memory.", false),
        Refusal::Full => Error(StatusCode::CONFLICT, "memory_full", "A project holds at most 64 notes.", false),
        Refusal::Missing => missing(),
        Refusal::Storage => unavailable(),
    }
}

/// Confirms the project exists and returns its id.
async fn known(backend: &Backend, id: &str) -> Result<Uuid> {
    authorized(backend).await?;
    let id = Uuid::parse_str(id).map_err(|_| missing())?;
    backend.agents.projects().await?.into_iter().any(|project| project.id == id).then_some(id).ok_or_else(missing)
}

/// Lists a project's notes by name.
pub(super) async fn list(State(backend): State<Shared>, Path(id): Path<String>) -> Result<Json<Value>> {
    let project = known(&backend, &id).await?;
    Ok(Json(json!({ "notes": backend.agents.notes(project).await? })))
}

/// Adds a note, or replaces the one of the same name.
pub(super) async fn add(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Draft>) -> Result<(StatusCode, Json<Value>)> {
    let draft = body(input)?;
    let project = known(&backend, &id).await?;
    let note = backend.agents.save_note(project, draft, None, None).await.map_err(refuse)?;
    Ok((StatusCode::CREATED, Json(json!({ "note": note }))))
}

/// Changes one note, which may rename it.
pub(super) async fn edit(State(backend): State<Shared>, Path((id, name)): Path<(String, String)>, input: Body<Draft>) -> Result<Json<Value>> {
    let draft = body(input)?;
    let project = known(&backend, &id).await?;
    let note = backend.agents.save_note(project, draft, Some(name), None).await.map_err(refuse)?;
    Ok(Json(json!({ "note": note })))
}

/// Deletes one note.
pub(super) async fn remove(State(backend): State<Shared>, Path((id, name)): Path<(String, String)>, input: Body<Activate>) -> Result<Json<Value>> {
    body(input)?;
    let project = known(&backend, &id).await?;
    if !backend.agents.remove_note(project, name).await? { return Err(missing()); }
    Ok(Json(json!({})))
}
