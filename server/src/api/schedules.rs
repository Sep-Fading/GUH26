//! Scheduled runs: the routes, and the runner that starts what is due. A run is created and
//! sent through the same paths as the page uses. Nobody is watching it, so an approval waits
//! for the time its schedule allows and then the run is terminated: it is never left waiting
//! and never carries on past a refusal nobody saw.

// Reuses chat creation, sending and interruption exactly as the page uses them.
use super::*;
use crate::schedules::Schedule;

/// The longest any scheduled run may take.
const LIMIT: Duration = Duration::from_secs(3600);

/// One minute, which tests shorten so a configured wait passes quickly.
pub(super) fn minute() -> Duration {
    Duration::from_millis(std::env::var("MINICLAW_TEST_MINUTE_MS").ok()
        .and_then(|text| text.parse().ok()).unwrap_or(60_000))
}

/// Registers the periodic check with the background watcher.
pub(super) fn install(backend: &Shared) {
    let _ = backend.schedule_tick.set(Box::new(|backend| { tokio::spawn(async move { check(&backend).await; }); }));
}

/// Starts every enabled schedule whose period has passed and that is not already running.
async fn check(backend: &Shared) {
    let now = chrono::Utc::now().timestamp();
    for schedule in backend.agents.schedules().await.unwrap_or_default() {
        let due = schedule.enabled && schedule.last_status.as_deref() != Some("running")
            && schedule.last_run.is_none_or(|last| now - last >= schedule.period());
        if due { start(backend.clone(), schedule).await; }
    }
}

/// Marks the schedule as running and runs it in the background.
async fn start(backend: Shared, schedule: Schedule) {
    let now = chrono::Utc::now().timestamp();
    if backend.agents.mark_schedule(schedule.id.clone(), "running".into(), None, None, Some(now)).await.is_err() { return; }
    tokio::spawn(async move {
        let (status, result, thread) = run(&backend, &schedule).await;
        if let Some(thread) = &thread {
            if let Ok(mut runs) = backend.unattended.lock() { runs.remove(thread); }
            if let Ok(mut expired) = backend.unattended_expired.lock() { expired.remove(thread); }
        }
        let _ = backend.agents.mark_schedule(schedule.id.clone(), status.into(), Some(result), thread, None).await;
        if status != "done" {
            let owner = backend.data.lock().await.owner();
            if let Some(owner) = owner { let _ = backend.agents.scheduled_run_ended(owner, schedule.id.clone(), status).await; }
        }
        backend.changed();
    });
}

/// Runs one schedule to its end and returns its status, what to show, and its conversation.
async fn run(backend: &Shared, schedule: &Schedule) -> (&'static str, String, Option<String>) {
    let words = |error: Error| error.2.to_owned();
    // The project decides the machine. A VM that is off fails the run; a schedule never starts one.
    let machine = match schedule.project_id {
        Some(id) => backend.agents.projects().await.ok().into_iter().flatten().find(|project| project.id == id)
            .map(|project| if matches!(project.definition.environment, Some(ProjectChoice::Vm { .. })) {
                ConversationChoice::Vm {} } else { ConversationChoice::Host {} }),
        None => Some(ConversationChoice::Host {}),
    };
    let Some(machine) = machine else { return ("failed", "Its project no longer exists.".into(), None) };
    let created = match create_chat(backend.clone(), Create { capabilities: Default::default(), skill_selection: None, project_id: schedule.project_id,
        agent_id: Some(schedule.agent_id), model: None, effort: None, speed: None, computer_use: false,
        environment: Some(machine), delegated: false }).await {
        Ok(created) => created,
        Err(error) => return ("failed", words(error), None),
    };
    let Some(thread) = created.1.0["thread"]["id"].as_str().map(str::to_owned)
        else { return ("failed", "The run could not be created.".into(), None) };
    if let Ok(mut runs) = backend.unattended.lock() {
        runs.insert(thread.clone(), minute() * u32::from(schedule.approval_minutes));
    }
    let _ = backend.agents.mark_schedule(schedule.id.clone(), "running".into(), None, Some(thread.clone()), None).await;
    let sent = match send(State(backend.clone()), Path(thread.clone()), Ok(Json(Send { attachments: Vec::new(), text: schedule.prompt.clone(),
        client_message_id: Uuid::new_v4(), model: None, effort: None, speed: None }))).await {
        Ok(sent) => sent,
        Err(error) => return ("failed", words(error), Some(thread)),
    };
    let Some(turn) = sent.1.0["turnId"].as_str().map(str::to_owned)
        else { return ("failed", "The run did not start.".into(), Some(thread)) };
    let deadline = tokio::time::Instant::now() + LIMIT;
    // A run belongs to the account that started it.
    let generation = backend.data.lock().await.generation;
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let expired = backend.unattended_expired.lock().ok().and_then(|expired| expired.get(&thread).cloned());
        let late = tokio::time::Instant::now() >= deadline;
        if expired.is_some() || late {
            let _ = interrupt(State(backend.clone()), Path(thread.clone()), Ok(Json(Interrupt { turn_id: turn.clone() }))).await;
            return ("terminated", match expired {
                Some(target) => format!("It needed an approval and nobody answered within {} min. Request: {target}. \
                    Add a permission rule for the agent if this should be allowed.", schedule.approval_minutes),
                None => "It reached the one hour limit for scheduled runs.".into(),
            }, Some(thread));
        }
        let data = backend.data.lock().await;
        if data.generation != generation {
            drop(data);
            let _ = interrupt(State(backend.clone()), Path(thread.clone()), Ok(Json(Interrupt { turn_id: turn.clone() }))).await;
            return ("failed", "The signed-in account changed, so the run was ended.".into(), Some(thread));
        }
        let Some(chat) = data.chats.get(&thread) else { return ("failed", "The run's conversation was lost.".into(), Some(thread)) };
        if chat.finished.contains(&turn) {
            let answer = chat.messages.iter().rev().find(|message| message["role"] == "assistant")
                .and_then(|message| message["text"].as_str()).map(|text| text.chars().take(2000).collect::<String>());
            return match answer {
                Some(text) if !text.trim().is_empty() => ("done", text, Some(thread.clone())),
                _ => ("failed", "It ended without an answer.".into(), Some(thread.clone())),
            };
        }
    }
}

