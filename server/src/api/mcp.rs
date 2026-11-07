//! Routes of the MCP server list. Only the user reaches them, through the page; no tool a
//! model can call adds, changes or switches on a server. A secret goes from the request to
//! the Keychain and is not part of any answer.

// Reuses the shared request checks and error shapes.
use super::*;
// Saves, reads and removes Keychain items.
use crate::secrets::{Secrets, env_key, header_key, token_key};
// The list's entry types and Keychain changes.
use crate::mcp_servers::{Draft, SecretChange, Server};

/// Says the Keychain (Windows Credential Manager on Windows) refused, which leaves a server saved
/// without its new secret.
fn keychain() -> Error {
    Error(StatusCode::SERVICE_UNAVAILABLE, "keychain_unavailable", keychain_message(std::env::consts::OS), true)
}

/// The words for a refused secret, naming the store of the platform MiniClaw runs on.
fn keychain_message(os: &str) -> &'static str {
    match os {
        "windows" => "Windows Credential Manager refused the change. The server was saved; enter its secret again.",
        "linux" => "MiniClaw's private secrets file could not be changed. The server was saved; enter its secret again.",
        _ => "The macOS Keychain refused the change. The server was saved; enter its secret again.",
    }
}

/// Applies one save's Keychain changes off the asynchronous workers.
async fn apply(changes: Vec<SecretChange>) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let secrets = Secrets::from_env();
        changes.into_iter().try_for_each(|change| match change {
            SecretChange::Set(key, value) => secrets.set(&key, &value),
            SecretChange::Remove(key) => secrets.remove(&key),
        })
    }).await.map_err(|_| keychain())?.map_err(|_| keychain())
}

/// A saved server as the page sees it. On Windows a server saved as `npx`/`npm exec` also says
/// what really runs (MiniClaw's Node and npm, never the batch launcher), in `runs`.
fn shown(server: &Server) -> Value {
    let mut value = json!(server);
    if cfg!(windows) && let Some(runs) = crate::npm_launch::describe(server.command.as_deref().unwrap_or_default(), &server.args) {
        value["runs"] = json!(runs);
    }
    value
}

/// Lists the saved servers. No entry carries a secret.
pub(super) async fn list(State(backend): State<Shared>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    let servers: Vec<Value> = backend.agents.mcp_servers().await?.iter().map(shown).collect();
    Ok(Json(json!({ "servers": servers })))
}

/// Saves a new server. It is switched on nowhere.
pub(super) async fn add(State(backend): State<Shared>, input: Body<Draft>) -> Result<(StatusCode, Json<Value>)> {
    let draft = body(input)?;
    authorized(&backend).await?;
    let (server, changes) = backend.agents.save_mcp_server(None, draft, Vec::new()).await?;
    apply(changes).await?;
    Ok((StatusCode::CREATED, Json(json!({ "server": shown(&server) }))))
}

