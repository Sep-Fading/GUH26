//! A small MCP server over standard input and output, for tests on Windows.
//!
//! It answers exactly as `tests/support/fake-mcp.mjs` does. That script needs Node, which cannot
//! start inside the Windows sandbox yet; this program needs nothing but its own image, so the
//! node suites can run a confined server there. The Mac suites keep the script.

// Encodes and decodes one JSON-RPC frame per line.
use serde_json::{Value, json};
// Reads requests, writes answers and probes files and ports.
use std::{io::{BufRead, Write}, net::{SocketAddr, TcpListener, TcpStream}, time::Duration};

/// Says whether a file can be read, without ever returning its content.
fn can_read(name: &str) -> &'static str {
    match std::env::var_os(name) {
        None => "unset",
        Some(path) => if std::fs::read(path).is_ok() { "read" } else { "denied" },
    }
}

/// Names a failure the way Node names it, so both fixtures give the tests one vocabulary.
fn code(error: &std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::NotFound => "ENOENT",
        std::io::ErrorKind::PermissionDenied => "EPERM",
        std::io::ErrorKind::ConnectionRefused => "ECONNREFUSED",
        std::io::ErrorKind::TimedOut => "ETIMEDOUT",
        std::io::ErrorKind::AddrInUse => "EADDRINUSE",
        // Winsock refuses a sandboxed socket with WSAEACCES, or cannot start at all.
        _ => match error.raw_os_error() { Some(10013) => "EACCES", Some(10107) => "ENOSYS", _ => "EIO" },
    }
}

/// Builds the tool list, with the sandbox probe written into the first description.
fn tools(mode: &str) -> Vec<Value> {
    let secret = std::env::var("API_KEY").map(|value| value.len()).unwrap_or(0);
    let probe = format!("outside:{} inside:{} secret:{secret}", can_read("PROBE_OUTSIDE"), can_read("PROBE_INSIDE"));
    let mut tools = vec![
        json!({ "name": "probe", "description": probe, "inputSchema": { "type": "object", "properties": {} } }),
        json!({ "name": "echo", "title": "Echo", "description": "Returns its text.", "inputSchema": { "type": "object",
            "properties": { "text": { "type": "string" } }, "required": ["text"] } }),
        json!({ "name": "write_note", "description": "Writes a file.", "inputSchema": { "type": "object",
            "properties": { "path": { "type": "string" }, "text": { "type": "string" } }, "required": ["path", "text"] } }),
        json!({ "name": "fail", "description": "Always fails.", "inputSchema": { "type": "object", "properties": {} } }),
        json!({ "name": "snapshot", "description": "Returns a picture, as a viewport screenshot would.", "inputSchema": { "type": "object", "properties": {} } }),
        json!({ "name": "dial", "description": "Connects to a port on this Mac.", "inputSchema": { "type": "object", "properties": { "port": { "type": "number" } }, "required": ["port"] } }),
        json!({ "name": "serve", "description": "Listens on a port on this Mac, as a server an app connects to would.", "inputSchema": { "type": "object", "properties": { "port": { "type": "number" } }, "required": ["port"] } }),
    ];
    if mode == "many" {
        let filler = "Does one small thing in the app, described at length so the whole list is far bigger than a model should be handed. ".repeat(3);
        tools.push(json!({ "name": "play_test_start", "description": "Starts a play test of the open place. Stops with play_test_stop.",
            "inputSchema": { "type": "object", "properties": { "mode": { "type": "string", "enum": ["play", "run"] } }, "required": ["mode"] } }));
        for n in 0..120 {
            let properties: serde_json::Map<String, Value> = ["a", "b", "c", "d"].iter().map(|key| ((*key).to_owned(),
                json!({ "type": "string", "description": format!("The {key} of thing {n}, {}", &filler[..60]) }))).collect();
            tools.push(json!({ "name": format!("thing_{n}"), "description": format!("Thing {n}. {filler}"),
                "inputSchema": { "type": "object", "properties": properties } }));
        }
    }
    tools
}

/// Wraps one line of text as a tool result.
fn said(text: &str, failed: bool) -> Value {
    let mut result = json!({ "content": [{ "type": "text", "text": text }] });
    if failed { result["isError"] = json!(true); }
    result
}

/// Runs one tool; `None` means the tool never answers.
fn call(name: &str, input: &Value, probe: &str) -> Option<Result<Value, String>> {
    let port = input["port"].as_u64().unwrap_or(0) as u16;
    let local = SocketAddr::from(([127, 0, 0, 1], port));
    Some(Ok(match name {
        "echo" => said(&format!("echo: {}", input["text"].as_str().unwrap_or("undefined")), false),
        "fail" => said("It broke.", true),
        "write_note" => match std::fs::write(input["path"].as_str().unwrap_or_default(), input["text"].as_str().unwrap_or_default()) {
            Ok(()) => said("written", false),
            Err(error) => said(code(&error), true),
        },
        "snapshot" => json!({ "content": [{ "type": "text", "text": "Viewport, 1x1." },
            { "type": "image", "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "mimeType": "image/png" },
            { "type": "audio", "data": "AAAA", "mimeType": "audio/wav" }] }),
        "dial" => {
            if input["hang"] == true { return None; }
            match TcpStream::connect_timeout(&local, Duration::from_secs(5)) {
                Ok(_) => said("connected", false),
                Err(error) => said(code(&error), true),
            }
        }
        "serve" => match TcpListener::bind(local) {
            Err(error) => said(code(&error), true),
            Ok(listener) => {
                std::thread::spawn(move || for mut socket in listener.incoming().flatten() { let _ = socket.write_all(b"served\n"); });
                match TcpStream::connect_timeout(&local, Duration::from_secs(5)) {
                    Ok(_) => said("listening, self-connected", false),
                    Err(error) => said(&format!("listening, self-connect {}", code(&error)), true),
                }
            }
        },
        "probe" => said(probe, false),
        other => return Some(Err(format!("Unknown tool: {other}"))),
    }))
}

/// Answers requests line by line until standard input closes.
fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "ok".into());
    if mode == "exit" { std::process::exit(3); }
    let tools = tools(&mode);
    let probe = tools[0]["description"].as_str().unwrap_or_default().to_owned();
    eprintln!("fake-mcp: started");
    let mut output = std::io::stdout().lock();
    let mut send = |message: Value| { let _ = writeln!(output, "{message}"); let _ = output.flush(); };
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(frame) = serde_json::from_str::<Value>(&line) else { continue };
        if mode == "silent" || frame.get("id").is_none() { continue; }
        let (id, params) = (frame["id"].clone(), &frame["params"]);
        match frame["method"].as_str().unwrap_or_default() {
            "initialize" => send(json!({ "jsonrpc": "2.0", "id": id, "result": { "protocolVersion": params["protocolVersion"],
                "capabilities": { "tools": {} }, "serverInfo": { "name": "fake-mcp", "version": "1.0.0" } } })),
            "tools/list" if mode == "notools" => send(json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": [] } })),
            // Two pages, so the client's paging is exercised.
            "tools/list" => send(json!({ "jsonrpc": "2.0", "id": id, "result": if params["cursor"] == "page-2" {
                json!({ "tools": tools[2..] }) } else { json!({ "tools": tools[..2], "nextCursor": "page-2" }) } })),
            "tools/call" => match call(params["name"].as_str().unwrap_or_default(), &params["arguments"], &probe) {
                None => {}
                Some(Ok(result)) => send(json!({ "jsonrpc": "2.0", "id": id, "result": result })),
                Some(Err(message)) => send(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32602, "message": message } })),
            },
            _ => send(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "Method not found" } })),
        }
    }
}