/// Lists the saved schedules with the state of their last run.
pub(super) async fn list(State(backend): State<Shared>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    Ok(Json(json!({ "schedules": backend.agents.schedules().await? })))
}

/// Checks that a schedule names a saved agent and, if any, a saved project.
async fn check_references(backend: &Backend, schedule: &Schedule) -> Result<()> {
    if !backend.agents.list().await?.iter().any(|agent| agent.id == schedule.agent_id) { return Err(missing()); }
    if let Some(project) = schedule.project_id
        && !backend.agents.projects().await?.iter().any(|saved| saved.id == project) { return Err(missing()); }
    Ok(())
}

/// Saves a new schedule. It is switched off unless the request says otherwise.
pub(super) async fn add(State(backend): State<Shared>, input: Body<Schedule>) -> Result<(StatusCode, Json<Value>)> {
    let schedule = body(input)?;
    authorized(&backend).await?;
    if !schedule.valid() { return Err(invalid()); }
    check_references(&backend, &schedule).await?;
    Ok((StatusCode::CREATED, Json(json!({ "schedule": backend.agents.save_schedule(None, schedule).await? }))))
}

/// Replaces a schedule's definition, which includes switching it on or off.
pub(super) async fn edit(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Schedule>) -> Result<Json<Value>> {
    let schedule = body(input)?;
    authorized(&backend).await?;
    if !schedule.valid() { return Err(invalid()); }
    check_references(&backend, &schedule).await?;
    Ok(Json(json!({ "schedule": backend.agents.save_schedule(Some(id), schedule).await? })))
}

/// Deletes a schedule; the conversations of its runs stay.
pub(super) async fn remove(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Value>) -> Result<Json<Value>> {
    let _ = input.map_err(|_| invalid())?;
    authorized(&backend).await?;
    backend.agents.remove_schedule(id).await?;
    Ok(Json(json!({})))
}

/// Runs one schedule now, whether or not it is switched on. One run of a schedule at a time.
pub(super) async fn run_now(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Value>)
    -> Result<(StatusCode, Json<Value>)> {
    let _ = input.map_err(|_| invalid())?;
    authorized(&backend).await?;
    let schedule = backend.agents.schedules().await?.into_iter().find(|schedule| schedule.id == id).ok_or_else(missing)?;
    if schedule.last_status.as_deref() == Some("running") { return Err(busy()); }
    start(backend.clone(), schedule).await;
    Ok((StatusCode::ACCEPTED, Json(json!({}))))
}

/// Checks the schedules now instead of at the watcher's next pass.
pub(super) async fn tick(State(backend): State<Shared>, input: Body<Value>) -> Result<Json<Value>> {
    let _ = input.map_err(|_| invalid())?;
    authorized(&backend).await?;
    check(&backend).await;
    Ok(Json(json!({})))
}