/// Changes a server whose revision the page has seen.
pub(super) async fn edit(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Draft>) -> Result<Json<Value>> {
    let draft = body(input)?;
    authorized(&backend).await?;
    let (server, changes) = backend.agents.save_mcp_server(Some(id), draft, Vec::new()).await?;
    // The running server belongs to the old entry; the next call starts the new one.
    backend.mcp.forget(&server.id).await;
    apply(changes).await?;
    Ok(Json(json!({ "server": shown(&server) })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Removal { revision: u64 }

/// Deletes a server and removes its secrets from the Keychain.
pub(super) async fn remove(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Removal>) -> Result<Json<Value>> {
    let removal = body(input)?;
    authorized(&backend).await?;
    let _gate = backend.agent_gate.write().await;
    let Server { id, env, headers, .. } = backend.agents.remove_mcp_server(id, removal.revision).await?;
    // Its rules go with it, so a later server of the same name starts switched on nowhere.
    backend.agents.forget_mcp_server(id.clone()).await?;
    backend.permission_revision.fetch_add(1, Ordering::AcqRel);
    backend.mcp.forget(&id).await;
    // Its private npm install goes too (Windows servers saved as `npx`/`npm exec`).
    #[cfg(windows)]
    { let id = id.clone(); tokio::task::spawn_blocking(move || crate::mcp_npm::forget(&id)); }
    let mut changes = vec![SecretChange::Remove(token_key(&id))];
    changes.extend(env.iter().map(|name| SecretChange::Remove(env_key(&id, name))));
    changes.extend(headers.iter().map(|name| SecretChange::Remove(header_key(&id, name))));
    apply(changes).await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct From { #[serde(default)] provider: skills::Provider }

/// Lists the servers found in one provider's configuration, with names of secrets only.
pub(super) async fn sources(State(backend): State<Shared>,
    query: std::result::Result<Query<From>, axum::extract::rejection::QueryRejection>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    let Query(from) = query.map_err(|_| invalid())?;
    let sources = tokio::task::spawn_blocking(move || crate::mcp_import::sources(from.provider)).await
        .map_err(|_| unavailable())??;
    Ok(Json(json!({ "sources": sources })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Import { #[serde(default)] provider: skills::Provider, name: String }

/// Copies one provider entry into the list, without its secrets and switched on nowhere.
pub(super) async fn import(State(backend): State<Shared>, input: Body<Import>) -> Result<(StatusCode, Json<Value>)> {
    let Import { provider, name } = body(input)?;
    authorized(&backend).await?;
    let (draft, needs) = tokio::task::spawn_blocking(move || crate::mcp_import::draft(provider, &name)).await
        .map_err(|_| unavailable())??;
    let (server, _) = backend.agents.save_mcp_server(None, draft, needs).await?;
    Ok((StatusCode::CREATED, Json(json!({ "server": shown(&server) }))))
}

/// Says a server could not be reached, in the client's own words. The shared error type holds
/// fixed text only, so this answer is built here in the same shape. A refusal that a retry cannot
/// change (a sandboxed server that listens, on Windows) is not retryable.
pub(super) fn unreachable(fault: crate::mcp_client::Fault) -> Response {
    let retryable = fault.retryable();
    (StatusCode::BAD_GATEWAY, Json(json!({ "error": { "code": "mcp_unreachable", "message": fault.0,
        "retryable": retryable, "requestId": Uuid::new_v4(), "outcomeUnknown": false } }))).into_response()
}

/// Connects to one saved server, lists its tools and disconnects. Nothing is switched on by this.
pub(super) async fn check(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Value>) -> Result<Response> {
    let _ = input.map_err(|_| invalid())?;
    authorized(&backend).await?;
    let server = backend.agents.mcp_servers().await?.into_iter().find(|server| server.id == id).ok_or_else(missing)?;
    let mut link = match crate::mcp_client::Link::open(&server, &Secrets::from_env()).await {
        Ok(link) => link,
        Err(fault) => return Ok(unreachable(fault)),
    };
    Ok(match link.tools().await {
        // Every tool the server has is shown, with whether the model is offered it.
        Ok(tools) => {
            let mut answer = json!({ "server": link.info, "tools": tools.iter().map(|tool| {
                let mut described = describe(tool);
                described["offered"] = json!(server.offers(tool["name"].as_str().unwrap_or_default()));
                described
            }).collect::<Vec<_>>() });
            // An empty list is not a success to show as one: some servers learn their tools from their app.
            if tools.is_empty() {
                answer["note"] = json!("The server offers no tools yet. Some, such as Roblox Studio's, list them only after their app connects to them.");
            }
            Json(answer).into_response()
        }
        Err(fault) => unreachable(fault),
    })
}

/// Keeps what the page shows of a tool, with its text bounded.
fn describe(tool: &Value) -> Value {
    let short = |key: &str, most: usize| tool[key].as_str().map(|text| text.chars().take(most).collect::<String>());
    json!({ "name": tool["name"], "title": short("title", 120), "description": short("description", 2000),
        "inputSchema": tool["inputSchema"] })
}

#[cfg(test)]
mod tests {
    /// Reads the error object of an `mcp_unreachable` answer.
    async fn answered(fault: crate::mcp_client::Fault) -> serde_json::Value {
        let response = super::unreachable(fault);
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 16).await.unwrap();
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["error"].clone()
    }

    /// A sandboxed server that declares listening ports is refused by its own settings on Windows,
    /// so trying again cannot help: the answer says it is not retryable. Other faults still are.
    #[tokio::test]
    async fn a_refused_listener_is_not_retryable() {
        let server: crate::mcp_servers::Server = serde_json::from_value(serde_json::json!({ "name": "roblox_studio",
            "kind": "stdio", "command": r"C:\WINDOWS\System32\cmd.exe", "args": ["/d", "/s", "/c", ".\\mcp.bat"],
            "listen": [13469] })).unwrap();
        let refused = crate::mcp_client::declared_listener(&server, false).expect("a sandboxed listener is refused");
        let error = answered(refused).await;
        assert_eq!(error["code"], "mcp_unreachable");
        assert_eq!(error["retryable"], false, "{error}");
        let error = answered(crate::mcp_client::Fault::new("The server did not answer in time.")).await;
        assert_eq!(error["retryable"], true, "{error}");
    }

    /// The secret store is named for the machine MiniClaw runs on; the Mac keeps its words.
    #[test]
    fn the_refused_secret_names_this_platforms_store() {
        assert_eq!(super::keychain_message("macos"), "The macOS Keychain refused the change. The server was saved; enter its secret again.");
        let windows = super::keychain_message("windows");
        assert!(windows.starts_with("Windows Credential Manager refused the change.") && !windows.contains("macOS"), "{windows}");
        // Linux keeps secrets in a private file in MiniClaw's data folder (secrets.rs).
        let linux = super::keychain_message("linux");
        assert!(linux.starts_with("MiniClaw's private secrets file") && !linux.contains("Keychain"), "{linux}");
    }
}
