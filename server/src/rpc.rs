// Tracks connection health and unique request numbers.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
// Sets request, write, and approval timeouts.
use std::time::Duration;
// Stores pending replies and shares process state.
use std::{
    collections::HashMap,
    process::Stdio,
    sync::{Arc, Mutex},
};

// Reads asynchronous lines from Codex output.
use futures_util::StreamExt;
// Encodes requests and decodes Codex messages.
use serde_json::{Value, json};
// Coordinates replies, notifications, approvals, and shutdown.
use tokio::sync::{OwnedSemaphorePermit, Semaphore, broadcast, mpsc, oneshot, watch};
// Starts Codex and writes asynchronous process input.
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    process::Command,
};
// Contains native provider trees for cleanup.
use crate::provider_process::Child;
// Frames Codex output into bounded JSON lines.
use tokio_util::bytes::{Buf, BytesMut};
use tokio_util::codec::{Decoder, FramedRead};

/// The largest frame read from a provider. A saved tool result can run to several megabytes,
/// and history is read in small pages, so a frame this large is a fault, not a load.
pub(crate) const MAX_FRAME: usize = 64 * 1024 * 1024;

/// Bytes of notifications that may be held unread at once before the connection is ended as
/// overloaded. Replies do not count; a reply larger than a frame is dropped by itself.
pub(crate) const EVENT_BUDGET: usize = 32 * 1024 * 1024;

/// One line of provider output, or the place of a line too long to keep.
#[derive(Debug, PartialEq)]
pub(crate) enum Frame { Line(String), Oversized }

/// Splits provider output into lines of at most `MAX_FRAME` bytes. A longer line is dropped
/// whole and reported as `Frame::Oversized`, and reading goes on with the next line; the
/// standard line codec ends the stream at the first such line, which would take the provider
/// down with it.
#[derive(Default)]
pub(crate) struct Frames {
    /// Bytes of the current line already searched for its end.
    scanned: usize,
    /// The current line is over the limit; its remainder is dropped up to the newline.
    discarding: bool,
}

impl Decoder for Frames {
    type Item = Frame;
    type Error = std::io::Error;

    /// Returns the next complete line, the mark of a dropped one, or nothing until more arrives.
    fn decode(&mut self, src: &mut BytesMut) -> std::io::Result<Option<Frame>> {
        let newline = src[self.scanned..].iter().position(|byte| *byte == b'\n')
            .map(|offset| self.scanned + offset);
        if self.discarding {
            return Ok(match newline {
                Some(end) => {
                    src.advance(end + 1);
                    self.scanned = 0;
                    self.discarding = false;
                    Some(Frame::Oversized)
                }
                None => { src.clear(); self.scanned = 0; None }
            });
        }
        match newline {
            Some(end) if end > MAX_FRAME => {
                src.advance(end + 1);
                self.scanned = 0;
                Ok(Some(Frame::Oversized))
            }
            Some(end) => {
                let line = src.split_to(end + 1);
                self.scanned = 0;
                let text = &line[..end];
                let text = text.strip_suffix(b"\r").unwrap_or(text);
                String::from_utf8(text.to_vec()).map(|line| Some(Frame::Line(line)))
                    .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "provider output is not UTF-8"))
            }
            None if src.len() > MAX_FRAME => {
                src.clear();
                self.scanned = 0;
                self.discarding = true;
                Ok(None)
            }
            None => { self.scanned = src.len(); Ok(None) }
        }
    }
}

type Reply = Result<Value, Failure>;
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>;

#[derive(Clone, Copy)]
enum End { Expected, Disconnected, Overloaded, Lagged }

#[derive(Clone)]
pub struct Notification(Arc<NotificationData>);

struct NotificationData {
    json: Box<str>,
    _permit: Option<OwnedSemaphorePermit>,
}

impl Notification {
    /// Decodes an immutable, previously validated frame while its byte lease stays held.
    pub fn value(&self) -> Value {
        serde_json::from_str(&self.0.json).expect("validated immutable notification")
    }

    /// Builds a fixed-size terminal frame outside ordinary notification capacity.
    fn terminal(end: End) -> Self {
        Self(Arc::new(NotificationData {
            json: json!({ "method": "miniclaw/disconnected",
                "params": { "expected": matches!(end, End::Expected) } })
                .to_string().into_boxed_str(),
            _permit: None,
        }))
    }
}

