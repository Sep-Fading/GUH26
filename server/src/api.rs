// Routes HTTP requests and streams chat responses.
use axum::{
    body::{Body as HttpBody, Bytes},
    Json, Router,
    extract::Request,
    extract::rejection::JsonRejection,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, Method, StatusCode},
    middleware::{self, Next},
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, post},
};
// Decodes request bodies and history cursors.
use serde::{Deserialize, Serialize};
// Reads Codex payloads and builds JSON responses.
use serde_json::{Value, json};
// Shares backend state and configures event streams.
use std::{collections::BTreeMap, convert::Infallible, sync::{Arc, atomic::Ordering}, time::Duration};
// Serves built frontend files and fallback HTML.
use tower_http::services::{ServeDir, ServeFile};
// Creates request identifiers and validates task IDs.
use uuid::Uuid;

// Connects handlers to chat and Codex logic.
use crate::{
    chat::{Backend, Chat, Receipt, ScopedChoice, VmStartApproval}, computer,
    agents::{Definition, Issue}, history, recovery, skills, projects, skill_groups,
    models::{self, Choice, Model},
    environment::{Assignment, ConversationChoice, ProjectChoice},
    permissions::{Effect, Scope, Target, vetted_applications},
    rpc::{Client, Failure},
};

type Shared = Arc<Backend>;
type Result<T> = std::result::Result<T, Error>;
type Body<T> = std::result::Result<Json<T>, JsonRejection>;

mod delegation;
// Swarms: many agents, one goal, a shared ledger that survives broken links.
mod swarm;
/// The issue actor of a swarm agent's conversation, for the issue tool.
pub use swarm::actor as swarm_actor;
/// A swarm Lead's own issue actions, for the issue tool.
pub use swarm::lead_action as swarm_lead_action;
mod schedules;
mod mcp;
mod queue;
mod capabilities;
mod ask;
mod attachments;
mod project_memory;
mod issues;
mod terminal;
mod workspace;
mod search;
mod usage;

pub struct Error(StatusCode, &'static str, &'static str, bool);

impl IntoResponse for Error {
    /// Returns a JSON error response with a request ID and send-outcome status.
    fn into_response(self) -> Response {
        (
            self.0,
            Json(json!({ "error": {
            "code": self.1, "message": self.2, "retryable": false,
            "requestId": Uuid::new_v4(), "outcomeUnknown": self.3,
        } })),
        )
            .into_response()
    }
}

impl From<Failure> for Error {
    /// Maps a Codex failure to an HTTP error without exposing upstream details.
    fn from(error: Failure) -> Self {
        match error {
            Failure::Timeout => Self(
                StatusCode::GATEWAY_TIMEOUT,
                "timeout",
                "Codex did not acknowledge the request. Do not resend yet.",
                true,
            ),
            Failure::Unavailable => unavailable(),
            Failure::Rejected => Self(
                StatusCode::BAD_GATEWAY,
                "codex_rejected",
                "Codex rejected the request. Check your account or try a new chat.",
                false,
            ),
        }
    }
}

impl From<Issue> for Error {
    /// Maps profile failures to bounded public errors without database details.
    fn from(issue: Issue) -> Self {
        match issue {
            Issue::CreationLimit => Self(StatusCode::CONFLICT, "creation_journal_full",
                "Chat creation recovery is full. Open Notifications, then Creation recovery. Your message was not sent.", false),
            Issue::CreationUncertain => Self(StatusCode::CONFLICT, "chat_creation_uncertain",
                "This chat's creation was not confirmed. It cannot run or enable tools. Start a new chat, or delete this one.", false),
            Issue::Invalid => invalid(),
            Issue::Missing => missing(),
            Issue::NotAllowed => Self(StatusCode::FORBIDDEN, "capability_not_allowed",
                "This conversation's agent does not allow that. Allow it in the agent's settings first.", false),
            Issue::NameTaken => Self(StatusCode::CONFLICT, "name_taken",
                "Another entry already has this name.", false),
            Issue::ProjectAccess => Self(StatusCode::FORBIDDEN, "project_agent_denied",
                "This agent is not enabled in this project.", false),
            Issue::Inactive => Self(StatusCode::CONFLICT, "agent_inactive",
                "This agent is inactive. Activate it before starting more work.", false),
            Issue::SkillInactive => Self(StatusCode::CONFLICT, "skill_inactive",
                "An attached skill is inactive. Activate it before starting more work.", false),
            Issue::Unsupported => Self(StatusCode::UNPROCESSABLE_ENTITY,
                "agent_policy_unsupported",
                "This capability or access policy cannot yet be enforced. Keep the agent inactive.", false),
            Issue::Conflict => Self(StatusCode::CONFLICT, "agent_revision_conflict",
                "The agent changed. Reload its settings before saving.", false),
            Issue::EnvironmentLocked => Self(StatusCode::CONFLICT, "environment_locked",
                "This project's machine choice is permanent.", false),
            Issue::ConversationLocked => Self(StatusCode::CONFLICT, "conversation_machine_locked",
                "This conversation's machine choice is permanent.", false),
            Issue::VmUnavailable => Self(StatusCode::SERVICE_UNAVAILABLE, "vm_unavailable",
                "A dedicated VM is not ready. Complete its setup before saving this project.", false),
            Issue::Limit => Self(StatusCode::CONFLICT, "agent_storage_limit",
                "Agent storage has reached its limit.", false),
            Issue::Storage => Self(StatusCode::SERVICE_UNAVAILABLE, "agent_storage_unavailable",
                "Agent storage is unavailable. No new work was authorized.", false),
        }
    }
}

/// Rejects permission changes that would bypass a defined agent's access policy.
fn agent_access() -> Error {
    Error(StatusCode::FORBIDDEN, "agent_access_denied",
        "This agent does not have Computer Use access.", false)
}

/// Returns a 503 error when Codex or the requested state is unavailable.
fn unavailable() -> Error {
    Error(
        StatusCode::SERVICE_UNAVAILABLE,
        "codex_unavailable",
        "Codex is unavailable. Restart the MiniClaw backend.",
        false,
    )
}

/// Reports an unavailable assigned worker without retrying on another machine.
fn machine_unavailable(assignment: &Assignment) -> Error {
    match assignment {
        Assignment::Host {} => Error(StatusCode::SERVICE_UNAVAILABLE, "host_unavailable",
            "The Mac worker is unavailable. This task was not sent to a VM.", false),
        Assignment::Vm { .. } => Error(StatusCode::SERVICE_UNAVAILABLE, "vm_unavailable",
            "The assigned VM is unavailable. This task was not sent to your Mac.", false),
    }
}

/// Reports a missing Mac-side VM controller without implying task fallback.
fn vm_control_unavailable() -> Error {
    Error(StatusCode::SERVICE_UNAVAILABLE, "vm_control_unavailable",
        "The Mac VM controller is unavailable. No VM task was sent to your Mac.", false)
}

/// Confirms that the current client belongs to the task's pinned machine.
fn assigned_client(backend: &Backend, assignment: &Assignment)
    -> Result<crate::rpc::Client>
{
    let client = match assignment {
        Assignment::Host {} if backend.vm_routing => backend.host_worker(),
        Assignment::Host {} => backend.primary_client(),
        Assignment::Vm { vm_id } if backend.vm_routing && vm_id == "master" =>
            backend.primary_client(),
        Assignment::Vm { vm_id } if backend.vm_routing => backend.dedicated_client(vm_id),
        Assignment::Vm { .. } => None,
    };
    client.filter(|client| client.available())
        .ok_or_else(|| machine_unavailable(assignment))
}

/// Chooses a connected account-control client without changing any task assignment.
fn control_client(backend: &Backend) -> Result<crate::rpc::Client> {
    backend.host_worker()
        .or_else(|| backend.primary_client())
        .ok_or_else(unavailable)
}

/// Returns the native workspace path of the machine handling this conversation.
fn assigned_cwd<'a>(backend: &'a Backend, assignment: &Assignment) -> Result<&'a str> {
    assigned_client(backend, assignment)?;
    match assignment {
        Assignment::Host {} if backend.vm_routing => backend.host_cwd.as_deref()
            .ok_or_else(|| machine_unavailable(assignment)),
        _ => Ok(&backend.cwd),
    }
}

/// Returns a Computer Use helper only for its matching execution machine.
fn assigned_computer<'a>(backend: &'a Backend, assignment: &Assignment)
    -> Option<&'a computer::Target>
{
    match (assignment, backend.vm_routing, backend.computer_helper.as_ref()) {
        // A chat on this Mac uses the Mac's helper whether or not VM links exist.
        (Assignment::Host {}, _, _) if backend.is_local() => backend.local_computer(),
        (Assignment::Vm { vm_id }, true, Some(computer::Target::Guest))
            if backend.vm_computer_ready(vm_id) => backend.computer_helper.as_ref(),
        _ => None,
    }
}

/// Returns the pinned task workspace, keeping older unassigned reads on their old worker.
async fn thread_cwd<'a>(backend: &'a Backend, id: &str) -> Result<&'a str> {
    match backend.agents.assignment(id.to_owned()).await? {
        Some(assignment) => assigned_cwd(backend, &assignment),
        None => Ok(&backend.cwd),
    }
}

/// Requires an older conversation to receive an explicit machine choice first.
fn machine_choice_required() -> Error {
    Error(StatusCode::CONFLICT, "machine_choice_required",
        "Choose this conversation's machine before sending more work.", false)
}
/// Returns the same 404 error for missing routes and inaccessible tasks.
fn missing() -> Error {
    Error(
        StatusCode::NOT_FOUND,
        "not_found",
        "Task or route not found.",
        false,
    )
}
/// Returns a 409 error when a request conflicts with pending work or current state.
fn busy() -> Error {
    Error(
        StatusCode::CONFLICT,
        "busy",
        "A response is already pending.",
        false,
    )
}

/// Reports a full cache whose entries still hold work, send receipts, or readers.
fn task_limit() -> Error {
    Error(StatusCode::CONFLICT, "task_limit",
        "Close unused chat tabs. If the cache stays full, wait for responses to finish and restart MiniClaw.",
        false)
}
/// Returns a 409 error when a computer approval can no longer be answered.
fn approval_gone() -> Error {
    Error(
        StatusCode::CONFLICT,
        "approval_expired",
        "This approval is no longer active. Retry the request to get a new prompt.",
        false,
    )
}
/// Returns a 400 error for invalid request data.
fn invalid() -> Error {
    Error(
        StatusCode::BAD_REQUEST,
        "invalid_request",
        "Invalid request.",
        false,
    )
}
/// Returns the parsed request body or maps JSON rejection to a 400 error.
fn body<T>(value: Body<T>) -> Result<T> {
    value.map(|Json(value)| value).map_err(|_| invalid())
}

/// Builds the shared UI/API router with local-request checks and a body-size limit.
pub fn router(backend: Shared) -> Router {
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../web/dist");
    router_at(backend, &assets)
}

/// Serves a specified asset directory with the same local API and request guards.
pub fn router_at(backend: Shared, assets: &std::path::Path) -> Router {
    delegation::install(&backend);
    schedules::install(&backend);
    queue::install(&backend);
    ask::install(&backend);
    Router::new()
        .route("/api/health", get(health))
        .route("/api/sandbox", get(sandbox))
        .route("/api/sandbox/repair", post(sandbox_repair))
        .route("/api/account", get(account))
        .route("/api/models", get(catalog))
        .route("/api/settings/appearance", get(read_appearance).post(save_appearance))
        .route("/api/virtualisation", get(virtualisation))
        .route("/api/virtualisation/limits", post(virtualisation_limits))
        .route("/api/virtualisation/{action}", post(virtualisation_repair))
        .route("/api/providers", get(list_providers))
        .route("/api/providers/{id}", post(set_provider))
        .route("/api/notifications", get(notifications))
        .route("/api/creation-recovery", get(creation_recovery).post(compact_creations))
        .route("/api/notifications/read-all", post(read_all_notifications))
        .route("/api/notifications/clear-read", post(clear_read_notifications))
        .route("/api/notifications/{id}/read", post(read_notification))
        .route("/api/agents", get(agents).post(add_agent))
        .route("/api/agents/{id}", post(edit_agent))
        .route("/api/agents/{id}/delete", post(delete_agent))
        .route("/api/projects", get(list_projects).post(add_project))
        .route("/api/projects/{id}", post(edit_project))
        .route("/api/projects/{id}/rules", get(project_rules))
        .route("/api/projects/{id}/memory", get(project_memory::list).post(project_memory::add))
        .route("/api/projects/{id}/memory/{name}", post(project_memory::edit))
        .route("/api/projects/{id}/memory/{name}/delete", post(project_memory::remove))
        .route("/api/projects/{id}/issues", get(issues::list))
        .route("/api/projects/{id}/issues/live", get(issues::stream))
        .route("/api/projects/{id}/issues/{number}", get(issues::read))
        .route("/api/projects/{id}/issues/{number}/comment", post(issues::comment))
        .route("/api/projects/{id}/issues/{number}/owner", post(issues::owner))
        .route("/api/projects/{id}/issues/{number}/close", post(issues::close))
        .route("/api/projects/{id}/issues/{number}/reopen", post(issues::reopen))
        .route("/api/projects/{id}/issues/{number}/severity", post(issues::severity))
        .route("/api/projects/{id}/issue-settings", get(issues::settings).post(issues::save_settings))
        .route("/api/permissions/{kind}/{id}",
            get(permission_settings).post(change_permission))
        .route("/api/permissions/{kind}/{id}/app-requests",
            post(request_application))
        .route("/api/skills", get(list_skills).post(add_skill))
        .route("/api/skill-groups", get(list_groups).post(add_group))
        .route("/api/skill-groups/{id}", post(edit_group))
        .route("/api/skill-groups/{id}/delete", post(delete_group))
        .route("/api/threads/{id}/skills", get(chat_skills).post(save_chat_skills))
        .route("/api/skills/sources", get(skill_sources))
        .route("/api/slash-commands", get(slash_commands))
        .route("/api/main-agent", get(read_main_agent).post(save_main_agent))
        .route("/api/capabilities", get(capabilities::list))
        .route("/api/agents/{id}/capability-defaults", get(capabilities::defaults).post(capabilities::save_default))
        .route("/api/threads/{id}/capabilities", post(capabilities::toggle))
        .route("/api/skills/import", post(import_skill))
        .route("/api/skills/{id}", post(edit_skill))
        .route("/api/skills/{id}/delete", post(delete_skill))
        .route("/api/computer", get(computer_status))
        .route("/api/vms/master/status", get(master_status))
        .route("/api/vms/projects", post(provision_project_vm))
        .route("/api/vms/{vm_id}/status", get(dedicated_status))
        .route("/api/vms/{vm_id}/desktop", get(desktop_stream))
        .route("/api/vms/{vm_id}/desktop/show", post(show_desktop))
        .route("/api/vms/{vm_id}/desktop/shutdown", post(shutdown_desktop))
        .route("/api/vms/{vm_id}/auth/start", post(dedicated_auth_start))
        .route("/api/vms/{vm_id}/retry", post(retry_project_vm))
        .route("/api/vms/{vm_id}/cancel", post(cancel_project_vm))
        .route("/api/vms/master/prepare", post(prepare_master_start))
        .route("/api/vms/{vm_id}/prepare", post(prepare_dedicated_start))
        .route("/api/vms/master/decline", post(decline_master_start))
        .route("/api/vms/{vm_id}/decline", post(decline_dedicated_start))
        .route("/api/vms/{vm_id}/transfers/pick", post(transfer_pick))
        .route("/api/vms/{vm_id}/transfers/files", get(transfer_files))
        .route("/api/vms/{vm_id}/transfers", post(transfer_start))
        .route("/api/transfers/{job}", get(transfer_view))
        .route("/api/transfers/{job}/resolve", post(transfer_resolve))
        .route("/api/transfers/{job}/cancel", post(transfer_cancel))
        .route("/api/vms/{vm_id}/backups", get(backup_view))
        .route("/api/vms/{vm_id}/backups/settings", post(backup_settings))
        .route("/api/vms/{vm_id}/backups/run", post(backup_run))
        .route("/api/vms/{vm_id}/backups/{backup}/restore", post(backup_restore))
        .route("/api/threads/{id}/reviews/{review}", post(decide_review))
        .route("/api/threads/{id}/workspace/files", get(workspace::files))
        .route("/api/threads/{id}/workspace/file", get(workspace::file))
        .route("/api/threads/{id}/workspace/changes", get(workspace::changes))
        .route("/api/threads/{id}/workspace/diff", get(workspace::diff))
        .route("/api/threads/{id}/workspace/discard", post(workspace::discard))
        .route("/api/threads/{id}/delegations/{run}", post(delegation::answer))
        .route("/api/delegations/{run}/detach", post(delegation::detach))
        .route("/api/delegation/auto", get(delegation::auto_view).post(delegation::auto_save))
        .route("/api/swarms", get(swarm::list).post(swarm::start))
        .route("/api/swarms/{id}", get(swarm::view))
        .route("/api/swarms/{id}/link", post(swarm::link))
        .route("/api/swarms/{id}/tasks", post(swarm::add))
        .route("/api/swarms/{id}/stop", post(swarm::stop))
        .route("/api/swarms/{id}/delete", post(swarm::delete))
        .route("/api/threads/{id}/questions/{question}", post(ask::answer))
        .route("/api/threads/{id}/queue", get(queue::list).post(queue::add))
        .route("/api/threads/{id}/queue/clear", post(queue::clear))
        .route("/api/threads/{id}/queue/pause", post(queue::pause))
        .route("/api/threads/{id}/queue/resume", post(queue::resume))
        .route("/api/threads/{id}/queue/{item}", post(queue::edit))
        .route("/api/threads/{id}/queue/{item}/delete", post(queue::remove))
        .route("/api/threads/{id}/queue/{item}/send-now", post(queue::send_now))
        .route("/api/mcp/servers", get(mcp::list).post(mcp::add))
        .route("/api/mcp/servers/{id}", post(mcp::edit))
        .route("/api/mcp/servers/{id}/delete", post(mcp::remove))
        .route("/api/mcp/servers/{id}/check", post(mcp::check))
        .route("/api/mcp/sources", get(mcp::sources))
        .route("/api/mcp/import", post(mcp::import))
        .route("/api/schedules", get(schedules::list).post(schedules::add))
        .route("/api/schedules/tick", post(schedules::tick))
        .route("/api/schedules/{id}", post(schedules::edit))
        .route("/api/schedules/{id}/delete", post(schedules::remove))
        .route("/api/schedules/{id}/run", post(schedules::run_now))
        .route("/api/vms/idle", get(idle_warnings))
        .route("/api/vms/{vm_id}/idle", get(read_idle).post(save_idle))
        .route("/api/vms/{vm_id}/idle/keep", post(keep_running))
        .route("/api/vms/{vm_id}/power", post(vm_power))
        .route("/api/vms/{vm_id}/delete", post(delete_orphaned_vm))
        .route("/api/projects/{id}/delete", post(delete_project))
        .route("/api/vms/master/start", post(start_master))
        .route("/api/vms/{vm_id}/start", post(start_dedicated))
        .route("/api/threads/{id}/computer", post(set_computer))
        .route("/api/auth/start", post(login))
        .route("/api/auth/status", get(login_status))
        .route("/api/auth/cancel", post(cancel_login))
        .route("/api/auth/code", post(login_code))
        .route("/api/usage", get(usage::read))
        .route("/api/analytics", get(analytics))
        .route("/api/analytics/agents", get(agent_analytics))
        .route("/api/active-turns", get(active))
        .route("/api/threads", get(tasks).post(create))
        .route("/api/threads/working", get(working_tasks))
        .route("/api/search", get(search::search))
        .route("/api/search/refresh", post(search::refresh))
        .route("/api/threads/{id}", get(snapshot))
        .route("/api/threads/{id}/delete", post(delete_chat))
        .route("/api/threads/{id}/activate", post(activate))
        .route("/api/threads/{id}/history", get(older))
        .route("/api/threads/{id}/messages", post(send))
        .route("/api/threads/{id}/terminal/open", post(terminal::open))
        .route("/api/threads/{id}/terminal/stream", get(terminal::stream))
        .route("/api/threads/{id}/terminal/input", post(terminal::input))
        .route("/api/threads/{id}/terminal/resize", post(terminal::resize))
        .route("/api/threads/{id}/terminal/restart", post(terminal::restart))
        .route("/api/threads/{id}/terminal/close", post(terminal::close))
        .route("/api/threads/{id}/name", post(name_chat))
        // An upload is written to disk as it arrives and stops at its own limit.
        .route("/api/threads/{id}/attachments", post(attachments::upload).layer(DefaultBodyLimit::disable()))
        .route("/api/threads/{id}/attachments/{attachment}", get(attachments::content).delete(attachments::discard))
        .route("/api/threads/{id}/environment", post(assign_existing_machine))
        .route("/api/threads/{id}/interrupt", post(interrupt))
        .route("/api/threads/{id}/approvals/{approval}", post(decide))
        .route("/api/threads/{id}/scoped-approvals/{approval}", post(decide_scoped))
        .route("/api/threads/{id}/events", get(events))
        .route("/api/{*path}", get(not_found).post(not_found))
        .fallback_service(
            ServeDir::new(assets)
                .not_found_service(ServeFile::new(assets.join("index.html"))),
        )
        .layer(DefaultBodyLimit::max(256 * 1024))
        .layer(middleware::from_fn_with_state(backend.origin.clone(), guard))
        .with_state(backend)
}

/// Reads Master state from the Mac supervisor without starting the VM.
async fn master_status(State(backend): State<Shared>) -> Result<Json<Value>> {
    if !backend.vm_routing {
        return Err(Error(StatusCode::SERVICE_UNAVAILABLE, "vm_status_unavailable",
            "Master VM status is unavailable in this backend.", false));
    }
    let host = backend.host_worker().ok_or_else(vm_control_unavailable)?;
    let mut status = host.call("miniclaw/vm/status", json!({})).await?;
    if !matches!(status["state"].as_str(),
        Some("off" | "starting" | "running" | "failed" | "stopping" | "unknown")) {
        return Err(Error(StatusCode::BAD_GATEWAY, "vm_status_invalid",
            "The Mac supervisor returned an invalid VM status.", false));
    }
    let master = backend.primary_client();
    status["ready"] = json!(status["state"] == "running" && master.is_some());
    // Setup needs to know when the guest's own Codex is signed in and its helper is found.
    status["signedIn"] = json!(match &master {
        Some(client) => client.call("account/read", json!({})).await.ok()
            .is_some_and(|result| result["account"]["type"] == "chatgpt"),
        None => false,
    });
    status["computerReady"] = json!(backend.master_computer_ready
        .load(std::sync::atomic::Ordering::Acquire));
    Ok(Json(status))
}

/// Reads one known dedicated VM without consulting or starting Master.
async fn dedicated_status(State(backend): State<Shared>, Path(vm_id): Path<String>)
    -> Result<Json<Value>> {
    if Uuid::parse_str(&vm_id).ok().is_none_or(|id| id.to_string() != vm_id)
        || !backend.vm_routing { return Err(missing()); }
    let saved = backend.agents.vm_bindings().await?.into_iter()
        .any(|(_, id)| id == vm_id)
        || backend.agents.projects().await?.into_iter().any(|project|
            project.definition.environment.as_ref()
                .and_then(ProjectChoice::dedicated_id) == Some(vm_id.as_str()));
    let host = backend.host_worker().ok_or_else(vm_control_unavailable)?;
    if !saved && !registered_vm(&host, &vm_id).await? { return Err(missing()); }
    let mut status = host.call("miniclaw/vm/status", json!({ "vmId": vm_id })).await?;
    if !matches!(status["state"].as_str(),
        Some("off" | "starting" | "running" | "failed" | "stopping" | "unknown" | "missing")) {
        return Err(Error(StatusCode::BAD_GATEWAY, "vm_status_invalid",
            "The Mac supervisor returned an invalid VM status.", false));
    }
    status["workerConnected"] = json!(backend.dedicated_client(&vm_id).is_some());
    status["computerReady"] = json!(backend.vm_computer_ready(&vm_id));
    status["ready"] = json!(status["state"] == "running"
        && dedicated_ready(&backend, &vm_id).await);
    Ok(Json(status))
}

/// Confirms one selected VM is running before exposing its worker or controls.
async fn desktop_target(backend: &Shared, vm_id: &str) -> Result<Client> {
    let status = if vm_id == "master" {
        master_status(State(backend.clone())).await?.0
    } else {
        dedicated_status(State(backend.clone()), Path(vm_id.to_owned())).await?.0
    };
    if status["state"] != "running" || status["ready"] != true {
        return Err(Error(StatusCode::CONFLICT, "desktop_off",
            "This VM desktop is not ready.", false));
    }
    let client = if vm_id == "master" { backend.primary_client() }
        else { backend.dedicated_client(vm_id) };
    client.ok_or(Error(StatusCode::SERVICE_UNAVAILABLE, "desktop_unavailable",
        "The guest desktop worker is unavailable.", false))
}

/// Reads one JPEG through the authenticated guest worker without saving it on Linux.
/// `viewer` gives this stream its own place in the VM's frame sequence.
async fn desktop_frame(client: &Client, viewer: Option<&str>) -> Result<Vec<u8>> {
    let params = viewer.map_or_else(|| json!({}), |id| json!({ "viewer": id }));
    let value = client.call("miniclaw/desktop/frame", params).await
        .map_err(|_| Error(StatusCode::SERVICE_UNAVAILABLE, "capture_unavailable",
            "Guest screen capture is unavailable.", false))?;
    crate::frames::jpeg(&value["jpeg"]).ok_or(Error(StatusCode::BAD_GATEWAY, "capture_invalid",
        "The guest returned an invalid desktop frame.", false))
}

