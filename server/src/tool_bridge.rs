// Speaks line-delimited MCP on stdio.
use std::io::{self, BufRead, BufReader, Write};
// Reaches the backend's private tool socket.
#[cfg(unix)]
use std::os::unix::net::UnixStream;
// Bounds each wait for a decision.
use std::time::Duration;

// Builds MCP replies and tool results.
use serde_json::{Value, json};

/// Serves the one MiniClaw tool to a Claude Code CLI over stdio MCP. Every call is
/// forwarded to the backend, which applies the conversation's permissions and sandbox;
/// this process holds no authority of its own beyond its per-turn token.
pub fn run() -> io::Result<()> {
    let socket = std::env::var_os("MINICLAW_TOOL_SOCKET").ok_or(io::ErrorKind::NotFound)?;
    let token = std::env::var("MINICLAW_TOOL_TOKEN").map_err(|_| io::ErrorKind::NotFound)?;
    let mut output = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let Ok(frame) = serde_json::from_str::<Value>(&line?) else { continue; };
        // Notifications carry no id and need no reply.
        let Some(id) = frame.get("id").filter(|id| !id.is_null()) else { continue; };
        let reply = match frame["method"].as_str().unwrap_or_default() {
            "initialize" => json!({ "result": {
                "protocolVersion": frame["params"]["protocolVersion"].as_str()
                    .unwrap_or("2025-06-18"),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "miniclaw", "version": env!("CARGO_PKG_VERSION") },
            } }),
            "ping" => json!({ "result": {} }),
            "tools/list" => {
                let listed: Vec<Value> = tools().iter().map(|tool| json!({ "name": tool["name"],
                    "description": tool["description"], "inputSchema": tool["inputSchema"] })).collect();
                json!({ "result": { "tools": listed } })
            }
            "tools/call" if tools().iter().any(|tool| tool["name"] == frame["params"]["name"]) =>
                json!({ "result": call(&socket, &token, frame["params"]["name"].as_str().unwrap_or_default(),
                    &frame["params"]["arguments"]) }),
            _ => json!({ "error": { "code": -32601, "message": "Method not found" } }),
        };
        let mut reply = reply;
        reply["jsonrpc"] = json!("2.0");
        reply["id"] = id.clone();
        writeln!(output, "{reply}")?;
        output.flush()?;
    }
    Ok(())
}

/// Lists the tools the backend offered this conversation: the file and command tool, the
/// delegation tool when the conversation's agent has recipients, and the MCP gateway. The backend checks every
/// call again, so listing a tool here grants nothing.
fn tools() -> Vec<Value> {
    let mut tools = Vec::new();
    if std::env::var_os("MINICLAW_NO_FILE_TOOL").is_none() { tools.push(crate::scoped::tool_definition()); }
    if let Some(delegate) = std::env::var("MINICLAW_DELEGATE_TOOL").ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(|tool| tool["name"] == "miniclaw_delegate") { tools.push(delegate); }
    if std::env::var_os("MINICLAW_MCP_TOOL").is_some() { tools.push(crate::mcp_gateway::tool_definition()); }
    if std::env::var_os("MINICLAW_ASK_TOOL").is_some() { tools.push(crate::ask::tool_definition()); }
    if std::env::var_os("MINICLAW_PROJECT_TOOL").is_some() { tools.push(crate::project_tool::tool_definition()); }
    tools
}

/// Forwards one call and converts the backend's bounded result into MCP content.
/// Any transport failure is reported to the model as a refused operation.
fn call(socket: &std::ffi::OsStr, token: &str, tool: &str, arguments: &Value) -> Value {
    let refused = json!({ "isError": true, "content": [{ "type": "text",
        "text": "MiniClaw refused or could not complete this operation." }] });
    let exchange = || -> io::Result<Value> {
        let mut stream = connect(socket)?;
        // The backend bounds every call itself; this only ends one it will never answer.
        stream.set_read_timeout(Some(Duration::from_secs(3960)))?;
        writeln!(stream, "{}", json!({ "token": token, "tool": tool, "arguments": arguments }))?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        serde_json::from_str(&line).map_err(Into::into)
    };
    let Ok(result) = exchange() else { return refused; };
    // Text stays text, and a picture (a data URL from the backend) becomes MCP image content.
    let content: Vec<Value> = result["contentItems"].as_array().into_iter().flatten().filter_map(|item| {
        if let Some(text) = item["text"].as_str() { return Some(json!({ "type": "text", "text": text })); }
        let (kind, data) = item["imageUrl"].as_str()?.strip_prefix("data:")?.split_once(";base64,")?;
        Some(json!({ "type": "image", "data": data, "mimeType": kind }))
    }).collect();
    if content.is_empty() { return refused; }
    // A refusal with a reason reaches the model as an error with that reason, so it can adapt.
    json!({ "isError": result["success"] != true, "content": content })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bridge address cannot make the shim send its token off the host.
    #[test]
    fn loopback_address_only() {
        assert!(loopback_address("127.0.0.1:1234").is_ok());
        for address in ["192.168.1.2:1234", "0.0.0.0:1234", "localhost:1234", "127.0.0.1:0"] {
            assert!(loopback_address(address).is_err(), "{address}");
        }
    }
}

/// Parses only numeric localhost endpoints before sending a per-turn credential.
#[cfg(any(windows, test))]
fn loopback_address(address: &str) -> io::Result<std::net::SocketAddr> {
    let address: std::net::SocketAddr = address.parse()
        .map_err(|_| io::ErrorKind::InvalidInput)?;
    if address.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST) || address.port() == 0 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(address)
}

/// Connects the shim through its private Unix endpoint.
#[cfg(unix)]
fn connect(socket: &std::ffi::OsStr) -> io::Result<UnixStream> {
    UnixStream::connect(socket)
}

/// Connects the Windows shim to an authenticated loopback bridge with a bounded wait.
#[cfg(windows)]
fn connect(socket: &std::ffi::OsStr) -> io::Result<std::net::TcpStream> {
    let address = loopback_address(socket.to_str().ok_or(io::ErrorKind::InvalidInput)?)?;
    std::net::TcpStream::connect_timeout(&address, Duration::from_secs(5))
}