struct Events {
    sender: broadcast::Sender<Notification>,
    budget: Arc<Semaphore>,
    terminal: Mutex<Option<End>>,
    stopping: Arc<AtomicBool>,
    failed: watch::Sender<bool>,
}

impl Events {
    /// Reports a latched terminal decision before transport cleanup has finished.
    fn ended(&self) -> bool {
        self.terminal.lock().unwrap().is_some()
    }

    /// Publishes one validated JSON frame without waiting for byte capacity or readers.
    fn publish(&self, json: Box<str>) -> bool {
        let mut terminal = self.terminal.lock().unwrap();
        if terminal.is_some() { return false; }
        let permit = u32::try_from(json.len()).ok()
            .and_then(|bytes| self.budget.clone().try_acquire_many_owned(bytes).ok());
        let Some(permit) = permit else {
            *terminal = Some(if self.stopping.load(Ordering::Acquire) {
                End::Expected
            } else { End::Overloaded });
            self.failed.send_replace(true);
            return false;
        };
        let _ = self.sender.send(Notification(Arc::new(NotificationData {
            json, _permit: Some(permit),
        })));
        true
    }

    /// Latches the first terminal reason, excluding every later ordinary publication.
    fn finish(&self, reason: End) -> End {
        let mut terminal = self.terminal.lock().unwrap();
        *terminal.get_or_insert_with(|| if self.stopping.load(Ordering::Acquire) {
            End::Expected
        } else { reason })
    }

    /// Requests failure cleanup without marking the transport as intentionally stopped.
    fn fail(&self, reason: End) {
        self.finish(reason);
        self.failed.send_replace(true);
    }
}

#[cfg(test)]
mod tests;

pub struct ServerRequest {
    pub id: Value,
    pub params: Value,
    pub answer: oneshot::Sender<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Failure {
    Unavailable,
    Rejected,
    Timeout,
}

#[derive(Clone)]
pub struct Client {
    outbound: mpsc::Sender<Value>,
    pending: Pending,
    next_id: Arc<AtomicU64>,
    alive: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    event_completion: Arc<Mutex<Option<watch::Receiver<bool>>>>,
    events: Arc<Events>,
    shutdown: mpsc::Sender<()>,
    done: watch::Receiver<Option<End>>,
    requests: Arc<Mutex<Option<mpsc::Receiver<ServerRequest>>>>,
    tool_requests: Arc<Mutex<Option<mpsc::Receiver<ServerRequest>>>>,
    tools_enabled: Arc<AtomicBool>,
}

pub(crate) struct Completion {
    done: watch::Receiver<Option<End>>,
}

impl Completion {
    /// Observes transport cleanup without retaining its child or shutdown sender.
    pub(crate) async fn closed(&mut self) -> Notification {
        let _ = self.done.wait_for(Option::is_some).await;
        Notification::terminal(self.done.borrow().unwrap_or(End::Disconnected))
    }
}

/// Lets an event consumer report lost events without owning the transport, so a
/// forgotten listener can never keep a child process alive.
pub(crate) struct Lag {
    events: Arc<Events>,
    done: watch::Receiver<Option<End>>,
}

impl Lag {
    /// Ends a transport whose consumer fell behind and waits for its cleanup.
    pub(crate) async fn lost(&mut self) {
        self.events.fail(End::Lagged);
        let _ = self.done.wait_for(Option::is_some).await;
    }
}

struct Waiting {
    id: u64,
    pending: Pending,
}

impl Drop for Waiting {
    /// Removes the pending reply when a call completes, times out, or is cancelled.
    fn drop(&mut self) {
        self.pending.lock().unwrap().remove(&self.id);
    }
}

impl Client {
    /// Starts and initializes an owned stdio App Server, returning a client that
    /// routes replies, notifications, and approval requests in the background.
    pub async fn spawn(mut command: Command) -> Result<Self, Failure> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = crate::provider_process::spawn(&mut command)
            .map_err(|_| Failure::Unavailable)?;
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        Self::transport(output, input, Some(child)).await
    }

