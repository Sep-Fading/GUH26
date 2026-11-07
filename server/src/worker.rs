// Uses existing RPC results and failures.
use crate::rpc::{Client, Failure};
// Resolves guest capabilities before forwarding requests.
use crate::guest_computer::{Policy, Route};
// Executes only approved scoped operations under the selected guest account.
use crate::{scoped::Operation, executor};
// Verifies peer proofs without timing comparisons.
use hmac::{Hmac, KeyInit, Mac};
// Reads bounded frames from asynchronous streams.
use futures_util::StreamExt;
// Hashes role-separated worker authentication challenge messages.
use sha2::Sha256;
// Validates complete JSON objects before forwarding.
use serde_json::Value;
// Sets authentication deadlines and child pipes.
use std::{collections::HashMap, path::PathBuf, process::Stdio, sync::Arc, time::Duration};
// Remembers a permission request already made.
use std::sync::atomic::{AtomicBool, Ordering};
// Accepts private streams and process factories.
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    process::Command,
    sync::{Mutex, watch},
};
// Enforces the existing JSON line limit.
use tokio_util::codec::{FramedRead, LinesCodec};
// Generates fresh operating-system-backed random connection challenges.
use uuid::Uuid;

// Connects only the Unix VM supervisor.
#[cfg(unix)]
use tokio::net::UnixStream;

const MAGIC: &[u8; 8] = b"MCWORK01";
const AUTH_TIMEOUT: Duration = Duration::from_secs(3);

/// Holds the latest guest frame and stops its capture process with this connection.
type Frames = Arc<tokio::sync::Mutex<watch::Receiver<Option<Arc<Vec<u8>>>>>>;

/// Returns the next frame this viewer has not had, waiting up to `wait` for one; after
/// that the newest frame is returned again so an idle screen still answers. One shared
/// receiver remembers what was sent, so a frame is neither repeated nor skipped.
async fn next_frame(latest: &Frames, wait: Duration) -> Option<Arc<Vec<u8>>> {
    let mut latest = latest.lock().await;
    match tokio::time::timeout(wait, latest.changed()).await {
        Ok(Err(_)) => None,
        _ => latest.borrow_and_update().clone(),
    }
}

/// Keeps one place in the frame sequence per viewer, so two streams of the same VM each
/// get every frame instead of splitting them. A request without a viewer uses a shared place.
struct Viewers {
    // Never read from, so a copy of it still counts the current frame as new.
    base: watch::Receiver<Option<Arc<Vec<u8>>>>,
    places: HashMap<String, (Frames, tokio::time::Instant)>,
}

impl Viewers {
    /// The most streams one VM serves at once; the longest-idle place goes first.
    const LIMIT: usize = 8;

    fn new(base: watch::Receiver<Option<Arc<Vec<u8>>>>) -> Self {
        Self { base, places: HashMap::new() }
    }

    /// Returns the viewer's place, creating it at the current frame.
    fn place(&mut self, viewer: &str) -> Frames {
        let now = tokio::time::Instant::now();
        if !self.places.contains_key(viewer) && self.places.len() >= Self::LIMIT
            && let Some(oldest) = self.places.iter().min_by_key(|(_, (_, used))| *used)
                .map(|(id, _)| id.clone()) {
            self.places.remove(&oldest);
        }
        let entry = self.places.entry(viewer.to_owned()).or_insert_with(||
            (Arc::new(tokio::sync::Mutex::new(self.base.clone())), now));
        entry.1 = now;
        entry.0.clone()
    }
}

/// Reads the viewer named by a frame request: none for `{}`, one canonical UUID otherwise.
fn frame_viewer(params: &Value) -> Option<String> {
    let map = params.as_object()?;
    if map.is_empty() { return Some(String::new()); }
    if map.len() != 1 { return None; }
    let id = map.get("viewer")?.as_str()?;
    (Uuid::parse_str(id).ok()?.to_string() == id).then(|| id.to_owned())
}

struct DesktopCapture {
    viewers: Viewers,
    keepalive: watch::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

impl DesktopCapture {
    /// Starts one continuous guest display capture and returns its frame subscriber.
    fn start() -> Self {
        let (sender, latest) = watch::channel(None);
        let (keepalive, alive) = watch::channel(());
        let task = tokio::spawn(capture_desktop(sender, alive));
        Self { viewers: Viewers::new(latest), keepalive, task }
    }
}

impl Drop for DesktopCapture {
    /// Stops capture when the authenticated worker connection ends.
    fn drop(&mut self) { self.task.abort(); }
}

pub struct Control {
    pub path: PathBuf,
    pub key: [u8; 32],
}

/// Authenticates a worker before initializing RPC. The caller must supply a private
/// VM stream and a per-worker key; this protocol does not encrypt network traffic.
pub async fn connect<S>(mut stream: S, key: [u8; 32]) -> Result<Client, Failure>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    authenticate(&mut stream, &key, false).await?;
    Client::attach(stream).await
}

