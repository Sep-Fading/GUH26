//! Adapts the installed Windows Computer Use helper to MCP without bypassing app approvals.

// Validates MCP requests and emits bounded image content.
use serde_json::{Value, json};
use std::io;

const ACTIONS: &[(&str, &[&str])] = &[
    ("list_apps", &[]), ("list_windows", &[]), ("launch_app", &["app"]),
    ("get_window", &["app", "id"]), ("activate_window", &["window"]),
    ("get_window_state", &["window", "include_screenshot", "include_text"]),
    ("click", &["window", "x", "y", "click_count", "mouse_button", "screenshotId"]),
    ("click_element", &["window", "element_index", "click_count", "mouse_button"]),
    ("scroll", &["window", "x", "y", "scrollX", "scrollY", "screenshotId"]),
    ("drag", &["window", "from_x", "from_y", "to_x", "to_y", "screenshotId"]),
    ("press_key", &["window", "key"]), ("type_text", &["window", "text"]),
    ("set_value", &["window", "element_index", "value"]),
    ("perform_secondary_action", &["window", "element_index", "action"]),
];

/// Rejects command runners and sensitive apps by Windows executable or packaged-app identity.
fn protected(app: &str) -> bool {
    let value = app.trim().to_lowercase();
    let basename = value.rsplit(['/', '\\']).next().unwrap_or(&value).trim_end_matches(".exe");
    super::protected_app(basename) || ["cmd", "powershell", "pwsh", "wt", "windowsterminal", "conhost",
        "openconsole", "bash", "wsl", "mintty", "regedit", "mmc", "control", "systemsettings",
        "credentialui", "consent", "runas", "rundll32", "taskmgr", "mshta", "cscript", "wscript",
        "python", "python3", "node", "miniclaw-server"].contains(&basename)
        || value.starts_with("microsoft.windowsterminal_") || value.starts_with("windows.immersivecontrolpanel_")
        || value.starts_with("openai.") || value.starts_with("anthropic.")
}

/// Validates only known helper methods and native app/window shapes; metadata is never accepted.
fn request<'a>(method: &str, params: &'a Value) -> io::Result<&'a str> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "Invalid Windows Computer Use request");
    let fields = ACTIONS.iter().find(|(name, _)| *name == method).ok_or_else(invalid)?.1;
    if params.to_string().len() > 2048 { return Err(invalid()); }
    let object = params.as_object().ok_or_else(invalid)?;
    if object.keys().any(|key| !fields.contains(&key.as_str())) { return Err(invalid()); }
    if method == "list_apps" || method == "list_windows" { return Ok("Windows app list"); }
    let app = if fields.contains(&"window") {
        let window = params["window"].as_object().ok_or_else(invalid)?;
        if window.keys().any(|key| !["id", "app"].contains(&key.as_str()))
            || window.get("id").and_then(Value::as_u64).is_none() { return Err(invalid()); }
        params["window"]["app"].as_str().ok_or_else(invalid)?
    } else {
        if method == "get_window" && params["id"].as_u64().is_none() { return Err(invalid()); }
        params["app"].as_str().ok_or_else(invalid)?
    };
    if app.is_empty() || app.len() > 1024 || app.contains(['\0', '\r', '\n']) || protected(app) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "Computer Use cannot access this app"));
    }
    Ok(app)
}

/// Accepts only a matching explicit consent response, never inherited helper allow-list state.
fn approved(frame: &Value, id: &str) -> bool {
    frame["id"] == id && frame.get("error").is_none() && frame["result"]["action"] == "accept"
}

/// Returns image data supplied by the helper without opening helper-returned file paths or URLs.
fn result_content(mut result: Value) -> io::Result<Vec<Value>> {
    let mut images = Vec::new();
    if let Some(shots) = result.get_mut("screenshots").and_then(Value::as_array_mut) {
        if shots.len() > 8 { return Err(io::Error::other("Too many Computer Use screenshots")); }
        for shot in shots {
            let url = shot["url"].as_str().ok_or_else(|| io::Error::other("Missing screenshot data"))?;
            let (mime, data) = ["image/png", "image/jpeg"].iter().find_map(|mime|
                url.strip_prefix(&format!("data:{mime};base64,")).map(|data| (*mime, data)))
                .ok_or_else(|| io::Error::other("Computer Use screenshot must contain image data"))?;
            if data.len() > 16 * 1024 * 1024 { return Err(io::Error::other("Computer Use screenshot is too large")); }
            images.push(json!({"type":"image", "mimeType":mime, "data":data}));
            shot.as_object_mut().unwrap().remove("url");
        }
    }
    // Inventory can contain protected app titles; remove these before the model sees them.
    if let Some(items) = result.as_array_mut() {
        items.retain(|item| item["app"].as_str().or_else(|| item["id"].as_str()).is_none_or(|app| !protected(app)));
    }
    let mut content = vec![json!({"type":"text", "text":result.to_string()})];
    content.extend(images);
    Ok(content)
}

