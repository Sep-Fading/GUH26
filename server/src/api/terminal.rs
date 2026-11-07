//! The routes behind the terminal in the sidebar: open the conversation's shell, stream what it
//! prints, type into it, resize it, restart it. The shell is the same one the agent's approved
//! commands run in, under the same sandbox, so nothing typed here reaches more than a command
//! could.

// Reuses the shared request checks.
use super::*;
// The shells and what they are built from.
use crate::terminal::{INPUT_BYTES, Spec};
// Streams output as server-sent events.
use axum::response::sse::{Event, KeepAlive, Sse};
// Encodes output chunks for the page.
use crate::frames::encode;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Size { cols: u16, rows: u16 }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Typed { text: String }

#[derive(Deserialize)]
pub(super) struct After { after: Option<u64> }

/// Refuses a size no terminal has.
fn sized(size: &Size) -> Result<()> {
    ((20..=500).contains(&size.cols) && (5..=200).contains(&size.rows)).then_some(()).ok_or_else(invalid)
}

/// Builds the shell's spec for a conversation on this Mac: its working folder, the grants its
/// commands run under, and the hosts its network may reach while the user types.
async fn spec(backend: &Shared, thread: &str, cols: u16, rows: u16) -> Result<(Spec, Option<Client>)> {
    load(backend.clone(), thread.to_owned()).await?;
    authorized_thread(backend, thread).await?;
    if !crate::seatbelt::available() {
        // Windows says which precondition is missing; the Mac sentence is unchanged.
        let message = crate::seatbelt::kept_refusal("The sandbox required by the terminal is not available on this platform.",
            crate::seatbelt::unavailable_reason().as_deref());
        return Err(Error(StatusCode::CONFLICT, "terminal_unavailable", message, false));
    }
    let remote = match backend.agents.assignment(thread.to_owned()).await? {
        Some(Assignment::Host {}) => None,
        Some(assignment @ Assignment::Vm { .. }) => {
            if !backend.machine_carries(&assignment, "terminal").await {
                return Err(Error(StatusCode::CONFLICT, "terminal_unavailable", "This VM's worker has no terminal yet. Run miniclaw update-vm.", false));
            }
            let Assignment::Vm { vm_id } = &assignment else { unreachable!() };
            let client = if vm_id == "master" { backend.primary_client() } else { backend.dedicated_client(vm_id) };
            Some(client.ok_or_else(vm_control_unavailable)?)
        }
        None => return Err(machine_choice_required()),
    };
    let (cwd, grants, hosts) = backend.conversation_grants(thread).await.ok_or_else(|| Error(StatusCode::CONFLICT, "no_working_folder",
        "This conversation has no working folder. Give its project a workspace folder to get a terminal.", false))?;
    Ok((Spec { cwd, grants, hosts, cols, rows }, remote))
}

/// Starts or finds the shell for a spec, on this Mac or in the conversation's VM.
async fn ensure(backend: &Shared, thread: &str, spec: Spec, remote: Option<Client>) -> Result<std::sync::Arc<crate::terminal::Terminal>> {
    let made = match remote {
        Some(client) => backend.terminals.ensure_remote(thread, client, spec).await,
        None => backend.terminals.ensure(thread, spec).await,
    };
    made.map_err(|error| {
        // The page learns only that the shell did not start, unless the sandbox refused for a
        // reason the user can act on (an item in the folder they cannot manage, earlier records
        // still unresolved): those words, path included, are meant for them. The rest is logged.
        tracing::warn!(%error, "The terminal's shell could not be started");
        #[cfg(windows)]
        if let Some(words) = crate::windows_sandbox::refusal(&error) {
            return Error(StatusCode::CONFLICT, "terminal_refused", crate::seatbelt::kept_refusal("The shell could not be started.", Some(words)), false);
        }
        Error(StatusCode::SERVICE_UNAVAILABLE, "terminal_failed", "The shell could not be started.", false)
    })
}

/// What the page is told about the shell.
fn view(terminal: &crate::terminal::Terminal, cols: u16, rows: u16) -> Value {
    let exited = *terminal.exited.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    json!({ "seq": terminal.ring.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).end, "cols": cols, "rows": rows,
        "status": if exited.is_some() { "ended" } else { "live" } })
}

