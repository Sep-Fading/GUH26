//! What a conversation can use, for the composer's one dropdown. Each capability has two
//! levels: its agent's configuration (or its project's) says whether it is allowed at all, and
//! the conversation switches what is allowed on or off, starting from off. The two do not
//! move each other: an allowance makes a switch available. Separate agent defaults seed new
//! conversations; explicit composer choices override them and existing conversations keep their choices.
//! Plan mode and the coding session are restrictions, so they need nobody's permission.

// Reuses the shared request checks and error shapes.
use super::*;
// The switches behind each capability.
use crate::permissions::{Decision, Effect, Target};

/// One row of the dropdown.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Capability {
    key: String, name: String, allowed: bool, on: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expired: Option<bool>,
}

/// A capability's switch and how it reads: `inverse` means the switch turns the capability off.
/// `provider` is set for an MCP server that is kept to one provider's conversations.
struct Switch { key: String, name: String, target: Target, inverse: bool, bounded: bool, provider: crate::mcp_servers::Provider }

/// Lists the switches in the order the dropdown shows them, with one entry per saved MCP server.
async fn switches(backend: &Backend) -> Result<Vec<Switch>> {
    let feature = |key: &str, name: &str, switch: &str, inverse: bool, bounded: bool| Switch { key: key.into(), name: name.into(),
        target: Target::Feature { name: switch.into() }, inverse, bounded, provider: Default::default() };
    let mut all = vec![feature("web_search", "Web search", "no_web_search", true, true),
        feature("image_generation", "Image generation", "image_generation", false, true),
        feature("unsandboxed_shell", "Unsandboxed shell", "unsandboxed_shell", false, true),
        feature("network", "Network for commands", "network", false, true),
        feature("web_fetch", "Web fetch", "web_fetch", false, true),
        feature("subagents", "Subagents", "subagents", false, true),
        feature("task_tracking", "Task tracking", "task_tracking", false, true)];
    for server in backend.agents.mcp_servers().await? {
        all.push(Switch { key: format!("mcp:{}", server.id), name: server.name, provider: server.provider,
            target: Target::McpServer { server: server.id }, inverse: false, bounded: true });
    }
    all.push(feature("project_memory", "Project memory", "no_project_memory", true, false));
    all.push(feature("ask_commands", "Ask about every command", "ask_commands", false, false));
    all.push(feature("full_access", "Full access", "full_access", false, false));
    all.push(feature("questions", "Ask me questions", "no_questions", true, false));
    all.push(feature("plan_mode", "Plan mode", "plan_mode", false, false));
    all.push(feature("coding_session", "Coding session", "coding_session", false, false));
    Ok(all)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Of {
    thread: Option<String>, agent_id: Option<Uuid>, project_id: Option<Uuid>,
    #[serde(default)]
    claude: bool,
    #[serde(default)]
    vm: bool,
}

/// Lists the capabilities of one conversation, or of the conversation a chosen agent and
/// project would start. Drafts preview agent defaults within the current permission bounds.
pub(super) async fn list(State(backend): State<Shared>,
    query: std::result::Result<Query<Of>, axum::extract::rejection::QueryRejection>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    let Query(of) = query.map_err(|_| invalid())?;
    let context = match &of.thread {
        Some(thread) => Some(backend.agents.permission_context(thread.clone()).await?),
        None => None,
    };
    let (agent, project) = match &context {
        Some(context) => (context.agent(), context.project()),
        None => (of.agent_id, of.project_id),
    };
    let defaults = initial_defaults(&backend, agent).await?;
    let now = chrono::Utc::now().timestamp();
    // A VM's worker does not take the web search setting yet, so search is not available there.
    let in_vm = context.as_ref().map_or(of.vm, |context| context.machine() != "host");
    let mut capabilities = Vec::new();
    for switch in switches(&backend).await? {
        // Only a project has a memory to switch.
        if switch.key == "project_memory" && project.is_none() { continue; }
        let set = switch.bounded && backend.agents.configured_switch(agent, project, &switch.target).await?;
        // A server kept to the other provider is not available here, whatever the agent allows.
        let claude = of.thread.as_deref().map_or(of.claude, |thread| backend.is_claude(thread));
        let fits = switch.provider.serves(claude) && !(switch.key == "image_generation" && claude);
        let allowed = (!switch.bounded || set != switch.inverse) && !(in_vm && matches!(switch.key.as_str(), "web_search" | "image_generation" | "unsandboxed_shell")) && fits;
        let decided = match &context {
            Some(context) => matches!(backend.agents.permission(context, &switch.target, now).await?, Decision::Allow(_)),
            None => defaults.get(&switch.key).copied().unwrap_or(switch.inverse) != switch.inverse,
        };
        let mut row = Capability { key: switch.key, name: switch.name, allowed,
            on: allowed && decided != switch.inverse, expires_at: None, expired: None };
        if row.key == "unsandboxed_shell" {
            row.on = false;
            if let Some(thread) = &of.thread {
                if let Some(grant) = backend.shell_grants.lock().await.get(thread) {
                    row.expires_at = Some(grant.expires_at);
                    row.expired = Some(grant.expires_at <= chrono::Utc::now().timestamp_millis());
                    row.on = allowed && row.expired == Some(false) && context.as_ref() == Some(&grant.context);
                }
            }
        }
        capabilities.push(row);
    }
    Ok(Json(json!({ "capabilities": capabilities })))
}

/// Validates draft switches before provider creation and returns the rules to save with its bindings.
pub(super) async fn prepare(backend: &Backend, choices: &std::collections::BTreeMap<String, bool>,
    agent: Option<Uuid>, project: Option<Uuid>, claude: bool, in_vm: bool) -> Result<Vec<Target>> {
    let all = switches(backend).await?;
    let mut choices = choices.clone();
    for (key, on) in initial_defaults(backend, agent).await? {
        if choices.contains_key(&key) { continue; }
        let Some(switch) = all.iter().find(|switch| switch.key == key) else { continue; };
        if switch.key == "project_memory" && project.is_none() { continue; }
        let configured = switch.bounded && backend.agents.configured_switch(agent, project, &switch.target).await?;
        // Saved preferences never grant access or block creation on an incompatible provider.
        if on && ((switch.bounded && configured == switch.inverse)
            || !switch.provider.serves(claude) || (key == "image_generation" && claude)
            || (in_vm && matches!(key.as_str(), "web_search" | "image_generation" | "unsandboxed_shell"))) { continue; }
        choices.insert(key, on);
    }
    let mut rules = Vec::new();
    for (key, on) in &choices {
        let switch = all.iter().find(|switch| &switch.key == key).ok_or_else(invalid)?;
        if key == "unsandboxed_shell" { if *on { return Err(invalid()); } else { continue; } }
        if key == "project_memory" && project.is_none() { return Err(invalid()); }
        let configured = switch.bounded && backend.agents.configured_switch(agent, project, &switch.target).await?;
        if *on && ((switch.bounded && configured == switch.inverse)
            || !switch.provider.serves(claude) || (key == "image_generation" && claude)
            || (in_vm && matches!(key.as_str(), "web_search" | "image_generation" | "unsandboxed_shell"))) {
            return Err(Issue::NotAllowed.into());
        }
        // Every non-default choice is an allow rule: ordinary switches on, inverted switches off.
        if *on != switch.inverse { rules.push(switch.target.clone()); }
    }
    Ok(rules)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub(super) struct Toggle { key: String, on: bool, duration_seconds: Option<u64> }

/// Switches one capability on or off for a conversation, within what its agent allows. The
/// running turn goes on: the next tool call reads the switch, so nothing has to be stopped.
pub(super) async fn toggle(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Toggle>) -> Result<Json<Value>> {
    let input = body(input)?;
    if input.key == "image_generation" { return toggle_image(backend, id, input.on).await; }
    if input.key == "unsandboxed_shell" { return toggle_shell(backend, id, input).await; }
    let conversation = Uuid::parse_str(&id).map_err(|_| invalid())?;
    let switch = switches(&backend).await?.into_iter().find(|switch| switch.key == input.key).ok_or_else(invalid)?;
    // The stored rule follows the switch: a bounded capability is on without a rule and off with
    // a block; web search is off with its switch set; a restriction is on with an allow.
    let effect = match (switch.bounded, switch.inverse, input.on) {
        (true, false, true) => Some(Effect::Allow),
        (true, false, false) => Some(Effect::Block),
        (true, true, true) => Some(Effect::Block),
        (true, true, false) => Some(Effect::Allow),
        (false, false, true) => Some(Effect::Allow),
        (false, false, false) => None,
        // Asking is on without a rule and off with its switch set.
        (false, true, true) => None,
        (false, true, false) => Some(Effect::Allow),
    };
    let _admission = backend.agent_gate.write().await;
    let generation = authorized(&backend).await?;
    let scope = Scope::Conversation(conversation);
    backend.agents.permission_scope(scope).await?;
    let mut data = backend.data.lock().await;
    if data.generation != generation { return Err(unavailable()); }
    backend.agents.set_permission(scope, &switch.target, effect).await?;
    backend.permission_revision.fetch_add(1, Ordering::AcqRel);
    if let Some(chat) = data.chats.get_mut(&id) { let snapshot = chat.snapshot(); chat.emit("snapshot", snapshot); }
    backend.changed();
    Ok(Json(json!({})))
}

/// Grants host shell access for a finite duration; off removes it immediately.
async fn toggle_shell(backend: Shared, id: String, input: Toggle) -> Result<Json<Value>> {
    let _admission = backend.agent_gate.read().await;
    authorized_thread(&backend, &id).await?;
    let context = backend.agents.permission_context(id.clone()).await?;
    if input.on {
        if context.machine() != "host" || !backend.is_local()
            || !backend.agents.configured_switch(context.agent(), context.project(),
                &Target::Feature { name: "unsandboxed_shell".into() }).await? {
            return Err(Issue::NotAllowed.into());
        }
        let seconds = input.duration_seconds.filter(|seconds| (1..=86400).contains(seconds)).ok_or_else(invalid)?;
        backend.shell_grants.lock().await.insert(id, crate::shell_access::Grant {
            id: Uuid::new_v4(), context,
            expires_at: chrono::Utc::now().timestamp_millis() + seconds as i64 * 1000, swarm: false,
        });
    } else { backend.shell_grants.lock().await.remove(&id); }
    backend.changed();
    Ok(Json(json!({})))
}

/// Unloads an idle Codex task before changing its image tool, then resumes the same conversation.
/// Loaded App Server tasks keep their old tool configuration across a plain resume.
async fn toggle_image(backend: Shared, id: String, on: bool) -> Result<Json<Value>> {
    tokio::spawn(async move { toggle_image_owned(backend, id, on).await })
        .await.map_err(|_| unavailable())?
}

/// Runs the archive and resume to completion even if the browser disconnects mid-request.
async fn toggle_image_owned(backend: Shared, id: String, on: bool) -> Result<Json<Value>> {
    let _admission = backend.agent_gate.write().await;
    let generation = authorized_thread(&backend, &id).await?;
    if backend.is_claude(&id) { return Err(Issue::NotAllowed.into()); }
    let assignment = backend.agents.assignment(id.clone()).await?.ok_or_else(machine_choice_required)?;
    if assignment != (Assignment::Host {}) { return Err(Issue::NotAllowed.into()); }
    let target = Target::Feature { name: "image_generation".into() };
    let context = backend.agents.permission_context(id.clone()).await?;
    if on && !backend.agents.configured_switch(context.agent(), context.project(), &target).await? {
        return Err(Issue::NotAllowed.into());
    }
    let _lease = load(backend.clone(), id.clone()).await?;
    let before = image_generation(&backend, &id).await;
    let mut restore_pending = {
        let data = backend.data.lock().await;
        let chat = data.chats.get(&id).ok_or_else(missing)?;
        if chat.summary["computerUse"] == true {
            return Err(Error(StatusCode::CONFLICT, "image_computer_conflict",
                "Turn off Computer Use before changing image generation.", false));
        }
        chat.image_restore_pending
    };
    if before == on && !backend.data.lock().await.chats.get(&id).is_some_and(|chat| chat.image_error.is_some()) {
        let data = backend.data.lock().await;
        return Ok(Json(data.chats.get(&id).ok_or_else(missing)?.snapshot()));
    }
    {
        let mut data = backend.data.lock().await;
        let chat = data.chats.get_mut(&id).ok_or_else(missing)?;
        if chat.active.is_some() || chat.sending || chat.uncertain || chat.computer_changing || chat.image_changing {
            return Err(busy());
        }
        chat.image_changing = true;
        chat.emit("snapshot", chat.snapshot());
    }
    backend.changed();
    let mut modified = restore_pending;
    let mut permission_set = false;
    let result = async {
        if restore_pending { restore_pending = is_archived(&backend, &id).await?; }
        if !restore_pending {
            check_switch(&backend, &id).await?;
            recovery::mark(&backend.recovery, &id).await.map_err(|_| unavailable())?;
            modified = true;
            restore_pending = true;
            call(&backend, "thread/archive", json!({ "threadId": id })).await?;
        }
        let restored = call(&backend, "thread/unarchive", json!({ "threadId": id })).await?;
        if restored["thread"]["id"] != id { return Err(unavailable()); }
        restore_pending = false;
        recovery::clear(&backend.recovery, &id).await.map_err(|_| unavailable())?;
        let effect = if on { Effect::Allow } else { Effect::Block };
        backend.agents.set_permission(Scope::Conversation(Uuid::parse_str(&id).map_err(|_| invalid())?),
            &target, Some(effect)).await?;
        permission_set = true;
        let mut params = resume_params(&backend, &id, &assignment)?;
        params["cwd"] = json!(assigned_cwd(&backend, &assignment)?);
        configure_context(&backend, &id, &mut params, false).await?;
        let summary = backend.data.lock().await.chats.get(&id).ok_or_else(missing)?.summary.clone();
        if let Some(model) = summary["model"].as_str() { params["model"] = json!(model); }
        if let Some(effort) = summary["effort"].as_str() { params["config"]["model_reasoning_effort"] = json!(effort); }
        if let Some(speed) = backend.agents.chat_speed(id.clone()).await? { params["serviceTier"] = json!(speed); }
        let resumed = call(&backend, "thread/resume", params).await?;
        if history::summary(&resumed["thread"], thread_cwd(&backend, &id).await?)
            .is_none_or(|summary| summary["id"] != id) { return Err(unavailable()); }
        Ok::<_, Error>(())
    }.await;
    if result.is_err() && permission_set {
        let effect = if before { Effect::Allow } else { Effect::Block };
        let _ = backend.agents.set_permission(Scope::Conversation(Uuid::parse_str(&id).map_err(|_| invalid())?),
            &target, Some(effect)).await;
    }
    let mut data = backend.data.lock().await;
    if data.generation != generation { return Err(unavailable()); }
    let chat = data.chats.get_mut(&id).ok_or_else(missing)?;
    chat.image_changing = false;
    chat.image_restore_pending = restore_pending;
    chat.image_error = if result.is_err() && modified {
        Some("Could not reload this chat's image setting. Retry image generation.")
    } else { None };
    chat.image_retry_on = chat.image_error.map(|_| on);
    if result.is_ok() { backend.permission_revision.fetch_add(1, Ordering::AcqRel); }
    chat.emit("snapshot", chat.snapshot());
    backend.changed();
    result.map_err(|mut error| { if error.0 != StatusCode::CONFLICT {
        error.2 = "Could not reload this chat's image setting. Retry image generation.";
    } error })?;
    Ok(Json(chat.snapshot()))
}

/// Reads saved activation defaults, including the existing Full access preference.
async fn initial_defaults(backend: &Backend, agent: Option<Uuid>) -> Result<std::collections::BTreeMap<String, bool>> {
    let mut defaults = backend.agents.capability_defaults(agent).await?;
    defaults.insert("full_access".into(), backend.agents.feature_default(agent, None, "default_full_access").await?);
    Ok(defaults)
}

/// Lists an agent's default switches independently of a conversation's provider and project.
pub(super) async fn defaults(State(backend): State<Shared>, Path(id): Path<Uuid>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    backend.agents.permission_scope(Scope::Agent(id)).await?;
    let saved = initial_defaults(&backend, Some(id)).await?;
    let mut capabilities = Vec::new();
    for switch in switches(&backend).await? {
        if matches!(switch.key.as_str(), "project_memory" | "unsandboxed_shell") { continue; }
        let configured = switch.bounded && backend.agents.configured_switch(Some(id), None, &switch.target).await?;
        let allowed = !switch.bounded || configured != switch.inverse;
        capabilities.push(Capability { on: allowed && saved.get(&switch.key).copied().unwrap_or(switch.inverse),
            key: switch.key, name: switch.name, allowed, expires_at: None, expired: None });
    }
    Ok(Json(json!({ "capabilities": capabilities })))
}

/// Saves one future-conversation preference; permissions and running turns remain separate.
pub(super) async fn save_default(State(backend): State<Shared>, Path(id): Path<Uuid>, input: Body<Toggle>) -> Result<Json<Value>> {
    let input = body(input)?;
    let _admission = backend.agent_gate.write().await;
    let generation = authorized(&backend).await?;
    backend.agents.permission_scope(Scope::Agent(id)).await?;
    let switch = switches(&backend).await?.into_iter()
        .find(|switch| switch.key == input.key && !matches!(switch.key.as_str(), "project_memory" | "unsandboxed_shell")).ok_or_else(invalid)?;
    let configured = switch.bounded && backend.agents.configured_switch(Some(id), None, &switch.target).await?;
    if input.on && switch.bounded && configured == switch.inverse { return Err(Issue::NotAllowed.into()); }
    let data = backend.data.lock().await;
    if data.generation != generation { return Err(unavailable()); }
    if input.key == "full_access" {
        backend.agents.set_permission(Scope::Agent(id), &Target::Feature { name: "default_full_access".into() },
            input.on.then_some(Effect::Allow)).await?;
    } else {
        backend.agents.save_capability_default(id, input.key, input.on).await?;
    }
    Ok(Json(json!({})))
}