/// Authenticates one controller, then starts and reaps its App Server on disconnect.
/// The guest supplies the command and policy; no executable is accepted over the wire.
pub async fn serve<S, F>(stream: S, key: [u8; 32], command: F) -> Result<(), Failure>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    F: FnOnce() -> Command + Send,
{
    serve_with(stream, key, command, None).await
}

/// Applies guest-local computer policy on the authenticated worker channel.
pub async fn serve_with<S, F>(
    stream: S,
    key: [u8; 32],
    command: F,
    policy: Option<Policy>,
) -> Result<(), Failure>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    F: FnOnce() -> Command + Send,
{
    serve_with_control(stream, key, command, policy, None).await
}

/// Runs one tool operation under a Seatbelt profile built from the supplied grants.
/// Malformed requests and Apple Event operations are denied.
async fn scoped_execute(params: &Value) -> Value {
    let denied = serde_json::json!({ "status": "denied" });
    let Some(object) = params.as_object().filter(|object| object.len() == 2) else { return denied; };
    let (Some(operation), Some(grants)) = (object.get("operation"), object.get("grants"))
        else { return denied; };
    let Ok(operation) = Operation::parse(operation) else { return denied; };
    let Ok(grants) = serde_json::from_value::<Vec<crate::seatbelt::Grant>>(grants.clone())
        else { return denied; };
    if grants.len() > 512 { return denied; }
    executor::execute_host(operation, &grants).await
}

/// Runs the Mac worker with a local VM control channel kept outside Codex.
#[cfg(unix)]
pub async fn serve_with_control<S, F>(
    mut stream: S,
    key: [u8; 32],
    command: F,
    policy: Option<Policy>,
    control: Option<Control>,
) -> Result<(), Failure>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    F: FnOnce() -> Command + Send,
{
    authenticate(&mut stream, &key, true).await?;
    let mut child = command()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| Failure::Unavailable)?;
    let input = Mutex::new(child.stdin.take().unwrap());
    let mut output = child.stdout.take().unwrap();
    let (reader, writer) = tokio::io::split(stream);
    let writer = std::sync::Arc::new(Mutex::new(writer));
    let result = {
        let replies = forward(&mut output, &writer);
        tokio::pin!(replies);
        tokio::select! {
            result = requests(reader, &input, &writer, policy.as_ref(), control.as_ref()) => result,
            result = &mut replies => result,
            _ = child.wait() => {
                // Process exit can precede pipe EOF; drain its final reply with a deadline.
                tokio::time::timeout(Duration::from_secs(5), &mut replies)
                    .await.unwrap_or(Err(Failure::Timeout))
            },
        }
    };
    drop(input);
    let _ = child.kill().await;
    child.wait().await.map_err(|_| Failure::Unavailable)?;
    result
}

/// Forwards complete, valid JSON lines with bounded memory and five-second writes.
async fn forward<R, W>(reader: R, writer: &Mutex<W>) -> Result<(), Failure>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = FramedRead::new(reader, LinesCodec::new_with_max_length(32 * 1024 * 1024));
    while let Some(line) = lines.next().await {
        let frame = decode(line.map_err(|_| Failure::Unavailable)?)?;
        send(writer, frame).await?;
    }
    Ok(())
}

