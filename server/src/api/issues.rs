//! The routes behind the Issues tab: the user lists and searches a project's issues, reads one
//! with its history, comments, gives or takes ownership, closes what waits for review, reopens,
//! sets severity, and chooses what agents may close without review.

// Reuses the shared request checks.
use super::*;
// Issues, their actor and the reasons a change is refused.
use crate::issues::{self as store, Actor, Refusal};

/// Turns a refused change into the answer the page shows.
fn refuse(refusal: Refusal) -> Error {
    match refusal {
        Refusal::Missing => missing(),
        Refusal::Storage => unavailable(),
        _ => invalid(),
    }
}

/// Confirms the project exists and returns its id.
async fn known(backend: &Backend, id: &str) -> Result<Uuid> {
    authorized(backend).await?;
    let id = Uuid::parse_str(id).map_err(|_| missing())?;
    backend.agents.projects().await?.into_iter().any(|project| project.id == id).then_some(id).ok_or_else(missing)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Search { q: Option<String>, status: Option<String> }

/// Lists a project's issues, optionally by words and status, with the project's review level.
pub(super) async fn list(State(backend): State<Shared>, Path(id): Path<String>, Query(search): Query<Search>) -> Result<Json<Value>> {
    let project = known(&backend, &id).await?;
    let issues = backend.agents.issues(project, search.q.filter(|q| !q.trim().is_empty()), search.status).await?;
    let level = backend.agents.review_level(project).await?;
    Ok(Json(json!({ "issues": issues, "closeWithoutReview": level })))
}

/// Returns one issue with its history.
pub(super) async fn read(State(backend): State<Shared>, Path((id, number)): Path<(String, i64)>) -> Result<Json<Value>> {
    let project = known(&backend, &id).await?;
    let issue = backend.agents.issue(project, number).await?.ok_or_else(missing)?;
    Ok(Json(json!({ "issue": issue })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Comment { text: String }

/// Adds the user's comment to an issue's history.
pub(super) async fn comment(State(backend): State<Shared>, Path((id, number)): Path<(String, i64)>, input: Body<Comment>) -> Result<Json<Value>> {
    let input = body(input)?;
    let project = known(&backend, &id).await?;
    backend.agents.comment_issue(project, number, store::USER.into(), input.text).await.map_err(refuse)?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Owner { owner: Option<String> }

/// Gives an issue to an agent (its id, or `main` for the main agent), or takes it away with null.
pub(super) async fn owner(State(backend): State<Shared>, Path((id, number)): Path<(String, i64)>, input: Body<Owner>) -> Result<Json<Value>> {
    let input = body(input)?;
    let project = known(&backend, &id).await?;
    let actor = match input.owner.as_deref() {
        None => None,
        Some("main") => Some(Actor { id: "main".into(), name: "Main agent".into() }),
        Some(agent) => {
            let agent = backend.agents.list().await?.into_iter().find(|known| known.id.to_string() == agent).ok_or_else(invalid)?;
            Some(Actor { id: agent.id.to_string(), name: agent.definition.name })
        }
    };
    backend.agents.assign_issue(project, number, actor).await.map_err(refuse)?;
    Ok(Json(json!({})))
}

/// Closes an issue, approving an agent's work when it waited for review.
pub(super) async fn close(State(backend): State<Shared>, Path((id, number)): Path<(String, i64)>, input: Body<Value>) -> Result<Json<Value>> {
    body(input)?;
    let project = known(&backend, &id).await?;
    backend.agents.set_issue_status(project, number, true).await.map_err(refuse)?;
    Ok(Json(json!({})))
}

/// Reopens an issue; its owner keeps it.
pub(super) async fn reopen(State(backend): State<Shared>, Path((id, number)): Path<(String, i64)>, input: Body<Value>) -> Result<Json<Value>> {
    body(input)?;
    let project = known(&backend, &id).await?;
    backend.agents.set_issue_status(project, number, false).await.map_err(refuse)?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Severity { severity: String }

/// Sets severity; the user may lower it as well as raise it.
pub(super) async fn severity(State(backend): State<Shared>, Path((id, number)): Path<(String, i64)>, input: Body<Severity>) -> Result<Json<Value>> {
    let input = body(input)?;
    let project = known(&backend, &id).await?;
    backend.agents.set_issue_severity(project, number, input.severity, None).await.map_err(refuse)?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Settings { close_without_review: String }

/// Returns what agents may close without review in this project.
pub(super) async fn settings(State(backend): State<Shared>, Path(id): Path<String>) -> Result<Json<Value>> {
    let project = known(&backend, &id).await?;
    Ok(Json(json!({ "closeWithoutReview": backend.agents.review_level(project).await? })))
}

/// Saves what agents may close without review: none, low, medium or high.
pub(super) async fn save_settings(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Settings>) -> Result<Json<Value>> {
    let input = body(input)?;
    let project = known(&backend, &id).await?;
    if !store::valid_level(&input.close_without_review) { return Err(invalid()); }
    backend.agents.set_review_level(project, input.close_without_review.clone()).await?;
    Ok(Json(json!({ "closeWithoutReview": input.close_without_review })))
}

/// Streams a signal each time this project's issues change, so open pages reload at once.
/// It holds one of the shared stream slots and ends when the page goes away.
pub(super) async fn stream(State(backend): State<Shared>, Path(id): Path<String>) -> Result<Response> {
    let project = known(&backend, &id).await?;
    let slot = backend.streams.clone().try_acquire_owned().map_err(|_| Error(
        StatusCode::SERVICE_UNAVAILABLE, "stream_limit", "Too many live views are open. Close unused tabs, then reload.", false))?;
    let mut changes = store::subscribe();
    let stream = async_stream::stream! {
        let _slot = slot;
        yield Ok::<_, Infallible>(Event::default().event("ready").data("{}"));
        while changes.changed().await.is_ok() {
            let (_, which) = *changes.borrow_and_update();
            if which == project { yield Ok(Event::default().event("issues").data("{}")); }
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::default().interval(Duration::from_secs(10))).into_response())
}
