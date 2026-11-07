// Starts initialized clients and reports runtime failures.
use crate::rpc::{Client, Failure};
// Reads runtime configuration and builds overrides.
use serde_json::{Value, json};
// Configures the owned Codex child process.
use tokio::process::Command;

/// Starts Codex with local chat restrictions and checks inherited configuration.
/// Returns a client after disabling inherited MCP servers, or a startup failure.
pub async fn launch(binary: &str, cwd: &str) -> Result<Client, Failure> {
    let mut disabled = Vec::new();
    for _ in 0..2 {
        let command = command(binary, cwd, &disabled)?;
        let client = Client::spawn(command).await?;
        let result = match client
            .call("config/read", json!({ "includeLayers": false }))
            .await
        {
            Ok(result) => result,
            Err(error) => {
                client.stop().await;
                return Err(error);
            }
        };
        let config = &result["config"];
        if !official(config)
            || std::env::var_os("OPENAI_BASE_URL").is_some()
            || config["mcp_servers"].get(crate::computer::SERVER).is_some()
        {
            client.stop().await;
            return Err(Failure::Rejected);
        }
        // An empty MCP map merges with inherited entries; disable each on the next launch.
        disabled = config["mcp_servers"]
            .as_object()
            .map(|servers| {
                servers
                    .iter()
                    .filter(|(_, server)| server["enabled"] != false)
                    .map(|(id, _)| id.clone())
                    .collect()
            })
            .unwrap_or_default();
        if disabled.is_empty() {
            return Ok(client);
        }
        client.stop().await;
    }
    Err(Failure::Rejected)
}

/// Preflights guest configuration and returns an unstarted, restricted worker command.
pub async fn worker_command(binary: &str, cwd: &str) -> Result<Command, Failure> {
    let client = launch(binary, cwd).await?;
    let result = client
        .call("config/read", json!({ "includeLayers": false }))
        .await;
    client.stop().await;
    let result = result?;
    let disabled = result["config"]["mcp_servers"]
        .as_object()
        .map(|servers| servers.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    guest_path(command(binary, cwd, &disabled)?, binary)
}

/// Starts the Mac's own Codex behind the worker protocol so it also answers
/// `miniclaw/vm/*` from the local VM supervisor. No separate Mac worker process or
/// socket exists; the link is an in-process pipe with a one-time key. Inherited MCP
/// servers are disabled by one relaunch, as in `launch`.
pub async fn launch_with_control(binary: &str, cwd: &str, control: crate::worker::Control)
    -> Result<Client, Failure> {
    let mut disabled = Vec::new();
    for _ in 0..2 {
        let command = command(binary, cwd, &disabled)?;
        let mut key = [0; 32];
        key[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        key[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        let (near, far) = tokio::io::duplex(64 * 1024);
        let link = crate::worker::Control { path: control.path.clone(), key: control.key };
        tokio::spawn(crate::worker::serve_with_control(far, key, move || command, None,
            Some(link)));
        let client = crate::worker::connect(near, key).await?;
        let result = match client.call("config/read", json!({ "includeLayers": false })).await {
            Ok(result) => result,
            Err(error) => { client.stop().await; return Err(error); }
        };
        let config = &result["config"];
        if !official(config) || std::env::var_os("OPENAI_BASE_URL").is_some()
            || config["mcp_servers"].get(crate::computer::SERVER).is_some() {
            client.stop().await;
            return Err(Failure::Rejected);
        }
        disabled = config["mcp_servers"].as_object().map(|servers| servers.iter()
            .filter(|(_, server)| server["enabled"] != false)
            .map(|(id, _)| id.clone()).collect()).unwrap_or_default();
        if disabled.is_empty() { return Ok(client); }
        client.stop().await;
    }
    Err(Failure::Rejected)
}

/// Applies process-local restrictions without changing the guest's saved Codex config.
fn command(binary: &str, cwd: &str, disabled: &[String]) -> Result<Command, Failure> {
    let mut command = Command::new(binary);
    command
        .args(["app-server", "--listen", "stdio://"])
        .current_dir(cwd)
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY");
    let policy: serde_json::Map<String, Value> =
        serde_json::from_str(include_str!("../chat-policy.json")).unwrap();
    for (key, value) in policy {
        command.arg("-c").arg(format!("{key}={value}"));
    }
    for id in disabled {
        if !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(Failure::Rejected);
        }
        command
            .arg("-c")
            .arg(format!("mcp_servers.{id}.enabled=false"));
    }
    Ok(command)
}

/// Returns whether provider and endpoint settings match the supported OpenAI defaults.
fn official(config: &Value) -> bool {
    config["model_provider"]
        .as_str()
        .is_none_or(|v| v == "openai")
        && config["openai_base_url"].is_null()
        && config["chatgpt_base_url"]
            .as_str()
            .is_none_or(|v| v.trim_end_matches('/') == "https://chatgpt.com/backend-api")
}

/// Sets the guest runtime search path without changing the login environment.
fn guest_path(mut command: Command, binary: &str) -> Result<Command, Failure> {
    let binary = std::path::Path::new(binary);
    if !binary.is_absolute() {
        return Err(Failure::Rejected);
    }
    let directory = binary.parent().ok_or(Failure::Rejected)?;
    let paths = std::iter::once(directory).chain(
        ["/usr/bin", "/bin", "/usr/sbin", "/sbin"].map(std::path::Path::new),
    );
    command.env("PATH", std::env::join_paths(paths).map_err(|_| Failure::Rejected)?);
    Ok(command)
}

#[cfg(test)]
mod tests {
    // Checks guest environment without starting Codex.
    use super::*;

    /// Child tools can resolve the installed CLI even without a shell profile.
    #[cfg(unix)]
    #[test]
    fn guest_cli_path() {
        let command = guest_path(Command::new("/usr/bin/env"),
            "/guest/Application Support/Runtime/bin/codex").unwrap();
        let path = command.as_std().get_envs()
            .find(|(key, _)| key == &"PATH").and_then(|(_, value)| value);
        assert_eq!(path.and_then(|value| value.to_str()),
            Some("/guest/Application Support/Runtime/bin:/usr/bin:/bin:/usr/sbin:/sbin"));
    }

    /// A relative runtime path cannot introduce the current directory into PATH.
    #[test]
    fn guest_path_is_absolute() {
        assert!(guest_path(Command::new("/usr/bin/env"), "codex").is_err());
    }
}