/// Replies to guest capability requests locally; forwards other permitted frames to Codex.
async fn requests<R, W, O>(
    reader: R,
    input: &Mutex<W>,
    output: &std::sync::Arc<Mutex<O>>,
    policy: Option<&Policy>,
    control: Option<&Control>,
) -> Result<(), Failure>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    O: AsyncWrite + Unpin + Send + 'static,
{
    let mut lines = FramedRead::new(reader, LinesCodec::new_with_max_length(32 * 1024 * 1024));
    let mut desktop: Option<DesktopCapture> = None;
    while let Some(line) = lines.next().await {
        let frame = decode(line.map_err(|_| Failure::Unavailable)?)?;
        // The worker runs MiniClaw's tool itself, confined by the grants sent with the
        // call. Codex never sees these frames.
        if frame["method"] == "miniclaw/scoped/execute" {
            let output = output.clone();
            let id = frame["id"].clone();
            let params = frame["params"].clone();
            tokio::spawn(async move {
                let _ = send(&output, serde_json::json!({ "id": id,
                    "result": scoped_execute(&params).await })).await;
            });
            continue;
        }
        // The VM's own shells, one per conversation, driven from the Mac. Only a worker that
        // applies the guest policy runs them: the real guest worker, and the tests' stand-in,
        // which carries a control channel as well.
        if let Some(method) = frame["method"].as_str().and_then(|name| name.strip_prefix("miniclaw/terminal/")) {
            let (output, id, params, method) = (output.clone(), frame["id"].clone(), frame["params"].clone(), method.to_owned());
            let guest = policy.is_some();
            tokio::spawn(async move {
                let result = if guest { crate::terminal::guest_call(&method, &params).await } else { serde_json::json!({ "error": "not a VM" }) };
                let _ = send(&output, serde_json::json!({ "id": id, "result": result })).await;
            });
            continue;
        }
        // File transfer reaches only the VM's two offered folders, and only in a VM worker.
        if let Some(method) = frame["method"].as_str().and_then(|name| name.strip_prefix("miniclaw/transfer/")) {
            let (output, id, params, method) = (output.clone(), frame["id"].clone(),
                frame["params"].clone(), method.to_owned());
            let guest = control.is_none() && policy.is_some();
            tokio::spawn(async move {
                let result = if guest {
                    tokio::task::spawn_blocking(move || crate::transfer::handle(
                        &crate::transfer::Guest::standard(), &method, &params)).await
                        .unwrap_or_else(|_| serde_json::json!({ "refused": "failed" }))
                } else { serde_json::json!({ "refused": "path" }) };
                let _ = send(&output, serde_json::json!({ "id": id, "result": result })).await;
            });
            continue;
        }
        if frame["method"] == "miniclaw/desktop/frame" {
            if control.is_none() && policy.is_some()
                && let Some(viewer) = frame_viewer(&frame["params"]) {
                if desktop.as_ref().is_some_and(|capture| capture.task.is_finished()) {
                    desktop = None;
                }
                let capture = desktop.get_or_insert_with(DesktopCapture::start);
                capture.keepalive.send_replace(());
                let latest = capture.viewers.place(&viewer);
                let output = output.clone();
                tokio::spawn(async move {
                    // The first frame of a new capture may take a few seconds to arrive.
                    let started = latest.lock().await.borrow().is_some();
                    let bytes = next_frame(&latest, Duration::from_secs(if started { 1 } else { 5 })).await;
                    let reply = match bytes {
                        Some(bytes) => serde_json::json!({"id": frame["id"],
                            "result": {"jpeg": crate::frames::encode(bytes.as_ref())}}),
                        None => serde_json::json!({"id": frame["id"], "error": {
                            "code": -32000, "message": "Guest desktop capture unavailable"}}),
                    };
                    let _ = send(&output, reply).await;
                });
            } else {
                send(output, serde_json::json!({"id": frame["id"], "error": {
                    "code": -32602, "message": "Desktop capture denied"}})).await?;
            }
            continue;
        }
        // Lets the backend tell a VM worker built before a protocol change from a current one.
        if frame["method"] == "miniclaw/worker/info" {
            // A test can stand in for a guest that was installed before the tools were accepted.
            #[cfg(debug_assertions)]
            if std::env::var_os("MINICLAW_TEST_OLD_WORKER").is_some() {
                send(output, serde_json::json!({"id": frame["id"], "result": {"protocol": 4}})).await?;
                continue;
            }
            send(output, serde_json::json!({"id": frame["id"],
                "result": {"protocol": PROTOCOL, "tools": crate::guest_computer::MAC_TOOLS, "features": FEATURES}})).await?;
            continue;
        }
        // Requests about the shared VM budget name no VM; only `limits` carries a value.
        if let Some(method @ ("ledger" | "limits" | "reconcile" | "forget")) =
            frame["method"].as_str().and_then(|name| name.strip_prefix("miniclaw/vm/")) {
            let limits = frame["params"]["limits"].as_str().filter(|text| percentages(text));
            let shaped = frame["params"].as_object().is_some_and(|params|
                params.len() == usize::from(method == "limits"))
                && (method == "limits") == limits.is_some();
            let reply = match control {
                Some(control) if shaped => ledger_request(control, method, limits).await,
                _ => Err(Failure::Rejected),
            };
            send(output, match reply {
                Ok(ledger) => serde_json::json!({"id": frame["id"], "result": ledger}),
                Err(_) => serde_json::json!({"id": frame["id"], "error": {
                    "code": -32602, "message": "VM budget request denied"}}),
            }).await?;
            continue;
        }
        if matches!(frame["method"].as_str(),
            Some("miniclaw/vm/status" | "miniclaw/vm/start" | "miniclaw/vm/list"
                | "miniclaw/vm/provision" | "miniclaw/vm/retry" | "miniclaw/vm/cancel"
                | "miniclaw/vm/stop" | "miniclaw/vm/show" | "miniclaw/vm/delete")) {
            let start = frame["method"] == "miniclaw/vm/start";
            let target = if frame["method"] == "miniclaw/vm/list" {
                (frame["params"] == serde_json::json!({})).then_some(None)
            } else if frame["method"] == "miniclaw/vm/provision" {
                (frame["params"].as_object().is_some_and(|params| params.len() == 1)
                    && frame["params"]["requestId"].as_str().is_some_and(|id|
                        Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id)))
                    .then_some(None)
            } else { vm_control_target(&frame["params"]) };
            let request_id = if frame["method"] == "miniclaw/vm/provision" {
                frame["params"]["requestId"].as_str()
            } else { None };
            let reply = match (control, target) {
                (Some(control), Some(vm_id)) =>
                    control_request(control, &frame, vm_id, request_id).await,
                _ => Err(Failure::Rejected),
            };
            let response = match reply {
                Ok(status) => serde_json::json!({"id": frame["id"], "result": status}),
                Err(_) if control.is_some() && target.is_some()
                    && frame["method"] != "miniclaw/vm/list" =>
                    serde_json::json!({"id": frame["id"], "result": {
                        "state": if start || frame["method"] != "miniclaw/vm/status" {
                            "failed" } else { "unknown" },
                        "reason": "Mac VM controller is unavailable." }}),
                Err(_) => serde_json::json!({"id": frame["id"], "error": {
                    "code": -32602, "message": "VM control request denied"}}),
            };
            send(output, response).await?;
            continue;
        }
        let route = match policy {
            Some(policy) => policy.route(frame),
            None => Route::Forward(frame),
        };
        match route {
            Route::Forward(frame) => send(input, frame).await?,
            Route::Reply(frame) => send(output, frame).await?,
        }
    }
    Ok(())
}