/// Starts the conversation's shell if it is not running, sizes it, and returns where the stream is.
pub(super) async fn open(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Size>) -> Result<Json<Value>> {
    let size = body(input)?;
    sized(&size)?;
    let (spec, remote) = spec(&backend, &id, size.cols, size.rows).await?;
    let terminal = ensure(&backend, &id, spec, remote).await?;
    let _ = terminal.resize(size.cols, size.rows).await;
    Ok(Json(view(&terminal, size.cols, size.rows)))
}

/// Streams the shell's output from an offset: `output` events carry base64 bytes and the offset
/// reached as their id, `reset` says the offset had left the ring, `exit` that the shell ended,
/// and `notice` words the page shows once.
pub(super) async fn stream(State(backend): State<Shared>, Path(id): Path<String>, Query(query): Query<After>) -> Result<Response> {
    authorized_thread(&backend, &id).await?;
    let terminal = backend.terminals.get(&id).ok_or_else(|| Error(StatusCode::CONFLICT, "terminal_closed", "Open the terminal first.", false))?;
    let slot = backend.streams.clone().try_acquire_owned().map_err(|_| Error(StatusCode::SERVICE_UNAVAILABLE, "stream_limit",
        "Too many streams are open. Close unused tabs, then reconnect.", false))?;
    let mut after = query.after.unwrap_or(0);
    let mut seq = terminal.seq.subscribe();
    let mut told_exit = false;
    let feed = async_stream::stream! {
        let _slot = slot;
        loop {
            let notices: Vec<String> = std::mem::take(&mut *terminal.notices.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
            for notice in notices { yield Ok::<_, Infallible>(Event::default().event("notice").data(json!({ "text": notice }).to_string())); }
            // The offset reached is the ring's end at the moment of the read, whether or not the reader was reset.
            let (bytes, reset, end) = { let ring = terminal.ring.lock().unwrap_or_else(|poisoned| poisoned.into_inner()); let (bytes, reset) = ring.since(after); (bytes, reset, ring.end) };
            if reset { yield Ok(Event::default().event("reset").data("{}")); }
            if !bytes.is_empty() {
                after = end;
                yield Ok(Event::default().event("output").id(after.to_string()).data(json!({ "bytes": encode(&bytes) }).to_string()));
            }
            let exited = *terminal.exited.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(code) = exited && !told_exit {
                told_exit = true;
                yield Ok(Event::default().event("exit").data(json!({ "code": code }).to_string()));
            }
            if exited.is_none() { told_exit = false; }
            if seq.changed().await.is_err() { break; }
        }
    };
    Ok(Sse::new(feed).keep_alive(KeepAlive::new().interval(Duration::from_secs(10))).into_response())
}

/// Types into the shell.
pub(super) async fn input(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Typed>) -> Result<Json<Value>> {
    let typed = body(input)?;
    if typed.text.is_empty() || typed.text.len() > INPUT_BYTES { return Err(invalid()); }
    authorized_thread(&backend, &id).await?;
    let terminal = backend.terminals.get(&id).ok_or_else(|| Error(StatusCode::CONFLICT, "terminal_closed", "Open the terminal first.", false))?;
    terminal.touch();
    terminal.type_in(&typed.text).await.map_err(|_| Error(StatusCode::CONFLICT, "terminal_ended", "The shell has ended. Restart it.", false))?;
    Ok(Json(json!({})))
}

/// Resizes the shell's terminal.
pub(super) async fn resize(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Size>) -> Result<Json<Value>> {
    let size = body(input)?;
    sized(&size)?;
    authorized_thread(&backend, &id).await?;
    let terminal = backend.terminals.get(&id).ok_or_else(|| Error(StatusCode::CONFLICT, "terminal_closed", "Open the terminal first.", false))?;
    terminal.resize(size.cols, size.rows).await.map_err(|_| unavailable())?;
    Ok(Json(json!({})))
}

/// Ends the shell and starts a fresh one with an empty screen.
pub(super) async fn restart(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Activate>) -> Result<Json<Value>> {
    body(input)?;
    let (cols, rows) = match backend.terminals.get(&id) { Some(terminal) => terminal.size().await, None => (80, 24) };
    backend.terminals.close(&id).await;
    let (spec, remote) = spec(&backend, &id, cols, rows).await?;
    let terminal = ensure(&backend, &id, spec, remote).await?;
    Ok(Json(view(&terminal, cols, rows)))
}

/// Ends the shell.
pub(super) async fn close(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Activate>) -> Result<Json<Value>> {
    body(input)?;
    authorized_thread(&backend, &id).await?;
    backend.terminals.close(&id).await;
    Ok(Json(json!({})))
}