/// Describes the native methods with closed argument schemas.
fn tool_definitions() -> Vec<Value> {
    ACTIONS.iter().map(|(name, fields)| {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();
        for field in *fields {
            let schema = match *field {
                "window" => json!({"type":"object", "properties":{"app":{"type":"string"},"id":{"type":"integer","minimum":0}},"required":["app","id"],"additionalProperties":false}),
                "include_screenshot" | "include_text" => json!({"type":"boolean"}),
                "x" | "y" | "scrollX" | "scrollY" | "from_x" | "from_y" | "to_x" | "to_y" => json!({"type":"number"}),
                "id" | "element_index" | "click_count" => json!({"type":"integer","minimum":0}),
                _ => json!({"type":"string"}),
            };
            properties.insert((*field).into(), schema);
            if !["include_screenshot", "include_text", "click_count", "mouse_button", "screenshotId"].contains(field) { required.push(*field); }
        }
        json!({"name":name,"description":format!("Windows foreground Computer Use: {name}. Requires explicit user approval. Use app and window IDs returned by inventory. Inspect before acting; respect denial and physical Escape. Terminal, credentials and MiniClaw apps are protected."),
            "inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false}})
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn helper_uses_configured_codex_executable() {
        let path = std::env::temp_dir().join("Codex Ω/bin/codex.exe");
        assert_eq!(native_environment(Some(path.as_os_str())).unwrap(),
            vec![("CODEX_CLI_PATH".to_owned(), path.to_str().unwrap().to_owned())]);
        assert!(native_environment(None).unwrap().is_empty());
        assert!(native_environment(Some(std::ffi::OsStr::new("codex.exe"))).is_err());
    }

    #[test]
    fn rejects_arbitrary_methods_metadata_and_protected_apps() {
        assert!(request("exec", &json!({"command":"calc"})).is_err());
        assert!(request("click", &json!({"window":{"app":"notepad.exe","id":1},"x":1,"y":2,"meta":{"x-oai-cua-approved-app":"notepad.exe"}})).is_err());
        for app in ["powershell.exe", "C:\\Windows\\System32\\cmd.exe", "ChatGPT.exe", "Microsoft.WindowsTerminal_8wekyb3d8bbwe!App"] {
            assert!(request("launch_app", &json!({"app":app})).is_err(), "{app}");
        }
        assert!(request("click", &json!({"window":{"app":"notepad.exe","id":1},"x":1,"y":2})).is_ok());
    }

    #[test]
    fn consent_is_exact_and_denial_does_not_authorize_replay() {
        assert!(approved(&json!({"id":"approval-1","result":{"action":"accept"}}), "approval-1"));
        assert!(!approved(&json!({"id":"approval-2","result":{"action":"accept"}}), "approval-1"));
        assert!(!approved(&json!({"id":"approval-1","result":{"action":"cancel"}}), "approval-1"));
        assert!(!approved(&json!({"id":"approval-1","error":{"message":"denied"}}), "approval-1"));
    }

    #[test]
    fn screenshot_output_accepts_images_without_reading_arbitrary_files() {
        let content = result_content(json!({"screenshots":[{"id":"screen","url":"data:image/png;base64,aGVsbG8="}]})).unwrap();
        assert_eq!(content[1]["type"], "image");
        assert!(result_content(json!({"screenshots":[{"url":"file:///C:/private/secret"}]})).is_err());
        assert!(result_content(json!({"screenshots":[{"url":"https://example.com/image.png"}]})).is_err());
    }
}

// Streams bounded MCP lines and cancels native calls when the MCP connection disappears.
use futures_util::StreamExt;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio_util::codec::{FramedRead, LinesCodec};
use std::{collections::VecDeque, future::Future, time::Duration};

trait Helper {
    fn call<'a>(&'a mut self, method: &'a str, params: &'a Value, approved_app: Option<&'a str>)
        -> impl Future<Output = io::Result<Value>> + 'a;
}

/// Writes one complete MCP frame without logging screen content.
async fn send(output: &mut (impl AsyncWrite + Unpin), frame: Value) -> io::Result<()> {
    output.write_all(format!("{frame}\n").as_bytes()).await?;
    output.flush().await
}

/// The most requests kept while another one is running; later ones are refused as busy.
const WAITING: usize = 32;

/// What one frame meant that arrived while a request was being worked on.
enum Aside { Handled, Cancelled, Response(Value) }

/// Handles a frame that arrives during a request: a ping is answered at once, another request
/// is kept for later, a cancellation of the running request is reported and one of a waiting
/// request removes it. Other notifications are ignored. A response is handed back.
async fn aside(frame: Value, current: &Value, output: &mut (impl AsyncWrite + Unpin),
    waiting: &mut VecDeque<Value>) -> io::Result<Aside> {
    let Some(method) = frame.get("method").and_then(Value::as_str) else { return Ok(Aside::Response(frame)); };
    match frame.get("id") {
        Some(id) if method == "ping" => send(output, json!({"jsonrpc":"2.0","id":id,"result":{}})).await?,
        Some(id) if waiting.len() >= WAITING => send(output, json!({"jsonrpc":"2.0","id":id,
            "error":{"code":-32000,"message":"Computer Use is busy with another request"}})).await?,
        Some(_) => waiting.push_back(frame),
        None if method == "notifications/cancelled" => {
            let cancelled = &frame["params"]["requestId"];
            if cancelled == current { return Ok(Aside::Cancelled); }
            waiting.retain(|kept| kept.get("id") != Some(cancelled));
        }
        None => {}
    }
    Ok(Aside::Handled)
}

/// Reads one frame; the end of the MCP stream is the only disconnect.
async fn incoming<R: AsyncRead + Unpin>(input: &mut FramedRead<R, LinesCodec>) -> io::Result<Option<Value>> {
    match input.next().await {
        Some(line) => Ok(Some(serde_json::from_str(&line.map_err(io::Error::other)?)?)),
        None => Ok(None),
    }
}

/// Waits for native work while the MCP stream stays served. Only the stream's end interrupts
/// the work. A cancelled request still lets the native call finish, so the helper is never
/// given two actions at once; `None` then tells the caller to drop the result unanswered.
async fn during<T, R: AsyncRead + Unpin>(input: &mut FramedRead<R, LinesCodec>, output: &mut (impl AsyncWrite + Unpin),
    waiting: &mut VecDeque<Value>, current: &Value, work: impl Future<Output = T>) -> io::Result<Option<T>> {
    tokio::pin!(work);
    let mut cancelled = false;
    loop {
        tokio::select! {
            value = &mut work => return Ok((!cancelled).then_some(value)),
            frame = incoming(input) => {
                let Some(frame) = frame? else {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "Computer Use connection interrupted"));
                };
                cancelled |= matches!(aside(frame, current, output, waiting).await?, Aside::Cancelled);
            }
        }
    }
}

/// Asks through the existing MiniClaw approval UI; there is no persistent approval fast path.
/// Only the response to this question decides; `None` means the request itself was cancelled.
async fn consent<R: AsyncRead + Unpin>(input: &mut FramedRead<R, LinesCodec>, output: &mut (impl AsyncWrite + Unpin),
    waiting: &mut VecDeque<Value>, current: &Value, app: &str, method: &str, params: &Value) -> io::Result<Option<bool>> {
    let id = uuid::Uuid::new_v4().to_string();
    send(output, json!({"jsonrpc":"2.0","id":id,"method":"elicitation/create","params":{
        "mode":"form","message":format!("Allow MiniClaw to use {app}? Action: {method}. Exact inputs: {params}. Desktop actions use your normal app access, outside the shell sandbox."),
        "requestedSchema":{"type":"object","properties":{},"required":[]},
        "_meta":{"tool_params":{"app":app}}
    }})).await?;
    let expiry = tokio::time::sleep(Duration::from_secs(55));
    tokio::pin!(expiry);
    loop {
        let frame = tokio::select! {
            _ = &mut expiry => return Err(io::Error::new(io::ErrorKind::TimedOut,"Computer Use approval expired")),
            frame = incoming(input) => frame?.ok_or(io::ErrorKind::UnexpectedEof)?,
        };
        match aside(frame, current, output, waiting).await? {
            Aside::Handled => {}
            Aside::Response(response) => return Ok(Some(approved(&response, &id))),
            Aside::Cancelled => {
                // The question is withdrawn with the request it belonged to.
                send(output, json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":id}})).await?;
                return Ok(None);
            }
        }
    }
}

/// Serves only declared native actions, obtaining fresh user consent before any helper request.
async fn serve<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(reader: R, mut output: W, helper: &mut impl Helper) -> io::Result<()> {
    let mut input = FramedRead::new(reader, LinesCodec::new_with_max_length(1024 * 1024));
    let mut elicitations = false;
    let mut halted = false;
    let mut known_apps = std::collections::HashSet::<String>::new();
    // Requests that arrived while another one was running, in their order.
    let mut waiting = VecDeque::<Value>::new();
    loop {
        let frame = match waiting.pop_front() {
            Some(frame) => frame,
            None => match incoming(&mut input).await? { Some(frame) => frame, None => break },
        };
        // A late response, such as to a withdrawn approval, is not a request.
        if frame.get("method").is_none() { continue; }
        let Some(id) = frame.get("id").cloned() else { continue; };
        let result: io::Result<Value> = match frame["method"].as_str() {
            Some("initialize") => {
                elicitations = frame["params"]["capabilities"].get("elicitation").is_some();
                Ok(json!({"protocolVersion":frame["params"]["protocolVersion"],"capabilities":{"tools":{}},
                    "serverInfo":{"name":"miniclaw-windows-computer","version":"1.0.0"}}))
            }
            Some("ping") => Ok(json!({})),
            Some("tools/list") => Ok(json!({"tools":tool_definitions()})),
            Some("tools/call") => {
                let method = frame["params"]["name"].as_str().unwrap_or("");
                let args = &frame["params"]["arguments"];
                match request(method, args) {
                    Err(error) => Err(error),
                    Ok(_) if !elicitations => Err(io::Error::new(io::ErrorKind::PermissionDenied,"Computer Use requires app approval support")),
                    Ok(app) => {
                        if method == "launch_app" && !known_apps.contains(app) {
                            send(&mut output, json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":"Use an exact app ID returned by list_apps before launching"}})).await?;
                            continue;
                        }
                        // A cancelled request is dropped without a response, as MCP asks.
                        let Some(allowed) = consent(&mut input, &mut output, &mut waiting, &id, app, method, args).await? else { continue; };
                        if !allowed {
                            Err(io::Error::new(io::ErrorKind::PermissionDenied,"Computer Use was not approved"))
                        } else {
                            if let Some(window_id) = args.get("window").and_then(|window| window.get("id")) {
                                // Resolve actual HWND ownership; a model-supplied app name grants nothing.
                                let lookup = json!({"id":window_id});
                                let Some(canonical) = during(&mut input, &mut output, &mut waiting, &id,
                                    tokio::time::timeout(Duration::from_secs(15), helper.call("get_window", &lookup, None))).await? else { continue; };
                                let canonical = canonical.map_err(|_| io::Error::new(io::ErrorKind::TimedOut,"Computer Use window lookup timed out"))??;
                                if canonical["ok"] != true || canonical["result"]["id"] != *window_id
                                    || canonical["result"]["app"] != app || protected(app) {
                                    send(&mut output, json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":"Window ownership changed or could not be verified; inspect the app list again"}})).await?;
                                    continue;
                                }
                            }
                            let mut granted: Option<String> = None;
                            let mut result = Err(io::Error::other("Computer Use requested too many approvals"));
                            let mut cancelled = false;
                            for _ in 0..3 {
                                let Some(response) = during(&mut input, &mut output, &mut waiting, &id,
                                    tokio::time::timeout(Duration::from_secs(20), helper.call(method, args, granted.as_deref()))).await?
                                    else { cancelled = true; break; };
                                let response = response.map_err(|_| io::Error::new(io::ErrorKind::TimedOut,"Computer Use helper timed out"))??;
                                if response["ok"] == true {
                                    if method == "get_window" && (response["result"]["app"] != app
                                        || response["result"]["id"] != args["id"] || protected(app)) {
                                        result = Err(io::Error::new(io::ErrorKind::PermissionDenied,"Window identity did not match the requested app"));
                                        break;
                                    }
                                    if method == "list_apps" {
                                        known_apps.clear();
                                        if let Some(apps) = response["result"].as_array() {
                                            known_apps.extend(apps.iter().filter_map(|item| item["id"].as_str())
                                                .filter(|app| !protected(app)).map(str::to_owned));
                                        }
                                    }
                                    result = result_content(response["result"].clone()).map(|content| json!({"content":content}));
                                    break;
                                }
                                if let Some(app) = response["approvalRequest"]["app"].as_str() {
                                    if app.len() > 1024 || protected(app) || app.contains(['\r','\n','\0']) {
                                        result = Err(io::Error::new(io::ErrorKind::PermissionDenied,"Computer Use was not approved for the requested app"));
                                        break;
                                    }
                                    match consent(&mut input, &mut output, &mut waiting, &id, app, method, args).await? {
                                        Some(true) => granted = Some(app.to_owned()),
                                        Some(false) => {
                                            result = Err(io::Error::new(io::ErrorKind::PermissionDenied,"Computer Use was not approved for the requested app"));
                                            break;
                                        }
                                        None => { cancelled = true; break; }
                                    }
                                } else {
                                    // Native errors may contain window text. Stop this bridge after refusal or Escape.
                                    halted = true;
                                    result = Err(io::Error::other("Computer Use helper refused the action or the user stopped it. Stop Computer Use and report this failure."));
                                    break;
                                }
                            }
                            if cancelled { continue; }
                            result
                        }
                    }
                }
            }
            _ => Err(io::Error::new(io::ErrorKind::Unsupported,"Unsupported MCP method")),
        };
        let response = match result {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(error) => json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":error.to_string()}}),
        };
        send(&mut output, response).await?;
        if halted { break; }
    }
    Ok(())
}

#[cfg(windows)]
struct Native {
    process: crate::windows_process::Process,
    input: tokio::fs::File,
    output: FramedRead<tokio::fs::File, LinesCodec>,
    next: u64,
}

#[cfg(windows)]
impl Helper for Native {
    /// Forwards one approved action; only the adapter can construct native approval metadata.
    async fn call(&mut self, method: &str, params: &Value, approved_app: Option<&str>) -> io::Result<Value> {
        if self.process.try_wait()?.is_some() { return Err(io::Error::other("Computer Use helper stopped; do not retry this turn")); }
        self.next += 1;
        let mut frame = json!({"id":self.next,"method":method,"params":params,
            "meta":{"x-oai-cua-request-budget-ms":15000}});
        if let Some(app) = approved_app { frame["meta"]["x-oai-cua-approved-app"] = json!(app); }
        send(&mut self.input, frame).await?;
        while let Some(line) = self.output.next().await {
            let response: Value = serde_json::from_str(&line.map_err(io::Error::other)?)?;
            if response["id"] == self.next { return Ok(response); }
        }
        Err(io::Error::other("Computer Use helper stopped; do not retry this turn"))
    }
}

/// Gives native authentication the reviewed Codex executable rather than an unrelated PATH entry.
#[cfg(any(windows, test))]
fn native_environment(codex: Option<&std::ffi::OsStr>) -> io::Result<Vec<(String, String)>> {
    let Some(codex) = codex.filter(|value| !value.is_empty()) else { return Ok(Vec::new()); };
    let path = std::path::Path::new(codex);
    if !path.is_absolute() { return Err(io::Error::other("Computer Use needs an absolute configured Codex path")); }
    let value = codex.to_str().ok_or_else(|| io::Error::other("Codex path is not valid Unicode"))?;
    Ok(vec![("CODEX_CLI_PATH".into(), value.into())])
}

/// Starts the installed helper with the real parent PID and restricted handle inheritance.
#[cfg(windows)]
pub async fn run(path: &std::ffi::OsStr) -> io::Result<()> {
    let path = std::path::Path::new(path);
    if !path.is_absolute() || !path.is_file() { return Err(io::ErrorKind::InvalidInput.into()); }
    let environment = native_environment(std::env::var_os("CODEX_BIN").as_deref())?;
    let child = crate::windows_process::stdio(path, &["--parent-pid".into(), std::process::id().to_string()],
        path.parent().ok_or(io::ErrorKind::InvalidInput)?, &environment)?;
    // Drain diagnostic output without exposing screenshots, titles, or credentials.
    let mut errors = tokio::fs::File::from_std(child.errors);
    tokio::spawn(async move { let _ = tokio::io::copy(&mut errors, &mut tokio::io::sink()).await; });
    let mut native = Native { process:child.process, input:tokio::fs::File::from_std(child.input),
        output:FramedRead::new(tokio::fs::File::from_std(child.output), LinesCodec::new_with_max_length(32 * 1024 * 1024)), next:0 };
    serve(tokio::io::stdin(), tokio::io::stdout(), &mut native).await
}

#[cfg(test)]
mod protocol_tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader};

    struct Fake { calls: usize }
    impl Helper for Fake {
        async fn call(&mut self, _method: &str, _params: &Value, _approved_app: Option<&str>) -> io::Result<Value> {
            self.calls += 1;
            Ok(json!({"ok":true,"result":[]}))
        }
    }

    struct NativeApproval { calls: Vec<Option<String>> }
    impl Helper for NativeApproval {
        async fn call(&mut self, _method: &str, _params: &Value, app: Option<&str>) -> io::Result<Value> {
            self.calls.push(app.map(str::to_owned));
            Ok(if app.is_none() { json!({"ok":false,"approvalRequest":{"app":"notepad.exe","displayName":"Notepad"}}) }
                else { json!({"ok":true,"result":{}}) })
        }
    }

    /// The helper's canonical app identity gets another real popup before approval metadata exists.
    #[tokio::test]
    async fn native_approval_metadata_is_added_only_after_consent() {
        let (client, server) = tokio::io::duplex(16 * 1024);
        let (reader, writer) = tokio::io::split(server);
        let task = tokio::spawn(async move {
            let mut helper = NativeApproval { calls: Vec::new() };
            serve(reader, writer, &mut helper).await.unwrap();
            helper.calls
        });
        let (reader, mut writer) = tokio::io::split(client);
        let mut reader = BufReader::new(reader).lines();
        send(&mut writer, json!({"id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{"elicitation":{}}}})).await.unwrap();
        reader.next_line().await.unwrap();
        send(&mut writer, json!({"id":2,"method":"tools/call","params":{"name":"list_apps","arguments":{}}})).await.unwrap();
        for _ in 0..2 {
            let approval: Value = serde_json::from_str(&reader.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(approval["method"], "elicitation/create");
            assert!(["notepad.exe", "Windows app list"].contains(&approval["params"]["_meta"]["tool_params"]["app"].as_str().unwrap()));
            send(&mut writer, json!({"id":approval["id"],"result":{"action":"accept"}})).await.unwrap();
        }
        let result: Value = serde_json::from_str(&reader.next_line().await.unwrap().unwrap()).unwrap();
        assert!(result.get("error").is_none());
        drop(writer); drop(reader);
        assert_eq!(task.await.unwrap(), vec![None, Some("notepad.exe".into())]);
    }

    /// Expiry closes the bridge without touching the native process.
    #[tokio::test(start_paused = true)]
    async fn expired_approval_does_not_execute() {
        let (client, server) = tokio::io::duplex(16 * 1024);
        let (reader, writer) = tokio::io::split(server);
        let task = tokio::spawn(async move {
            let mut helper = Fake { calls:0 };
            let result = serve(reader, writer, &mut helper).await;
            (result.unwrap_err().kind(), helper.calls)
        });
        let (reader, mut writer) = tokio::io::split(client);
        let mut reader = BufReader::new(reader).lines();
        send(&mut writer, json!({"id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{"elicitation":{}}}})).await.unwrap();
        reader.next_line().await.unwrap();
        send(&mut writer, json!({"id":2,"method":"tools/call","params":{"name":"list_apps","arguments":{}}})).await.unwrap();
        reader.next_line().await.unwrap();
        tokio::time::advance(Duration::from_secs(56)).await;
        assert_eq!(task.await.unwrap(), (io::ErrorKind::TimedOut, 0));
    }

    /// Drives actual MCP framing; denying consent never calls the native helper.
    #[tokio::test]
    async fn denied_consent_never_reaches_the_native_helper() {
        for allow in [false, true] {
            let (client, server) = tokio::io::duplex(16 * 1024);
            let (reader, writer) = tokio::io::split(server);
            let task = tokio::spawn(async move {
                let mut helper = Fake { calls:0 };
                serve(reader, writer, &mut helper).await.unwrap();
                helper.calls
            });
            let (reader, mut writer) = tokio::io::split(client);
            let mut reader = BufReader::new(reader).lines();
            send(&mut writer, json!({"id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{"elicitation":{}}}})).await.unwrap();
            reader.next_line().await.unwrap().unwrap();
            send(&mut writer, json!({"id":2,"method":"tools/call","params":{"name":"list_apps","arguments":{}}})).await.unwrap();
            let approval: Value = serde_json::from_str(&reader.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(approval["method"],"elicitation/create");
            send(&mut writer, json!({"id":approval["id"],"result":{"action":if allow {"accept"} else {"decline"}}})).await.unwrap();
            let result: Value = serde_json::from_str(&reader.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(result.get("error").is_some(), !allow);
            drop(writer);
            drop(reader);
            assert_eq!(task.await.unwrap(), usize::from(allow));
        }
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader};
    struct Impostor { methods: Vec<String> }
    impl Helper for Impostor {
        async fn call(&mut self, method: &str, _params: &Value, _app: Option<&str>) -> io::Result<Value> {
            self.methods.push(method.into());
            Ok(json!({"ok":true,"result":{"id":1,"app":"powershell.exe"}}))
        }
    }
    #[tokio::test]
    async fn spoofed_window_identity_never_reaches_the_action() {
        let (client, server) = tokio::io::duplex(16 * 1024);
        let (reader, writer) = tokio::io::split(server);
        let task = tokio::spawn(async move {
            let mut helper = Impostor { methods:vec![] };
            serve(reader, writer, &mut helper).await.unwrap();
            helper.methods
        });
        let (reader, mut writer) = tokio::io::split(client);
        let mut reader = BufReader::new(reader).lines();
        send(&mut writer, json!({"id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{"elicitation":{}}}})).await.unwrap();
        reader.next_line().await.unwrap();
        send(&mut writer, json!({"id":2,"method":"tools/call","params":{"name":"click","arguments":{"window":{"id":1,"app":"notepad.exe"},"x":1,"y":1}}})).await.unwrap();
        let approval: Value = serde_json::from_str(&reader.next_line().await.unwrap().unwrap()).unwrap();
        send(&mut writer, json!({"id":approval["id"],"result":{"action":"accept"}})).await.unwrap();
        let result: Value = serde_json::from_str(&reader.next_line().await.unwrap().unwrap()).unwrap();
        assert!(result.get("error").is_some());
        drop(writer); drop(reader);
        assert_eq!(task.await.unwrap(), vec!["get_window"]);
    }
}

#[cfg(test)]
mod interleaving_tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::{AsyncBufReadExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf};
    use tokio::sync::Notify;

    /// A helper whose call stays open until the test releases it.
    struct Held { started: Arc<Notify>, release: Arc<Notify>, calls: usize }
    impl Helper for Held {
        async fn call(&mut self, _method: &str, _params: &Value, _app: Option<&str>) -> io::Result<Value> {
            self.calls += 1;
            self.started.notify_one();
            self.release.notified().await;
            Ok(json!({"ok":true,"result":[]}))
        }
    }

    type Client = (Lines<BufReader<ReadHalf<DuplexStream>>>, WriteHalf<DuplexStream>);

    /// Starts the bridge and returns the client side with its helper controls.
    fn bridge() -> (tokio::task::JoinHandle<(io::Result<()>, usize)>, Client, Arc<Notify>, Arc<Notify>) {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (reader, writer) = tokio::io::split(server);
        let (started, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        let mut helper = Held { started: started.clone(), release: release.clone(), calls: 0 };
        let task = tokio::spawn(async move {
            let result = serve(reader, writer, &mut helper).await;
            (result, helper.calls)
        });
        let (reader, writer) = tokio::io::split(client);
        (task, (BufReader::new(reader).lines(), writer), started, release)
    }

    /// Reads the next frame the bridge wrote.
    async fn next(client: &mut Client) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(5), client.0.next_line()).await
            .expect("the bridge must answer").unwrap().unwrap();
        serde_json::from_str(&line).unwrap()
    }

    /// Initializes, asks for the app list and approves it, leaving the helper call open.
    async fn open_call(client: &mut Client, started: &Notify) {
        send(&mut client.1, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{"elicitation":{}}}})).await.unwrap();
        assert_eq!(next(client).await["id"], 1);
        send(&mut client.1, json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_apps","arguments":{}}})).await.unwrap();
        let approval = next(client).await;
        assert_eq!(approval["method"], "elicitation/create");
        send(&mut client.1, json!({"jsonrpc":"2.0","id":approval["id"],"result":{"action":"accept"}})).await.unwrap();
        started.notified().await;
    }

    /// A ping during a helper call is answered at once and the call still completes.
    #[tokio::test]
    async fn ping_during_a_helper_call_is_answered_and_the_call_completes() {
        let (task, mut client, started, release) = bridge();
        open_call(&mut client, &started).await;
        send(&mut client.1, json!({"jsonrpc":"2.0","id":9,"method":"ping"})).await.unwrap();
        let pong = next(&mut client).await;
        assert_eq!(pong, json!({"jsonrpc":"2.0","id":9,"result":{}}));
        release.notify_one();
        let result = next(&mut client).await;
        assert_eq!(result["id"], 2);
        assert!(result.get("error").is_none(), "{result}");
        drop(client);
        let (served, calls) = task.await.unwrap();
        served.unwrap();
        assert_eq!(calls, 1);
    }

    /// A second request during a helper call is kept and answered after the first.
    #[tokio::test]
    async fn a_second_request_during_a_helper_call_is_answered_afterwards() {
        let (task, mut client, started, release) = bridge();
        open_call(&mut client, &started).await;
        send(&mut client.1, json!({"jsonrpc":"2.0","id":10,"method":"tools/list"})).await.unwrap();
        send(&mut client.1, json!({"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":"x","progress":1}})).await.unwrap();
        release.notify_one();
        let first = next(&mut client).await;
        assert_eq!(first["id"], 2);
        assert!(first.get("error").is_none(), "{first}");
        let second = next(&mut client).await;
        assert_eq!(second["id"], 10);
        assert!(second["result"]["tools"].is_array(), "{second}");
        drop(client);
        assert_eq!(task.await.unwrap().1, 1);
    }

    /// Cancelling the running request leaves it unanswered; the bridge keeps serving.
    #[tokio::test]
    async fn a_cancel_notification_ends_only_the_running_request() {
        let (task, mut client, started, release) = bridge();
        open_call(&mut client, &started).await;
        // A cancellation that names another request changes nothing.
        send(&mut client.1, json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":77}})).await.unwrap();
        send(&mut client.1, json!({"jsonrpc":"2.0","id":11,"method":"ping"})).await.unwrap();
        assert_eq!(next(&mut client).await["id"], 11);
        send(&mut client.1, json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2,"reason":"user"}})).await.unwrap();
        send(&mut client.1, json!({"jsonrpc":"2.0","id":12,"method":"ping"})).await.unwrap();
        assert_eq!(next(&mut client).await, json!({"jsonrpc":"2.0","id":12,"result":{}}));
        // The native call ends, and the cancelled request still gets no response.
        release.notify_one();
        send(&mut client.1, json!({"jsonrpc":"2.0","id":13,"method":"ping"})).await.unwrap();
        assert_eq!(next(&mut client).await, json!({"jsonrpc":"2.0","id":13,"result":{}}));
        drop(client);
        let (served, calls) = task.await.unwrap();
        served.unwrap();
        assert_eq!(calls, 1);
    }

    /// Cancelling during the approval withdraws the question and never reaches the helper.
    #[tokio::test]
    async fn a_cancel_during_consent_never_reaches_the_helper() {
        let (task, mut client, _started, _release) = bridge();
        send(&mut client.1, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{"elicitation":{}}}})).await.unwrap();
        next(&mut client).await;
        send(&mut client.1, json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_apps","arguments":{}}})).await.unwrap();
        let approval = next(&mut client).await;
        send(&mut client.1, json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2}})).await.unwrap();
        let withdrawn = next(&mut client).await;
        assert_eq!(withdrawn["method"], "notifications/cancelled");
        assert_eq!(withdrawn["params"]["requestId"], approval["id"]);
        // A late approval of the withdrawn question authorizes nothing.
        send(&mut client.1, json!({"jsonrpc":"2.0","id":approval["id"],"result":{"action":"accept"}})).await.unwrap();
        send(&mut client.1, json!({"jsonrpc":"2.0","id":3,"method":"ping"})).await.unwrap();
        assert_eq!(next(&mut client).await, json!({"jsonrpc":"2.0","id":3,"result":{}}));
        drop(client);
        let (served, calls) = task.await.unwrap();
        served.unwrap();
        assert_eq!(calls, 0);
    }

    /// Only the end of the MCP stream interrupts a helper call.
    #[tokio::test]
    async fn end_of_input_during_a_helper_call_interrupts_it() {
        let (task, mut client, started, _release) = bridge();
        open_call(&mut client, &started).await;
        drop(client);
        let (served, _) = task.await.unwrap();
        assert_eq!(served.unwrap_err().kind(), io::ErrorKind::Interrupted);
    }

    /// A ping while the approval is open is answered and does not count as a denial.
    #[tokio::test]
    async fn ping_during_consent_is_not_a_denial() {
        let (task, mut client, started, release) = bridge();
        send(&mut client.1, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{"elicitation":{}}}})).await.unwrap();
        next(&mut client).await;
        send(&mut client.1, json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_apps","arguments":{}}})).await.unwrap();
        let approval = next(&mut client).await;
        send(&mut client.1, json!({"jsonrpc":"2.0","id":9,"method":"ping"})).await.unwrap();
        assert_eq!(next(&mut client).await, json!({"jsonrpc":"2.0","id":9,"result":{}}));
        send(&mut client.1, json!({"jsonrpc":"2.0","id":approval["id"],"result":{"action":"accept"}})).await.unwrap();
        started.notified().await;
        release.notify_one();
        let result = next(&mut client).await;
        assert_eq!(result["id"], 2);
        assert!(result.get("error").is_none(), "{result}");
        drop(client);
        assert_eq!(task.await.unwrap().1, 1);
    }
}