// macOS shows its Screen Recording request once per process lifetime at most.
static AUTHORIZATION_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Asks macOS for Screen Recording from inside the worker job, which is the program
/// macOS holds responsible for the helper. The request lists the worker in System Settings.
#[cfg(unix)]
async fn request_capture_access(binary: &std::path::Path) {
    if AUTHORIZATION_REQUESTED.swap(true, Ordering::SeqCst) { return; }
    let _ = Command::new(binary).arg("--authorize")
        .stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true).status().await;
}

/// Runs one ScreenCaptureKit helper and publishes frames until the worker disconnects.
#[cfg(unix)]
async fn capture_desktop(sender: watch::Sender<Option<Arc<Vec<u8>>>>,
    mut alive: watch::Receiver<()>) {
    let Some(home) = std::env::var_os("HOME") else { return };
    let binary = PathBuf::from(home).join(
        "Library/Application Support/MiniClaw/Runtime/miniclaw-desktop-stream");
    let Ok(mut child) = Command::new(&binary)
        .stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true).spawn() else { return };
    let Some(mut stdout) = child.stdout.take() else { return };
    let mut deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut delivered = false;
    loop {
        tokio::select! {
            frame = read_capture_frame(&mut stdout) => match frame {
                Ok(bytes) => {
                    delivered = true;
                    if sender.send(Some(Arc::new(bytes))).is_err() { break; }
                },
                Err(_) => break,
            },
            _ = alive.changed() => deadline = tokio::time::Instant::now()
                + Duration::from_secs(30),
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
    // A helper that ends without one frame was refused capture; ask for the permission.
    if !delivered { request_capture_access(&binary).await; }
}

/// Reads one length-delimited JPEG, rejecting invalid or unbounded helper output.
async fn read_capture_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Vec<u8>, Failure> {
    let mut length = [0; 4];
    reader.read_exact(&mut length).await.map_err(|_| Failure::Unavailable)?;
    let size = u32::from_be_bytes(length) as usize;
    if !(4..=768 * 1024).contains(&size) { return Err(Failure::Rejected); }
    let mut bytes = vec![0; size];
    reader.read_exact(&mut bytes).await.map_err(|_| Failure::Unavailable)?;
    if !bytes.starts_with(&[0xff, 0xd8, 0xff]) || !bytes.ends_with(&[0xff, 0xd9]) {
        return Err(Failure::Rejected);
    }
    Ok(bytes)
}

/// Accepts only Master or one canonical dedicated VM identity as control input.
fn vm_control_target(params: &Value) -> Option<Option<&str>> {
    let map = params.as_object()?;
    if map.is_empty() { return Some(None); }
    if map.len() != 1 { return None; }
    let id = map.get("vmId")?.as_str()?;
    if Uuid::parse_str(id).ok()?.to_string() != id { return None; }
    Some(Some(id))
}

/// Raised whenever the backend starts to rely on something a worker must support, so the
/// settings page can say that a VM needs `miniclaw update-vm`. 2: viewers, folder and search
/// operations, command network. 3 and 4: file transfer and the capture helper's delivery. 5: the
/// worker accepts MiniClaw's tools that the Mac answers and names them in `miniclaw/worker/info`.
/// 6: the worker runs a shell per conversation (`miniclaw/terminal/*`) and names it in `features`.
/// The number only drives that notice. What the backend may rely on is asked for by name, as
/// `tools` and `features` here, so an older guest keeps working as it did and a newer one is
/// simply offered more.
pub const PROTOCOL: u64 = 6;

/// What this worker can do beyond tools, named for the Mac to ask about.
pub const FEATURES: [&str; 1] = ["terminal"];

/// Accepts "cpu,memory,disk" as three whole percentages from 1 to 100.
fn percentages(text: &str) -> bool {
    let parts: Vec<&str> = text.split(',').collect();
    parts.len() == 3 && parts.iter().all(|part| !part.is_empty() && part.len() <= 3
        && part.bytes().all(|byte| byte.is_ascii_digit())
        && part.parse::<u16>().is_ok_and(|value| (1..=100).contains(&value)))
}

/// Sends one signed budget request to the VM session and returns its ledger view. The
/// percentages, when present, are covered by the signature.
#[cfg(unix)]
async fn ledger_request(control: &Control, method: &str, limits: Option<&str>)
    -> Result<Value, Failure> {
    let nonce = nonce().iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    let mut mac = Hmac::<Sha256>::new_from_slice(&control.key).expect("fixed control key");
    mac.update(format!("{nonce}:{method}{}",
        limits.map(|value| format!(":{value}")).unwrap_or_default()).as_bytes());
    let proof = mac.finalize().into_bytes().iter()
        .map(|byte| format!("{byte:02x}")).collect::<String>();
    let mut request = serde_json::json!({"method": method, "nonce": nonce, "mac": proof});
    if let Some(value) = limits { request["limits"] = serde_json::json!(value); }
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut socket = UnixStream::connect(&control.path).await
            .map_err(|_| Failure::Unavailable)?;
        let mut bytes = serde_json::to_vec(&request).map_err(|_| Failure::Rejected)?;
        bytes.push(b'\n');
        socket.write_all(&bytes).await.map_err(|_| Failure::Unavailable)?;
        let mut line = Vec::new();
        let mut byte = [0];
        while line.len() < 65_536 {
            socket.read_exact(&mut byte).await.map_err(|_| Failure::Unavailable)?;
            if byte[0] == b'\n' { break; }
            line.push(byte[0]);
        }
        let reply: Value = serde_json::from_slice(&line).map_err(|_| Failure::Rejected)?;
        if reply["ok"] != true || !reply["ledger"].is_object() { return Err(Failure::Unavailable); }
        Ok(reply["ledger"].clone())
    }).await.map_err(|_| Failure::Timeout)?
}