    /// Initializes an already authenticated private stream without spawning local Codex.
    pub(crate) async fn attach<S>(stream: S) -> Result<Self, Failure>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (output, input) = tokio::io::split(stream);
        Self::transport(output, input, None).await
    }

    /// Routes bounded JSON lines and owns stream cleanup, plus any local child.
    async fn transport<R, W>(output: R, mut input: W, mut child: Option<Child>)
        -> Result<Self, Failure>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let mut lines = FramedRead::new(output, Frames::default());
        let (outbound, mut writes) = mpsc::channel::<Value>(128);
        let (sender, _) = broadcast::channel(1024);
        let (failed, mut failure) = watch::channel(false);
        let (shutdown, mut stops) = mpsc::channel(1);
        let (finished, done) = watch::channel(None);
        let (requests, incoming) = mpsc::channel(8);
        let (tool_requests, tool_incoming) = mpsc::channel(8);
        let slots = Arc::new(Semaphore::new(8));
        let tools_enabled = Arc::new(AtomicBool::new(false));
        let pending: Pending = Arc::default();
        let alive = Arc::new(AtomicBool::new(true));
        let stopping = Arc::new(AtomicBool::new(false));
        let events = Arc::new(Events {
            sender, budget: Arc::new(Semaphore::new(EVENT_BUDGET)),
            terminal: Mutex::new(None), stopping: stopping.clone(), failed,
        });
        let client = Self {
            outbound,
            pending: pending.clone(),
            events: events.clone(),
            next_id: Arc::new(AtomicU64::new(1)),
            alive: alive.clone(),
            stopping: stopping.clone(),
            event_completion: Arc::new(Mutex::new(None)),
            shutdown,
            done,
            requests: Arc::new(Mutex::new(Some(incoming))),
            tool_requests: Arc::new(Mutex::new(Some(tool_incoming))),
            tools_enabled: tools_enabled.clone(),
        };
        let replies = client.outbound.clone();
        let closed = client.done.clone();

        // One task owns the pipes and routes replies and notifications.
        tokio::spawn(async move {
            let termination = loop {
                tokio::select! {
                    _ = stops.recv() => break events.finish(End::Expected),
                    _ = failure.changed() => break events.finish(End::Overloaded),
                    message = writes.recv() => {
                        let Some(message) = message else { break events.finish(End::Expected) };
                        let line = format!("{message}\n");
                        if events.ended() { break events.finish(End::Disconnected); }
                        let write = async { input.write_all(line.as_bytes()).await?; input.flush().await };
                        if !matches!(tokio::time::timeout(
                            Duration::from_secs(5), write,
                        ).await, Ok(Ok(()))) { break events.finish(End::Disconnected); }
                    }
                    line = lines.next() => {
                        let line = match line {
                            Some(Ok(Frame::Line(line))) => line,
                            // A frame beyond MAX_FRAME cannot be read, and the decoder skips to
                            // the next line. Which request it answered is unknown, so every
                            // waiting call fails; the connection and its process carry on.
                            Some(Ok(Frame::Oversized)) => {
                                for (_, sender) in pending.lock().unwrap().drain() {
                                    let _ = sender.send(Err(Failure::Unavailable));
                                }
                                continue;
                            }
                            _ => break events.finish(End::Disconnected),
                        };
                        let Ok(frame) = serde_json::from_str::<Value>(&line)
                            else { break events.finish(End::Disconnected) };
                        if events.ended() { break events.finish(End::Disconnected); }
                        // Methods are requests or notifications; replies match a pending ID.
                        if frame.get("method").is_some() {
                            if let Some(id) = frame.get("id") {
                                if frame["method"] == "mcpServer/elicitation/request"
                                    && let Ok(slot) = slots.clone().try_acquire_owned()
                                {
                                    let (answer, decision) = oneshot::channel();
                                    let request = ServerRequest {
                                        id: id.clone(), params: frame["params"].clone(), answer,
                                    };
                                    if requests.try_send(request).is_ok() {
                                        let id = id.clone();
                                        let thread = frame["params"]["threadId"].clone();
                                        let replies = replies.clone();
                                        let events = events.clone();
                                        let mut closed = closed.clone();
                                        // Approval waits run outside the reader, so Stop stays usable.
                                        tokio::spawn(async move {
                                            let _slot = slot;
                                            let result = tokio::select! {
                                                _ = closed.changed() => return,
                                                result = tokio::time::timeout(
                                                    Duration::from_secs(60), decision,
                                                ) => result.ok().and_then(Result::ok)
                                                    .unwrap_or(json!({
                                                        "action": "decline", "content": null,
                                                    })),
                                            };
                                            let _ = replies.send(json!({
                                                "id": id, "result": result,
                                            })).await;
                                            events.publish(json!({
                                                "method": "miniclaw/approvalResolved",
                                                "params": { "threadId": thread, "requestId": id },
                                            }).to_string().into_boxed_str());
                                        });
                                        continue;
                                    }
                                }
                                // A delegated run may take an hour, and an unattended run may wait
                                // up to an hour for an approval. The handlers keep their own,
                                // shorter limits; this one only ends a call nobody will answer.
                                let delegated = frame["params"]["tool"] == "miniclaw_delegate";
                                if frame["method"] == "item/tool/call"
                                    && (delegated || frame["params"]["tool"] == "miniclaw_scoped_native"
                                        || frame["params"]["tool"] == crate::mcp_gateway::TOOL
                                        || frame["params"]["tool"] == crate::ask::TOOL
                                        || frame["params"]["tool"] == crate::project_tool::TOOL)
                                    && tools_enabled.load(Ordering::Acquire)
                                    && let Ok(slot) = slots.clone().try_acquire_owned()
                                {
                                    let (answer, decision) = oneshot::channel();
                                    let request = ServerRequest {
                                        id: id.clone(), params: frame["params"].clone(), answer,
                                    };
                                    if tool_requests.try_send(request).is_ok() {
                                        let id = id.clone();
                                        let replies = replies.clone();
                                        let mut closed = closed.clone();
                                        tokio::spawn(async move {
                                            let _slot = slot;
                                            let result = tokio::select! {
                                                _ = closed.changed() => return,
                                                result = tokio::time::timeout(
                                                    Duration::from_secs(3900), decision,
                                                ) => result.ok().and_then(Result::ok)
                                                    .unwrap_or(json!({
                                                        "success": false, "contentItems": [],
                                                    })),
                                            };
                                            let _ = replies.send(json!({
                                                "id": id, "result": result,
                                            })).await;
                                        });
                                        continue;
                                    }
                                }
                                let reply = match refusal(&frame) {
                                    Some(result) => json!({ "id": id, "result": result }),
                                    None => json!({ "id": id, "error": {
                                        "code": -32601, "message": "Unsupported method",
                                    } }),
                                };
                                let line = format!("{reply}\n");
                                if events.ended() { break events.finish(End::Disconnected); }
                                if !matches!(tokio::time::timeout(
                                    Duration::from_secs(5),
                                    async { input.write_all(line.as_bytes()).await?; input.flush().await },
                                ).await, Ok(Ok(()))) { break events.finish(End::Disconnected); }
                            } else {
                                if !events.publish(line.into_boxed_str()) {
                                    break events.finish(End::Overloaded);
                                }
                            }
                        } else if let Some(id) = frame["id"].as_u64()
                            && let Some(sender) = pending.lock().unwrap().remove(&id) {
                                let reply = if frame.get("error").is_some() {
                                    Err(Failure::Rejected)
                                } else if let Some(result) = frame.get("result") {
                                    Ok(result.clone())
                                } else { Err(Failure::Unavailable) };
                                let _ = sender.send(reply);
                        }
                    }
                }
            };
            // Fail pending calls before closing the stream and any owned process.
            alive.store(false, Ordering::Release);
            for (_, sender) in pending.lock().unwrap().drain() {
                let _ = sender.send(Err(Failure::Unavailable));
            }
            // Exactly one fixed terminal frame bypasses the normal byte budget.
            let _ = events.sender.send(Notification::terminal(termination));
            drop(input);
            drop(lines);
            if let Some(child) = child.as_mut() {
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
            let _ = finished.send(Some(termination));
        });

        // The installed Codex build requires experimentalApi for empty environments.
        if let Err(error) = client
            .call(
                "initialize",
                json!({
                    "clientInfo": { "name": "miniclaw", "version": "0.1.0" },
                    "capabilities": { "experimentalApi": true,
                        "mcpServerOpenaiFormElicitation": true }
                }),
            )
            .await
        {
            client.stop().await;
            return Err(error);
        }
        client
            .outbound
            .send(json!({ "method": "initialized" }))
            .await
            .map_err(|_| Failure::Unavailable)?;
        Ok(client)
    }

    /// Returns a new receiver for Codex notifications and local connection events.
    pub fn subscribe(&self) -> broadcast::Receiver<Notification> {
        self.events.sender.subscribe()
    }

    /// Hands off the approval-request receiver once; later callers receive None.
    pub fn take_requests(&self) -> Option<mpsc::Receiver<ServerRequest>> {
        self.requests.lock().unwrap().take()
    }

    /// Hands the scoped-tool receiver to one trusted backend listener only.
    pub(crate) fn take_tool_requests(&self) -> Option<mpsc::Receiver<ServerRequest>> {
        let receiver = self.tool_requests.lock().unwrap().take();
        if receiver.is_some() { self.tools_enabled.store(true, Ordering::Release); }
        receiver
    }

    /// Returns whether the transport is still running, without probing Codex.
    pub fn available(&self) -> bool {
        self.alive.load(Ordering::Acquire) && !self.events.ended()
    }

    /// Waits for transport cleanup without interrupting an active connection.
    pub async fn closed(&self) {
        let mut done = self.done.clone();
        let _ = done.wait_for(Option::is_some).await;
    }

    /// Tracks backend event processing separately from transport shutdown.
    pub(crate) fn track_events(&self) -> watch::Sender<bool> {
        let (complete, completed) = watch::channel(false);
        *self.event_completion.lock().unwrap() = Some(completed);
        complete
    }

    /// Waits for the registered backend listener to finish before controller exit.
    pub async fn drain_events(&self) {
        let completed = self.event_completion.lock().unwrap().clone();
        if let Some(mut completed) = completed {
            let _ = completed.wait_for(|done| *done).await;
        }
    }

    /// Returns a lightweight completion observer that does not keep transport ownership.
    pub(crate) fn completion(&self) -> Completion {
        Completion { done: self.done.clone() }
    }

    /// Sends one RPC and matches its reply by ID, returning a result or failure
    /// within 30 seconds. Pending entries are removed even if the caller stops waiting.
    pub async fn call(&self, method: &str, params: Value) -> Reply {
        if !self.available() {
            return Err(Failure::Unavailable);
        }
        // Register the reply channel before the request can reach Codex.
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap();
            if pending.len() >= 128 {
                return Err(Failure::Unavailable);
            }
            pending.insert(id, sender);
        }
        let _waiting = Waiting {
            id,
            pending: self.pending.clone(),
        };
        let request = json!({ "id": id, "method": method, "params": params });
        tokio::time::timeout(Duration::from_secs(30), async {
            self.outbound
                .send(request)
                .await
                .map_err(|_| Failure::Unavailable)?;
            receiver.await.map_err(|_| Failure::Unavailable)?
        })
        .await
        .map_err(|_| Failure::Timeout)?
    }

    /// Returns a lost-event reporter that holds no transport ownership.
    pub(crate) fn lag(&self) -> Lag {
        Lag { events: self.events.clone(), done: self.done.clone() }
    }

    /// Closes the transport and waits for cleanup, including any owned local child.
    pub async fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
        let _ = self.shutdown.try_send(());
        let mut done = self.done.clone();
        let _ = done.wait_for(Option::is_some).await;
    }
}

/// Returns the refusal payload for a known server request, or None for unknown methods.
fn refusal(frame: &Value) -> Option<Value> {
    Some(match frame["method"].as_str().unwrap_or_default() {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            json!({ "decision": "decline" })
        }
        "item/permissions/requestApproval" => {
            json!({ "permissions": {}, "scope": "turn" })
        }
        "item/tool/requestUserInput" => json!({ "answers": {} }),
        "mcpServer/elicitation/request" => {
            json!({ "action": "decline", "content": null })
        }
        "item/tool/call" => json!({ "success": false, "contentItems": [] }),
        _ => return None,
    })
}