/// Streams fresh JPEGs from one running guest through the existing loopback server.
async fn desktop_stream(State(backend): State<Shared>, Path(vm_id): Path<String>)
    -> Result<Response> {
    let client = desktop_target(&backend, &vm_id).await?;
    // Each stream names itself, so two viewers of one VM both get every frame. A worker
    // installed before viewers existed refuses the name and is asked the plain way.
    let named = Uuid::new_v4().to_string();
    let (first, viewer) = match desktop_frame(&client, Some(&named)).await {
        Ok(frame) => (frame, Some(named)),
        Err(_) => (desktop_frame(&client, None).await?, None),
    };
    // Opening the desktop view is use of the VM. Later frames are not: a menu bar clock alone
    // would keep a VM awake for ever.
    backend.idle.touch(&vm_id);
    let frames = async_stream::stream! {
        let mut next = Some(first);
        let mut fresh = crate::frames::Fresh::default();
        let mut failures = 0;
        loop {
            let frame = match next.take() {
                Some(frame) => frame,
                None => match desktop_frame(&client, viewer.as_deref()).await {
                    Ok(frame) => { failures = 0; frame }
                    // The guest's capture restarts around login and display changes. One
                    // failed read must not end the stream; half a minute of them does.
                    Err(_) if failures < 60 && client.available() => {
                        failures += 1;
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        continue;
                    }
                    Err(_) => break,
                },
            };
            // A guest may answer at once with the frame it already sent. Sending it again
            // floods the browser, so wait one 30 fps interval and ask again.
            if !fresh.accept(&frame) {
                tokio::time::sleep(Duration::from_millis(33)).await;
                continue;
            }
            yield Ok::<Bytes, Infallible>(Bytes::from(crate::frames::part(&frame)));
        }
    };
    Ok(Response::builder()
        .header("Content-Type", "multipart/x-mixed-replace; boundary=frame")
        .header("Cache-Control", "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .body(HttpBody::from_stream(frames)).unwrap())
}

/// Reveals the already-running guest in its native host window.
async fn show_desktop(State(backend): State<Shared>, Path(vm_id): Path<String>)
    -> Result<Json<Value>> {
    desktop_action(&backend, &vm_id, "show").await
}

/// Requests shutdown of exactly the selected guest without ending the backend.
async fn shutdown_desktop(State(backend): State<Shared>, Path(vm_id): Path<String>)
    -> Result<Json<Value>> {
    desktop_action(&backend, &vm_id, "stop").await
}

/// Sends a signed host action only after VM ownership and readiness checks.
async fn desktop_action(backend: &Shared, vm_id: &str, action: &str)
    -> Result<Json<Value>> {
    let _ = desktop_target(backend, vm_id).await?;
    backend.idle.touch(vm_id);
    let host = backend.host_worker().ok_or_else(vm_control_unavailable)?;
    let params = if vm_id == "master" { json!({}) } else { json!({"vmId": vm_id}) };
    let status = host.call(&format!("miniclaw/vm/{action}"), params).await?;
    if status["state"] != if action == "show" { "running" } else { "stopping" } {
        return Err(Error(StatusCode::BAD_GATEWAY, "desktop_action_failed",
            "The VM did not accept the desktop action.", false));
    }
    Ok(Json(status))
}

/// Starts Codex-managed device sign-in on this project guest only.
async fn dedicated_auth_start(State(backend): State<Shared>, Path(vm_id): Path<String>,
    body: Body<Login>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    let Json(input) = body.map_err(|_| invalid())?;
    let master = vm_id == "master";
    if input.method != "device" || input.provider.is_some() || !backend.vm_routing
        || (!master && Uuid::parse_str(&vm_id).ok().is_none_or(|id| id.to_string() != vm_id)) {
        return Err(invalid());
    }
    let host = backend.host_worker().ok_or_else(vm_control_unavailable)?;
    if !master && !registered_vm(&host, &vm_id).await? { return Err(missing()); }
    // Each guest signs in through its own worker; the Mac's Codex is never involved.
    let client = if master { backend.primary_client() } else { backend.dedicated_client(&vm_id) };
    let client = client.ok_or_else(||
        Error(StatusCode::SERVICE_UNAVAILABLE, "vm_worker_unavailable",
            "Project VM Codex is not connected. Finish guest setup first.", false))?;
    if client.call("account/read", json!({})).await?["account"]["type"] == "chatgpt" {
        return Err(busy());
    }
    let result = client.call("account/login/start",
        json!({ "type": "chatgptDeviceCode" })).await?;
    let url = result["verificationUrl"].as_str().ok_or_else(unavailable)?;
    let code = result["userCode"].as_str().ok_or_else(unavailable)?;
    if url != "https://auth.openai.com/codex/device"
        || code.len() < 4 || code.len() > 32
        || !code.bytes().all(|byte| byte.is_ascii_uppercase()
            || byte.is_ascii_digit() || byte == b'-') {
        return Err(unavailable());
    }
    Ok(Json(json!({ "status": "pending", "verificationUrl": url,
        "userCode": code })))
}

/// Requires both the assigned worker and its own ChatGPT OAuth session.
async fn dedicated_ready(backend: &Backend, vm_id: &str) -> bool {
    let Some(client) = backend.dedicated_client(vm_id) else { return false; };
    client.call("account/read", json!({})).await.ok()
        .is_some_and(|result| result["account"]["type"] == "chatgpt")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectVmRequest { request_id: Uuid }

/// Creates or resumes one idempotent fresh-install job without publishing a project.
async fn provision_project_vm(State(backend): State<Shared>, body: Body<ProjectVmRequest>)
    -> Result<(StatusCode, Json<Value>)> {
    authorized(&backend).await?;
    let Json(input) = body.map_err(|_| invalid())?;
    if !backend.vm_routing { return Err(vm_control_unavailable()); }
    let host = backend.host_worker().ok_or_else(vm_control_unavailable)?;
    let result = host.call("miniclaw/vm/provision",
        json!({ "requestId": input.request_id })).await?;
    if result["state"] == "failed" {
        return Err(if result["reason"] == "Project VM limit reached." {
            Error(StatusCode::SERVICE_UNAVAILABLE, "vm_setup_failed",
                "Project VM limit reached.", false)
        } else { project_setup_failed() });
    }
    let id = result["vmId"].as_str().ok_or_else(vm_control_unavailable)?;
    if Uuid::parse_str(id).ok().is_none_or(|uuid| uuid.to_string() != id)
        || !matches!(result["state"].as_str(), Some("starting" | "failed")) {
        return Err(vm_control_unavailable());
    }
    Ok((StatusCode::ACCEPTED, Json(json!({ "vmId": id,
        "state": result["state"], "stage": result["stage"] }))))
}

/// Checks a signed host inventory without exposing keys to the browser.
async fn registered_vm(host: &crate::rpc::Client, vm_id: &str) -> Result<bool> {
    let inventory = host.call("miniclaw/vm/list", json!({})).await?;
    let machines = inventory["machines"].as_array().ok_or_else(vm_control_unavailable)?;
    if machines.len() > 64 { return Err(vm_control_unavailable()); }
    Ok(machines.iter().any(|machine| machine["vmId"] == vm_id))
}

/// Restarts a retained failed setup under the same project VM identity.
async fn retry_project_vm(State(backend): State<Shared>, Path(vm_id): Path<String>,
    body: Body<Value>) -> Result<(StatusCode, Json<Value>)> {
    project_vm_action(backend, vm_id, body, "retry").await
}

/// Cancels an unfinished install while retaining its guest files.
async fn cancel_project_vm(State(backend): State<Shared>, Path(vm_id): Path<String>,
    body: Body<Value>) -> Result<(StatusCode, Json<Value>)> {
    project_vm_action(backend, vm_id, body, "cancel").await
}

/// Allows setup control only for an inventory-backed project VM, never Master.
async fn project_vm_action(backend: Shared, vm_id: String, body: Body<Value>, method: &str)
    -> Result<(StatusCode, Json<Value>)> {
    authorized(&backend).await?;
    let Json(input) = body.map_err(|_| invalid())?;
    if input != json!({}) || !backend.vm_routing
        || Uuid::parse_str(&vm_id).ok().is_none_or(|id| id.to_string() != vm_id) {
        return Err(invalid());
    }
    let host = backend.host_worker().ok_or_else(vm_control_unavailable)?;
    if !registered_vm(&host, &vm_id).await? { return Err(missing()); }
    let status = host.call(&format!("miniclaw/vm/{method}"),
        json!({ "vmId": vm_id })).await?;
    if !matches!(status["state"].as_str(), Some("starting" | "failed" | "off")) {
        return Err(vm_control_unavailable());
    }
    if method == "retry" && status["state"] == "failed" {
        return Err(project_setup_failed());
    }
    let response = json!({ "state": status["state"], "stage": status["stage"],
        "reason": status["reason"] });
    Ok((if method == "retry" { StatusCode::ACCEPTED } else { StatusCode::OK },
        Json(response)))
}

/// Reports a safe setup failure without a native path or worker secret.
fn project_setup_failed() -> Error {
    Error(StatusCode::SERVICE_UNAVAILABLE, "vm_setup_failed",
        "Project VM setup could not proceed. Check its status and host capacity.", false)
}

/// Returns the exact supervisor target or rejects an unrecognized VM identifier.
fn vm_control_params(vm_id: &str) -> Result<Value> {
    if vm_id == "master" { return Ok(json!({})); }
    if Uuid::parse_str(vm_id).ok().is_none_or(|id| id.to_string() != vm_id) {
        return Err(invalid());
    }
    Ok(json!({ "vmId": vm_id }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MasterPrepare {
    environment: Option<ConversationChoice>,
    thread_id: Option<String>,
    project_id: Option<Uuid>,
    client_message_id: Uuid,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MasterStart {
    approval_id: Uuid,
    client_message_id: Uuid,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MasterDecline { approval_id: Uuid }

/// Revokes a waiting startup choice without touching the VM.
async fn decline_master_start(State(backend): State<Shared>, body: Body<MasterDecline>)
    -> Result<Json<Value>> {
    decline_vm_start(backend, "master", body).await
}

/// Revokes a dedicated VM approval only on its own machine path.
async fn decline_dedicated_start(State(backend): State<Shared>, Path(vm_id): Path<String>,
    body: Body<MasterDecline>) -> Result<Json<Value>> {
    decline_vm_start(backend, &vm_id, body).await
}

/// Removes one matching start approval without contacting its supervisor.
async fn decline_vm_start(backend: Shared, vm_id: &str, body: Body<MasterDecline>)
    -> Result<Json<Value>> {
    authorized(&backend).await?;
    vm_control_params(vm_id)?;
    let Json(input) = body.map_err(|_| invalid())?;
    let mut approvals = backend.vm_start_approvals.lock().await;
    if approvals.get(&input.approval_id).is_some_and(|approval|
        approval.assignment == (Assignment::Vm { vm_id: vm_id.into() })) {
        approvals.remove(&input.approval_id);
    }
    Ok(Json(json!({})))
}

/// Issues one short-lived start approval for a selected VM message only.
async fn prepare_master_start(State(backend): State<Shared>, body: Body<MasterPrepare>)
    -> Result<Json<Value>> {
    prepare_vm_start(backend, "master", body).await
}

/// Prepares a send-bound start approval for one dedicated conversation.
async fn prepare_dedicated_start(State(backend): State<Shared>, Path(vm_id): Path<String>,
    body: Body<MasterPrepare>) -> Result<Json<Value>> {
    prepare_vm_start(backend, &vm_id, body).await
}

/// Verifies the task's permanent target before creating a one-use approval.
async fn prepare_vm_start(backend: Shared, vm_id: &str, body: Body<MasterPrepare>)
    -> Result<Json<Value>> {
    authorized(&backend).await?;
    let params = vm_control_params(vm_id)?;
    let Json(input) = body.map_err(|_| invalid())?;
    let (assignment, thread, project) = match (input.environment, input.thread_id,
        input.project_id) {
        (Some(ConversationChoice::Vm {}), None, project_id) => {
            let context = match project_id {
                Some(id) => Some(backend.agents.prepare_project(id, None).await?),
                None => None,
            };
            let target = Assignment::for_project(ConversationChoice::Vm {},
                context.as_ref().and_then(|context|
                    context.project.definition.environment.as_ref()));
            (target, None, project_id)
        }
        (None, Some(id), None) if Uuid::parse_str(&id).is_ok() =>
            (backend.agents.assignment(id.clone()).await?
                .ok_or_else(machine_choice_required)?, Some(id), None),
        _ => return Err(invalid()),
    };
    if assignment != (Assignment::Vm { vm_id: vm_id.into() }) {
        return Err(invalid());
    }
    if !backend.vm_routing {
        return Err(machine_unavailable(&assignment));
    }
    let host = backend.host_worker().ok_or_else(vm_control_unavailable)?;
    let status = host.call("miniclaw/vm/status", params).await?;
    if status["state"] == "missing" {
        return Err(Error(StatusCode::SERVICE_UNAVAILABLE, "vm_missing",
            "The assigned VM does not exist. No task was sent to your Mac.", false));
    }
    if !matches!(status["state"].as_str(), Some("off" | "starting")) {
        return Err(Error(StatusCode::CONFLICT, "vm_start_not_needed",
            "The assigned VM is not off. Check its status before sending.", false));
    }
    let approval_id = Uuid::new_v4();
    let mut approvals = backend.vm_start_approvals.lock().await;
    approvals.retain(|_, value| value.expires > std::time::Instant::now());
    if approvals.len() >= 128 { return Err(busy()); }
    approvals.insert(approval_id, VmStartApproval { assignment,
        thread, project, message: input.client_message_id,
        expires: std::time::Instant::now() + Duration::from_secs(120) });
    Ok(Json(json!({ "approvalId": approval_id })))
}

/// Consumes one matching approval before asking the Mac supervisor to start Master.
async fn start_master(State(backend): State<Shared>, body: Body<MasterStart>)
    -> Result<(StatusCode, Json<Value>)>
{
    start_vm(backend, "master", body).await
}

/// Starts only the dedicated VM named by an existing task's approval.
async fn start_dedicated(State(backend): State<Shared>, Path(vm_id): Path<String>,
    body: Body<MasterStart>) -> Result<(StatusCode, Json<Value>)> {
    start_vm(backend, &vm_id, body).await
}

/// Consumes one matching approval before asking for that same VM to start.
async fn start_vm(backend: Shared, vm_id: &str, body: Body<MasterStart>)
    -> Result<(StatusCode, Json<Value>)>
{
    authorized(&backend).await?;
    let params = vm_control_params(vm_id)?;
    let Json(input) = body.map_err(|_| invalid())?;
    let approval = {
        let mut approvals = backend.vm_start_approvals.lock().await;
        let Some(saved) = approvals.get(&input.approval_id) else { return Err(busy()); };
        if saved.assignment != (Assignment::Vm { vm_id: vm_id.into() })
            || saved.message != input.client_message_id
            || saved.expires <= std::time::Instant::now() { return Err(busy()); }
        approvals.remove(&input.approval_id).ok_or_else(busy)?
    };
    if let Some(id) = &approval.thread {
        if backend.agents.chat_removed(id.clone()).await?
            || backend.agents.assignment(id.clone()).await? != Some(approval.assignment.clone()) {
            return Err(busy());
        }
    }
    if let Some(id) = approval.project {
        let context = backend.agents.prepare_project(id, None).await?;
        if Assignment::for_project(ConversationChoice::Vm {},
            context.project.definition.environment.as_ref()) != approval.assignment {
            return Err(busy());
        }
    }
    let host = backend.host_worker().ok_or_else(vm_control_unavailable)?;
    let mut status = host.call("miniclaw/vm/start", params).await?;
    if status["state"] == "failed" {
        let message = match status["reason"].as_str() {
            Some("VM resource limit exceeded.") => "VM resource limit exceeded.",
            Some("Master VM network disconnected.") => "Master VM network disconnected.",
            Some("Master VM stopped before it was ready.") =>
                "Master VM stopped before it was ready.",
            Some("Master VM resource ownership is unresolved.") =>
                "Master VM resource ownership is unresolved.",
            Some("Master VM setup is incomplete.") => "Master VM setup is incomplete.",
            Some("Mac VM controller is unavailable.") => "Mac VM controller is unavailable.",
            Some("Assigned VM does not exist.") => "Assigned VM does not exist.",
            _ => "Master VM could not start. Check VM diagnostics.",
        };
        return Err(Error(StatusCode::SERVICE_UNAVAILABLE, "vm_start_failed", message, false));
    }
    if !matches!(status["state"].as_str(), Some("starting" | "running")) {
        return Err(Error(StatusCode::BAD_GATEWAY, "vm_start_invalid",
            "The Mac supervisor did not confirm Master startup.", false));
    }
    status["ready"] = json!(status["state"] == "running"
        && if vm_id == "master" { backend.primary_client().is_some() }
            else { backend.dedicated_client(vm_id).is_some() });
    Ok((StatusCode::ACCEPTED, Json(status)))
}

/// Returns a JSON 404 for unmatched API routes instead of serving the frontend.
async fn not_found() -> Error {
    missing()
}

/// Rejects disallowed hosts, origins, and mutation headers; adds security headers
/// to the resulting response.
async fn guard(State(expected_origin): State<String>, request: Request, next: Next) -> Response {
    // Loopback binding alone does not prevent requests from unrelated websites.
    let headers = request.headers();
    let host = headers.get("host").and_then(|value| value.to_str().ok());
    let origin = headers.get("origin").and_then(|value| value.to_str().ok());
    let expected = expected_origin.strip_prefix("http://");
    let cross_site = headers
        .get("sec-fetch-site")
        .is_some_and(|value| value == "cross-site");
    let mutation = request.method() != Method::GET && request.method() != Method::HEAD;
    // An upload's body is the file itself. It still needs the custom header, which a page of
    // another site cannot send without a preflight this backend never answers.
    let upload = request.method() == Method::POST && request.uri().path().ends_with("/attachments");
    let allowed = host == expected
        && !cross_site
        && origin.is_none_or(|value| value == expected_origin)
        && (!mutation
            || (headers
                .get("x-miniclaw-request")
                .is_some_and(|value| value == "1")
                && headers
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| value.starts_with("application/json")
                        || (upload && value == "application/octet-stream"))));
    let mut response = if allowed {
        next.run(request).await
    } else {
        Error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Request origin or headers are not allowed.",
            false,
        )
        .into_response()
    };
    for (name, value) in [
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("x-frame-options", "DENY"),
        (
            "content-security-policy",
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data: blob: https:; frame-ancestors 'none'; base-uri 'none'",
        ),
    ] {
        response.headers_mut().insert(
            axum::http::HeaderName::from_static(name),
            value.parse().unwrap(),
        );
    }
    response
}

/// Sends a Codex RPC and returns its result or a browser-safe HTTP error.
async fn call(backend: &Backend, method: &str, params: Value) -> Result<Value> {
    let client = if let Some(id) = params["threadId"].as_str()
        .or_else(|| params["ancestorThreadId"].as_str())
    {
        if backend.is_claude(id) { return claude_call(backend, method, params).await; }
        match backend.agents.assignment(id.to_owned()).await? {
            Some(assignment) => assigned_client(backend, &assignment)?,
            None if matches!(method, "thread/read" | "thread/resume" | "thread/delete" | "thread/list") =>
                backend.primary_client().ok_or_else(unavailable)?,
            None => return Err(machine_choice_required()),
        }
    } else {
        control_client(backend)?
    };
    client.call(method, params).await.map_err(Into::into)
}

/// Sends one RPC for a Claude task to the bridge, never to a Codex worker.
async fn claude_call(backend: &Backend, method: &str, params: Value) -> Result<Value> {
    let client = backend.claude_client().ok_or_else(claude_unavailable)?;
    client.call(method, params).await.map_err(Into::into)
}

/// Returns the executor and saved-task provider for one task.
fn thread_client(backend: &Backend, id: &str, assignment: Option<&Assignment>)
    -> Result<(crate::rpc::Client, &'static str)>
{
    if backend.is_claude(id) {
        return Ok((backend.claude_client().ok_or_else(claude_unavailable)?,
            crate::claude::PROVIDER));
    }
    Ok((match assignment {
        Some(assignment) => assigned_client(backend, assignment)?,
        None => backend.primary_client().ok_or_else(unavailable)?,
    }, "openai"))
}

/// Reports that the Claude Code CLI bridge is not running.
fn claude_unavailable() -> Error {
    Error(StatusCode::SERVICE_UNAVAILABLE, "claude_unavailable",
        "Claude Code is unavailable. Install the claude CLI and restart MiniClaw.", false)
}

/// Requires the Claude CLI's own sign-in before a Claude chat is created.
async fn claude_authorized(client: &crate::rpc::Client) -> Result<()> {
    if client.call("account/read", json!({})).await?["account"]["type"] == "claude" {
        return Ok(());
    }
    Err(Error(StatusCode::UNAUTHORIZED, "claude_sign_in_required",
        "Sign in to Claude Code with `claude auth login`, then try again.", false))
}

/// Returns the Claude catalog, or nothing while the bridge is not running.
async fn claude_models(backend: &Backend) -> Result<Vec<Model>> {
    match backend.claude_client() {
        Some(client) => read_models(&client).await,
        None => Ok(Vec::new()),
    }
}

/// Routes Claude IDs before sign-in and discovers other picker aliases when available.
/// The selected provider still validates the exact model before accepting any work.
async fn claude_model(backend: &Backend, name: &str) -> bool {
    name.starts_with("claude-") || claude_models(backend).await.unwrap_or_default()
        .iter().any(|entry| entry.model == name)
}

/// Returns backend health and the current Codex connection status without an RPC.
async fn health(State(backend): State<Shared>) -> Json<Value> {
    // The page words itself for this operating system ("macos" or "windows").
    Json(json!({ "backend": "ok", "platform": std::env::consts::OS, "codex":
        if control_client(&backend).is_ok() {
            "connected"
        } else { "unavailable" },
        "claude": if backend.claude_client().is_some() { "connected" } else { "unavailable" },
    }))
}

/// Reports whether sandboxed work can run on this installation and, when it cannot, the reason:
/// the gate itself, the product's own Git views, and approved hosts for commands.
async fn sandbox() -> Result<Json<Value>> {
    #[cfg(windows)]
    let report = tokio::task::spawn_blocking(crate::windows_gate::report).await.map_err(|_| unavailable())?;
    #[cfg(not(windows))]
    let report = {
        let part = || crate::windows_gate::Part { available: crate::seatbelt::available(), reason: None };
        crate::windows_gate::Report { available: crate::seatbelt::available(), reason: None, recovery: false, git: part(), network: part(), access: None }
    };
    Ok(Json(json!(report)))
}

/// Sets up the network service's Windows access entries again after Windows changed them. The
/// running service does it without elevation. When it cannot (the service is missing, stopped or
/// refused), the answer says so and names the command that repairs it with one approval.
/// Returns `{ok, report}` and, when not repaired, `reason` and `command`.
async fn sandbox_repair(input: Body<Value>) -> Result<Json<Value>> {
    let _ = input.map_err(|_| invalid())?;
    #[cfg(windows)]
    {
        use crate::windows_gate::Missing;
        let outcome = tokio::task::spawn_blocking(|| -> std::result::Result<(), (String, Option<&'static str>)> {
            match crate::windows_gate::state() {
                // Only a missing, stopped or unverified service is set up again by `miniclaw repair`.
                Err(missing @ (Missing::ServiceNotConfigured | Missing::ServiceNotInstalled | Missing::ServiceStopped | Missing::ServiceUnverified)) =>
                    Err((missing.reason(), Some("miniclaw repair"))),
                // Records of an earlier run: recovery is tried again, as at the next start.
                Err(Missing::Recovery(_)) => {
                    let retried = crate::windows_sandbox::recover();
                    crate::windows_gate::forget();
                    retried.map_err(|_| (crate::windows_gate::state().err().map_or_else(String::new, |missing| missing.reason()), None))
                }
                Err(missing) => Err((missing.reason(), None)),
                Ok(()) => crate::windows_network_broker::repair_access().map_err(|error| (error.to_string(), Some("miniclaw repair"))),
            }
        }).await.map_err(|_| unavailable())?;
        let report = tokio::task::spawn_blocking(crate::windows_gate::report).await.map_err(|_| unavailable())?;
        return Ok(Json(match outcome {
            Ok(()) => json!({ "ok": true, "report": report }),
            Err((reason, command)) => json!({ "ok": false, "reason": reason, "command": command, "report": report }),
        }));
    }
    #[cfg(not(windows))]
    Err(missing())
}

/// Refreshes and returns the public account fields, clearing cached chats on identity
/// changes and rejecting reads invalidated by an account notification.
async fn read_account(backend: &Backend) -> Result<Value> {
    let _guard = backend.auth_gate.lock().await;
    let generation = backend.data.lock().await.generation;
    let result = call(backend, "account/read", json!({})).await?;
    let account = &result["account"];
    let status = if account.is_null() {
        "signed_out"
    } else if account["type"] == "chatgpt" {
        "signed_in"
    } else {
        "unsupported_auth"
    };
    // Expose only the account fields the UI needs.
    let normalized = json!({
        "status": status, "email": account["email"].as_str(),
        "planType": account["planType"].as_str(),
        "requiresOpenaiAuth": result["requiresOpenaiAuth"].as_bool()
            .unwrap_or(true),
    });
    let mut data = backend.data.lock().await;
    // An account notification may have invalidated this read while it was awaiting Codex.
    if generation != data.generation {
        return Err(unavailable());
    }
    if !data.account.is_null()
        && (data.account["email"] != normalized["email"]
            || data.account["status"] != normalized["status"])
    {
        backend.forget(&mut data);
        data.usage.clear();
        data.generation += 1;
        backend.changed();
    }
    data.account = normalized.clone();
    Ok(normalized)
}

/// Reports that ChatGPT sign-in is missing.
fn sign_in_required() -> Error {
    Error(StatusCode::UNAUTHORIZED, "sign_in_required", "Sign in with ChatGPT to continue.", false)
}

/// Reads Claude's own sign-in state, or None while its bridge is not running.
async fn claude_account(backend: &Backend) -> Option<Value> {
    let client = backend.claude_client()?;
    let result = client.call("account/read", json!({})).await.ok()?;
    let account = &result["account"];
    let normalized = json!({
        "status": if account["type"] == "claude" { "signed_in" } else { "signed_out" },
        "email": account["email"].as_str(), "planType": account["planType"].as_str(),
    });
    let mut data = backend.data.lock().await;
    if data.claude_account != normalized { data.usage.remove("claude"); }
    data.claude_account = normalized.clone();
    Some(normalized)
}

/// Requires ChatGPT sign-in for work that only Codex can do.
async fn codex_authorized(backend: &Backend) -> Result<u64> {
    if read_account(backend).await?["status"] != "signed_in" { return Err(sign_in_required()); }
    Ok(backend.data.lock().await.generation)
}

/// Requires a signed-in provider, any one of them, for provider-neutral routes.
/// Returns the generation used to detect identity changes.
async fn authorized(backend: &Backend) -> Result<u64> {
    let codex = read_account(backend).await;
    if !codex.as_ref().is_ok_and(|account| account["status"] == "signed_in") {
        match claude_account(backend).await {
            Some(claude) if claude["status"] == "signed_in" => {}
            // With Codex as the only provider its own failure is the answer.
            None => { codex?; return Err(sign_in_required()); }
            Some(_) => return Err(Error(StatusCode::UNAUTHORIZED, "sign_in_required",
                "Sign in to ChatGPT or Claude to continue.", false)),
        }
    }
    Ok(backend.data.lock().await.generation)
}

/// Requires the sign-in of the provider that owns this task.
async fn authorized_thread(backend: &Backend, id: &str) -> Result<u64> {
    if !backend.is_claude(id) { return codex_authorized(backend).await; }
    claude_authorized(&backend.claude_client().ok_or_else(claude_unavailable)?).await?;
    Ok(backend.data.lock().await.generation)
}

/// Lists every provider with its saved choice, install state and sign-in state.
async fn provider_states(backend: &Backend) -> (Result<Value>, Option<Value>, Value) {
    let codex = read_account(backend).await;
    let claude = claude_account(backend).await;
    let settings = backend.providers.get()
        .map(|runtime| crate::providers::Settings::read(&runtime.settings)).unwrap_or_default();
    let states = crate::providers::Id::ALL.map(|id| {
        let account = match id {
            crate::providers::Id::Codex => codex.as_ref().ok().cloned(),
            crate::providers::Id::Claude => claude.clone(),
        };
        json!({ "id": id.name(), "enabled": settings.enabled(id),
            "installed": backend.providers.get().is_none_or(|runtime| runtime.installed(id)),
            "status": account.as_ref().and_then(|account| account["status"].as_str())
                .unwrap_or("unavailable"),
            "email": account.as_ref().and_then(|account| account["email"].as_str()),
            "planType": account.as_ref().and_then(|account| account["planType"].as_str()) })
    });
    (codex, claude, json!(states))
}

/// Returns the identity the UI shows: ChatGPT when signed in, else Claude, plus every
/// provider's own state. Fails only when no provider is running.
async fn account(State(backend): State<Shared>) -> Result<Json<Value>> {
    let (codex, claude, providers) = provider_states(&backend).await;
    let signed_in = |account: &Value| account["status"] == "signed_in";
    let mut account = match (codex, claude) {
        (Ok(codex), Some(claude)) if !signed_in(&codex) && signed_in(&claude) => claude,
        (Ok(codex), _) => codex,
        (Err(_), Some(claude)) => claude,
        (Err(error), None) => return Err(error),
    };
    account["providers"] = providers;
    Ok(Json(account))
}

/// Returns provider settings without requiring any sign-in.
async fn list_providers(State(backend): State<Shared>) -> Json<Value> {
    Json(json!({ "providers": provider_states(&backend).await.2 }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderChoice {
    enabled: bool,
}

/// Saves one provider's choice and starts or stops it; no restart is needed.
async fn set_provider(State(backend): State<Shared>, Path(id): Path<String>,
    input: Body<ProviderChoice>) -> Result<Json<Value>> {
    let id = crate::providers::Id::parse(&id).ok_or_else(missing)?;
    let input = body(input)?;
    tokio::spawn(async move {
        backend.set_provider(id, input.enabled).await?;
        Ok(Json(json!({ "providers": provider_states(&backend).await.2 })))
    }).await.map_err(|_| unavailable())?
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Login {
    method: String,
    provider: Option<String>,
}

/// Starts Codex-managed sign-in or reuses a pending attempt, returning its status
/// and browser URL or device code. Already signed-in accounts are rejected.
async fn login(State(backend): State<Shared>, input: Body<Login>) -> Result<Json<Value>> {
    let input = body(input)?;
    let kind = match input.method.as_str() {
        "browser" => "chatgpt",
        "device" => "chatgptDeviceCode",
        _ => return Err(invalid()),
    };
    // Serialize sign-in clicks so they reuse one pending attempt.
    let _guard = backend.login_gate.lock().await;
    if input.provider.as_deref() == Some("claude") { return claude_login(&backend).await; }
    if input.provider.as_deref().is_some_and(|provider| provider != "codex") {
        return Err(invalid());
    }
    if read_account(&backend).await?["status"] == "signed_in" {
        return Err(busy());
    }
    {
        let data = backend.data.lock().await;
        if data.login["status"] == "pending" {
            return Ok(Json(data.login.clone()));
        }
    }
    let result = call(&backend, "account/login/start", json!({ "type": kind })).await?;
    let id = result["loginId"].as_str().ok_or_else(unavailable)?;
    let mut login = json!({
        "attemptId": Uuid::new_v4(), "status": "pending",
    });
    for key in ["authUrl", "verificationUrl", "userCode"] {
        if let Some(value) = result[key].as_str() {
            if key.ends_with("Url") && !auth_url(value) {
                return Err(unavailable());
            }
            login[key] = json!(value);
        }
    }
    let mut data = backend.data.lock().await;
    if let Some(success) = data.login_results.get(id) {
        login["status"] = json!(if *success { "succeeded" } else { "failed" });
    }
    data.login_id = Some(id.into());
    data.login = login.clone();
    Ok(Json(login))
}

/// Starts `claude auth login` and waits briefly for the sign-in page it prints, so the page can
/// offer it as a link at once. The status route reports the rest: a later page, the CLI's error
/// or exit, and the account once signed in.
async fn claude_login(backend: &Backend) -> Result<Json<Value>> {
    let client = backend.claude_client().ok_or_else(claude_unavailable)?;
    if claude_authorized(&client).await.is_ok() { return Err(already_signed_in()); }
    let started = client.call("account/login/start", json!({})).await?;
    let mut login = json!({ "attemptId": Uuid::new_v4(), "status": "pending", "provider": "claude" });
    if let Some(error) = started["error"].as_str() {
        login["status"] = json!("failed");
        login["error"] = json!(error);
    } else {
        // The CLI prints its page within a second or two of starting (measured on Windows ARM64).
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            claude_progress(&mut login, &client.call("account/login/read", json!({})).await?);
            if login["status"] != "pending" || login.get("authUrl").is_some()
                || tokio::time::Instant::now() >= deadline { break; }
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }
    }
    let mut data = backend.data.lock().await;
    data.login_id = None;
    data.login = login.clone();
    Ok(Json(login))
}

/// Copies what the running Claude sign-in has shown into the page's sign-in status: the page
/// to open, a hint, or the CLI's error once it has stopped.
fn claude_progress(login: &mut Value, progress: &Value) {
    if let Some(url) = progress["url"].as_str().filter(|url| crate::claude_login::sign_in_page(url)) {
        login["authUrl"] = json!(url);
        // The CLI accepts the code Claude's page shows when the browser cannot return to it.
        login["codeEntry"] = json!(true);
    }
    let message = progress["message"].as_str().and_then(crate::claude_login::message);
    match progress["exited"].as_bool() {
        Some(false) => {
            login["status"] = json!("failed");
            login["error"] = json!(message.unwrap_or_else(|| "Claude Code stopped before the sign-in finished.".into()));
        }
        // A successful exit becomes "succeeded" once the account reads as signed in.
        _ => if let Some(message) = message { login["notice"] = json!(message); },
    }
    if progress["running"] == false && login["status"] == "pending" {
        login["status"] = json!("failed");
        login["error"] = json!("The sign-in was stopped.");
    }
}

/// Reports that Claude Code is signed in already, so there is nothing to start.
fn already_signed_in() -> Error {
    Error(StatusCode::CONFLICT, "already_signed_in", "Claude Code is already signed in.", false)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginCode {
    code: String,
}

/// Hands the code Claude's sign-in page showed to the running `claude auth login`. The code is
/// never stored or logged; only a `code#state` value is accepted.
async fn login_code(State(backend): State<Shared>, input: Body<LoginCode>) -> Result<Json<Value>> {
    let input = body(input)?;
    let code = input.code.trim();
    if !crate::claude_login::code(code) {
        return Err(Error(StatusCode::BAD_REQUEST, "invalid_sign_in_code",
            "That is not a Claude sign-in code. Copy the whole code Claude shows after you sign in.", false));
    }
    let _guard = backend.login_gate.lock().await;
    let pending = {
        let data = backend.data.lock().await;
        data.login["provider"] == "claude" && data.login["status"] == "pending"
    };
    if !pending {
        return Err(Error(StatusCode::CONFLICT, "no_sign_in", "No Claude sign-in is waiting for a code. Start it again.", false));
    }
    let client = backend.claude_client().ok_or_else(claude_unavailable)?;
    client.call("account/login/code", json!({ "code": code })).await?;
    Ok(Json(json!({})))
}

/// Returns whether a login URL uses HTTPS and an allowed OpenAI authentication host.
fn auth_url(value: &str) -> bool {
    value.parse::<axum::http::Uri>().ok().is_some_and(|uri| {
        uri.scheme_str() == Some("https")
            && matches!(
                uri.host(),
                Some("auth.openai.com" | "chatgpt.com" | "auth0.openai.com")
            )
    })
}

/// Returns the cached sign-in attempt, or an idle status if no attempt exists.
async fn login_status(State(backend): State<Shared>) -> Json<Value> {
    let claude_pending = {
        let data = backend.data.lock().await;
        data.login["provider"] == "claude" && data.login["status"] == "pending"
    };
    // The Claude CLI reports no completion event, so its sign-in state is read instead.
    if claude_pending && claude_account(&backend).await
        .is_some_and(|account| account["status"] == "signed_in") {
        backend.data.lock().await.login["status"] = json!("succeeded");
    } else if claude_pending && let Some(client) = backend.claude_client()
        && let Ok(progress) = client.call("account/login/read", json!({})).await {
        let mut data = backend.data.lock().await;
        if data.login["provider"] == "claude" && data.login["status"] == "pending" {
            claude_progress(&mut data.login, &progress);
        }
    }
    let data = backend.data.lock().await;
    Json(if data.login.is_null() {
        json!({ "status": "idle" })
    } else {
        data.login.clone()
    })
}

/// Cancels the stored Codex login attempt and returns an empty acknowledgement.
async fn cancel_login(State(backend): State<Shared>) -> Result<Json<Value>> {
    let _guard = backend.login_gate.lock().await;
    let id = backend.data.lock().await.login_id.clone();
    if backend.data.lock().await.login["provider"] == "claude" {
        if let Some(client) = backend.claude_client() {
            client.call("account/login/cancel", json!({})).await?;
        }
        backend.data.lock().await.login = json!({ "status": "cancelled" });
    } else if let Some(id) = id {
        call(&backend, "account/login/cancel", json!({ "loginId": id })).await?;
        backend.data.lock().await.login = json!({ "status": "cancelled" });
    }
    Ok(Json(json!({})))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryQuery {
    cursor: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkerCursors {
    version: u8,
    host: Option<String>,
    vm: Option<String>,
    host_done: bool,
    vm_done: bool,
    #[serde(default)]
    dedicated: BTreeMap<String, Option<String>>,
    #[serde(default)]
    dedicated_done: Vec<String>,
}

impl Default for WorkerCursors {
    /// Starts both workers at their first saved-task page.
    fn default() -> Self {
        Self { version: 1, host: None, vm: None, host_done: false, vm_done: false,
            dedicated: BTreeMap::new(), dedicated_done: Vec::new() }
    }
}

/// Reads one page from each worker without transferring a cursor across machines.
async fn assigned_task_pages(backend: &Backend, after: Option<&str>) -> Result<Value> {
    if backend.host_cwd.is_none() {
        let client = backend.primary_client().ok_or_else(unavailable)?;
        return Ok(history::list(&client, &backend.cwd, after).await?);
    }
    let mut cursors: WorkerCursors = match after {
        Some(after) => serde_json::from_str(after).map_err(|_| invalid())?,
        None => WorkerCursors::default(),
    };
    if after.is_none() {
        for (id, _) in backend.dedicated_workers() {
            cursors.dedicated.insert(id, None);
        }
    }
    if cursors.version != 1
        || (cursors.host_done && cursors.host.is_some())
        || (cursors.vm_done && cursors.vm.is_some())
    { return Err(invalid()); }
    for cursor in [&cursors.host, &cursors.vm].into_iter().flatten() {
        history::cursor(&json!(cursor)).map_err(|_| invalid())?;
    }
    for (id, cursor) in &cursors.dedicated {
        if Uuid::parse_str(id).is_err() { return Err(invalid()); }
        if let Some(cursor) = cursor {
            history::cursor(&json!(cursor)).map_err(|_| invalid())?;
        }
    }
    if cursors.dedicated_done.iter().any(|id|
        !cursors.dedicated.contains_key(id)) { return Err(invalid()); }
    let mut threads = Vec::new();
    let host_unavailable = !cursors.host_done && backend.host_worker().is_none();
    if !cursors.host_done && !host_unavailable {
        let host = backend.host_worker().ok_or_else(unavailable)?;
        let cwd = backend.host_cwd.as_deref().ok_or_else(unavailable)?;
        let page = history::list(&host, cwd, cursors.host.as_deref()).await?;
        for summary in page["threads"].as_array().ok_or_else(unavailable)? {
            let id = summary["id"].as_str().ok_or_else(unavailable)?;
            // A deleted conversation that a worker still lists has no machine to check.
            if backend.agents.chat_removed(id.to_owned()).await? { continue; }
            if backend.agents.assignment(id.to_owned()).await?
                .is_some_and(|binding| binding != (Assignment::Host {}))
            { return Err(unavailable()); }
            threads.push(summary.clone());
        }
        cursors.host = history::cursor(&page["nextCursor"])?;
        cursors.host_done = cursors.host.is_none();
    } else if host_unavailable {
        cursors.host_done = true;
        cursors.host = None;
    }
    let vm_unavailable = !cursors.vm_done && backend.primary_client().is_none();
    if !cursors.vm_done && !vm_unavailable {
        let client = backend.primary_client().ok_or_else(unavailable)?;
        let page = history::list(&client, &backend.cwd, cursors.vm.as_deref()).await?;
        for summary in page["threads"].as_array().ok_or_else(unavailable)? {
            let id = summary["id"].as_str().ok_or_else(unavailable)?;
            // A deleted conversation that a worker still lists has no machine to check.
            if backend.agents.chat_removed(id.to_owned()).await? { continue; }
            if backend.agents.assignment(id.to_owned()).await?
                .is_some_and(|binding| binding != (Assignment::Vm { vm_id: "master".into() }))
                || threads.iter().any(|existing| existing["id"] == id)
            { return Err(unavailable()); }
            threads.push(summary.clone());
        }
        cursors.vm = history::cursor(&page["nextCursor"])?;
        cursors.vm_done = cursors.vm.is_none();
    } else if vm_unavailable {
        cursors.vm_done = true;
        cursors.vm = None;
    }
    let mut incomplete: Vec<String> = [
        host_unavailable.then(|| "host".to_owned()),
        vm_unavailable.then(|| "master".to_owned()),
    ].into_iter().flatten().collect();
    for (vm_id, after) in cursors.dedicated.clone() {
        if cursors.dedicated_done.contains(&vm_id) { continue; }
        let Some(client) = backend.dedicated_client(&vm_id) else {
            cursors.dedicated_done.push(vm_id.clone());
            cursors.dedicated.insert(vm_id.clone(), None);
            incomplete.push(vm_id);
            continue;
        };
        let page = history::list(&client, &backend.cwd, after.as_deref()).await?;
        for summary in page["threads"].as_array().ok_or_else(unavailable)? {
            let id = summary["id"].as_str().ok_or_else(unavailable)?;
            if backend.agents.chat_removed(id.to_owned()).await? { continue; }
            let binding = backend.agents.assignment(id.to_owned()).await?;
            if binding == Some(Assignment::Vm { vm_id: vm_id.clone() }) {
                if threads.iter().any(|existing| existing["id"] == id) {
                    return Err(unavailable());
                }
                threads.push(summary.clone());
            } else if binding.is_some() { return Err(unavailable()); }
        }
        let next = history::cursor(&page["nextCursor"])?;
        cursors.dedicated.insert(vm_id.clone(), next.clone());
        if next.is_none() { cursors.dedicated_done.push(vm_id); }
    }
    for (_, vm_id) in backend.agents.vm_bindings().await? {
        if vm_id != "master" && backend.dedicated_client(&vm_id).is_none()
            && !incomplete.contains(&vm_id) {
            incomplete.push(vm_id);
        }
    }
    threads.sort_by_key(|thread| std::cmp::Reverse(thread["createdAt"].as_i64()));
    let next = if cursors.host_done && cursors.vm_done
        && cursors.dedicated_done.len() == cursors.dedicated.len() { Value::Null }
        else { json!(serde_json::to_string(&cursors).map_err(|_| unavailable())?) };
    Ok(json!({ "threads": threads, "nextCursor": next,
        "incompleteMachines": incomplete }))
}

/// Returns running ids, activity times, and reply receipts without loading histories.
async fn working_tasks(State(backend): State<Shared>) -> Result<Json<Value>> {
    let generation = authorized(&backend).await?;
    let runs = backend.agents.running_runs().await?;
    let data = backend.data.lock().await;
    if data.generation != generation { return Err(unavailable()); }
    let threads: Vec<&str> = data.chats.iter()
        .filter(|(_, chat)| chat.sending || chat.active.is_some())
        .map(|(id, _)| id.as_str()).collect();
    let activity: serde_json::Map<String, Value> = data.chats.iter()
        .filter_map(|(id, chat)| chat.summary["updatedAt"].as_i64()
            .map(|time| (id.clone(), json!(time)))).collect();
    let replies: serde_json::Map<String, Value> = data.chats.iter()
        .filter_map(|(id, chat)| chat.last_response.as_ref()
            .map(|(turn_id, completed_at)| (id.clone(),
                json!({ "turnId": turn_id, "completedAt": completed_at })))).collect();
    let now = chrono::Utc::now().timestamp_millis();
    let mut prompts: Vec<Value> = backend.delegation_prompts.lock().await.iter()
        .filter_map(|(thread, (prompt, _))| {
            let chat = data.chats.get(thread)?;
            let end = prompt["expiresAt"].as_i64()?;
            if end <= now || chat.active.is_none() || chat.stopping { return None; }
            let mut view = prompt.clone();
            view["thread"] = json!(thread);
            view["threadName"] = chat.summary["name"].as_str().map_or_else(|| json!(thread), |name| json!(name));
            view["expiresInSeconds"] = json!((end - now + 999) / 1000);
            Some(view)
        }).collect();
    prompts.sort_by_key(|prompt| (prompt["expiresAt"].as_i64(), prompt["id"].as_str().unwrap_or_default().to_owned()));
    Ok(Json(json!({ "threads": threads, "activity": activity, "replies": replies,
        "delegationPrompts": prompts, "delegatedRequests": delegation::pending_requests(&data, &runs) })))
}

/// Returns a page of permitted saved tasks, using current summaries for loaded chats
/// and including not-yet-saved chats on the first page.
async fn tasks(
    State(backend): State<Shared>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Value>> {
    let generation = authorized(&backend).await?;
    check_cursor(&query.cursor)?;
    let codex = codex_authorized(&backend).await.is_ok();
    if query.cursor.is_none() && codex {
        recover_tasks(&backend, generation).await?;
    }
    let mut result = if codex {
        assigned_task_pages(&backend, query.cursor.as_deref()).await?
    } else { json!({ "threads": [], "nextCursor": null }) };
    let claude = claude_account(&backend).await
        .is_some_and(|account| account["status"] == "signed_in");
    // Claude tasks all arrive with the first page; later cursors belong to Codex alone.
    if query.cursor.is_none() && claude && let Some(client) = backend.claude_client() {
        let mut cursor = None;
        for _ in 0..128 {
            let page = history::list_for(&client, backend.local_cwd(), cursor.as_deref(),
                crate::claude::PROVIDER).await?;
            result["threads"].as_array_mut().unwrap()
                .extend(page["threads"].as_array().unwrap().iter().cloned());
            cursor = history::cursor(&page["nextCursor"])?;
            if cursor.is_none() { break; }
        }
    }
    if query.cursor.is_none() && backend.vm_routing {
        for (id, vm_id) in backend.agents.vm_bindings().await? {
            let available = if vm_id == "master" { backend.primary_client().is_some() }
                else { backend.dedicated_client(&vm_id).is_some() };
            if !available && !result["threads"].as_array().unwrap()
                .iter().any(|task| task["id"] == id) {
                result["threads"].as_array_mut().unwrap()
                    .push(offline_summary(&id, &vm_id));
            }
        }
    }
    let hidden_runs = backend.agents.hidden_run_threads().await?;
    let mut visible = Vec::new();
    for mut summary in result["threads"].as_array().unwrap().clone() {
        let id = summary["id"].as_str().ok_or_else(unavailable)?.to_owned();
        if backend.agents.chat_removed(id.clone()).await? { continue; }
        // A delegated run shows inside its parent until the user makes it a conversation.
        if hidden_runs.contains(&id) { continue; }
        match tag_context(&backend, &id, &mut summary).await {
            Err(error) if error.1 == "chat_creation_uncertain" => {
                summary["recoveryRequired"] = json!(true);
            }
            result => result?,
        }
        // A swarm agent's conversation shows under its swarm; older ones without a project stay hidden.
        if summary["projectId"].is_null() && (swarm::hides(&id) || swarm::looks_like_swarm(&summary)) { continue; }
        visible.push(summary);
    }
    result["threads"] = json!(visible);
    let data = backend.data.lock().await;
    if data.generation != generation {
        return Err(unavailable());
    }
    let threads = result["threads"].as_array_mut().unwrap();
    for summary in threads.iter_mut() {
        if let Some(chat) = summary["id"].as_str().and_then(|id| data.chats.get(id)) {
            let updated = summary["updatedAt"].as_i64().or(summary["createdAt"].as_i64()).unwrap_or(0)
                .max(chat.summary["updatedAt"].as_i64()
                    .or(chat.summary["createdAt"].as_i64()).unwrap_or(0));
            *summary = chat.summary.clone();
            summary["updatedAt"] = json!(updated);
        }
    }
    // Newly created chats may not have reached Codex's saved list yet.
    if query.cursor.is_none() {
        for chat in data.chats.values() {
            if chat.summary["id"].as_str().is_some_and(|id| hidden_runs.contains(id)) { continue; }
            if chat.summary["projectId"].is_null() && (chat.summary["id"].as_str().is_some_and(swarm::hides) || swarm::looks_like_swarm(&chat.summary)) { continue; }
            if !threads.iter().any(|summary| summary["id"] == chat.summary["id"]) {
                threads.push(chat.summary.clone());
            }
        }
        threads.sort_by_key(|thread| std::cmp::Reverse(thread["createdAt"].as_i64()));
    }
    if backend.vm_routing {
        for summary in threads.iter_mut() {
            if let Some(vm_id) = summary["vmId"].as_str() {
                if (vm_id == "master" && backend.primary_client().is_none())
                    || (vm_id != "master" && backend.dedicated_client(vm_id).is_none()) {
                    summary["vmOffline"] = json!(true);
                }
            }
        }
    }
    // A conversation's own folder, so deleting it can offer to move the folder to the Trash too.
    for summary in threads.iter_mut() {
        if let Some(folder) = summary["id"].as_str().and_then(|id| backend.chat_folders.recorded(id)) {
            summary["folder"] = json!(folder);
        }
    }
    Ok(Json(result))
}

/// Builds a minimal task marker without claiming to have read its VM transcript.
fn offline_summary(id: &str, vm_id: &str) -> Value {
    json!({ "id": id, "name": if vm_id == "master" { "VM task" } else { "Project VM task" },
        "model": null, "computerUse": false,
        "createdAt": 0,
        "environment": { "kind": "vm" }, "vmId": vm_id, "vmOffline": true })
}

/// Adds local project and pinned agent identity to a permitted Codex summary.
async fn tag_context(backend: &Backend, id: &str, summary: &mut Value) -> Result<()> {
    let agent = backend.agents.binding(id.to_owned(), false).await?;
    if let Some(agent) = &agent { agent.tag(summary); }
    if let Some(project) = backend.agents.project_context(
        id.to_owned(), agent.map(|a| a.id), false).await?
    { project.tag(summary); }
    if let Some(assignment) = backend.agents.assignment(id.to_owned()).await? {
        summary["environment"] = json!(assignment.choice());
        if let Assignment::Vm { vm_id } = assignment { summary["vmId"] = json!(vm_id); }
    } else {
        summary["environmentRequired"] = json!(true);
    }
    // A name the user gave lies over the provider's, which is kept so it can come back.
    if let Some(name) = backend.agents.chat_name(id.to_owned()).await? {
        summary["providerName"] = summary["name"].take();
        summary["name"] = json!(name);
    }
    // The speed is MiniClaw's own record: Codex does not keep it on the thread.
    summary["speed"] = json!(backend.agents.chat_speed(id.to_owned()).await?);
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Named { name: String }

/// Gives a conversation the user's own name, or with an empty name gives back the one it had.
/// Returns the conversation's summary.
async fn name_chat(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Named>) -> Result<Json<Value>> {
    let name = crate::names::checked(&body(input)?.name).ok_or_else(invalid)?;
    load(backend.clone(), id.clone()).await?;
    authorized_thread(&backend, &id).await?;
    backend.agents.name_chat(id.clone(), name.clone()).await?;
    let summary = {
        let mut data = backend.data.lock().await;
        let chat = data.chats.get_mut(&id).ok_or_else(missing)?;
        let provider = chat.summary.as_object_mut().and_then(|summary| summary.remove("providerName"));
        let provider = provider.unwrap_or_else(|| chat.summary["name"].clone());
        if name.is_empty() {
            chat.summary["name"] = provider;
        } else {
            chat.summary["providerName"] = provider;
            chat.summary["name"] = json!(name);
        }
        chat.emit("snapshot", chat.snapshot());
        chat.summary.clone()
    };
    backend.changed();
    Ok(Json(json!({ "thread": summary })))
}


#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Limits { cpu_percent: u8, memory_percent: u8, disk_percent: u8 }

/// Asks the VM session about the shared budget and adds what the settings page shows with it.
async fn virtualisation_view(backend: &Shared, method: &str, params: Value) -> Result<Json<Value>> {
    authorized(backend).await?;
    let host = backend.host_worker().filter(|_| backend.vm_routing).ok_or_else(vm_control_unavailable)?;
    let ledger = host.call(&format!("miniclaw/vm/{method}"), params).await?;
    let master = master_status(State(backend.clone())).await?.0;
    // An older worker does not know the question, which is the answer.
    let worker = match backend.primary_client() {
        Some(client) if master["state"] == "running" => Some(
            client.call("miniclaw/worker/info", json!({})).await.ok()
                .and_then(|info| info["protocol"].as_u64())
                .is_some_and(|protocol| protocol >= crate::worker::PROTOCOL)),
        _ => None,
    };
    Ok(Json(json!({ "available": true, "ledger": ledger, "master": master,
        "workerCurrent": worker, "projectVms": project_vms(backend, &host).await? })))
}

/// Lists every project VM with its state and owner. One without an owner is orphaned.
async fn project_vms(backend: &Shared, host: &Client) -> Result<Vec<Value>> {
    let list = host.call("miniclaw/vm/list", json!({})).await?;
    let projects = backend.agents.projects().await?;
    let mut rows = Vec::new();
    for machine in list["machines"].as_array().into_iter().flatten().take(64) {
        let Some(vm_id) = machine["vmId"].as_str() else { continue };
        let status = host.call("miniclaw/vm/status", json!({ "vmId": vm_id })).await?;
        let owner = projects.iter().find(|project| project.definition.environment.as_ref()
            .and_then(ProjectChoice::dedicated_id) == Some(vm_id));
        rows.push(json!({ "vmId": vm_id, "state": status["state"],
            "projectId": owner.map(|project| project.id),
            "projectName": owner.map(|project| project.definition.name.clone()) }));
    }
    Ok(rows)
}

/// Shows the VM budget, the Master VM and whether its worker is current.
async fn virtualisation(State(backend): State<Shared>) -> Result<Json<Value>> {
    if !backend.vm_routing || backend.host_worker().is_none() {
        authorized(&backend).await?;
        return Ok(Json(json!({ "available": false })));
    }
    virtualisation_view(&backend, "ledger", json!({})).await
}

/// Changes the share of this Mac that all MiniClaw VMs together may use.
async fn virtualisation_limits(State(backend): State<Shared>, input: Body<Limits>)
    -> Result<Json<Value>> {
    let Json(limits) = input.map_err(|_| invalid())?;
    let values = [limits.cpu_percent, limits.memory_percent, limits.disk_percent];
    if values.iter().any(|value| !(1..=100).contains(value)) { return Err(invalid()); }
    virtualisation_view(&backend, "limits", json!({ "limits":
        format!("{},{},{}", values[0], values[1], values[2]) })).await
}

/// Releases idle guests whose supervisor died, or forgets guests deleted from disk.
async fn virtualisation_repair(State(backend): State<Shared>, Path(action): Path<String>,
    input: Body<Value>) -> Result<Json<Value>> {
    let _ = input.map_err(|_| invalid())?;
    if !matches!(action.as_str(), "reconcile" | "forget") { return Err(missing()); }
    virtualisation_view(&backend, &action, json!({})).await
}

/// Returns how the page should look. It needs no sign-in, so the page can theme itself first.
async fn read_appearance(State(backend): State<Shared>) -> Json<crate::appearance::Appearance> {
    Json(backend.appearance.get().map(|path| crate::appearance::load(path)).unwrap_or_default())
}

/// Saves display choices from the closed set the page offers.
async fn save_appearance(State(backend): State<Shared>, input: Body<crate::appearance::Appearance>)
    -> Result<Json<crate::appearance::Appearance>> {
    let Json(value) = input.map_err(|_| invalid())?;
    if !value.valid() { return Err(invalid()); }
    let path = backend.appearance.get().ok_or_else(unavailable)?.clone();
    let stored = value.clone();
    tokio::task::spawn_blocking(move || crate::appearance::save(&path, &stored)).await
        .map_err(|_| unavailable())?.map_err(|_| unavailable())?;
    Ok(Json(value))
}

/// Reports whether provider web search is on for a conversation. It is on by default; the
/// `no_web_search` switch turns it off for an agent, a project or a conversation.
async fn web_search(backend: &Backend, id: &str) -> Result<bool> {
    use crate::permissions::{Decision, Target};
    let context = backend.agents.permission_context(id.to_owned()).await?;
    let off = Target::Feature { name: "no_web_search".into() };
    Ok(!matches!(backend.agents.permission(&context, &off, chrono::Utc::now().timestamp()).await?, Decision::Allow(_)))
}

/// Reports whether the Codex image switch is permitted and on for a host conversation.
async fn image_generation(backend: &Backend, id: &str) -> bool {
    use crate::permissions::{Decision, Target};
    let Ok(context) = backend.agents.permission_context(id.to_owned()).await else { return false; };
    let target = Target::Feature { name: "image_generation".into() };
    backend.agents.configured_switch(context.agent(), context.project(), &target).await.unwrap_or(false)
        && matches!(backend.agents.permission(&context, &target, chrono::Utc::now().timestamp()).await,
            Ok(Decision::Allow(_)))
}

/// Returns the independent Claude tool switches and plan mode, with host rules for fetch.
/// Returns nothing when every switch is off.
async fn claude_tools(backend: &Backend, id: &str) -> Result<Option<Value>> {
    use crate::permissions::{Decision, Target};
    let context = backend.agents.permission_context(id.to_owned()).await?;
    let now = chrono::Utc::now().timestamp();
    let mut on = [false; 4];
    for (index, name) in ["web_fetch", "subagents", "task_tracking", "plan_mode"].into_iter().enumerate() {
        let switch = Target::Feature { name: name.into() };
        on[index] = matches!(backend.agents.permission(&context, &switch, now).await?,
            Decision::Allow(_));
    }
    let search = web_search(backend, id).await?;
    if !on.iter().any(|on| *on) && !search { return Ok(None); }
    let mut rules = json!({ "fetch": on[0], "subagents": on[1], "tracking": on[2], "plan": on[3],
        "search": search, "hosts": [], "domains": [], "blockedHosts": [], "blockedDomains": [] });
    for entry in backend.agents.permission_summary(&context, now).await?.effective {
        let (kind, name) = match entry.target {
            Target::Host { host } => ("Hosts", host),
            Target::Domain { domain } => ("Domains", domain),
            _ => continue,
        };
        let key = match entry.decision {
            "allow" => kind.to_lowercase(),
            "block" => format!("blocked{kind}"),
            _ => continue,
        };
        rules[key].as_array_mut().expect("rule list").push(json!(name));
    }
    Ok(Some(rules))
}

/// Looks at a working folder on this Mac the way the Changes panel does: through the sandboxed
/// executor, with a grant for that folder only and git unable to run anything of the repository's.
async fn survey(root: &str) -> crate::briefing::Folder {
    use crate::scoped::Operation;
    let grants = [crate::seatbelt::Grant { path: root.to_owned(), write: false, folder: true, blocked: false }];
    let mut folder = crate::briefing::Folder { path: root.to_owned(), ..Default::default() };
    let listed = crate::executor::execute_host(Operation::ListFolder { path: root.to_owned() }, &grants).await;
    if listed["status"] == "ok" {
        // Each line is `name<TAB>kind<TAB>size`; hidden entries tell the model little and cost room.
        let names: Vec<String> = listed["text"].as_str().unwrap_or_default().lines().filter_map(|line| {
            let mut parts = line.split('\t');
            let (name, kind) = (parts.next()?, parts.next().unwrap_or_default());
            (!name.starts_with('.') && name.len() <= 80).then(|| if kind == "folder" { format!("{name}/") } else { name.to_owned() })
        }).collect();
        folder.more = names.len().saturating_sub(40);
        folder.names = names.into_iter().take(40).collect();
    }
    let arguments = ["status", "--porcelain=v1", "--branch"];
    // While Windows cannot enforce the sandbox, the survey names the files and leaves Git out.
    #[cfg(windows)]
    let status = crate::executor::workspace_git(&arguments, root, &grants).await.unwrap_or_else(|| json!({ "status": "denied" }));
    // Git that could not run says nothing about a repository, and the model is told so.
    #[cfg(windows)]
    { folder.unchecked = status["status"] != "ok" || (status["exitCode"] != 0
        && !status["stderr"].as_str().is_some_and(|words| words.contains("not a git repository"))); }
    #[cfg(not(windows))]
    let status = crate::executor::execute_host(Operation::RunShell { text: crate::workspace_commands::git_script(&arguments), cwd: root.to_owned(), hosts: Vec::new() }, &grants).await;
    if status["status"] == "ok" && status["exitCode"] == 0 {
        let text = status["text"].as_str().unwrap_or_default();
        let branch: String = text.lines().next().unwrap_or_default().trim_start_matches("## ").trim_start_matches("No commits yet on ")
            .split("...").next().unwrap_or_default().chars().filter(|c| !c.is_control()).take(80).collect();
        let changed = text.lines().skip(1).filter(|line| !line.trim().is_empty()).count();
        folder.git = Some(format!("branch {branch}, {}", match changed {
            0 => "no uncommitted changes".to_owned(),
            count => format!("{count} file{} with uncommitted changes", if count == 1 { "" } else { "s" }),
        }));
    }
    folder
}

/// Writes the standing part of the briefing from the tools the conversation was really given.
/// It is called last, when a conversation starts and when it is loaded again.
async fn brief(backend: &Backend, assignment: &Assignment, project: Option<&projects::Context>, web_search: bool,
    computer_use: bool, params: &mut Value) {
    crate::briefing::detect().await;
    let tools: Vec<String> = params["dynamicTools"].as_array().into_iter().flatten()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned)).collect();
    let in_vm = matches!(assignment, Assignment::Vm { .. });
    let folder = match (assignment, project.and_then(|context| context.project.definition.workspace.clone())) {
        (Assignment::Vm { .. }, _) => Some(crate::briefing::Folder { path: backend.cwd.clone(), ..Default::default() }),
        (Assignment::Host {}, Some(workspace)) if tools.iter().any(|tool| tool == crate::scoped::TOOL) => Some(survey(&workspace).await),
        _ => None,
    };
    let machine = match assignment { Assignment::Host {} => "this Mac".to_owned(), Assignment::Vm { vm_id } if vm_id == "master" => "the Master VM".to_owned(),
        Assignment::Vm { .. } => "this project's VM".to_owned() };
    let mut rules = crate::briefing::Rules { instructions: project.map(|context| context.project.definition.instructions.clone()).unwrap_or_default(),
        ..Default::default() };
    if let (Assignment::Host {}, Some(workspace)) = (assignment, project.and_then(|context| context.project.definition.workspace.clone())) {
        (rules.files, rules.cut) = tokio::task::spawn_blocking(move || rule_files(&workspace)).await.unwrap_or_default();
    }
    params["baseInstructions"] = json!(crate::briefing::standing(&crate::briefing::Standing { machine: &machine, in_vm, tools,
        web_search, computer_use, folder, rules }));
}

/// Names the instruction files a project's conversations are told, with their sizes, for its settings.
async fn project_rules(State(backend): State<Shared>, Path(id): Path<Uuid>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    let project = backend.agents.projects().await?.into_iter().find(|project| project.id == id).ok_or_else(missing)?;
    let (files, cut) = match project.definition.workspace.filter(|_| matches!(project.definition.environment, Some(ProjectChoice::Host {}) | None)) {
        Some(workspace) => tokio::task::spawn_blocking(move || rule_files(&workspace)).await.unwrap_or_default(),
        None => Default::default(),
    };
    Ok(Json(json!({ "cut": cut, "files": files.iter().map(|(name, text)| json!({ "name": name, "bytes": text.len() })).collect::<Vec<_>>() })))
}

/// Reads `AGENTS.md` and `CLAUDE.md` from the top of a project folder, the files Codex and Claude
/// Code read, up to 32 KiB together; the flag says whether they were cut. A file is read only when
/// it really lies inside the folder, so a link cannot bring in text from elsewhere, and never from
/// a folder MiniClaw protects. A `CLAUDE.md` that repeats or only imports `AGENTS.md` is left out.
fn rule_files(root: &str) -> (Vec<(String, String)>, bool) { rule_files_within(root, &crate::seatbelt::protected()) }

/// As `rule_files`, with the protected roots given.
fn rule_files_within(root: &str, protected: &[String]) -> (Vec<(String, String)>, bool) {
    let Ok(folder) = std::fs::canonicalize(root) else { return Default::default(); };
    let (mut files, mut room, mut cut) = (Vec::<(String, String)>::new(), crate::briefing::RULES_BYTES, false);
    for name in ["AGENTS.md", "CLAUDE.md"] {
        let Ok(real) = std::fs::canonicalize(folder.join(name)) else { continue; };
        if !real.starts_with(&folder) || !real.is_file() || crate::seatbelt::is_protected(&real.to_string_lossy(), protected) { continue; }
        use std::io::Read;
        let mut bytes = Vec::new();
        if std::fs::File::open(&real).and_then(|file| file.take(crate::briefing::RULES_BYTES as u64 + 1).read_to_end(&mut bytes)).is_err() { continue; }
        let mut text = String::from_utf8_lossy(&bytes).replace('\0', "");
        let plain = text.trim();
        if plain.is_empty() || files.iter().any(|(_, known)| known.trim() == plain)
            || (!files.is_empty() && plain.lines().all(|line| line.trim().is_empty() || line.trim().eq_ignore_ascii_case("@agents.md"))) { continue; }
        if text.len() > room {
            let mut end = room;
            while !text.is_char_boundary(end) { end -= 1; }
            text.truncate(end);
            cut = true;
        }
        room -= text.len();
        files.push((name.to_owned(), text));
        if room == 0 { break; }
    }
    (files, cut)
}

/// Writes the note that travels with each message: the date, who the conversation belongs to, and
/// what is on, off or not allowed right now.
async fn environment_note(backend: &Backend, id: &str) -> Option<String> {
    use crate::permissions::{Decision, Target};
    let context = backend.agents.permission_context(id.to_owned()).await.ok()?;
    let now = chrono::Utc::now().timestamp();
    let summary = backend.data.lock().await.chats.get(id).map(|chat| chat.summary.clone())?;
    let on = |target: Target| { let context = context.clone(); async move {
        matches!(backend.agents.permission(&context, &target, now).await, Ok(Decision::Allow(_))) } };
    let mut capabilities = Vec::new();
    // Web search and asking are on unless their switch is set; the others are on only where allowed.
    let search_off = on(Target::Feature { name: "no_web_search".into() }).await;
    let search_barred = backend.agents.configured_switch(context.agent(), context.project(), &Target::Feature { name: "no_web_search".into() }).await.unwrap_or(false);
    // A VM's worker does not take the web search setting yet, so search is not available there.
    let in_vm = summary["environment"]["kind"] == "vm";
    capabilities.push(crate::briefing::Capability { name: "Web search".into(), allowed: !search_barred && !in_vm, on: !search_off && !in_vm });
    for (name, feature) in [("Image generation", "image_generation"), ("Network for commands", "network"), ("Web fetch", "web_fetch"),
        ("Subagents", "subagents"), ("Task tracking", "task_tracking")] {
        let target = Target::Feature { name: feature.into() };
        let allowed = backend.agents.configured_switch(context.agent(), context.project(), &target).await.unwrap_or(false);
        let provider = feature != "image_generation" || (!backend.is_claude(id) && !in_vm);
        capabilities.push(crate::briefing::Capability { name: name.into(), allowed: allowed && provider,
            on: allowed && provider && on(target).await });
    }
    let swarm_shell = backend.shell_grant(id).await.is_some_and(|grant| grant.swarm && grant.context == context);
    let shell_allowed = swarm_shell || (!in_vm && backend.agents.configured_switch(context.agent(), context.project(),
        &Target::Feature { name: "unsandboxed_shell".into() }).await.unwrap_or(false));
    capabilities.push(crate::briefing::Capability { name: "Unsandboxed shell (explicit timed access outside the sandbox)".into(),
        allowed: shell_allowed, on: shell_allowed && backend.shell_grant(id).await.is_some_and(|grant| grant.context == context) });
    // Both are the conversation's own switches; the model is told so it knows what will and will not ask.
    capabilities.push(crate::briefing::Capability { name: "Full access (nothing you are allowed is asked about)".into(), allowed: true,
        on: on(Target::Feature { name: "full_access".into() }).await });
    capabilities.push(crate::briefing::Capability { name: "Read-only commands run without asking".into(), allowed: true,
        on: !on(Target::Feature { name: "ask_commands".into() }).await });
    capabilities.push(crate::briefing::Capability { name: "Asking the user questions".into(), allowed: true,
        on: !on(Target::Feature { name: "no_questions".into() }).await });
    for (name, feature) in [("Coding session (the user reviews every change)", "coding_session"), ("Plan mode (plan, do not act)", "plan_mode")] {
        capabilities.push(crate::briefing::Capability { name: name.into(), allowed: true, on: on(Target::Feature { name: feature.into() }).await });
    }
    // Only the servers meant for this conversation's provider are reachable, so only they are named.
    let servers: Vec<String> = crate::mcp_gateway::switched_on(backend, &context, backend.is_claude(id)).await
        .into_iter().map(|server| server.name).collect();
    // Named only where the tool is offered: on this Mac, without Computer Use, and not in a delegated run.
    let offered = !in_vm && summary["computerUse"] != true
        && !backend.agents.run_of_thread(id.to_owned()).await.ok().flatten().is_some_and(|run| run.is_delegated());
    // The limits are read as a hand-off reads them, so a change to the main agent's shows from the next message.
    let (delegates, limits) = match backend.agents.binding(id.to_owned(), false).await.ok() {
        Some(agent) if offered => (delegation::recipients(backend, agent.as_ref()).await.into_iter().map(|(_, name)| name).collect(),
            delegation::limits(backend, agent.as_ref()).await.ok()),
        _ => (Vec::new(), None),
    };
    let machine = match summary["environment"]["kind"].as_str() { Some("vm") => "a MiniClaw virtual machine", _ => "this Mac" }.to_owned();
    // A VM conversation is told of servers only when its worker carries the gateway to it.
    let carried = match backend.agents.assignment(id.to_owned()).await.ok().flatten() {
        Some(assignment) => backend.machine_carries(&assignment, crate::mcp_gateway::TOOL).await,
        None => !in_vm,
    };
    let servers = carried.then_some(servers);
    // A conversation on this computer with no project and no agent is told of its own folder, where
    // the file and command tool is offered.
    let own_folder = if !in_vm && offered && context.agent().is_none() && context.project().is_none() && crate::seatbelt::available() {
        match backend.own_folder(id, true) {
            Some(folder) => Some(crate::briefing::OwnFolder::Made(folder)),
            None => crate::chat_folder::root().map(|root| crate::briefing::OwnFolder::Later(root.to_string_lossy().into_owned())),
        }
    } else { None };
    let mut note = crate::briefing::state(&crate::briefing::State { machine, agent: summary["agentName"].as_str().map(str::to_owned),
        project: summary["projectName"].as_str().map(str::to_owned), capabilities, servers, delegates, limits, own_folder });
    // What this project lets an agent close without the user; it changes only when the user changes it.
    if let Some(project) = context.project() && let Ok(level) = backend.agents.review_level(project).await {
        note.push_str(&format!("- Issues: {}\n", crate::issues::level_sentence(&level)));
    }
    Some(note)
}

/// Offers the native tool on the host or a routed VM. Dispatch still requires a binding
/// or a live timed shell grant; exposing the schema grants no access.
async fn host_tools(backend: &Backend, assignment: &Assignment,
    agent: Option<Uuid>, project: Option<Uuid>) -> bool {
    match assignment {
        Assignment::Host {} => backend.is_local() && (crate::seatbelt::available()
            || backend.agents.configured_switch(agent, project,
                &Target::Feature { name: "unsandboxed_shell".into() }).await.unwrap_or(false)),
        // The VM is the sandbox, so every chat in it may act there, bound or not.
        Assignment::Vm { .. } => backend.vm_routing,
    }
}

/// Offers the MCP gateway tool to a conversation on this Mac. The tool reaches nothing until the
/// user switches a server on, and that is decided at each call, so it is offered from the start.
async fn offer_mcp(backend: &Backend, assignment: &Assignment, params: &mut Value) {
    if !backend.is_local() || !backend.machine_carries(assignment, crate::mcp_gateway::TOOL).await { return; }
    let mut tools = params["dynamicTools"].as_array().cloned().unwrap_or_default();
    tools.push(crate::mcp_gateway::tool_definition());
    params["dynamicTools"] = json!(tools);
}

/// Offers the question tool to a conversation on this Mac. Whether asking is switched off is
/// decided at each call. A delegated run is not offered it: its questions would land in a
/// conversation the user may not be looking at, so it decides for itself.
async fn offer_ask(backend: &Backend, assignment: &Assignment, thread: Option<&str>, delegated: bool, params: &mut Value) {
    if delegated || !backend.is_local() || !backend.machine_carries(assignment, crate::ask::TOOL).await { return; }
    if let Some(thread) = thread
        && backend.agents.run_of_thread(thread.to_owned()).await.ok().flatten().is_some_and(|run| run.is_delegated()) { return; }
    let mut tools = params["dynamicTools"].as_array().cloned().unwrap_or_default();
    tools.push(crate::ask::tool_definition());
    params["dynamicTools"] = json!(tools);
}

/// Offers the project tool to a project's conversation, on this Mac or in a VM whose worker
/// accepts it; the Mac answers every call either way. A delegated run works on one task and is
/// not offered it.
async fn offer_project(backend: &Backend, assignment: &Assignment, thread: Option<&str>, in_project: bool, delegated: bool, params: &mut Value) {
    if !in_project || delegated || !backend.is_local() || !backend.machine_carries(assignment, crate::project_tool::TOOL).await { return; }
    if let Some(thread) = thread
        && backend.agents.run_of_thread(thread.to_owned()).await.ok().flatten().is_some_and(|run| run.is_delegated()) { return; }
    let mut tools = params["dynamicTools"].as_array().cloned().unwrap_or_default();
    tools.push(crate::project_tool::tool_definition());
    params["dynamicTools"] = json!(tools);
}

/// Applies pinned instructions during resume without enabling additional tools.
/// `computer_use` leaves the MiniClaw tool out: a chat carries one tool set or the other,
/// and a VM's worker refuses both together.
async fn configure_context(backend: &Backend, id: &str, params: &mut Value,
    computer_use: bool) -> Result<()> {
    let agent = backend.agents.binding(id.to_owned(), false).await?;
    if let Some(agent) = &agent { agent.configure(params); }
    let project = backend.agents.project_context(
        id.to_owned(), agent.as_ref().map(|a| a.id), false).await?;
    // Codex is launched with web search disabled; a conversation on this Mac gets it unless it was
    // switched off. A VM's worker accepts only the bundled policy's values, so a VM conversation
    // keeps search off until the worker learns this one.
    let on_mac = matches!(backend.agents.assignment(id.to_owned()).await, Ok(Some(Assignment::Host {})));
    if !backend.is_claude(id) && on_mac && web_search(backend, id).await.unwrap_or(false) {
        params["config"]["web_search"] = json!("live");
    }
    if !backend.is_claude(id) && on_mac && image_generation(backend, id).await {
        params["config"]["features.image_generation"] = json!(true);
    }
    let assignment = backend.agents.assignment(id.to_owned()).await?;
    if !computer_use
        && let Some(assignment) = &assignment
        && host_tools(backend, assignment, agent.as_ref().map(|agent| agent.id),
            project.as_ref().map(|project| project.project.id)).await {
        params["dynamicTools"] = json!([crate::scoped::tool_definition()]);
    }
    // Computer Use excludes the file and command tool, not these two: the gateway asks about every
    // tool by itself, and a question grants nothing. Both helpers leave a VM conversation alone.
    if let Some(assignment) = &assignment {
        offer_mcp(backend, assignment, params).await;
        offer_ask(backend, assignment, Some(id), false, params).await;
        offer_project(backend, assignment, Some(id), project.is_some(), false, params).await;
    }
    // A VM's worker refuses the delegation tool, and with it the whole conversation, so it is offered on this Mac only.
    if !computer_use && matches!(assignment, Some(Assignment::Host {})) { delegation::offer(backend, agent.as_ref(), Some(id), params).await; }
    // The briefing comes last, so it describes exactly the tools this conversation was given.
    if let Some(assignment) = &assignment {
        let search = matches!(assignment, Assignment::Host {}) && web_search(backend, id).await.unwrap_or(false);
        brief(backend, assignment, project.as_ref(), search, computer_use, params).await;
    }
    if let Some(project) = project { project.configure(params); }
    Ok(())
}

/// Cancels loaded turns whose current agent, skill, or project authorization was revoked.
async fn cancel_revoked(backend: &Backend, data: &crate::chat::Data) {
    for (id, chat) in &data.chats {
        if let Some(turn) = &chat.active
            && backend.agents.admit(id.clone()).await.is_err()
        { backend.cancel(id, turn); }
    }
}

/// Returns project settings while the same account remains authorized.
async fn list_projects(State(backend): State<Shared>) -> Result<Json<Value>> {
    let generation = authorized(&backend).await?;
    let projects = backend.agents.projects().await?;
    if backend.data.lock().await.generation != generation { return Err(unavailable()); }
    Ok(Json(json!({ "projects": projects })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PermissionChange { target: Target, effect: String }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AppRequestInput { name: String }

/// Parses one settings identity without accepting session grants as lasting rules.
fn permission_scope(kind: &str, id: &str) -> Result<Scope> {
    let id = Uuid::parse_str(id).map_err(|_| invalid())?;
    match kind {
        "agent" => Ok(Scope::Agent(id)),
        "project" => Ok(Scope::Project(id)),
        "conversation" => Ok(Scope::Conversation(id)),
        _ => Err(invalid()),
    }
}

/// Returns saved rules and conversation-effective sources to the trusted UI.
async fn permission_settings(State(backend): State<Shared>,
    Path((kind, id)): Path<(String, String)>) -> Result<Json<Value>> {
    let generation = authorized(&backend).await?;
    let scope = permission_scope(&kind, &id)?;
    let mut value = if let Scope::Conversation(_) = scope {
        let context = backend.agents.permission_context(id).await?;
        json!(backend.agents.permission_summary(&context, chrono::Utc::now().timestamp()).await?)
    } else {
        json!({ "rules": backend.agents.permission_rules(scope).await? })
    };
    value["appCatalog"] = json!(vetted_applications());
    value["appRequests"] = json!(backend.agents.application_requests(scope).await?);
    if backend.data.lock().await.generation != generation { return Err(unavailable()); }
    Ok(Json(value))
}

/// Records a user's app review request without changing effective permissions.
async fn request_application(State(backend): State<Shared>,
    Path((kind, id)): Path<(String, String)>, input: Body<AppRequestInput>)
    -> Result<(StatusCode, Json<Value>)> {
    let input = body(input)?;
    let scope = permission_scope(&kind, &id)?;
    let _admission = backend.agent_gate.write().await;
    let generation = authorized(&backend).await?;
    let data = backend.data.lock().await;
    if data.generation != generation { return Err(unavailable()); }
    backend.agents.request_application(scope, &input.name).await?;
    Ok((StatusCode::CREATED, Json(json!({}))))
}

/// Saves one exact setting and cancels active work affected by its removal.
async fn change_permission(State(backend): State<Shared>,
    Path((kind, id)): Path<(String, String)>, input: Body<PermissionChange>)
    -> Result<Json<Value>> {
    let input = body(input)?;
    let scope = permission_scope(&kind, &id)?;
    let effect = match input.effect.as_str() {
        "allow" => Some(Effect::Allow),
        "block" => Some(Effect::Block),
        "remove" => None,
        _ => return Err(invalid()),
    };
    let _admission = backend.agent_gate.write().await;
    let generation = authorized(&backend).await?;
    backend.agents.permission_scope(scope).await?;
    let mut data = backend.data.lock().await;
    if data.generation != generation { return Err(unavailable()); }
    let image_rule = matches!(&input.target, Target::Feature { name } if name == "image_generation");
    let mut image_before = Vec::new();
    if image_rule {
        for (thread, chat) in &data.chats {
            let affected = match scope {
                Scope::Agent(value) if value == crate::main_agent::ID =>
                    chat.summary["agentId"].is_null() || chat.summary["agentId"] == value.to_string(),
                Scope::Agent(value) => chat.summary["agentId"] == value.to_string(),
                Scope::Project(value) => chat.summary["projectId"] == value.to_string(),
                Scope::Conversation(value) => thread == &value.to_string(),
                Scope::Session => false,
            };
            if affected && !backend.is_claude(thread)
                && chat.summary["environment"]["kind"] != "vm" {
                image_before.push((thread.clone(), image_generation(&backend, thread).await));
            }
        }
    }
    if matches!(&input.target, Target::Feature { name } if name == "unsandboxed_shell")
        && matches!(scope, Scope::Conversation(_) | Scope::Session) { return Err(invalid()); }
    backend.agents.set_permission(scope, &input.target, effect).await?;
    if effect != Some(Effect::Allow)
        && matches!(&input.target, Target::Feature { name } if name == "unsandboxed_shell") {
        backend.shell_grants.lock().await.retain(|_, grant| match scope {
            Scope::Agent(agent) => grant.context.agent().unwrap_or(crate::main_agent::ID) != agent,
            Scope::Project(project) => grant.context.project() != Some(project),
            _ => true,
        });
    }
    backend.permission_revision.fetch_add(1, Ordering::AcqRel);
    for (thread, before) in image_before {
        let after = image_generation(&backend, &thread).await;
        if before != after {
            if let Some(chat) = data.chats.get_mut(&thread) {
                chat.image_error = Some("Image access changed. Reload this chat before sending.");
                chat.image_retry_on = Some(after);
                chat.emit("snapshot", chat.snapshot());
            }
        }
    }
    let future_only = matches!(&input.target, Target::Feature { name } if name == "default_full_access");
    if effect != Some(Effect::Allow) && !future_only {
        for (thread, chat) in &mut data.chats {
            let affected = match scope {
                Scope::Agent(value) if value == crate::main_agent::ID =>
                    chat.summary["agentId"].is_null() || chat.summary["agentId"] == value.to_string(),
                Scope::Agent(value) => chat.summary["agentId"] == value.to_string(),
                Scope::Project(value) => chat.summary["projectId"] == value.to_string(),
                Scope::Conversation(value) => thread == &value.to_string(),
                Scope::Session => false,
            };
            if affected {
                chat.stopping = true;
                chat.scoped_approval = None;
                if let Some(turn) = &chat.active { backend.cancel(thread, turn); }
                chat.emit("snapshot", chat.snapshot());
            }
        }
    }
    backend.changed();
    Ok(Json(json!({})))
}

/// Creates an organizational project without granting filesystem access.
async fn add_project(State(backend): State<Shared>, input: Body<projects::Definition>)
    -> Result<(StatusCode, Json<Value>)>
{
    Ok((StatusCode::CREATED, save_project(&backend, None, body(input)?).await?))
}

/// Replaces an observed project revision and enforces removed agent access.
async fn edit_project(State(backend): State<Shared>, Path(id): Path<String>,
    input: Body<projects::Definition>) -> Result<Json<Value>>
{
    save_project(&backend, Some(Uuid::parse_str(&id).map_err(|_| invalid())?), body(input)?).await
}

/// Serializes project edits with turn admission and reports settings-specific conflicts.
async fn save_project(backend: &Backend, id: Option<Uuid>, definition: projects::Definition)
    -> Result<Json<Value>>
{
    let _guard = backend.agent_gate.write().await;
    let generation = authorized(backend).await?;
    // A Mac project's workspace must be a real folder under its resolved path, because
    // the sandbox and the file tool both match resolved paths.
    if let Some(workspace) = &definition.workspace
        && !matches!(definition.environment, Some(ProjectChoice::Vm { .. }))
        && !crate::permissions::exact_folder(workspace) {
        return Err(Error(StatusCode::BAD_REQUEST, "workspace_invalid",
            "Choose an existing folder by its full resolved path.", false));
    }
    if let Some(vm_id) = definition.environment.as_ref().and_then(ProjectChoice::dedicated_id) {
        let first_binding = match id {
            None => true,
            Some(id) => backend.agents.projects().await?.into_iter()
                .find(|project| project.id == id)
                .is_some_and(|project| project.definition.environment.is_none()),
        };
        if first_binding { verify_dedicated_project(backend, vm_id).await?; }
    }
    let data = backend.data.lock().await;
    if data.generation != generation { return Err(unavailable()); }
    let project = backend.agents.save_project(id, definition).await.map_err(|issue| {
        if matches!(issue, Issue::Conflict) {
            Error(StatusCode::CONFLICT, "project_revision_conflict",
                "The project changed. Reload its settings before saving.", false)
        } else { issue.into() }
    })?;
    cancel_revoked(backend, &data).await;
    Ok(Json(json!({ "project": project })))
}

/// Saves a project only while its registered dedicated worker is actually ready.
async fn verify_dedicated_project(backend: &Backend, vm_id: &str) -> Result<()> {
    if !backend.vm_routing || Uuid::parse_str(vm_id).ok()
        .is_none_or(|id| id.to_string() != vm_id) {
        return Err(Issue::VmUnavailable.into());
    }
    let host = backend.host_worker().ok_or_else(vm_control_unavailable)?;
    if !registered_vm(&host, vm_id).await? { return Err(Issue::VmUnavailable.into()); }
    let status = host.call("miniclaw/vm/status", json!({ "vmId": vm_id })).await?;
    if status["state"] != "running" || !dedicated_ready(backend, vm_id).await {
        return Err(Issue::VmUnavailable.into());
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision { revision: u64 }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectDelete {
    revision: u64,
    #[serde(default)] delete_vm: bool,
    confirm_name: Option<String>,
}

/// Finds the project that owns a project VM, if any.
async fn vm_owner(backend: &Backend, vm_id: &str) -> Result<Option<projects::Project>> {
    Ok(backend.agents.projects().await?.into_iter().find(|project|
        project.definition.environment.as_ref()
            .and_then(ProjectChoice::dedicated_id) == Some(vm_id)))
}

/// Deletes one stopped project VM's disk for good and drops its worker link. Master has no
/// identifier of this shape, so it can never be named here.
async fn delete_vm_disk(backend: &Backend, vm_id: &str) -> Result<()> {
    let params = vm_control_params(vm_id)?;
    if vm_id == "master" { return Err(invalid()); }
    let host = backend.host_worker().filter(|_| backend.vm_routing)
        .ok_or_else(vm_control_unavailable)?;
    let status = host.call("miniclaw/vm/status", params.clone()).await?;
    if matches!(status["state"].as_str(), Some("running" | "starting" | "stopping")) {
        return Err(Error(StatusCode::CONFLICT, "vm_running",
            "Shut this VM down before deleting it.", false));
    }
    if status["state"] != "missing" {
        let result = host.call("miniclaw/vm/delete", params).await?;
        if result["state"] != "missing" {
            return Err(Error(StatusCode::BAD_GATEWAY, "vm_delete_failed",
                "The VM could not be deleted. Its project was kept.", false));
        }
    }
    backend.detach_dedicated(vm_id);
    backend.idle.forget(vm_id);
    Ok(())
}

/// Deletes the observed project with its conversations. With `deleteVm` and the typed project
/// name, the project's own stopped VM is deleted first, so a failed VM deletion keeps the
/// project and its conversations.
async fn delete_project(State(backend): State<Shared>, Path(id): Path<String>,
    input: Body<ProjectDelete>) -> Result<Json<Value>>
{
    let input = body(input)?;
    let id = Uuid::parse_str(&id).map_err(|_| invalid())?;
    let _guard = backend.agent_gate.write().await;
    let generation = authorized(&backend).await?;
    let project = backend.agents.projects().await?.into_iter()
        .find(|project| project.id == id).ok_or_else(missing)?;
    if project.definition.revision != Some(input.revision) {
        return Err(Error(StatusCode::CONFLICT, "project_revision_conflict",
            "The project changed. Reload its settings before deleting it.", false));
    }
    let own_vm = project.definition.environment.as_ref()
        .and_then(ProjectChoice::dedicated_id).map(str::to_owned);
    if input.delete_vm {
        if own_vm.is_none() { return Err(invalid()); }
        if input.confirm_name.as_deref() != Some(project.definition.name.as_str()) {
            return Err(Error(StatusCode::BAD_REQUEST, "confirmation_required",
                "Type the project's name to delete its VM.", false));
        }
    }
    // Every conversation is checked before the first one is deleted. One that runs on the VM
    // being deleted needs no provider: its copy goes with the disk.
    let _loading = backend.load_gate.lock().await;
    let mut doomed = Vec::new();
    for thread in backend.agents.project_threads(id).await? {
        let assignment = backend.agents.deletion_assignment(thread.clone()).await?;
        let on_deleted_disk = input.delete_vm && matches!(&assignment,
            Some(Assignment::Vm { vm_id }) if Some(vm_id) == own_vm.as_ref());
        if !on_deleted_disk && thread_client(&backend, &thread, assignment.as_ref()).is_err() {
            return Err(Error(StatusCode::CONFLICT, "conversations_unreachable",
                "Start the VM this project's conversations run in, so they can be deleted, or delete the VM with the project.", false));
        }
        let data = backend.data.lock().await;
        if data.chats.get(&thread).is_some_and(|chat| chat.active.is_some() || chat.sending || chat.uncertain) {
            return Err(busy());
        }
        doomed.push((thread, on_deleted_disk));
    }
    if let (true, Some(vm_id)) = (input.delete_vm, &own_vm) { delete_vm_disk(&backend, vm_id).await?; }
    for (thread, local_only) in doomed {
        remove_conversation(&backend, &thread, generation, local_only).await?;
    }
    let mut data = backend.data.lock().await;
    if data.generation != generation { return Err(unavailable()); }
    backend.agents.remove_project(id, input.revision).await?;
    // Open conversations drop the project's label at once, not after a restart.
    let label = json!(id);
    for chat in data.chats.values_mut().filter(|chat| chat.summary["projectId"] == label) {
        if let Some(summary) = chat.summary.as_object_mut() {
            summary.remove("projectId");
            summary.remove("projectName");
        }
    }
    cancel_revoked(&backend, &data).await;
    Ok(Json(json!({})))
}

/// Deletes a stopped project VM that no project owns any more.
async fn delete_orphaned_vm(State(backend): State<Shared>, Path(vm_id): Path<String>,
    input: Body<Value>) -> Result<Json<Value>>
{
    let _ = input.map_err(|_| invalid())?;
    let _guard = backend.agent_gate.write().await;
    authorized(&backend).await?;
    if vm_id == "master" { return Err(invalid()); }
    if vm_owner(&backend, &vm_id).await?.is_some() {
        return Err(Error(StatusCode::CONFLICT, "vm_owned",
            "This VM belongs to a project. Delete it with its project.", false));
    }
    delete_vm_disk(&backend, &vm_id).await?;
    Ok(Json(json!({})))
}

/// Shows one VM's experimental backup settings and its whole backups.
async fn backup_view(State(backend): State<Shared>, Path(vm_id): Path<String>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    vm_control_params(&vm_id)?;
    let backups = backend.backups.get().ok_or_else(unavailable)?;
    Ok(Json(json!({ "experimental": true, "settings": backups.settings(&vm_id), "backups": backups.list(&vm_id) })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupChoice { schedule: crate::backups::Schedule, keep: u8 }

/// Saves the schedule and how many backups stay. Off is the default and stays until changed.
async fn backup_settings(State(backend): State<Shared>, Path(vm_id): Path<String>, input: Body<BackupChoice>)
    -> Result<Json<Value>> {
    let input = body(input)?;
    authorized(&backend).await?;
    vm_control_params(&vm_id)?;
    let backups = backend.backups.get().ok_or_else(unavailable)?;
    let settings = crate::backups::Settings { schedule: input.schedule, keep: input.keep, ..backups.settings(&vm_id) };
    backups.save(&vm_id, settings).map_err(|error|
        if error.kind() == std::io::ErrorKind::InvalidInput { invalid() } else { unavailable() })?;
    backup_view(State(backend), Path(vm_id)).await
}

/// Starts one backup now. The VM must be running, because its worker reads the files.
async fn backup_run(State(backend): State<Shared>, Path(vm_id): Path<String>, input: Body<Value>)
    -> Result<(StatusCode, Json<Value>)> {
    let _ = input.map_err(|_| invalid())?;
    authorized(&backend).await?;
    desktop_target(&backend, &vm_id).await?;
    if !crate::backups::start(backend.clone(), vm_id, Duration::from_secs(60)) { return Err(busy()); }
    Ok((StatusCode::ACCEPTED, Json(json!({}))))
}

/// Sends a checked backup to the VM's Transfers folder; the Workspace is never overwritten.
async fn backup_restore(State(backend): State<Shared>, Path((vm_id, backup)): Path<(String, String)>,
    input: Body<Value>) -> Result<(StatusCode, Json<Value>)> {
    let _ = input.map_err(|_| invalid())?;
    authorized(&backend).await?;
    let client = desktop_target(&backend, &vm_id).await?;
    let backups = backend.backups.get().ok_or_else(unavailable)?;
    let job = backups.restore(&backend.transfers, crate::transfer_jobs::Remote::Worker(client), &vm_id, &backup).await
        .map_err(|_| Error(StatusCode::CONFLICT, "backup_unusable",
            "This backup is missing or no longer matches its checksums, so it was not restored.", false))?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "jobId": job }))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PickInput { kind: String }

/// Opens the macOS panel for the user and keeps what they chose under an identifier.
async fn transfer_pick(State(backend): State<Shared>, Path(vm_id): Path<String>, input: Body<PickInput>)
    -> Result<Json<Value>> {
    use crate::transfer_jobs::Pick;
    let input = body(input)?;
    authorized(&backend).await?;
    vm_control_params(&vm_id)?;
    let pick = match input.kind.as_str() {
        "files" => Pick::Files, "folders" => Pick::Folders, "destination" => Pick::Destination,
        _ => return Err(invalid()),
    };
    let chosen = backend.picker.get_or_init(Default::default).choose(pick).await
        .map_err(|reason| Error(StatusCode::SERVICE_UNAVAILABLE, "picker_unavailable", reason, false))?;
    Ok(Json(match chosen {
        Some(paths) => backend.selections.keep(pick, paths),
        None => json!({ "cancelled": true }),
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FilesQuery { folder: String, #[serde(default)] path: String }

/// Lists one of the VM's two offered folders for the page's small file list.
async fn transfer_files(State(backend): State<Shared>, Path(vm_id): Path<String>,
    Query(query): Query<FilesQuery>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    let client = desktop_target(&backend, &vm_id).await?;
    let reply = client.call("miniclaw/transfer/list", json!({ "folder": query.folder, "path": query.path })).await
        .map_err(|_| Error(StatusCode::SERVICE_UNAVAILABLE, "transfer_unavailable",
            "This VM's worker does not support file transfer yet. Run miniclaw update-vm.", false))?;
    if reply["refused"].is_string() { return Err(invalid()); }
    Ok(Json(json!({ "entries": reply["entries"] })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TransferSource { folder: String, path: String }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TransferStart { direction: String, selection_id: Uuid, #[serde(default)] sources: Vec<TransferSource> }

/// Starts one transfer from a selection the user made in the macOS panel.
async fn transfer_start(State(backend): State<Shared>, Path(vm_id): Path<String>, input: Body<TransferStart>)
    -> Result<(StatusCode, Json<Value>)> {
    use crate::transfer_jobs::{Remote, Source};
    let input = body(input)?;
    authorized(&backend).await?;
    let client = desktop_target(&backend, &vm_id).await?;
    let expired = || Error(StatusCode::CONFLICT, "selection_expired",
        "Choose the files again; the earlier choice is no longer held.", false);
    let job = match input.direction.as_str() {
        "to_vm" if input.sources.is_empty() => {
            let paths = backend.selections.take(input.selection_id, false).ok_or_else(expired)?;
            backend.transfers.to_vm(Remote::Worker(client), paths,
                chrono::Local::now().format("%Y-%m-%d").to_string())
        }
        "to_mac" if !input.sources.is_empty() && input.sources.len() <= 100 => {
            let destination = backend.selections.take(input.selection_id, true)
                .and_then(|mut paths| paths.pop()).ok_or_else(expired)?;
            backend.transfers.to_mac(Remote::Worker(client), input.sources.into_iter()
                .map(|source| Source { folder: source.folder, path: source.path }).collect(), destination)
        }
        _ => return Err(invalid()),
    };
    backend.idle.touch(&vm_id);
    Ok((StatusCode::ACCEPTED, Json(json!({ "jobId": job }))))
}

/// Reports one transfer's progress, open name conflict or outcome.
async fn transfer_view(State(backend): State<Shared>, Path(job): Path<Uuid>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    backend.transfers.view(job).map(Json).ok_or_else(missing)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConflictAnswer { choice: String, #[serde(default)] all: bool }

/// Answers a transfer's open name conflict.
async fn transfer_resolve(State(backend): State<Shared>, Path(job): Path<Uuid>, input: Body<ConflictAnswer>)
    -> Result<Json<Value>> {
    use crate::transfer_jobs::Choice;
    let input = body(input)?;
    authorized(&backend).await?;
    let choice = match input.choice.as_str() {
        "replace" => Choice::Replace, "keep_both" => Choice::KeepBoth, "skip" => Choice::Skip,
        _ => return Err(invalid()),
    };
    if !backend.transfers.resolve(job, choice, input.all) { return Err(busy()); }
    Ok(Json(json!({})))
}

/// Stops a transfer between two pieces; its partial file is removed.
async fn transfer_cancel(State(backend): State<Shared>, Path(job): Path<Uuid>, input: Body<Value>)
    -> Result<Json<Value>> {
    let _ = input.map_err(|_| invalid())?;
    authorized(&backend).await?;
    if !backend.transfers.cancel(job) { return Err(missing()); }
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdleChoice { minutes: u16 }

/// Lists VMs that are about to be shut down for being idle.
async fn idle_warnings(State(backend): State<Shared>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    Ok(Json(json!({ "warnings": backend.idle.warnings() })))
}

/// Returns one VM's idle limit in minutes and the times the page may offer.
async fn read_idle(State(backend): State<Shared>, Path(vm_id): Path<String>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    vm_control_params(&vm_id)?;
    Ok(Json(json!({ "minutes": backend.idle.minutes(&vm_id), "choices": crate::idle::CHOICES })))
}

/// Saves one VM's idle limit from the offered list; zero switches idle shutdown off.
async fn save_idle(State(backend): State<Shared>, Path(vm_id): Path<String>,
    input: Body<IdleChoice>) -> Result<Json<Value>> {
    let input = body(input)?;
    authorized(&backend).await?;
    vm_control_params(&vm_id)?;
    backend.idle.set_minutes(&vm_id, input.minutes).map_err(|error|
        if error.kind() == std::io::ErrorKind::InvalidInput { invalid() } else { unavailable() })?;
    Ok(Json(json!({ "minutes": input.minutes, "choices": crate::idle::CHOICES })))
}

/// Answers an idle warning: the VM keeps running and its idle period starts again.
async fn keep_running(State(backend): State<Shared>, Path(vm_id): Path<String>,
    input: Body<Value>) -> Result<Json<Value>> {
    let _ = input.map_err(|_| invalid())?;
    authorized(&backend).await?;
    vm_control_params(&vm_id)?;
    backend.idle.touch(&vm_id);
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Power { action: String }

/// Starts, stops or shows one VM because the user pressed its control. A message that needs
/// a stopped VM still goes through the approval routes; this one is the user's own request.
async fn vm_power(State(backend): State<Shared>, Path(vm_id): Path<String>, input: Body<Power>)
    -> Result<Json<Value>>
{
    let input = body(input)?;
    authorized(&backend).await?;
    let params = vm_control_params(&vm_id)?;
    let expected = match input.action.as_str() {
        "start" => &["starting", "running"][..],
        "stop" => &["stopping"],
        "show" => &["running"],
        _ => return Err(invalid()),
    };
    let host = backend.host_worker().filter(|_| backend.vm_routing)
        .ok_or_else(vm_control_unavailable)?;
    backend.idle.touch(&vm_id);
    let status = host.call(&format!("miniclaw/vm/{}", input.action), params).await?;
    if !expected.contains(&status["state"].as_str().unwrap_or_default()) {
        return Err(Error(StatusCode::CONFLICT, "vm_power_failed",
            match status["reason"].as_str() {
                Some("VM resource limit exceeded.") => "VM resource limit exceeded.",
                Some("Assigned VM does not exist.") => "Assigned VM does not exist.",
                Some("Mac VM controller is unavailable.") => "Mac VM controller is unavailable.",
                _ => "The VM did not accept the request.",
            }, false));
    }
    Ok(Json(status))
}

/// Removes the observed agent revision from management and cancels its active work.
async fn delete_agent(State(backend): State<Shared>, Path(id): Path<String>,
    input: Body<Revision>) -> Result<Json<Value>>
{
    let input = body(input)?;
    let id = Uuid::parse_str(&id).map_err(|_| invalid())?;
    let _guard = backend.agent_gate.write().await;
    let generation = authorized(&backend).await?;
    let data = backend.data.lock().await;
    if data.generation != generation { return Err(unavailable()); }
    backend.agents.remove_agent(id, input.revision).await?;
    cancel_revoked(&backend, &data).await;
    Ok(Json(json!({})))
}

/// Deletes the observed owned skill copy, preserving installed sources and chat snapshots.
async fn delete_skill(State(backend): State<Shared>, Path(id): Path<String>,
    input: Body<Revision>) -> Result<Json<Value>>
{
    let input = body(input)?;
    let id = Uuid::parse_str(&id).map_err(|_| invalid())?;
    let _guard = backend.agent_gate.write().await;
    let generation = authorized(&backend).await?;
    let data = backend.data.lock().await;
    if data.generation != generation { return Err(unavailable()); }
    backend.agents.remove_skill(id, input.revision).await.map_err(skill_error)?;
    cancel_revoked(&backend, &data).await;
    Ok(Json(json!({})))
}

/// Deletes one owned idle chat through Codex, then removes its local bindings and cache.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DeleteChat { #[serde(default)] delete_folder: bool }

/// Deletes a conversation. Its own folder under Documents/MiniClaw stays unless the user ticked
/// the box to move it to the Trash too. A folder that could not be moved is kept, and the answer
/// says `folderKept` with its path.
async fn delete_chat(State(backend): State<Shared>, Path(id): Path<String>,
    input: Body<DeleteChat>) -> Result<Json<Value>>
{
    let input = body(input)?;
    tokio::spawn(async move {
        let _admission = backend.agent_gate.write().await;
        let _loading = backend.load_gate.lock().await;
        let generation = authorized_thread(&backend, &id).await?;
        // The folder is checked before anything is deleted, so a refused move deletes nothing.
        if input.delete_folder && let Some(folder) = backend.chat_folders.recorded(&id)
            && std::path::Path::new(&folder).exists()
            && trash_dir(&backend).is_none() {
            return Err(Error(StatusCode::CONFLICT, "folder_kept", "MiniClaw has no Trash to move the folder to.", false));
        }
        remove_conversation(&backend, &id, generation, false).await?;
        let trash = if input.delete_folder { trash_dir(&backend) } else { None };
        // Read before the move: a folder that stays is named in the answer, so the user can find it
        // once the conversation is gone.
        let folder = backend.chat_folders.recorded(&id);
        let folders = backend.clone();
        let removed = tokio::task::spawn_blocking(move || folders.chat_folders.remove(&id, trash.as_deref())).await;
        if input.delete_folder && !matches!(removed, Ok(Ok(()))) {
            if let Ok(Err(error)) = &removed { tracing::warn!("A deleted conversation's folder was kept: {:?}", error.kind()); }
            return Ok(Json(json!({ "folderKept": true, "folder": folder })));
        }
        Ok(Json(json!({})))
    }).await.map_err(|_| unavailable())?
}

/// Deletes one conversation at its provider and in MiniClaw. The caller holds the admission
/// and loading gates. `local_only` skips the provider, for a VM whose disk is being deleted.
async fn remove_conversation(backend: &Shared, id: &str, generation: u64, local_only: bool) -> Result<()> {
    Uuid::parse_str(id).map_err(|_| invalid())?;
    if backend.agents.chat_removed(id.to_owned()).await? { return Ok(()); }
    {
        let data = backend.data.lock().await;
        if let Some(chat) = data.chats.get(id)
            && (chat.active.is_some() || chat.sending || chat.uncertain
                || chat.computer_changing || chat.computer_error.is_some()
                || chat.image_changing || chat.image_error.is_some())
        { return Err(busy()); }
    }
    let assignment = backend.agents.deletion_assignment(id.to_owned()).await?;
    // The VM's disk is being deleted with its project, so its copy of the conversation
    // goes with it; only MiniClaw's own records are left to remove.
    if local_only {
        backend.track_claude(id, false);
        backend.agents.remove_chat(id.to_owned()).await?;
        if let Some(dir) = backend.transcripts.get() { crate::transcripts::remove(dir, id); }
        backend.activity.remove(id);
        backend.terminals.close(id).await;
        if let Some(trash) = trash_dir(backend) { backend.attachments.trash(id, &trash); }
        backend.data.lock().await.chats.remove(id);
        backend.changed();
        return Ok(());
    }
    let (client, provider) = thread_client(backend, id, assignment.as_ref())?;
    let cwd = match &assignment {
        Some(assignment) if provider == "openai" => assigned_cwd(backend, assignment)?,
        _ if provider == "openai" => &backend.cwd,
        _ => backend.local_cwd(),
    };
    let metadata = client.call("thread/read", json!({
        "threadId": id, "includeTurns": false,
    })).await?;
    if history::summary_for(&metadata["thread"], cwd, provider)
        .is_none_or(|s| s["id"] != id) { return Err(missing()); }
    if !matches!(metadata["thread"]["status"]["type"].as_str(), Some("idle" | "notLoaded")) {
        return Err(busy());
    }
    // Codex deletion includes descendants, including archived ones. Do not broaden consent.
    for archived in [false, true] {
        let descendants = client.call("thread/list", json!({
            "ancestorThreadId": id, "archived": archived, "limit": 1,
            "modelProviders": [], "sourceKinds": ["cli", "vscode", "exec", "appServer",
                "subAgent", "subAgentReview", "subAgentCompact", "subAgentThreadSpawn",
                "subAgentOther", "unknown"],
        })).await?;
        if descendants["data"].as_array().is_none_or(|items| !items.is_empty())
            || !descendants["nextCursor"].is_null()
        {
            return Err(Error(StatusCode::CONFLICT, "chat_children",
                "This chat has child tasks. Delete them separately first.", false));
        }
    }
    if backend.data.lock().await.generation != generation { return Err(unavailable()); }
    client.call("thread/delete", json!({ "threadId": id })).await?;
    backend.track_claude(id, false);
    backend.agents.remove_chat(id.to_owned()).await?;
    // A deleted chat leaves no readable copy behind.
    if let Some(dir) = backend.transcripts.get() { crate::transcripts::remove(dir, id); }
    backend.activity.remove(id);
    backend.terminals.close(id).await;
    if let Some(trash) = trash_dir(backend) { backend.attachments.trash(id, &trash); }
    // Runs that lived inside this conversation go with it; a detached run is its own conversation.
    for child in backend.agents.remove_runs(id.to_owned()).await? {
        let _ = Box::pin(remove_conversation(backend, &child, generation, local_only)).await;
    }
    let mut data = backend.data.lock().await;
    data.chats.remove(id);
    backend.changed();
    if data.generation != generation { return Err(unavailable()); }
    Ok(())
}

/// Returns user-defined profiles only while the current account remains authorized.
async fn agents(State(backend): State<Shared>) -> Result<Json<Value>> {
    let generation = authorized(&backend).await?;
    let agents = backend.agents.list().await?;
    if backend.data.lock().await.generation != generation { return Err(unavailable()); }
    Ok(Json(json!({ "agents": agents })))
}

/// Creates an explicit user-defined profile; inactive is the default.
async fn add_agent(State(backend): State<Shared>, input: Body<Definition>)
    -> Result<(StatusCode, Json<Value>)>
{
    let value = save_agent(&backend, None, body(input)?).await?;
    Ok((StatusCode::CREATED, value))
}

/// Replaces the matching profile revision and cancels its active work on deactivation.
async fn edit_agent(State(backend): State<Shared>, Path(id): Path<String>,
    input: Body<Definition>) -> Result<Json<Value>>
{
    let id = Uuid::parse_str(&id).map_err(|_| invalid())?;
    save_agent(&backend, Some(id), body(input)?).await
}

/// Serializes profile changes with run admission and validates model choices before save.
async fn save_agent(backend: &Backend, id: Option<Uuid>, mut definition: Definition)
    -> Result<Json<Value>>
{
    let _guard = backend.agent_gate.write().await;
    let generation = authorized(backend).await?;
    if id.is_none() && definition.revision.is_some() { return Err(invalid()); }
    definition.validate()?;
    let unchanged_model = if let Some(id) = id {
        let old = backend.agents.get(id).await?;
        definition.model == old.definition.model && definition.effort == old.definition.effort && definition.speed == old.definition.speed
    } else { false };
    // Revocation does not depend on model discovery when execution settings are unchanged.
    if definition.active || !unchanged_model {
        let claude = match &definition.model {
            Some(name) => claude_model(backend, name).await,
            None => false,
        };
        let client = if claude { backend.claude_client().ok_or_else(claude_unavailable)? }
            else { control_client(backend)? };
        let choice = choose(&client, &Choice {
            model: definition.model.clone(), effort: definition.effort.clone(), speed: definition.speed.clone(),
        }).await?;
        definition.model = choice.model;
        definition.effort = choice.effort;
        definition.speed = choice.speed;
    }
    let data = backend.data.lock().await;
    if data.generation != generation { return Err(unavailable()); }
    let agent = backend.agents.save(id, definition).await?;
    if !agent.definition.active {
        for chat in data.chats.values() {
            if chat.summary["agentId"] == agent.id.to_string()
                && let (Some(id), Some(turn)) = (chat.summary["id"].as_str(), &chat.active)
            { backend.cancel(id, turn); }
        }
    }
    Ok(Json(json!({ "agent": agent })))
}

/// Maps skill management failures to public messages without leaking source paths.
fn skill_error(issue: Issue) -> Error {
    match issue {
        Issue::Conflict => Error(StatusCode::CONFLICT, "skill_revision_conflict",
            "The skill changed. Reload it before saving.", false),
        Issue::Storage => Error(StatusCode::SERVICE_UNAVAILABLE, "skill_storage_unavailable",
            "Skill storage or import sources are unavailable.", true),
        Issue::Limit => Error(StatusCode::CONFLICT, "skill_storage_limit",
            "Skill storage or import discovery has reached its limit.", false),
        _ => issue.into(),
    }
}

/// Returns the main agent's default model and effort, and its skills and skill groups.
async fn read_main_agent(State(backend): State<Shared>) -> Result<Json<crate::main_agent::Defaults>> {
    authorized(&backend).await?;
    Ok(Json(backend.agents.main_agent().await?))
}

/// Saves the main agent's default model and effort. They are checked against the catalog when
/// a conversation starts, like an agent's, so a model that disappears fails there in words.
async fn save_main_agent(State(backend): State<Shared>, input: Body<crate::main_agent::Change>)
    -> Result<Json<crate::main_agent::Defaults>> {
    let defaults = body(input)?;
    authorized(&backend).await?;
    Ok(Json(backend.agents.save_main_agent(defaults).await?))
}

/// Lists what the composer's slash menu offers: built-in skills and the user's callable ones.
async fn slash_commands(State(backend): State<Shared>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    let skills = backend.agents.skills().await.map_err(skill_error)?;
    Ok(Json(json!({ "commands": crate::slash::commands(&skills) })))
}

/// Returns owned copies after verifying the current account generation.
async fn list_skills(State(backend): State<Shared>) -> Result<Json<Value>> {
    let generation = authorized(&backend).await?;
    let skills = backend.agents.skills().await.map_err(skill_error)?;
    if backend.data.lock().await.generation != generation { return Err(unavailable()); }
    Ok(Json(json!({ "skills": skills })))
}

/// Returns saved groups only while the requesting account remains authorized.
async fn list_groups(State(backend): State<Shared>) -> Result<Json<Value>> {
    let generation = authorized(&backend).await?;
    let groups = backend.agents.groups().await.map_err(group_error)?;
    if backend.data.lock().await.generation != generation { return Err(unavailable()); }
    Ok(Json(json!({ "groups": groups })))
}

/// Creates an independent skill group without activating its member skills globally.
async fn add_group(State(backend): State<Shared>, input: Body<skill_groups::Definition>)
    -> Result<(StatusCode, Json<Value>)>
{
    Ok((StatusCode::CREATED, save_group(&backend, None, body(input)?).await?))
}

/// Updates only the observed group revision; active responses continue unchanged.
async fn edit_group(State(backend): State<Shared>, Path(id): Path<String>,
    input: Body<skill_groups::Definition>) -> Result<Json<Value>>
{
    save_group(&backend, Some(Uuid::parse_str(&id).map_err(|_| invalid())?), body(input)?).await
}

/// Serializes group edits with next-turn resolution, without cancelling current work.
async fn save_group(backend: &Backend, id: Option<Uuid>, definition: skill_groups::Definition)
    -> Result<Json<Value>>
{
    let _guard = backend.agent_gate.write().await;
    let generation = authorized(backend).await?;
    let data = backend.data.lock().await;
    if data.generation != generation { return Err(unavailable()); }
    let group = backend.agents.save_group(id, definition).await.map_err(group_error)?;
    Ok(Json(json!({ "group": group })))
}

/// Deletes a group after revision confirmation; its skills and current responses remain intact.
async fn delete_group(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Revision>)
    -> Result<Json<Value>>
{
    let revision = body(input)?.revision;
    let id = Uuid::parse_str(&id).map_err(|_| invalid())?;
    let _guard = backend.agent_gate.write().await;
    let generation = authorized(&backend).await?;
    let data = backend.data.lock().await;
    if data.generation != generation { return Err(unavailable()); }
    backend.agents.remove_group(id, revision).await.map_err(group_error)?;
    Ok(Json(json!({})))
}

/// Returns explicit skill sources for a loaded, owned conversation.
async fn chat_skills(State(backend): State<Shared>, Path(id): Path<String>) -> Result<Json<Value>> {
    let generation = authorized(&backend).await?;
    known_conversation(&backend, &id).await?;
    let data = backend.data.lock().await;
    if data.generation != generation { return Err(unavailable()); }
    let selection = backend.agents.selection(id).await.map_err(group_error)?;
    Ok(Json(json!({ "selection": selection })))
}

/// Saves conversation selections for the next message without reloading its transcript.
async fn save_chat_skills(State(backend): State<Shared>, Path(id): Path<String>,
    input: Body<skill_groups::Selection>) -> Result<Json<Value>>
{
    let value = body(input)?;
    known_conversation(&backend, &id).await?;
    let _guard = backend.agent_gate.write().await;
    let generation = authorized(&backend).await?;
    let data = backend.data.lock().await;
    if data.generation != generation { return Err(unavailable()); }
    let selection = backend.agents.save_selection(id, value).await.map_err(group_error)?;
    Ok(Json(json!({ "selection": selection })))
}

/// Accepts a conversation that is open, or loads a saved one that nothing has opened since the
/// backend started. A VM conversation whose VM is off cannot be loaded; it is accepted when
/// MiniClaw itself recorded it, because its skill choices are kept on this Mac.
async fn known_conversation(backend: &Shared, id: &str) -> Result<()> {
    if backend.data.lock().await.chats.contains_key(id) { return Ok(()); }
    if load(backend.clone(), id.to_owned()).await.is_ok() { return Ok(()); }
    let recorded = !backend.agents.chat_removed(id.to_owned()).await?
        && matches!(backend.agents.assignment(id.to_owned()).await, Ok(Some(Assignment::Vm { .. })));
    if recorded { Ok(()) } else { Err(missing()) }
}

/// Reports group and selection failures without exposing database details.
fn group_error(issue: Issue) -> Error {
    match issue {
        Issue::Conflict => Error(StatusCode::CONFLICT, "skill_group_conflict",
            "These skill settings changed. Reload them before saving.", false),
        Issue::Limit => Error(StatusCode::CONFLICT, "skill_group_limit",
            "Too many skills or groups selected, or their instructions are too large.", false),
        Issue::Storage => Error(StatusCode::SERVICE_UNAVAILABLE, "skill_group_storage",
            "Skill group storage is unavailable. No message was sent.", false),
        _ => skill_error(issue),
    }
}

/// Lists local import candidates without creating or activating any copies.
async fn skill_sources(State(backend): State<Shared>, query: std::result::Result<Query<SkillSource>, axum::extract::rejection::QueryRejection>)
    -> Result<Json<Value>> {
    let generation = authorized(&backend).await?;
    let Query(query) = query.map_err(|_| invalid())?;
    let sources = skills::sources(query.provider).await.map_err(skill_error)?;
    if backend.data.lock().await.generation != generation { return Err(unavailable()); }
    Ok(Json(json!({ "sources": sources })))
}

/// Creates an owned instruction document through an explicit user request.
async fn add_skill(State(backend): State<Shared>, input: Body<skills::Definition>)
    -> Result<(StatusCode, Json<Value>)>
{
    let generation = authorized(&backend).await?;
    let result = save_skill(&backend, None, body(input)?, generation).await?;
    Ok((StatusCode::CREATED, result))
}

/// Replaces a saved skill revision; deactivation also requests active cancellation.
async fn edit_skill(State(backend): State<Shared>, Path(id): Path<String>,
    input: Body<skills::Definition>) -> Result<Json<Value>>
{
    let generation = authorized(&backend).await?;
    let id = Uuid::parse_str(&id).map_err(|_| invalid())?;
    save_skill(&backend, Some(id), body(input)?, generation).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillSource { #[serde(default)] provider: skills::Provider }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillImport { source: String, #[serde(default)] provider: skills::Provider }

/// Copies the selected SKILL.md as an inactive document without modifying Codex files.
async fn import_skill(State(backend): State<Shared>, input: Body<SkillImport>)
    -> Result<(StatusCode, Json<Value>)>
{
    let generation = authorized(&backend).await?;
    let input = body(input)?;
    let definition = skills::import(input.provider, input.source).await.map_err(skill_error)?;
    let result = save_skill(&backend, None, definition, generation).await?;
    Ok((StatusCode::CREATED, result))
}

/// Serializes skill changes with agent admission and checks pinned chats on revocation.
async fn save_skill(backend: &Backend, id: Option<Uuid>, definition: skills::Definition,
    generation: u64) -> Result<Json<Value>>
{
    let _guard = backend.agent_gate.write().await;
    let data = backend.data.lock().await;
    if data.generation != generation { return Err(unavailable()); }
    let skill = backend.agents.save_skill(id, definition).await.map_err(skill_error)?;
    if !skill.definition.active {
        cancel_revoked(backend, &data).await;
    }
    Ok(Json(json!({ "skill": skill })))
}

/// Accepts an absent or bounded nonempty history cursor; rejects invalid values.
fn check_cursor(cursor: &Option<String>) -> Result<()> {
    if cursor.as_ref().is_some_and(|value| value.is_empty() || value.len() > 4096) {
        return Err(invalid());
    }
    Ok(())
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogQuery {
    #[serde(default)]
    refresh: bool,
}

/// Returns current provider catalogs, optionally bypassing the Claude discovery cache.
async fn catalog(State(backend): State<Shared>, Query(query): Query<CatalogQuery>) -> Result<Json<Value>> {
    let generation = authorized(&backend).await?;
    // Each signed-in provider contributes its catalog; a signed-out one adds nothing.
    let mut models = if codex_authorized(&backend).await.is_ok() {
        read_models(&control_client(&backend)?).await?
    } else { Vec::new() };
    // Only the bridge may label a model as Claude's.
    for model in &mut models { model.provider = None; }
    let names: std::collections::HashSet<_> =
        models.iter().map(|model| model.model.clone()).collect();
    if claude_account(&backend).await.is_some_and(|account| account["status"] == "signed_in") {
        let mut claude = if query.refresh {
            models::refresh_claude(&backend.claude_client().ok_or_else(claude_unavailable)?)
                .await.map_err(|failure| {
                    let mut error = Error::from(failure);
                    error.2 = "Could not load Claude models. Retry the model list.";
                    error
                })?
        } else { claude_models(&backend).await? };
        // Codex keeps its default when both providers are available.
        if !models.is_empty() {
            for model in &mut claude { model.is_default = false; }
        } else if !claude.iter().any(|model| model.is_default)
            && let Some(model) = claude.first_mut() { model.is_default = true; }
        models.extend(claude.into_iter().filter(|model| !names.contains(&model.model)));
    }
    if backend.data.lock().await.generation != generation {
        return Err(unavailable());
    }
    Ok(Json(json!({ "models": models })))
}

/// Returns whether the selected executor has a helper, not whether access is granted.
/// An installed Windows helper is looked up again, and `reason` says why it cannot be used.
async fn computer_status(State(backend): State<Shared>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    let (host, reason) = match backend.local_computer() {
        Some(target) => match target.current().await {
            Ok(_) => (true, None),
            Err(reason) => (false, Some(reason.message())),
        },
        None => (false, None),
    };
    Ok(Json(
        // `host` is the Mac's own helper; each VM reports its helper in its status.
        json!({ "available": host || matches!(backend.computer_helper,
                Some(computer::Target::Guest | computer::Target::Local(_))),
            "host": host, "reason": reason }),
    ))
}

/// Returns the helper to use now for this machine: an installed Windows helper is found again and
/// its signature checked. Refuses with `missing` when no helper is configured, else with the reason.
async fn current_computer(backend: &Backend, assignment: &Assignment, missing: &'static str)
    -> Result<computer::Target>
{
    let target = assigned_computer(backend, assignment).ok_or(Error(StatusCode::SERVICE_UNAVAILABLE,
        "computer_unavailable", missing, false))?;
    target.current().await.map_err(|reason| Error(StatusCode::SERVICE_UNAVAILABLE,
        "computer_unavailable", reason.message(), false))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ComputerMode {
    enabled: bool,
}

/// Changes an idle chat's tool policy and returns its preserved snapshot.
/// Restoration continues even if the browser disconnects during the request.
async fn set_computer(
    State(backend): State<Shared>,
    Path(id): Path<String>,
    input: Body<ComputerMode>,
) -> Result<Json<Value>> {
    let input = body(input)?;
    tokio::spawn(async move {
        let _admission = backend.agent_gate.read().await;
        // Claude chats have no tools, so Computer Use cannot be switched on.
        if backend.is_claude(&id) { return Err(invalid()); }
        let assignment = backend.agents.assignment(id.clone()).await?
            .ok_or_else(machine_choice_required)?;
        assigned_client(&backend, &assignment)?;
        let _chat = load(backend.clone(), id.clone()).await?;
        let generation = codex_authorized(&backend).await?;
        if input.enabled && image_generation(&backend, &id).await {
            return Err(Error(StatusCode::CONFLICT, "image_computer_conflict",
                "Turn off image generation before enabling Computer Use.", false));
        }
        if input.enabled && backend.agents.binding(id.clone(), false).await?.is_some() {
            return Err(agent_access());
        }
        let computer = if input.enabled {
            Some(current_computer(&backend, &assignment, "Computer Use is unavailable on this machine.").await?)
        } else { None };
        let (summary, mut restore_pending) = {
            let mut data = backend.data.lock().await;
            let chat = data.chats.get_mut(&id).ok_or_else(missing)?;
            if chat.active.is_some() || chat.sending || chat.uncertain
                || chat.computer_changing
            {
                return Err(busy());
            }
            if chat.summary["computerUse"] == input.enabled && chat.computer_error.is_none() {
                return Ok(Json(chat.snapshot()));
            }
            if input.enabled && assigned_computer(&backend, &assignment).is_none() {
                return Err(Error(StatusCode::SERVICE_UNAVAILABLE, "computer_unavailable",
                    "The installed Computer Use helper is unavailable.", false));
            }
            chat.computer_changing = true;
            chat.computer_changed();
            (chat.summary.clone(), chat.restore_pending)
        };
        backend.changed();
        let mut modified = restore_pending;
        let mut resuming = false;
        let mut message = "Could not check this chat's computer settings. Try again.";
        // Installed Codex keeps old tools when resuming a loaded task.
        // Archive/unarchive unloads it so resume can apply new tools to the same chat.
        let result = async {
            if restore_pending {
                // A lost reply can hide successful restoration; check before retrying.
                restore_pending = is_archived(&backend, &id).await?;
            }
            if !restore_pending {
                check_switch(&backend, &id).await?;
                let data = backend.data.lock().await;
                if data.generation != generation {
                    return Err(unavailable());
                }
                let chat = data.chats.get(&id).ok_or_else(missing)?;
                if chat.active.is_some() || chat.sending || chat.uncertain {
                    return Err(busy());
                }
                drop(data);
                message = "Could not save the recovery record. Computer settings were not changed.";
                recovery::mark(&backend.recovery, &id).await.map_err(|_| unavailable())?;
                if backend.data.lock().await.generation != generation {
                    let _ = recovery::clear(&backend.recovery, &id).await;
                    return Err(unavailable());
                }
                message = "Could not close this chat's tool session. Retry Computer Use.";
                // An unacknowledged archive may still have moved the saved task.
                modified = true;
                restore_pending = true;
                if let Err(error) = call(&backend, "thread/archive", json!({ "threadId": id })).await {
                    if error.0 == StatusCode::BAD_GATEWAY {
                        modified = false;
                        restore_pending = false;
                        let _ = recovery::clear(&backend.recovery, &id).await;
                    }
                    return Err(error);
                }
            }
            message = "Could not restore this chat. Your messages are kept. Retry Computer Use.";
            // Restore the saved task even if account state changed after archiving.
            let restored = call(&backend, "thread/unarchive", json!({ "threadId": id })).await?;
            if restored["thread"]["id"] != id { return Err(unavailable()); }
            restore_pending = false;
            recovery::clear(&backend.recovery, &id).await.map_err(|_| unavailable())?;
            if backend.data.lock().await.generation != generation {
                return Err(unavailable());
            }
            message = "Could not reload this chat's tools. Your messages are kept. Retry Computer Use.";
            let mut params = resume_params(&backend, &id, &assignment)?;
            // `resume_params` names the VM's folder when VM links exist; a Mac chat resumes
            // in the Mac's folder, as `load` does.
            params["cwd"] = json!(assigned_cwd(&backend, &assignment)?);
            configure_context(&backend, &id, &mut params, input.enabled).await?;
            if let Some(model) = summary["model"].as_str() {
                params["model"] = json!(model);
            }
            if let Some(effort) = summary["effort"].as_str() {
                params["config"]["model_reasoning_effort"] = json!(effort);
            }
            if let Some(speed) = backend.agents.chat_speed(id.clone()).await? {
                params["serviceTier"] = json!(speed);
            }
            if let Some(computer) = &computer {
                computer.configure(&mut params);
            }
            resuming = true;
            let result = call(&backend, "thread/resume", params).await?;
            if history::summary(&result["thread"], thread_cwd(&backend, &id).await?)
                .is_none_or(|value| value["id"] != id)
            {
                return Err(unavailable());
            }
            Ok(())
        }.await;
        let mut data = backend.data.lock().await;
        if data.generation != generation {
            drop(data);
            // A late resume must not leave old-account tools loaded for the next account.
            if resuming && let Ok(client) = assigned_client(&backend, &assignment) {
                client.stop().await;
            }
            return Err(unavailable());
        }
        let chat = data.chats.get_mut(&id).ok_or_else(missing)?;
        chat.computer_changing = false;
        chat.restore_pending = restore_pending;
        if result.is_ok() {
            chat.summary["computerUse"] = json!(input.enabled);
            chat.computer_error = None;
        } else if modified {
            chat.computer_error = Some(message);
        }
        chat.computer_changed();
        backend.changed();
        result.map_err(|mut error| {
            if error.0 != StatusCode::CONFLICT { error.2 = message; }
            error.3 = false;
            error
        })?;
        Ok(Json(chat.snapshot()))
    }).await.map_err(|_| unavailable())?
}

/// Rejects active, foreign, or parent tasks before the archive workaround can alter them.
async fn check_switch(backend: &Backend, id: &str) -> Result<()> {
    let metadata = call(backend, "thread/read", json!({
        "threadId": id, "includeTurns": false,
    })).await?;
    if history::summary(&metadata["thread"], thread_cwd(backend, id).await?)
        .is_none_or(|summary| summary["id"] != id)
    {
        return Err(missing());
    }
    if !matches!(metadata["thread"]["status"]["type"].as_str(), Some("idle" | "notLoaded")) {
        return Err(busy());
    }
    // Codex archive also affects descendants; this toggle must not alter other tasks.
    let descendants = call(backend, "thread/list", json!({
        "ancestorThreadId": id, "archived": false, "limit": 1,
        "modelProviders": [], "sourceKinds": ["cli", "vscode", "exec", "appServer",
            "subAgent", "subAgentReview", "subAgentCompact", "subAgentThreadSpawn",
            "subAgentOther", "unknown"],
    })).await?;
    if descendants["data"].as_array().is_none_or(|items| !items.is_empty())
        || !descendants["nextCursor"].is_null()
    {
        return Err(Error(StatusCode::CONFLICT, "computer_children",
            "Computer Use cannot be switched on a chat with child tasks.", false));
    }
    Ok(())
}

/// Returns normal-chat resume parameters with all inherited computer tools disabled.
fn resume_params(backend: &Backend, id: &str, assignment: &Assignment) -> Result<Value> {
    let config: Value = serde_json::from_str(include_str!("../chat-policy.json")).unwrap();
    let mut params = json!({
        "threadId": id, "cwd": backend.cwd, "excludeTurns": true,
        "approvalPolicy": "never", "approvalsReviewer": "user",
        "sandbox": "read-only", "runtimeWorkspaceRoots": [],
        "baseInstructions": "You are MiniClaw, a helpful conversational assistant. Answer the user directly. No tools are available.",
        "developerInstructions": "", "config": config,
    });
    // A VM worker understands the guest flag; the Mac's own Codex takes the config form.
    let remote = backend.vm_routing && !(backend.is_local() && assignment == &(Assignment::Host {}));
    crate::computer::disable(&mut params, remote).map_err(|_| unavailable())?;
    Ok(params)
}

/// Restores only tasks recorded by an interrupted switch before listing saved chats.
/// Ownership checks run under the current account; computer access remains off.
async fn recover_tasks(backend: &Backend, generation: u64) -> Result<()> {
    let _guard = backend.load_gate.lock().await;
    let recovering = async {
        let ids = recovery::pending(&backend.recovery).await.map_err(|_| unavailable())?;
        for id in ids {
            if backend.data.lock().await.chats.contains_key(&id) { continue; }
            let metadata = call(backend, "thread/read", json!({
                "threadId": id, "includeTurns": false,
            })).await?;
            if history::summary(&metadata["thread"], thread_cwd(backend, &id).await?)
                .is_none_or(|summary| summary["id"] != id)
            {
                return Err(missing());
            }
            let archived = is_archived(backend, &id).await?;
            if backend.data.lock().await.generation != generation {
                return Err(unavailable());
            }
            if archived {
                let restored = call(backend, "thread/unarchive", json!({ "threadId": id })).await?;
                if restored["thread"]["id"] != id { return Err(unavailable()); }
            }
            recovery::clear(&backend.recovery, &id).await.map_err(|_| unavailable())?;
        }
        Ok(())
    }.await;
    recovering.map_err(|mut error: Error| {
        error.1 = "computer_recovery";
        error.2 = "Could not restore a chat after an interrupted tool switch. Check Codex and retry.";
        error
    })
}

/// Checks archived history through Codex, without assuming where its files are stored.
async fn is_archived(backend: &Backend, id: &str) -> Result<bool> {
    let mut cursor: Option<String> = None;
    for _ in 0..100 {
        let page = call(backend, "thread/list", json!({
            "cwd": thread_cwd(backend, id).await?, "modelProviders": ["openai"], "archived": true,
            "sourceKinds": ["cli", "vscode", "exec", "appServer", "subAgent",
                "subAgentReview", "subAgentCompact", "subAgentThreadSpawn", "subAgentOther", "unknown"],
            "limit": 50, "cursor": cursor,
        })).await?;
        let items = page["data"].as_array().filter(|items| items.len() <= 50)
            .ok_or_else(unavailable)?;
        if items.iter().any(|item| item["id"] == id) { return Ok(true); }
        let next = history::cursor(&page["nextCursor"])?;
        if next.is_none() { return Ok(false); }
        if next == cursor { return Err(unavailable()); }
        cursor = next;
    }
    Err(unavailable())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Create {
    #[serde(default)]
    capabilities: std::collections::BTreeMap<String, bool>,
    skill_selection: Option<skill_groups::Selection>,
    project_id: Option<Uuid>,
    agent_id: Option<Uuid>,
    model: Option<String>,
    effort: Option<String>,
    /// A speed tier of the model, such as Codex's Fast; none means standard.
    speed: Option<String>,
    #[serde(default)]
    computer_use: bool,
    environment: Option<ConversationChoice>,
    /// Set only in-process, for a delegated run; a request can never name it.
    #[serde(skip)]
    delegated: bool,
}

/// Loads a provider catalog or returns a catalog-specific error for the UI.
async fn read_models(client: &crate::rpc::Client) -> Result<Vec<Model>> {
    models::read(client).await.map_err(|failure| {
        let mut error = Error::from(failure);
        error.2 = "Could not load provider models. Retry the model list.";
        error.3 = false;
        error
    })
}

/// Validates the requested model and effort, returning the model's default effort
/// when omitted. No model selection leaves Codex's defaults in place.
async fn choose(client: &crate::rpc::Client, choice: &Choice) -> Result<Choice> {
    // Use one error for missing models and unsupported effort choices.
    let invalid_choice = || {
        Error(
            StatusCode::BAD_REQUEST,
            "invalid_model",
            "Choose an available model and supported reasoning effort.",
            false,
        )
    };
    let Some(name) = &choice.model else {
        return if choice.effort.is_none() && choice.speed.is_none() {
            Ok(choice.clone())
        } else {
            Err(invalid_choice())
        };
    };
    let catalog = read_models(client).await?;
    let model = catalog
        .iter()
        .find(|model| &model.model == name)
        .ok_or_else(invalid_choice)?;
    let effort = choice.effort.as_ref().or_else(|| {
        (!model.supported_reasoning_efforts.is_empty())
            .then_some(&model.default_reasoning_effort)
    });
    if effort.is_some_and(|value| {
        !model
            .supported_reasoning_efforts
            .iter()
            .any(|option| &option.reasoning_effort == value)
    }) {
        return Err(invalid_choice());
    }
    // A speed is one of the tiers the model advertises; a model with none has only standard.
    if choice.speed.as_ref().is_some_and(|value| !model.service_tiers.iter().any(|tier| &tier.id == value)) {
        return Err(invalid_choice());
    }
    Ok(Choice {
        model: Some(name.clone()),
        effort: effort.cloned(),
        speed: choice.speed.clone(),
    })
}

/// Starts and caches a chat with validated settings and explicit tool policy.
/// Returns 201 with the task summary and settings accepted by Codex.
async fn create(
    State(backend): State<Shared>,
    input: Body<Create>,
) -> Result<(StatusCode, Json<Value>)> {
    let input = body(input)?;
    tokio::spawn(create_chat(backend, input)).await.map_err(|_| unavailable())?
}

/// Completes the creation receipt even if the browser disconnects while Codex replies.
async fn create_chat(backend: Shared, input: Create) -> Result<(StatusCode, Json<Value>)> {
    let _guard = backend.create_gate.lock().await;
    let _agents = backend.agent_gate.read().await;
    let _loading = backend.load_gate.lock().await;
    let generation = authorized(&backend).await?;
    let choice = input.environment.clone().ok_or_else(invalid)?;
    let project = if let Some(id) = input.project_id {
        Some(backend.agents.prepare_project(id, input.agent_id).await?)
    } else { None };
    if project.as_ref().is_some_and(|p| p.project.definition.environment.is_none()) {
        return Err(machine_choice_required());
    }
    let assignment = Assignment::for_project(choice, project.as_ref()
        .and_then(|context| context.project.definition.environment.as_ref()));
    if let Some(selection) = &input.skill_selection {
        backend.agents.check_selection(selection.clone()).await.map_err(group_error)?;
    }
    let agent = if let Some(id) = input.agent_id {
        let agent = backend.agents.prepare(id).await?;
        if !agent.definition.active { return Err(Issue::Inactive.into()); }
        if !agent.definition.supported() { return Err(Issue::Unsupported.into()); }
        if input.computer_use { return Err(agent_access()); }
        Some(agent)
    } else { None };
    // A named agent brings its own model; a conversation without one follows the main agent.
    let (default_model, default_effort, default_speed) = match &agent {
        Some(agent) => (agent.definition.model.clone(), agent.definition.effort.clone(), agent.definition.speed.clone()),
        None if input.delegated => (None, None, None),
        None => { let main = backend.agents.main_agent().await?; (main.model, main.effort, main.speed) }
    };
    let model = input.model.clone().or(default_model);
    let effort = input.effort.clone().or_else(|| if input.model.is_none() { default_effort } else { None });
    let speed = input.speed.clone().or_else(|| if input.model.is_none() { default_speed } else { None });
    // The chosen model pins the chat to the Claude bridge or to a Codex worker.
    let claude = match &model {
        Some(name) => claude_model(&backend, name).await,
        None => false,
    };
    let initial_tools = capabilities::prepare(&backend, &input.capabilities, input.agent_id,
        input.project_id, claude, assignment != (Assignment::Host {})).await?;
    if input.computer_use && initial_tools.iter().any(|target|
        matches!(target, Target::Feature { name } if name == "image_generation")) {
        return Err(Error(StatusCode::CONFLICT, "image_computer_conflict",
            "Turn off Computer Use to start a chat with image generation.", false));
    }
    let client = if claude {
        if input.computer_use || assignment != (Assignment::Host {}) { return Err(invalid()); }
        let client = backend.claude_client().ok_or_else(claude_unavailable)?;
        claude_authorized(&client).await?;
        client
    } else {
        codex_authorized(&backend).await?;
        assigned_client(&backend, &assignment)?
    };
    // The bridge shares the host chat workspace and needs no Codex worker.
    let cwd = if claude { backend.local_cwd() } else { assigned_cwd(&backend, &assignment)? };
    let provider = if claude { crate::claude::PROVIDER } else { "openai" };
    let choice = choose(
        &client,
        &Choice {
            model,
            effort,
            speed,
        },
    )
    .await?;
    let computer = if input.computer_use {
        Some(current_computer(&backend, &assignment, "The installed Computer Use helper is unavailable.").await?)
    } else { None };
    if !backend.data.lock().await.make_room() {
        return Err(task_limit());
    }
    let mut params = json!({
        "cwd": cwd, "approvalPolicy": "never", "sandbox": "read-only",
        "environments": [], "selectedCapabilityRoots": [],
        "baseInstructions": "You are MiniClaw, a helpful conversational assistant. Answer the user directly. No tools are available.",
    });
    if let Some(model) = choice.model {
        params["model"] = json!(model);
        params["allowProviderModelFallback"] = json!(false);
    }
    if let Some(effort) = choice.effort {
        params["config"] = json!({ "model_reasoning_effort": effort });
    }
    // Codex takes the speed on the thread and on each turn; the thread does not keep it.
    if let Some(speed) = &choice.speed {
        params["serviceTier"] = json!(speed);
    }
    if let Some(computer) = &computer {
        computer.configure(&mut params);
    }
    // Codex takes configuration only when a thread starts or is resumed, so its web search
    // follows the web search switch at those two moments. Search is on unless switched off. Nothing else in the policy moves.
    let search = assignment == (Assignment::Host {})
        && !initial_tools.iter().any(|target| matches!(target, Target::Feature { name } if name == "no_web_search"))
        && !backend.agents.feature_default(input.agent_id, input.project_id, "no_web_search").await?;
    if !claude && search {
        params["config"]["web_search"] = json!("live");
    }
    if !claude && assignment == (Assignment::Host {}) && initial_tools.iter().any(|target|
        matches!(target, Target::Feature { name } if name == "image_generation")) {
        params["config"]["features.image_generation"] = json!(true);
    }
    if let Some(agent) = &agent { agent.configure(&mut params); }
    if !input.computer_use
        && host_tools(&backend, &assignment, input.agent_id, input.project_id).await {
        params["dynamicTools"] = json!([crate::scoped::tool_definition()]);
    }
    // Computer Use excludes the file and command tool, not these two: the gateway asks about every
    // tool by itself, and a question grants nothing. Both helpers leave a VM conversation alone.
    offer_mcp(&backend, &assignment, &mut params).await;
    offer_ask(&backend, &assignment, None, input.delegated, &mut params).await;
    offer_project(&backend, &assignment, None, project.is_some(), input.delegated, &mut params).await;
    if !input.delegated && matches!(assignment, Assignment::Host {}) { delegation::offer(&backend, agent.as_ref(), None, &mut params).await; }
    // The briefing comes last, so it describes exactly the tools this conversation was given.
    brief(&backend, &assignment, project.as_ref(), search, input.computer_use, &mut params).await;
    if let Some(project) = &project { project.configure(&mut params); }
    if backend.data.lock().await.generation != generation {
        return Err(unavailable());
    }
    let mut baseline = Vec::new();
    let mut cursor = None;
    let mut cursors = std::collections::HashSet::new();
    loop {
        let page = history::list_for(&client, cwd, cursor.as_deref(), provider).await?;
        baseline.extend(page["threads"].as_array().unwrap().iter()
            .filter_map(|t| t["id"].as_str().map(str::to_owned)));
        if baseline.len() > 4096 { return Err(Issue::Limit.into()); }
        cursor = history::cursor(&page["nextCursor"])?;
        if cursor.is_none() { break; }
        if !cursors.insert(cursor.clone()) || cursors.len() >= 128 {
            return Err(unavailable());
        }
    }
    baseline.extend(backend.data.lock().await.chats.keys().cloned());
    let intent = Uuid::new_v4().to_string();
    backend.agents.reserve(intent.clone(), assignment.clone(), agent.clone(), project.clone(),
        input.skill_selection, initial_tools, baseline).await?;
    if backend.data.lock().await.generation != generation {
        backend.agents.cancel_creation(intent).await?;
        return Err(unavailable());
    }
    // Even an error may follow partial upstream creation. Keep its admission fence.
    let result = client.call("thread/start", params).await?;
    let id = result["thread"]["id"].as_str().ok_or_else(unavailable)?;
    if claude { backend.track_claude(id, true); }
    backend.agents.created(intent, id.to_owned()).await?;
    backend.agents.check_creation(id.to_owned()).await?;
    if choice.speed.is_some() { backend.agents.set_chat_speed(id.to_owned(), choice.speed.clone()).await?; }
    let mut summary = json!({ "id": id, "name": "New chat",
        "model": result["model"].as_str(),
        "effort": result["reasoningEffort"].as_str(),
        "speed": choice.speed,
        "createdAt": chrono::Utc::now().timestamp_millis(),
        "computerUse": input.computer_use,
    });
    let mut data = backend.data.lock().await;
    if data.generation != generation {
        return Err(unavailable());
    }
    if claude { summary["provider"] = json!("claude"); }
    if let Some(agent) = &agent {
        agent.tag(&mut summary);
    }
    if let Some(project) = &project { project.tag(&mut summary); }
    summary["environment"] = json!(assignment.choice());
    if let Assignment::Vm { vm_id } = assignment { summary["vmId"] = json!(vm_id); }
    data.chats.insert(id.into(), Chat::new(summary.clone()));
    Ok((StatusCode::CREATED, Json(json!({ "thread": summary }))))
}

/// Loads the requested chat if needed and returns its current messages and state.
async fn snapshot(
    State(backend): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    if backend.vm_routing
        && let Some(Assignment::Vm { vm_id }) = backend.agents.assignment(id.clone()).await?
        && assigned_client(&backend, &Assignment::Vm { vm_id: vm_id.clone() }).is_err() {
        authorized(&backend).await?;
        if backend.agents.chat_removed(id.clone()).await? { return Err(missing()); }
        // The Mac's copy of this chat stands in for the VM's history while it is off.
        let kept = backend.transcripts.get()
            .and_then(|dir| crate::transcripts::load(dir, &id));
        let (mut summary, messages) = match kept {
            Some((mut thread, messages)) => { thread["vmOffline"] = json!(true); (thread, messages) }
            None => (offline_summary(&id, &vm_id), json!([])),
        };
        tag_context(&backend, &id, &mut summary).await?;
        return Ok(Json(json!({ "thread": summary, "messages": messages,
            "activeTurn": null, "pending": false, "vmOffline": true,
            "activities": backend.activity.list(&id) })))
    }
    let _chat = load(backend.clone(), id.clone()).await?;
    let data = backend.data.lock().await;
    let chat = data.chats.get(&id).ok_or_else(missing)?;
    // An idle chat is copied when it is read, which also covers chats from before the copy existed.
    if chat.active.is_none() && !chat.sending { backend.keep_transcript(&id, chat); }
    let mut snapshot = chat.snapshot();
    drop(data);
    snapshot["activities"] = json!(backend.activity.list(&id));
    snapshot["queue"] = json!(backend.agents.queue(id.clone()).await.unwrap_or_default());
    delegation::describe(&backend, &id, &mut snapshot).await;
    // The composer shows whether this conversation reviews every change.
    // The composer also shows full access, so nobody misses that nothing will ask.
    let switched = |name: &str| { let backend = backend.clone(); let id = id.clone(); let name = name.to_owned(); async move {
        match backend.agents.permission_context(id).await {
            Ok(context) => matches!(backend.agents.permission(&context, &crate::permissions::Target::Feature { name }, chrono::Utc::now().timestamp()).await,
                Ok(crate::permissions::Decision::Allow(_))),
            Err(_) => false,
        } } };
    snapshot["codingSession"] = json!(switched("coding_session").await);
    snapshot["fullAccess"] = json!(switched("full_access").await);
    // A conversation with no project and no agent shows its own folder once it has one.
    snapshot["folder"] = json!(backend.chat_folders.recorded(&id));
    Ok(Json(snapshot))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Activate {}

/// Ensures a saved chat is loaded and subscribed, then returns an empty acknowledgement.
async fn activate(
    State(backend): State<Shared>,
    Path(id): Path<String>,
    input: Body<Activate>,
) -> Result<Json<Value>> {
    authorized(&backend).await?;
    body(input)?;
    let _chat = load(backend, id).await?;
    Ok(Json(json!({})))
}

/// Loads a permitted saved chat under normal-chat policy and publishes its history
/// with buffered events. Returns a cache lease; rejects unfinished or invalid history.
async fn load(backend: Shared, id: String) -> Result<Arc<()>> {
    // Finish hydration even if its HTTP caller disconnects.
    tokio::spawn(async move {
        let _guard = backend.load_gate.lock().await;
        let generation = authorized_thread(&backend, &id).await?;
        if Uuid::parse_str(&id).is_err() {
            return Err(missing());
        }
        if backend.agents.chat_removed(id.clone()).await? { return Err(missing()); }
        let assignment = backend.agents.assignment(id.clone()).await?;
        let (client, provider) = thread_client(&backend, &id, assignment.as_ref())?;
        let cwd = match &assignment {
            Some(assignment) if provider == "openai" => assigned_cwd(&backend, assignment)?,
            _ if provider == "openai" => backend.cwd.as_str(),
            _ => backend.local_cwd(),
        };
        {
            let mut data = backend.data.lock().await;
            if let Some(chat) = data.chats.get_mut(&id) {
                return Ok(chat.pin());
            }
        }
        let metadata = call(&backend, "thread/read", json!({
            "threadId": id, "includeTurns": false,
        })).await.map_err(|error| {
            if error.0 == StatusCode::BAD_GATEWAY { missing() } else { error }
        })?;
        let mut summary = history::summary_for(&metadata["thread"], cwd, provider)
            .filter(|summary| summary["id"] == id).ok_or_else(missing)?;
        tag_context(&backend, &id, &mut summary).await?;
        {
            let mut data = backend.data.lock().await;
            if data.generation != generation {
                return Err(unavailable());
            }
            if !data.make_room() {
                return Err(task_limit());
            }
            data.loading.insert(id.clone(), history::Loading::default());
        }
        let loaded = async {
            // Resume with normal-chat policy, even if the saved task used computer tools.
            let mut params = match &assignment {
                Some(assignment) => resume_params(&backend, &id, assignment)?,
                None => resume_params(&backend, &id, &Assignment::Host {})?,
            };
            params["cwd"] = json!(cwd);
            configure_context(&backend, &id, &mut params, false).await?;
            let result = call(&backend, "thread/resume", params).await?;
            if history::summary_for(&result["thread"], cwd, provider)
                .is_none_or(|value| value["id"] != id)
            {
                return Err(missing());
            }
            summary["model"] = json!(result["model"].as_str());
            summary["effort"] = json!(result["reasoningEffort"].as_str());
            let page = history::page(&client, &id, None, &backend.attachments).await?;
            let mut chat = Chat::new(summary);
            chat.history_loaded = true;
            // A turn may still be running. Its saved partial text and the live deltas share no
            // cursor, so deltas for those messages are ignored until the whole message arrives.
            // The text is then shown once, never doubled.
            if page.active.is_some() {
                chat.frozen = page.messages.iter().filter(|message| message["role"] == "assistant"
                    && message["complete"] != true).filter_map(|message| message["id"].as_str().map(str::to_owned)).collect();
            }
            chat.messages = page.messages;
            attach(&backend, &id, &mut chat.messages);
            chat.active = page.active;
            chat.finished = page.finished;
            chat.history_cursor = page.cursor;
            Ok::<_, Error>(chat)
        }.await;
        let mut data = backend.data.lock().await;
        let pending = data.loading.remove(&id);
        if data.generation != generation {
            return Err(unavailable());
        }
        let mut chat = loaded?;
        let pending = pending.ok_or_else(unavailable)?;
        if pending.overflow {
            return Err(unavailable());
        }
        // Publish history and buffered notifications together.
        for frame in pending.frames {
            chat.notify(frame["method"].as_str().unwrap_or_default(), &frame["params"]);
        }
        let lease = chat.pin();
        data.chats.insert(id, chat);
        backend.changed();
        Ok(lease)
    }).await.map_err(|_| unavailable())?
}

/// Pins an older conversation to its existing worker and returns that choice.
async fn assign_existing_machine(
    State(backend): State<Shared>,
    Path(id): Path<String>,
    input: Body<ConversationChoice>,
) -> Result<Json<Value>> {
    let choice = body(input)?;
    let agent = backend.agents.binding(id.clone(), false).await?;
    let project = backend.agents.project_context(id.clone(), agent.map(|a| a.id), false).await?;
    if project.as_ref().is_some_and(|context| context.project.definition.environment.is_none()) {
        return Err(machine_choice_required());
    }
    let assignment = Assignment::for_project(choice, project.as_ref()
        .and_then(|context| context.project.definition.environment.as_ref()));
    let old = backend.agents.assignment(id.clone()).await?;
    if old.as_ref().is_some_and(|old| old != &assignment) {
        return Err(Issue::ConversationLocked.into());
    }
    let client = assigned_client(&backend, &assignment)?;
    let cwd = assigned_cwd(&backend, &assignment)?;
    let _agents = backend.agent_gate.read().await;
    if old.is_none() {
        let metadata = client.call("thread/read", json!({
            "threadId": id, "includeTurns": false,
        })).await.map_err(|_| missing())?;
        if history::summary(&metadata["thread"], cwd)
            .is_none_or(|summary| summary["id"] != id)
        { return Err(missing()); }
    }
    let assignment = backend.agents.assign_existing(id.clone(), assignment).await?;
    let _chat = load(backend.clone(), id.clone()).await?;
    let mut data = backend.data.lock().await;
    let chat = data.chats.get_mut(&id).ok_or_else(missing)?;
    chat.summary["environment"] = json!(assignment.choice());
    chat.summary.as_object_mut().unwrap().remove("environmentRequired");
    if let Assignment::Vm { vm_id } = &assignment { chat.summary["vmId"] = json!(vm_id); }
    chat.emit("snapshot", chat.snapshot());
    drop(data);
    backend.changed();
    Ok(Json(json!({ "environment": assignment.choice() })))
}

/// Returns a page of text-only history and its next cursor for an authorized chat.
async fn older(
    State(backend): State<Shared>,
    Path(id): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Value>> {
    let _chat = load(backend.clone(), id.clone()).await?;
    check_cursor(&query.cursor)?;
    let generation = authorized_thread(&backend, &id).await?;
    let assignment = backend.agents.assignment(id.clone()).await?;
    let (client, _) = thread_client(&backend, &id, assignment.as_ref())?;
    let mut page = history::page(&client, &id, query.cursor.as_deref(), &backend.attachments).await?;
    attach(&backend, &id, &mut page.messages);
    if backend.data.lock().await.generation != generation {
        return Err(unavailable());
    }
    Ok(Json(json!({ "messages": page.messages, "nextCursor": page.cursor })))
}

/// Returns the instance's recoverable deletion location. `MINICLAW_TRASH_DIR` replaces it in every
/// build, as it has on the Mac since before the Windows port; tests, including those run against an
/// installed build, use it, and README lists it.
fn trash_dir(backend: &Backend) -> Option<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("MINICLAW_TRASH_DIR") { return Some(path.into()); }
    // Linux has no Trash every desktop agrees on, so it keeps the Windows folder beside the data.
    #[cfg(any(windows, target_os = "linux"))]
    { backend.attachments.dir.get().and_then(|path| path.parent()).map(|data| data.join("Recovered files")) }
    #[cfg(all(unix, not(target_os = "linux")))]
    { let _ = backend; Some(std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".Trash")) }
}

/// Adds to each user message of a reloaded conversation the uploads it carried, found by turn.
fn attach(backend: &Backend, thread: &str, messages: &mut [Value]) {
    let found = backend.attachments.by_turn(thread);
    if found.is_empty() { return; }
    for message in messages.iter_mut().filter(|message| message["role"] == "user") {
        if let Some(uploads) = message["turnId"].as_str().and_then(|turn| found.get(turn)) {
            message["attachments"] = json!(uploads);
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Send {
    text: String,
    client_message_id: Uuid,
    model: Option<String>,
    effort: Option<String>,
    /// A speed tier of the model; none means standard.
    speed: Option<String>,
    /// Uploads of this conversation that travel with the message.
    #[serde(default)]
    attachments: Vec<Uuid>,
}

/// Returns a prior acknowledgement for an identical send, or None for a new ID.
/// Reused IDs with different input or an unconfirmed outcome return a conflict.
fn receipt(
    chat: &Chat,
    input: &Send,
    choice: &Choice,
) -> Result<Option<(StatusCode, Json<Value>)>> {
    let Some(receipt) = chat.receipts.get(&input.client_message_id.to_string()) else {
        return Ok(None);
    };
    if receipt.text != input.text || &receipt.choice != choice || receipt.attachments != input.attachments {
        return Err(busy());
    }
    let turn = receipt.turn.as_ref().ok_or_else(busy)?;
    Ok(Some((
        StatusCode::ACCEPTED,
        Json(json!({ "turnId": turn })),
    )))
}

/// Validates and reserves a chat, then starts one response or reuses its receipt.
/// Returns 202 with the turn ID; connection failures can leave the outcome unknown.
async fn send(
    State(backend): State<Shared>,
    Path(id): Path<String>,
    input: Body<Send>,
) -> Result<(StatusCode, Json<Value>)> {
    let _agents = backend.agent_gate.read().await;
    let generation = authorized_thread(&backend, &id).await?;
    let input = body(input)?;
    if input.text.trim().is_empty() && input.attachments.is_empty() {
        return Err(invalid());
    }
    if input.text.len() > 32768 {
        return Err(Error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "message_too_long",
            "Messages must be at most 32 KiB.",
            false,
        ));
    }
    let client_id = input.client_message_id.to_string();
    let requested = Choice {
        model: input.model.clone(),
        effort: input.effort.clone(),
        speed: input.speed.clone(),
    };
    // A conversation that left the cache still answers a retried send from its kept receipt.
    {
        let data = backend.data.lock().await;
        if !data.chats.contains_key(&id)
            && let Some((_, _, kept)) = data.kept_receipts.iter().find(|(thread, client, _)| *thread == id && *client == client_id) {
            if kept.text != input.text || kept.choice != requested || kept.attachments != input.attachments { return Err(busy()); }
            return Ok((StatusCode::ACCEPTED, Json(json!({ "turnId": kept.turn.as_ref().ok_or_else(busy)? }))));
        }
    }
    // Return an existing receipt even if the model catalog is now unavailable.
    let _chat = {
        let mut data = backend.data.lock().await;
        let chat = data.chats.get_mut(&id).ok_or_else(missing)?;
        if let Some(reply) = receipt(chat, &input, &requested)? {
            return Ok(reply);
        }
        if chat.active.is_some() || chat.sending || chat.uncertain
            || chat.computer_changing || chat.computer_error.is_some()
            || chat.image_changing || chat.image_error.is_some()
        {
            return Err(busy());
        }
        chat.pin()
    };
    let assignment = backend.agents.assignment(id.clone()).await?
        .ok_or_else(machine_choice_required)?;
    let (client, _) = thread_client(&backend, &id, Some(&assignment))?;
    backend.agents.admit(id.clone()).await?;
    let mut uploads = attachments::named(&backend, &id, &input.attachments, input.client_message_id)?;
    let (pictures, attached) = attachments::deliver(&backend, &id, &mut uploads).await?;
    let instructions = backend.agents.turn_skills(id.clone()).await.map_err(group_error)?;
    // A message that starts with a slash command carries that skill for this turn only. It travels
    // under its own key: the real Codex keeps an unchanged key as one copy and adds a changed key
    // again in full (tests/experiments/skill-context-cost.test.mjs), so a one-turn extra must not
    // make the conversation's own skills block look changed.
    // Which skills took part in this turn, for the Analytics tab. Names only, never their text.
    let mut skill_uses: Vec<crate::analytics::SkillUse> = Vec::new();
    {
        let use_of = |name: &str, source: &str| crate::analytics::SkillUse { thread: id.clone(), turn: String::new(),
            name: name.to_owned(), source: source.to_owned() };
        if let Ok(Some(agent)) = backend.agents.binding(id.clone(), false).await {
            skill_uses.extend(agent.skills.iter().map(|skill| use_of(&skill.definition.name, "agent")));
        }
        if let Ok(Some(project)) = backend.agents.project_context(id.clone(), None, false).await {
            skill_uses.extend(project.skills.iter().map(|skill| use_of(&skill.definition.name, "project")));
        }
        // The conversation's own skills, its groups and the main agent's reach the turn as text.
        for line in instructions.iter().flat_map(|text| text.lines()) {
            if let Some(name) = line.strip_prefix("## Skill: ") { skill_uses.push(use_of(name.trim(), "conversation")); }
        }
    }
    let called = match input.text.starts_with('/') {
        true => {
            // The creating commands are told which agents and models exist; the others need neither.
            let creating = input.text.starts_with("/agent-create");
            let agents = if creating { backend.agents.list().await.unwrap_or_default() } else { Vec::new() };
            let models = if !creating { Vec::new() } else {
                catalog(State(backend.clone()), Query(CatalogQuery::default())).await.ok().and_then(|Json(found)| found["models"].as_array().cloned()).unwrap_or_default()
                    .iter().filter(|model| model["hidden"] != true).filter_map(|model| Some((model["model"].as_str()?.to_owned(),
                        model["supportedReasoningEfforts"].as_array().into_iter().flatten()
                            .filter_map(|effort| effort["reasoningEffort"].as_str().map(str::to_owned)).collect()))).collect()
            };
            crate::slash::section(&input.text, &backend.agents.skills().await.map_err(skill_error)?,
                &crate::slash::Inventory { agents: &agents, models })
        }
        false => None,
    };
    let choice = choose(&client, &requested).await?;
    // Catalog lookup released the lock; another send may have claimed this chat.
    {
        let mut data = backend.data.lock().await;
        if data.generation != generation {
            return Err(unavailable());
        }
        let chat = data.chats.get_mut(&id).ok_or_else(missing)?;
        if let Some(reply) = receipt(chat, &input, &requested)? {
            return Ok(reply);
        }
        if chat.active.is_some() || chat.sending || chat.uncertain
            || chat.computer_changing || chat.computer_error.is_some()
            || chat.image_changing || chat.image_error.is_some()
        {
            return Err(busy());
        }
        if chat.receipts.len() >= 128 {
            return Err(Error(
                StatusCode::CONFLICT,
                "chat_limit",
                "Start a new chat to continue.",
                false,
            ));
        }
        // Reserve the chat before releasing the lock for turn/start.
        backend.attachments.claim(&id, &mut uploads, input.client_message_id).map_err(|_| unavailable())?;
        chat.sending = true;
        chat.stopping = false;
        chat.receipts.insert(
            client_id.clone(),
            Receipt {
                text: input.text.clone(),
                choice: requested,
                turn: None,
                attachments: input.attachments.clone(),
            },
        );
        let mut message = json!({ "id": client_id, "role": "user", "text": input.text, "complete": true });
        if !uploads.is_empty() {
            message["attachments"] = uploads.iter().map(|record| record.view()).collect();
        }
        chat.messages.push(message);
        chat.bound_messages();
        // The first message names a conversation, under the user's own name if there is one.
        let unnamed = if chat.summary["providerName"].is_string() { "providerName" } else { "name" };
        if chat.summary[unnamed] == "New chat" {
            let title = if input.text.trim().is_empty() { uploads[0].name.clone() } else { input.text.clone() };
            chat.summary[unnamed] = json!(title.chars().take(60).collect::<String>());
        }
        chat.emit("snapshot", chat.snapshot());
    }
    backend.changed();
    // Results of background delegations reach the agent with this message, once, under their own key.
    let delivered = delegation::results_context(&backend, &id).await;
    let mut params = json!({
            "threadId": id, "clientUserMessageId": client_id,
            "environments": [],
            "additionalContext": { "miniclaw_skills": { "kind": "application",
                "value": instructions.unwrap_or_else(|| "No conversation skill selections are active.".into()) } },
            "input": [],
    });
    // Pictures come first, then the words. A message of uploads alone still says what they are.
    let words = if input.text.trim().is_empty() {
        format!("(attached: {})", uploads.iter().map(|record| record.name.as_str()).collect::<Vec<_>>().join(", "))
    } else { input.text.clone() };
    params["input"] = pictures.into_iter().chain([json!({ "type": "text", "text": words, "text_elements": [] })]).collect();
    // The text of attached files travels under its own key, for this turn only, so the saved
    // message stays as typed.
    if let Some(attached) = attached {
        params["additionalContext"]["miniclaw_attachments"] = json!({ "kind": "application", "value": attached });
    }
    if let Some(called) = called {
        // The section names the skill that was called: `## Skill called with /name: Title`.
        if let Some(title) = called.lines().find_map(|line| line.split_once("## Skill called with /")
            .and_then(|(_, rest)| rest.split_once(": ")).map(|(_, title)| title.trim().to_owned())) {
            skill_uses.push(crate::analytics::SkillUse { thread: id.clone(), turn: String::new(), name: title, source: "slash".into() });
        }
        params["additionalContext"]["miniclaw_slash"] = json!({ "kind": "application",
            "value": format!("The user called a MiniClaw skill for this message. Follow it for this turn.\n{called}") });
    }
    // What is on, off or not allowed right now. Unchanged, it costs nothing to send again.
    if let Some(note) = environment_note(&backend, &id).await {
        params["additionalContext"]["miniclaw_environment"] = json!({ "kind": "application", "value": note });
    }
    // The project's memory index, with the first message and after someone else changed it.
    if let Some(shared) = crate::project_tool::note(&backend, &id).await {
        params["additionalContext"]["miniclaw_project"] = json!({ "kind": "application", "value": shared });
    }
    if let Some(delivered) = delivered {
        params["additionalContext"]["miniclaw_delegations"] = json!({ "kind": "application", "value": delivered });
    }
    if let Some(model) = &choice.model {
        params["model"] = json!(model);
    }
    if let Some(effort) = &choice.effort {
        params["effort"] = json!(effort);
    }
    if let Some(speed) = &choice.speed {
        params["serviceTier"] = json!(speed);
    }
    // Pass Claude each selected built-in tool and plan mode independently.
    if backend.is_claude(&id) && let Some(web) = claude_tools(&backend, &id).await? {
        params["miniclawNativeTools"] = web;
    }
    // A Claude turn gets the question tool unless asking is switched off for the conversation.
    if backend.is_claude(&id) && !ask::switched_off(&backend, &id).await {
        params["miniclawAsk"] = json!(true);
    }
    // A Claude turn gets the MCP gateway only while a server is switched on for the conversation.
    if backend.is_claude(&id) && crate::mcp_gateway::any_on(&backend, &id).await {
        params["miniclawMcp"] = json!(true);
    }
    let result = call(&backend, "turn/start", params).await;
    // A chosen model brings its speed, standard or a tier; MiniClaw keeps it, since Codex does not.
    if choice.model.is_some() && result.is_ok() { backend.agents.set_chat_speed(id.clone(), choice.speed.clone()).await?; }
    let mut data = backend.data.lock().await;
    if data.generation != generation {
        if let Ok(result) = &result
            && let Some(turn) = result["turn"]["id"].as_str()
        {
            backend.cancel(&id, turn);
        }
        return Err(unavailable());
    }
    let chat = data.chats.get_mut(&id).ok_or_else(missing)?;
    chat.sending = false;
    let reply = match result {
        Ok(result) => {
            let turn = result["turn"]["id"].as_str().ok_or_else(unavailable)?;
            chat.receipts.get_mut(&client_id).unwrap().turn = Some(turn.into());
            backend.attachments.started(&id, input.client_message_id, turn);
            // The skills are known before the turn has an id, so they are written down with it now.
            if !skill_uses.is_empty() {
                for use_ in &mut skill_uses { use_.turn = turn.to_owned(); }
                let (store, uses) = (backend.clone(), std::mem::take(&mut skill_uses));
                tokio::spawn(async move { let _ = store.agents.record_skills(uses).await; });
            }
            if let Some(model) = &choice.model {
                chat.summary["model"] = json!(model);
                chat.summary["effort"] = json!(choice.effort);
                chat.summary["speed"] = json!(choice.speed);
            }
            // Completion can arrive on the stream before this acknowledgement.
            if !chat.finished.iter().any(|id| id == turn) {
                chat.active = Some(turn.into());
            }
            chat.name_turn(Some(&client_id), turn);
            Ok((StatusCode::ACCEPTED, Json(json!({ "turnId": turn }))))
        }
        Err(mut error) => {
            // A broken connection after write is an uncertain send, never auto-retry.
            chat.uncertain = error.0 != StatusCode::BAD_GATEWAY;
            error.3 = chat.uncertain;
            chat.emit(
                "turn.error",
                json!({
                    "message": error.2, "outcomeUnknown": error.3,
                }),
            );
            Err(error)
        }
    };
    chat.emit("snapshot", chat.snapshot());
    backend.changed();
    reply
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Interrupt {
    turn_id: String,
}

/// Requests cancellation of the active turn and returns an empty acknowledgement.
/// The chat stays active until Codex reports completion.
async fn interrupt(
    State(backend): State<Shared>,
    Path(id): Path<String>,
    input: Body<Interrupt>,
) -> Result<Json<Value>> {
    authorized_thread(&backend, &id).await?;
    let input = body(input)?;
    {
        let mut data = backend.data.lock().await;
        let chat = data.chats.get_mut(&id).ok_or_else(missing)?;
        if chat.active.as_deref() != Some(&input.turn_id) {
            return Err(busy());
        }
        chat.stopping = true;
        chat.approval = None;
        chat.scoped_approval = None;
        chat.emit("snapshot", chat.snapshot());
    }
    backend.changed();
    call(
        &backend,
        "turn/interrupt",
        json!({
            "threadId": id, "turnId": input.turn_id,
        }),
    )
    .await?;
    // The interrupt acknowledgement is not completion; wait for the turn event.
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    action: String,
}

/// Answers the active computer approval and returns an empty acknowledgement.
/// Rejects expired requests, wrong task/turn IDs, and decisions made after Stop.
async fn decide(
    State(backend): State<Shared>,
    Path((id, approval_id)): Path<(String, String)>,
    input: Body<Decision>,
) -> Result<Json<Value>> {
    authorized(&backend).await?;
    let input = body(input)?;
    if !matches!(input.action.as_str(), "accept" | "decline") {
        return Err(invalid());
    }
    let mut data = backend.data.lock().await;
    let chat = data.chats.get_mut(&id).ok_or_else(missing)?;
    // Accept decisions only for the pending approval on the active turn.
    let valid = chat.approval.as_ref().is_some_and(|approval| {
        approval.id == approval_id
            && approval.current()
            && !chat.stopping
            && chat.active.as_deref() == approval.request.params["turnId"].as_str()
    });
    if !valid {
        return Err(approval_gone());
    }
    let approval = chat.approval.take().unwrap();
    let content = if input.action == "accept" {
        json!({})
    } else {
        Value::Null
    };
    let sent = approval
        .request
        .answer
        .send(json!({ "action": input.action, "content": content }));
    chat.emit("snapshot", chat.snapshot());
    backend.changed();
    sent.map_err(|_| approval_gone())?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScopedDecision { action: String }

/// Resolves one exact scoped request without letting the model grant itself access.
async fn decide_scoped(State(backend): State<Shared>,
    Path((id, approval_id)): Path<(String, String)>, input: Body<ScopedDecision>)
    -> Result<Json<Value>> {
    authorized(&backend).await?;
    let action = match body(input)?.action.as_str() {
        "approve_once" => ScopedChoice::Once,
        "approve_session" => ScopedChoice::Session,
        "approve_session_domains" => ScopedChoice::SessionDomains,
        "approve_programs" => ScopedChoice::Programs,
        "reject" => ScopedChoice::Reject,
        _ => return Err(invalid()),
    };
    let mut data = backend.data.lock().await;
    let chat = data.chats.get_mut(&id).ok_or_else(missing)?;
    let valid = chat.scoped_approval.as_ref().is_some_and(|approval| {
        approval.id.to_string() == approval_id && approval.current()
            && !chat.stopping && chat.active.as_deref() == Some(&approval.turn)
    });
    if !valid { return Err(approval_gone()); }
    let approval = chat.scoped_approval.take().unwrap();
    approval.answer.send(action).map_err(|_| approval_gone())?;
    chat.emit("snapshot", chat.snapshot());
    backend.changed();
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewDecision { action: String, text: Option<String>, note: Option<String> }

/// Takes the user's decision on the change a coding session is showing them.
async fn decide_review(State(backend): State<Shared>, Path((id, review_id)): Path<(String, String)>,
    input: Body<ReviewDecision>) -> Result<Json<Value>> {
    use crate::chat::ReviewChoice;
    authorized(&backend).await?;
    let input = body(input)?;
    // An edited proposal obeys the same size limit as a write from the agent.
    if input.text.as_ref().is_some_and(|text| text.len() > 32_768 || text.contains('\0'))
        || input.note.as_ref().is_some_and(|note| note.len() > 2000) { return Err(invalid()); }
    let choice = match input.action.as_str() {
        "apply" => ReviewChoice::Apply { text: input.text, rest_of_turn: false },
        "apply_all" => ReviewChoice::Apply { text: input.text, rest_of_turn: true },
        "reject" => ReviewChoice::Reject { note: input.note.unwrap_or_default() },
        _ => return Err(invalid()),
    };
    let mut data = backend.data.lock().await;
    let chat = data.chats.get_mut(&id).ok_or_else(missing)?;
    let valid = chat.review.as_ref().is_some_and(|review| review.id.to_string() == review_id && review.current()
        && !chat.stopping && chat.active.as_deref() == Some(&review.turn));
    if !valid { return Err(approval_gone()); }
    let review = chat.review.take().unwrap();
    review.answer.send(choice).map_err(|_| approval_gone())?;
    chat.emit("snapshot", chat.snapshot());
    backend.changed();
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
struct Cursor {
    after: Option<String>,
}

/// Returns the chat's SSE feed, replaying missed events or replacing stale cursors
/// with a snapshot. Closes the feed when authorization or the runtime is lost.
async fn events(
    State(backend): State<Shared>,
    Path(id): Path<String>,
    Query(query): Query<Cursor>,
    headers: HeaderMap,
) -> Result<Response> {
    let slot = backend.streams.clone().try_acquire_owned().map_err(|_| Error(
        StatusCode::SERVICE_UNAVAILABLE, "stream_limit",
        "Too many chat streams are open. Close unused chat tabs, then reconnect.", false,
    ))?;
    let lease = load(backend.clone(), id.clone()).await?;
    let generation = authorized_thread(&backend, &id).await?;
    let (event_client, _) = thread_client(&backend, &id,
        backend.agents.assignment(id.clone()).await?.as_ref())?;
    let mut refresh = tokio::time::Instant::now() + Duration::from_secs(15);
    // Subscribe before selecting any event so changes still wake an idle stream.
    let mut changed = backend.changes.subscribe();
    let mut cursor = headers.get("last-event-id")
        .and_then(|value| value.to_str().ok()).map(str::to_owned).or(query.after);
    let mut snapshot_pending = cursor.is_none();
    let stream = async_stream::stream! {
        let _slot = slot;
        let _chat = lease;
        loop {
            if tokio::time::Instant::now() >= refresh {
                if authorized_thread(&backend, &id).await.is_err() { break; }
                refresh = tokio::time::Instant::now() + Duration::from_secs(15);
            }
            let outgoing = {
                let data = backend.data.lock().await;
                if data.generation != generation { break; }
                if !event_client.available() {
                    break;
                }
                let Some(chat) = data.chats.get(&id) else { break };
                if snapshot_pending {
                    snapshot_pending = false;
                    Some((Some(chat.cursor()), "snapshot".to_owned(), chat.snapshot().to_string()))
                } else if cursor.as_deref() == Some(&chat.cursor()) {
                    None
                } else {
                    let next = if cursor.as_deref() == Some(&format!("{}:0", chat.epoch))
                        && chat.events.front().is_some_and(|event|
                            event.id == format!("{}:1", chat.epoch)) {
                        chat.events.front()
                    } else {
                        chat.events.iter().position(|event| Some(&event.id) == cursor.as_ref())
                            .and_then(|index| chat.events.get(index + 1))
                    };
                    if let Some(event) = next {
                        Some((Some(event.id.clone()), event.name.clone(), event.data.clone()))
                    } else {
                        // The marker acknowledges nothing; the next poll selects a fresh snapshot.
                        snapshot_pending = true;
                        Some((None, "resync.required".to_owned(), json!({ "threadId": id }).to_string()))
                    }
                }
            };
            if let Some((id, name, data)) = outgoing {
                let mut event = Event::default().event(name).data(data);
                if let Some(id) = id {
                    cursor = Some(id.clone());
                    event = event.id(id);
                }
                yield Ok::<_, Infallible>(event);
                continue;
            }
            tokio::select! {
                result = changed.changed() => { if result.is_err() { break; } }
                _ = tokio::time::sleep_until(refresh) => {}
            }
        }
    };
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::default().interval(Duration::from_secs(10)))
        .into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Range { days: Option<String> }

/// Returns what MiniClaw recorded about its own use over a range of days, for the Analytics tab.
async fn analytics(State(backend): State<Shared>,
    query: std::result::Result<Query<Range>, axum::extract::rejection::QueryRejection>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    let Query(range) = query.map_err(|_| invalid())?;
    let days: i64 = match range.days {
        Some(text) => text.parse().ok().filter(|days| (1..=crate::analytics::MOST_DAYS).contains(days)).ok_or_else(invalid)?,
        None => 30,
    };
    Ok(Json(backend.agents.analytics(days).await?))
}

/// Returns each agent's figures over a range of days, for the Agents page.
async fn agent_analytics(State(backend): State<Shared>,
    query: std::result::Result<Query<Range>, axum::extract::rejection::QueryRejection>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    let Query(range) = query.map_err(|_| invalid())?;
    let days: i64 = match range.days {
        Some(text) => text.parse().ok().filter(|days| (1..=crate::analytics::MOST_DAYS).contains(days)).ok_or_else(invalid)?,
        None => 30,
    };
    let mut report = backend.agents.agent_analytics(days).await?;
    report["active"] = active_turns(&backend).await;
    Ok(Json(report))
}

/// Lists the responses running right now, for the chat view's running-agents box. Cheap: it reads
/// only what is in memory, so the page can ask every few seconds.
async fn active(State(backend): State<Shared>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    Ok(Json(json!({ "active": active_turns(&backend).await, "swarms": swarm::live(&backend).await })))
}

/// Every response running right now, with its agent, so the Agents page can show it and stop it
/// through the ordinary interrupt route. The main agent is the empty name, as in the figures.
async fn active_turns(backend: &Backend) -> Value {
    let data = backend.data.lock().await;
    let mut turns: Vec<Value> = data.chats.iter().filter_map(|(id, chat)| chat.active.as_ref().map(|turn| json!({
        "threadId": id, "turnId": turn,
        "agent": chat.summary["agentName"].as_str().unwrap_or(""),
        "conversation": chat.summary["name"].as_str().unwrap_or("Conversation"),
        "startedAt": chat.turn_started, "stopping": chat.stopping,
    }))).collect();
    turns.sort_by_key(|turn| turn["startedAt"].as_i64().unwrap_or(i64::MAX));
    json!(turns)
}

/// Lists safe notices only while the requesting account remains authorized.
async fn notifications(State(backend): State<Shared>) -> Result<Json<Value>> {
    let generation = authorized(&backend).await?;
    let data = backend.data.lock().await;
    if data.generation != generation { return Err(unavailable()); }
    let owner = data.owner().ok_or_else(unavailable)?;
    drop(data);
    let notices = backend.agents.notices(owner).await?;
    let data = backend.data.lock().await;
    if data.generation != generation { return Err(unavailable()); }
    Ok(Json(json!({ "notifications": notices, "storageError": data.notice_error })))
}

/// Marks all of the current account's notices read.
async fn read_all_notifications(State(backend): State<Shared>, input: Body<Value>) -> Result<Json<Value>> {
    let _ = input.map_err(|_| invalid())?;
    let generation = authorized(&backend).await?;
    let data = backend.data.lock().await;
    if data.generation != generation { return Err(unavailable()); }
    let owner = data.owner().ok_or_else(unavailable)?;
    drop(data);
    Ok(Json(json!({ "read": backend.agents.read_all_notices(owner).await? })))
}

/// Clears read notices while holding the current account's authorization stable.
async fn clear_read_notifications(State(backend): State<Shared>, input: Body<Value>)
    -> Result<Json<Value>>
{
    let _ = input.map_err(|_| invalid())?;
    let generation = authorized(&backend).await?;
    let data = backend.data.lock().await;
    if data.generation != generation { return Err(unavailable()); }
    let owner = data.owner().ok_or_else(unavailable)?;
    Ok(Json(json!({ "cleared": backend.agents.clear_read_notices(owner).await? })))
}

/// Marks a notice read under the current account and origin guards.
async fn read_notification(State(backend): State<Shared>, Path(id): Path<String>)
    -> Result<Json<Value>>
{
    let generation = authorized(&backend).await?;
    let data = backend.data.lock().await;
    if data.generation != generation { return Err(unavailable()); }
    let owner = data.owner().ok_or_else(unavailable)?;
    let id = Uuid::parse_str(&id).map_err(|_| invalid())?;
    backend.agents.read_notice(owner, id).await?;
    Ok(Json(json!({ "ok": true })))
}


#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompactCreations {
    token: String,
    confirmed: bool,
}

/// Previews pending creation counts without exposing or changing provisional policies.
async fn creation_recovery(State(backend): State<Shared>) -> Result<Json<Value>> {
    let generation = authorized(&backend).await?;
    let preview = backend.agents.creation_recovery().await?;
    if backend.data.lock().await.generation != generation { return Err(unavailable()); }
    Ok(Json(json!(preview)))
}

/// Replaces an explicitly confirmed journal snapshot while preserving all unknown-chat denials.
async fn compact_creations(State(backend): State<Shared>, input: Body<CompactCreations>)
    -> Result<Json<Value>>
{
    let input = body(input)?;
    if !input.confirmed || input.token.len() != 64 || !input.token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    let _creating = backend.create_gate.lock().await;
    let _agents = backend.agent_gate.write().await;
    let _loading = backend.load_gate.lock().await;
    let generation = authorized(&backend).await?;
    let data = backend.data.lock().await;
    if data.generation != generation { return Err(unavailable()); }
    let released = backend.agents.compact_creations(input.token).await.map_err(|issue| {
        if matches!(issue, Issue::Conflict) {
            Error(StatusCode::CONFLICT, "creation_recovery_changed",
                "Creation recovery changed. Refresh the preview before confirming.", false)
        } else { issue.into() }
    })?;
    Ok(Json(json!({ "released": released })))
}

#[cfg(test)]
mod rule_file_tests {
    /// A rule file inside a folder MiniClaw protects is never read, in whatever form the
    /// platform resolves the folder (on Windows `canonicalize` adds a `\\?\` prefix).
    #[test]
    fn rule_files_in_a_protected_folder_are_left_out() {
        let folder = std::env::temp_dir().join(format!("mc-rules-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("AGENTS.md"), "Project rules.\n").unwrap();
        // The protected root is spelled as `seatbelt::protected` spells roots on each platform.
        #[cfg(windows)]
        let root = crate::platform_fs::permission_path(&folder).unwrap();
        #[cfg(not(windows))]
        let root = std::fs::canonicalize(&folder).unwrap().to_string_lossy().into_owned();
        let path = folder.to_string_lossy().into_owned();
        let (open, _) = super::rule_files_within(&path, &[]);
        assert_eq!(open.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["AGENTS.md"]);
        let (closed, cut) = super::rule_files_within(&path, &[root.clone()]);
        assert!(closed.is_empty() && !cut, "a protected folder's rules were read: {root}");
        std::fs::remove_dir_all(folder).unwrap();
    }
}