/// Sends one signed supervisor request and returns its bounded status object.
#[cfg(unix)]
async fn control_request(control: &Control, frame: &Value, vm_id: Option<&str>,
    request_id: Option<&str>)
    -> Result<Value, Failure> {
    let method = match frame["method"].as_str() {
        Some("miniclaw/vm/status") => "status",
        Some("miniclaw/vm/start") => "start",
        Some("miniclaw/vm/list") => "list",
        Some("miniclaw/vm/provision") => "provision",
        Some("miniclaw/vm/retry") => "retry",
        Some("miniclaw/vm/cancel") => "cancel",
        Some("miniclaw/vm/stop") => "stop",
        Some("miniclaw/vm/show") => "show",
        Some("miniclaw/vm/delete") => "delete",
        _ => return Err(Failure::Rejected),
    };
    let nonce = nonce().iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    let mut mac = Hmac::<Sha256>::new_from_slice(&control.key).expect("fixed control key");
    mac.update(format!("{nonce}:{method}{}",
        format!("{}{}", vm_id.map(|id| format!(":{id}")).unwrap_or_default(),
            request_id.map(|id| format!(":{id}")).unwrap_or_default())).as_bytes());
    let proof = mac.finalize().into_bytes().iter()
        .map(|byte| format!("{byte:02x}")).collect::<String>();
    let mut request = serde_json::json!({"method": method, "nonce": nonce, "mac": proof});
    if let Some(id) = vm_id { request["vmId"] = serde_json::json!(id); }
    if let Some(id) = request_id { request["requestId"] = serde_json::json!(id); }
    // Deleting a large disk image can take longer than a status read.
    tokio::time::timeout(Duration::from_secs(if method == "delete" { 120 } else { 15 }), async {
        let mut socket = UnixStream::connect(&control.path).await
            .map_err(|_| Failure::Unavailable)?;
        let mut bytes = serde_json::to_vec(&request).map_err(|_| Failure::Rejected)?;
        bytes.push(b'\n');
        socket.write_all(&bytes).await.map_err(|_| Failure::Unavailable)?;
        let mut line = Vec::new();
        let limit = if method == "list" { 32768 } else { 2048 };
        while line.len() < limit {
            let mut byte = [0];
            socket.read_exact(&mut byte).await.map_err(|_| Failure::Unavailable)?;
            if byte[0] == b'\n' { break; }
            line.push(byte[0]);
        }
        if line.len() >= limit { return Err(Failure::Rejected); }
        let reply: Value = serde_json::from_slice(&line).map_err(|_| Failure::Rejected)?;
        if method == "list" {
            let machines = reply["machines"].as_array().ok_or(Failure::Rejected)?;
            if reply["ok"] != true || machines.len() > 64 { return Err(Failure::Rejected); }
            let mut ids = Vec::new();
            let mut slots = Vec::new();
            for machine in machines {
                let id = machine["vmId"].as_str().ok_or(Failure::Rejected)?;
                let slot = machine["slot"].as_u64().ok_or(Failure::Rejected)?;
                let key = machine["key"].as_str().ok_or(Failure::Rejected)?;
                if Uuid::parse_str(id).ok().is_none_or(|value| value.to_string() != id)
                    || slot >= 64 || ids.contains(&id) || slots.contains(&slot)
                    || key.len() != 64 || !key.bytes().all(|byte|
                        byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                    || machine["cwd"] != "/Users/Shared/MiniClaw/Workspace"
                { return Err(Failure::Rejected); }
                ids.push(id);
                slots.push(slot);
            }
            return Ok(serde_json::json!({ "machines": machines }));
        }
        if reply["ok"] == false && reply["error"]["code"] == "setup_failed" {
            let reason = if reply["error"]["message"] == "Project VM limit reached." {
                "Project VM limit reached."
            } else {
                "Project VM setup could not proceed. Check its status and host capacity."
            };
            return Ok(serde_json::json!({ "state": "failed", "reason": reason }));
        }
        if method == "provision" {
            let machine = &reply["machine"];
            let id = machine["vmId"].as_str().ok_or(Failure::Rejected)?;
            if reply["ok"] != true || Uuid::parse_str(id).ok()
                .is_none_or(|uuid| uuid.to_string() != id)
                || !matches!(machine["state"].as_str(), Some("starting" | "failed"))
                || !matches!(machine["stage"].as_str(),
                    Some("preparing" | "installing" | "awaiting_setup" | "failed")) {
                return Err(Failure::Rejected);
            }
            return Ok(serde_json::json!({ "vmId": id, "state": machine["state"],
                "stage": machine["stage"] }));
        }
        if reply["ok"] == false && reply["error"]["code"] == "start_failed" {
            let reason = reply["error"]["message"].as_str().unwrap_or("");
            if matches!(reason,
                "VM resource limit exceeded."
                | "Master VM network disconnected."
                | "Master VM stopped before it was ready."
                | "Master VM resource ownership is unresolved."
                | "Master VM setup is incomplete."
                | "Assigned VM does not exist."
                | "Master VM could not start. Check VM diagnostics.") {
                return Ok(serde_json::json!({"state": "failed", "reason": reason}));
            }
        }
        if reply["ok"] != true || !reply["status"].is_object() {
            return Err(Failure::Unavailable);
        }
        Ok(reply["status"].clone())
    }).await.map_err(|_| Failure::Timeout)?
}

/// Rejects non-object frames before any policy or transport handling.
fn decode(line: String) -> Result<Value, Failure> {
    let frame: Value = serde_json::from_str(&line).map_err(|_| Failure::Unavailable)?;
    if !frame.is_object() {
        return Err(Failure::Rejected);
    }
    Ok(frame)
}

/// Serializes one complete frame within the write deadline, including lock contention.
async fn send<W: AsyncWrite + Unpin>(writer: &Mutex<W>, frame: Value) -> Result<(), Failure> {
    let mut line = serde_json::to_vec(&frame).map_err(|_| Failure::Rejected)?;
    if line.len() > 32 * 1024 * 1024 {
        return Err(Failure::Rejected);
    }
    line.push(b'\n');
    tokio::time::timeout(Duration::from_secs(5), async {
        writer
            .lock()
            .await
            .write_all(&line)
            .await
            .map_err(|_| Failure::Unavailable)
    })
    .await
    .map_err(|_| Failure::Timeout)?
}

/// Bounds the complete mutual handshake, including partial reads and stalled writes.
async fn authenticate<S>(stream: &mut S, key: &[u8; 32], worker: bool) -> Result<(), Failure>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    tokio::time::timeout(AUTH_TIMEOUT, exchange(stream, key, worker))
        .await
        .map_err(|_| Failure::Timeout)?
}

/// Exchanges fresh role-separated proofs without sending the shared worker key.
async fn exchange<S>(stream: &mut S, key: &[u8; 32], worker: bool) -> Result<(), Failure>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut controller_nonce = nonce();
    let mut worker_nonce = nonce();
    let mut proof = [0; 32];
    if worker {
        let mut magic = [0; 8];
        read(stream, &mut magic).await?;
        if &magic != MAGIC {
            return Err(Failure::Rejected);
        }
        read(stream, &mut controller_nonce).await?;
        write(stream, &worker_nonce).await?;
        let mac = proof_for(key, b"worker", &controller_nonce, &worker_nonce);
        write(stream, &mac.finalize().into_bytes()).await?;
        read(stream, &mut proof).await?;
        proof_for(key, b"controller", &controller_nonce, &worker_nonce)
            .verify_slice(&proof)
            .map_err(|_| Failure::Rejected)?;
        write(stream, &[1]).await?;
    } else {
        write(stream, MAGIC).await?;
        write(stream, &controller_nonce).await?;
        read(stream, &mut worker_nonce).await?;
        read(stream, &mut proof).await?;
        proof_for(key, b"worker", &controller_nonce, &worker_nonce)
            .verify_slice(&proof)
            .map_err(|_| Failure::Rejected)?;
        let mac = proof_for(key, b"controller", &controller_nonce, &worker_nonce);
        write(stream, &mac.finalize().into_bytes()).await?;
        let mut accepted = [0];
        read(stream, &mut accepted).await?;
        if accepted != [1] {
            return Err(Failure::Rejected);
        }
    }
    Ok(())
}

/// Builds a protocol-versioned proof bound to both challenges and the sender role.
fn proof_for(
    key: &[u8; 32],
    role: &[u8],
    controller: &[u8; 32],
    worker: &[u8; 32],
) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("fixed HMAC key length");
    mac.update(MAGIC);
    mac.update(role);
    mac.update(controller);
    mac.update(worker);
    mac
}

/// Returns a fresh challenge from two randomly generated UUIDs.
fn nonce() -> [u8; 32] {
    let mut value = [0; 32];
    value[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    value[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    value
}

/// Reads exactly one handshake field or reports a closed connection.
async fn read<S: AsyncRead + Unpin>(stream: &mut S, bytes: &mut [u8]) -> Result<(), Failure> {
    stream
        .read_exact(bytes)
        .await
        .map(|_| ())
        .map_err(|_| Failure::Unavailable)
}

/// Writes one handshake field or reports a closed connection.
async fn write<S: AsyncWrite + Unpin>(stream: &mut S, bytes: &[u8]) -> Result<(), Failure> {
    stream
        .write_all(bytes)
        .await
        .map_err(|_| Failure::Unavailable)
}

#[cfg(test)]
mod desktop_capture_tests {
    use super::*;

    /// Keeps consecutive live frames on one capture stream and rejects oversized data.
    #[tokio::test]
    async fn reads_bounded_live_frames() {
        let (mut sender, mut receiver) = tokio::io::duplex(64);
        let jpeg = [0xff, 0xd8, 0xff, 0xd9];
        tokio::spawn(async move {
            for _ in 0..2 {
                sender.write_all(&(jpeg.len() as u32).to_be_bytes()).await.unwrap();
                sender.write_all(&jpeg).await.unwrap();
            }
            sender.write_all(&((768 * 1024 + 1) as u32).to_be_bytes())
                .await.unwrap();
        });
        assert_eq!(read_capture_frame(&mut receiver).await.unwrap(), jpeg);
        assert_eq!(read_capture_frame(&mut receiver).await.unwrap(), jpeg);
        assert!(matches!(read_capture_frame(&mut receiver).await, Err(Failure::Rejected)));
    }

    /// Each new frame is delivered once, including one that arrived between two requests.
    #[tokio::test]
    async fn frames_are_neither_repeated_nor_skipped() {
        let (sender, receiver) = watch::channel(None);
        let latest: Frames = Arc::new(tokio::sync::Mutex::new(receiver));
        let frame = |n: u8| Some(Arc::new(vec![n]));
        let short = Duration::from_millis(40);
        sender.send(frame(1)).unwrap();
        assert_eq!(next_frame(&latest, short).await, frame(1));
        // Nothing new: the wait runs out and the newest frame answers an idle screen.
        let began = std::time::Instant::now();
        assert_eq!(next_frame(&latest, short).await, frame(1));
        assert!(began.elapsed() >= short);
        // A frame that arrived while no request was waiting is returned at once, and once.
        sender.send(frame(2)).unwrap();
        let began = std::time::Instant::now();
        assert_eq!(next_frame(&latest, Duration::from_secs(5)).await, frame(2));
        assert!(began.elapsed() < Duration::from_secs(1));
        let began = std::time::Instant::now();
        assert_eq!(next_frame(&latest, short).await, frame(2));
        assert!(began.elapsed() >= short, "the same frame is not handed out again at once");
    }

    /// Two viewers of one VM each receive every frame.
    #[tokio::test]
    async fn each_viewer_gets_every_frame() {
        let (sender, receiver) = watch::channel(None);
        let mut viewers = Viewers::new(receiver);
        let frame = |n: u8| Some(Arc::new(vec![n]));
        let short = Duration::from_millis(40);
        sender.send(frame(1)).unwrap();
        let (first, second) = (viewers.place("a"), viewers.place("b"));
        assert_eq!(next_frame(&first, short).await, frame(1));
        assert_eq!(next_frame(&second, short).await, frame(1));
        sender.send(frame(2)).unwrap();
        assert_eq!(next_frame(&second, short).await, frame(2));
        assert_eq!(next_frame(&first, short).await, frame(2));
        // The same viewer keeps its place between requests.
        assert!(Arc::ptr_eq(&first, &viewers.place("a")));
    }

    /// Places are bounded, and only `{}` or one canonical viewer id is accepted.
    #[test]
    fn viewer_places_are_bounded_and_ids_are_checked() {
        let (_sender, receiver) = watch::channel(None);
        let mut viewers = Viewers::new(receiver);
        for n in 0..20 { viewers.place(&format!("viewer-{n}")); }
        assert_eq!(viewers.places.len(), Viewers::LIMIT);
        let id = Uuid::new_v4().to_string();
        assert_eq!(frame_viewer(&serde_json::json!({})), Some(String::new()));
        assert_eq!(frame_viewer(&serde_json::json!({ "viewer": id })), Some(id));
        assert_eq!(frame_viewer(&serde_json::json!({ "viewer": "../x" })), None);
        assert_eq!(frame_viewer(&serde_json::json!({ "viewer": "a", "other": 1 })), None);
    }
}

/// Windows excludes virtual machine budget operations.
#[cfg(windows)]
async fn ledger_request(_control: &Control, _method: &str, _limits: Option<&str>)
    -> Result<Value, Failure> {
    Err(Failure::Rejected)
}

/// Windows excludes virtual machine lifecycle operations.
#[cfg(windows)]
async fn control_request(_control: &Control, _frame: &Value, _vm_id: Option<&str>,
    _request_id: Option<&str>) -> Result<Value, Failure> {
    Err(Failure::Rejected)
}

/// Windows does not launch any VM worker or its provider subprocesses.
#[cfg(windows)]
pub async fn serve_with_control<S, F>(_stream: S, _key: [u8; 32], _command: F,
    _policy: Option<Policy>, _control: Option<Control>) -> Result<(), Failure>
where S: AsyncRead + AsyncWrite + Unpin + Send + 'static, F: FnOnce() -> Command + Send {
    Err(Failure::Rejected)
}

/// Windows VM capture is unavailable and never launches a macOS helper.
#[cfg(windows)]
async fn capture_desktop(_sender: watch::Sender<Option<Arc<Vec<u8>>>>, _alive: watch::Receiver<()>) {}
