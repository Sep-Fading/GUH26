// Reads notifications and builds browser snapshots.
use serde_json::{Value, json};
// Stores chat caches and shares backend ownership.
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{Arc, OnceLock, RwLock as SyncRwLock, atomic::{AtomicBool, AtomicU64, Ordering}},
    time::{Duration, Instant},
};
// Protects shared state and wakes subscribers.
use tokio::sync::{Mutex, RwLock, Semaphore, oneshot, watch};
// Creates fresh epochs for replay cursors.
use uuid::Uuid;

// Validates computer approvals and finds the helper.
use crate::computer::{self, Approval};
// Records requested settings in send receipts.
use crate::models::Choice;
// Receives Codex events and requests cancellation.
use crate::rpc::{Client, Failure};
// Stores agent definitions and pinned chat bindings.
use crate::agents::Store;
// Selects only the pinned worker during cleanup.
use crate::environment::Assignment;
// Checks exact scoped operations and permission targets.
use crate::permissions::{Decision as PermissionDecision, Target};
use crate::scoped::Operation;

#[derive(Clone)]
pub struct Receipt {
    pub text: String,
    pub choice: Choice,
    pub turn: Option<String>,
    /// The uploads the message carried, so a repeated send is recognised as the same one.
    pub attachments: Vec<uuid::Uuid>,
}

pub struct Backend {
    pub search: crate::search::Search,
    pub client: Option<Client>,
    pub master: SyncRwLock<Option<Client>>,
    pub dedicated: SyncRwLock<HashMap<String, Client>>,
    pub host_client: Option<Client>,
    pub host: SyncRwLock<Option<Client>>,
    pub claude: SyncRwLock<Option<Client>>,
    pub claude_threads: SyncRwLock<HashSet<String>>,
    pub providers: OnceLock<crate::providers::Runtime>,
    // Where VM chats keep a readable copy on the Mac; unset in tests without storage.
    pub transcripts: OnceLock<std::path::PathBuf>,
    // Where display choices are kept; unset in tests without storage.
    pub appearance: OnceLock<std::path::PathBuf>,
    /// Per-VM idle limits, recent use and open shutdown warnings.
    pub idle: crate::idle::Idle,
    /// Running file transfers, the Mac paths the user chose, and the panel that asks for them.
    pub transfers: crate::transfer_jobs::Transfers,
    pub selections: crate::transfer_jobs::Selections,
    pub picker: OnceLock<crate::transfer_jobs::Picker>,
    /// Experimental folder backups; unset means the feature is unavailable.
    pub backups: OnceLock<crate::backups::Backups>,
    /// What MiniClaw's tool did in each conversation, shown to the user only.
    pub activity: crate::activity::Log,
    // Each conversation's own folder under Documents/MiniClaw, once it has one.
    pub chat_folders: crate::chat_folder::Store,
    /// Files and pictures added to messages, kept with their conversation.
    pub attachments: crate::attachments::Store,
    /// Handles a delegation tool call; set by the API layer, which owns chat creation.
    pub delegate: OnceLock<DelegateHandler>,
    /// Shows a model's question to the user and waits; set by the API layer, which can end the turn.
    pub ask: OnceLock<DelegateHandler>,
    /// Open connections to MCP servers, one per server.
    pub mcp: crate::mcp_gateway::Hub,
    /// Called when a conversation's turn ends, with its status and whether the user had stopped
    /// it; set by the API layer, which sends the next queued message or pauses the queue.
    pub turn_ended: OnceLock<Box<dyn Fn(Arc<Backend>, String, String, bool) + Send + Sync>>,
    /// Checks the saved schedules; set by the API layer, called by the background watcher.
    pub schedule_tick: OnceLock<Box<dyn Fn(Arc<Backend>) + Send + Sync>>,
    /// Conversations that run with nobody watching, with how long an approval may wait, and
    /// the request that ran out of time in each.
    /// The shared shell of each conversation that has one.
    pub terminals: crate::terminal::Terminals,
    /// The tools each VM's worker said it accepts beside the file tool; asked once per connection.
    pub worker_tools: std::sync::Mutex<HashMap<String, Vec<String>>>,
    /// The version of the project memory index each conversation was last told.
    pub project_seen: std::sync::Mutex<HashMap<String, u64>>,
    pub unattended: std::sync::Mutex<HashMap<String, Duration>>,
    pub unattended_expired: std::sync::Mutex<HashMap<String, String>>,
    /// The delegation each conversation is asking the user about, with the way to answer it.
    pub delegation_prompts: Mutex<HashMap<String, (Value, tokio::sync::oneshot::Sender<String>)>>,
    // The Mac's own Computer Use helper when `computer_helper` describes the VM.
    pub local_helper: OnceLock<computer::Target>,
    pub cwd: String,
    pub host_cwd: Option<String>,
    pub origin: String,
    pub data: Mutex<Data>,
    pub auth_gate: Mutex<()>,
    pub login_gate: Mutex<()>,
    pub create_gate: Mutex<()>,
    pub load_gate: Mutex<()>,
    pub changes: watch::Sender<u64>,
    pub streams: Arc<Semaphore>,
    pub computer_helper: Option<computer::Target>,
    pub master_computer_ready: AtomicBool,
    // Project VMs whose own worker reported an installed Computer Use helper.
    pub dedicated_computer_ready: SyncRwLock<HashSet<String>>,
    // True when VM links exist: the Mac's Codex is the Host worker and VM workers can
    // attach. False means a Mac with no VMs, where every chat runs locally.
    pub vm_routing: bool,
    // True when this backend runs on the Mac itself, with or without VM links.
    pub local: AtomicBool,
    pub recovery: PathBuf,
    pub agents: Store,
    pub agent_gate: RwLock<()>,
    pub vm_start_approvals: Mutex<HashMap<Uuid, VmStartApproval>>,
    pub permission_revision: AtomicU64,
    pub shell_grants: Mutex<HashMap<String, crate::shell_access::Grant>>,
}

pub struct VmStartApproval {
    pub assignment: Assignment,
    pub thread: Option<String>,
    pub project: Option<Uuid>,
    pub message: Uuid,
    pub expires: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopedChoice { Once, Session, SessionDomains, Programs, Reject }

/// What the user decided about one proposed change in a coding session.
pub enum ReviewChoice {
    /// Write the change; `text` is the user's own version when they edited the proposal.
    Apply { text: Option<String>, rest_of_turn: bool },
    Reject { note: String },
}

/// One proposed file change waiting for the user's review. Nothing is written while it waits.
pub struct Review {
    pub id: Uuid,
    pub turn: String,
    pub path: String,
    /// `edit` replaces one passage, `write` replaces the whole file, `create` makes a new one.
    pub kind: &'static str,
    pub diff: String,
    /// The complete new content, which the user may edit before applying.
    pub proposed: String,
    pub expires: Instant,
    pub answer: oneshot::Sender<ReviewChoice>,
}

impl Review {
    /// Reports whether this exact pending review can still be decided.
    pub fn current(&self) -> bool { Instant::now() < self.expires }

    /// Shows the file, the diff and the proposed content to the user.
    pub fn view(&self) -> Value {
        json!({ "id": self.id, "path": self.path, "kind": self.kind, "diff": self.diff, "proposed": self.proposed,
            "expiresInSeconds": self.expires.saturating_duration_since(Instant::now()).as_secs() })
    }
}

pub struct ScopedApproval {
    pub id: Uuid,
    pub turn: String,
    pub target: Target,
    // Hosts a command wants that are not yet allowed; decided together with the command.
    pub hosts: Vec<String>,
    /// What else the user must see before deciding: for an MCP tool, the server's name, the
    /// arguments about to be sent, and whether the server runs outside the sandbox.
    pub extra: Option<Value>,
    pub expires: Instant,
    pub answer: oneshot::Sender<ScopedChoice>,
}

/// A question the model asked and the user has not answered yet.
pub struct PendingQuestion {
    pub id: Uuid,
    pub turn: String,
    pub questions: Vec<crate::ask::Question>,
    pub expires: Instant,
    pub answer: oneshot::Sender<QuestionReply>,
}

/// What the user did with a question: answered it, or chose to reply with a message instead.
pub enum QuestionReply { Answers(Vec<crate::ask::Answer>), InChat }

impl PendingQuestion {
    /// Reports whether the question can still be answered.
    pub fn current(&self) -> bool { Instant::now() < self.expires }

    /// Shows the questions and how long they wait.
    pub fn view(&self) -> Value {
        json!({ "id": self.id, "questions": self.questions,
            "expiresInSeconds": self.expires.saturating_duration_since(Instant::now()).as_secs() })
    }
}

impl ScopedApproval {
    /// Reports whether this exact pending request can still be decided.
    pub fn current(&self) -> bool { Instant::now() < self.expires }

    /// Shows the exact target, command, and working directory to the user.
    pub fn view(&self) -> Value {
        // `domains` is what the wider choice would cover: the domain above each host, or the
        // host itself where no wider rule is reasonable.
        let domains: Vec<String> = self.hosts.iter().map(|host|
            crate::permissions::parent_domain(host).unwrap_or_else(|| host.clone())).collect();
        let mut view = json!({ "id": self.id, "target": self.target, "hosts": self.hosts, "domains": domains,
            "expiresInSeconds": self.expires.saturating_duration_since(Instant::now()).as_secs() });
        if let (Some(extra), Some(view)) = (self.extra.as_ref().and_then(Value::as_object), view.as_object_mut()) {
            for (name, value) in extra { view.entry(name.clone()).or_insert(value.clone()); }
        }
        view
    }
}

// This is a process-local cache. Codex owns the durable transcripts.
#[derive(Default)]
pub struct Data {
    /// Receipts of answered messages from conversations that left the cache, newest last. A
    /// retried send still finds its answer here and starts no second response.
    pub kept_receipts: VecDeque<(String, String, Receipt)>,
    pub account: Value,
    // Claude's identity is cached apart from Codex's so neither invalidates the other.
    pub claude_account: Value,
    pub login: Value,
    pub login_id: Option<String>,
    pub login_results: HashMap<String, bool>,
    pub chats: HashMap<String, Chat>,
    pub loading: HashMap<String, crate::history::Loading>,
    pub usage: HashMap<String, Value>,
    // Invalidate pending work when the account or runtime connection changes.
    pub generation: u64,
    pub notice_error: bool,
}

/// What came of a coding session's review of one change.
enum Reviewed { Unchanged, Replaced(Operation), Refused(String) }

/// Splits a path into its folder and its leaf name, as the machine that holds the file does.
/// A Windows path separates with either slash and must name both parts; a Mac or VM path
/// splits at its last `/` as it always has.
fn folder_and_name(path: &str, windows: bool) -> Option<(String, String)> {
    if !windows {
        let (folder, name) = path.rsplit_once('/').unwrap_or(("/", path));
        return Some((folder.to_owned(), name.to_owned()));
    }
    let cut = path.rfind(['\\', '/'])?;
    let (folder, name) = (&path[..cut], &path[cut + 1..]);
    if folder.is_empty() || name.is_empty() { return None; }
    // A drive's root keeps its separator: `C:` alone would mean the drive's current folder.
    let folder = if folder.ends_with(':') { format!("{folder}{}", &path[cut..=cut]) } else { folder.to_owned() };
    Some((folder, name.to_owned()))
}

/// What a folder's listing says about a file that could not be read as text.
#[derive(Debug, PartialEq)]
enum Listed { Absent, Present, Unknown }

/// Looks for a leaf name in a folder listing. Windows names one file by every spelling of
/// its case, so there a different spelling is still this file.
fn listed(entries: Option<&str>, name: &str, windows: bool) -> Listed {
    let Some(entries) = entries else { return Listed::Unknown };
    let same = |entry: &str| entry == name || windows && entry.to_lowercase() == name.to_lowercase();
    if entries.lines().any(|line| line.split('\t').next().is_some_and(same)) { Listed::Present } else { Listed::Absent }
}

/// Runs one delegation tool call to its answer.
pub type DelegateHandler = Box<dyn Fn(Arc<Backend>, crate::rpc::ServerRequest)
    -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

pub struct Chat {
    pub lease: Arc<()>,
    pub accessed: Instant,
    pub history_loaded: bool,
    pub summary: Value,
    pub messages: Vec<Value>,
    pub active: Option<String>,
    pub sending: bool,
    pub uncertain: bool,
    pub finished: VecDeque<String>,
    /// The newest completed turn that left a readable assistant response.
    pub last_response: Option<(String, i64)>,
    pub receipts: HashMap<String, Receipt>,
    pub events: VecDeque<Event>,
    replay_bytes: usize,
    pub sequence: u64,
    pub epoch: String,
    pub approval: Option<Approval>,
    pub scoped_approval: Option<ScopedApproval>,
    /// The question the model is waiting on, if any.
    pub question: Option<PendingQuestion>,
    /// The change a coding session is asking the user to review, and the turn whose remaining
    /// changes the user chose to apply without asking.
    /// Messages loaded half-written from history; live deltas for them are ignored until the
    /// complete message arrives, because the two cannot be lined up.
    pub frozen: HashSet<String>,
    pub review: Option<Review>,
    pub review_apply_all: Option<String>,
    pub stopping: bool,
    /// The turn the provider last reported tokens for, and its newest `last` breakdown.
    pub turn_tokens: Option<(String, Value)>,
    /// The final error the provider reported for a turn, until that turn completes.
    pub turn_error: Option<(String, Value)>,
    /// When the running turn started, so a finished one can be timed.
    pub turn_started: Option<i64>,
    pub computer_activity: Vec<Value>,
    pub history_cursor: Option<String>,
    pub computer_changing: bool,
    pub computer_error: Option<&'static str>,
    pub restore_pending: bool,
    /// A native image-tool reload must finish before another turn starts.
    pub image_changing: bool,
    pub image_error: Option<&'static str>,
    pub image_retry_on: Option<bool>,
    pub image_restore_pending: bool,
}

#[derive(Clone)]
pub struct Event {
    pub id: String,
    pub name: String,
    pub data: String,
}

impl Event {
    /// Returns the retained UTF-8 payload size, including its event name and cursor.
    fn bytes(&self) -> usize {
        self.id.len() + self.name.len() + self.data.len()
    }
}

impl Chat {
    /// Creates an empty chat cache with a new replay epoch from its task summary.
    pub fn new(summary: Value) -> Self {
        Self {
            lease: Arc::new(()),
            accessed: Instant::now(),
            history_loaded: false,
            summary,
            messages: Vec::new(),
            active: None,
            sending: false,
            uncertain: false,
            finished: VecDeque::new(),
            last_response: None,
            receipts: HashMap::new(),
            events: VecDeque::new(),
            replay_bytes: 0,
            sequence: 0,
            epoch: Uuid::new_v4().to_string(),
            approval: None,
            scoped_approval: None,
            question: None,
            frozen: HashSet::new(),
            review: None,
            review_apply_all: None,
            stopping: false,
            turn_tokens: None,
            turn_error: None,
            turn_started: None,
            computer_activity: Vec::new(),
            history_cursor: None,
            computer_changing: false,
            computer_error: None,
            restore_pending: false,
            image_changing: false,
            image_error: None,
            image_retry_on: None,
            image_restore_pending: false,
        }
    }

    /// Reports whether dropping this cache preserves running work and send deduplication.
    fn evictable(&self) -> bool {
        self.history_loaded && Arc::strong_count(&self.lease) == 1
            && self.active.is_none() && !self.sending && !self.stopping && !self.uncertain
            // A receipt protects a retried send. One whose turn has finished protects nothing
            // any more, and must not hold the slot for the life of the backend.
            && self.receipts.values().all(|receipt| receipt.turn.as_ref().is_some_and(|turn| self.finished.contains(turn)))
            && self.approval.is_none() && self.review.is_none()
            && self.scoped_approval.is_none()
            && !self.computer_changing && self.computer_error.is_none()
            && !self.restore_pending && self.summary["computerUse"] != true
            && !self.image_changing && self.image_error.is_none() && !self.image_restore_pending
    }

    /// Pins this cache for an HTTP request or stream and records its latest access.
    pub fn pin(&mut self) -> Arc<()> {
        self.accessed = Instant::now();
        self.lease.clone()
    }

    /// Returns this cache's epoch and event sequence as a reconnect cursor.
    pub fn cursor(&self) -> String {
        format!("{}:{}", self.epoch, self.sequence)
    }

    /// Returns the chat's browser-facing messages, response state, cursors, and approval.
    pub fn snapshot(&self) -> Value {
        json!({
            "thread": self.summary, "messages": self.messages,
            "activeTurn": self.active.as_ref().map(|id| json!({ "id": id })),
            "pending": self.sending, "outcomeUnknown": self.uncertain,
            "eventCursor": self.cursor(), "historyCursor": self.history_cursor,
            "approval": self.approval.as_ref().filter(|a| a.current()).map(Approval::view),
            "scopedApproval": self.scoped_approval.as_ref()
                .filter(|a| a.current()).map(ScopedApproval::view),
            "question": self.question.as_ref().filter(|open| open.current()).map(PendingQuestion::view),
            "review": self.review.as_ref().filter(|review| review.current()).map(Review::view),
            "computerActivity": self.computer_activity,
            "computerChanging": self.computer_changing,
            "computerError": self.computer_error,
            "imageChanging": self.image_changing, "imageError": self.image_error,
            "imageRetryOn": self.image_retry_on,
        })
    }

    /// Publishes permission state without replacing messages or the event stream.
    pub fn computer_changed(&mut self) {
        self.emit("computer.updated", json!({
            "thread": self.summary, "computerChanging": self.computer_changing,
            "computerError": self.computer_error,
        }));
    }

    /// Retains at most 512 events and 2 MiB of encoded replay; oversized events
    /// leave a cursor gap so subscribers recover the unchanged full snapshot.
    pub fn emit(&mut self, name: &str, data: Value) {
        if matches!(name, "turn.started" | "turn.completed" | "message.delta"
            | "message.completed" | "activity") {
            self.summary["updatedAt"] = json!(chrono::Utc::now().timestamp_millis());
        }
        self.sequence += 1;
        let event = Event {
            id: self.cursor(),
            name: name.into(),
            data: data.to_string(),
        };
        let bytes = event.bytes();
        if bytes > 2 * 1024 * 1024 {
            self.events.clear();
            self.replay_bytes = 0;
            return;
        }
        while self.events.len() >= 512 || self.replay_bytes + bytes > 2 * 1024 * 1024 {
            self.replay_bytes -= self.events.pop_front().unwrap().bytes();
        }
        self.replay_bytes += bytes;
        self.events.push_back(event);
    }

    /// Keeps a long-lived conversation within the same bound its saved history obeys: at most
    /// 10,000 messages in memory. The oldest leave first; the provider still holds them, and
    /// the page loads older messages from there.
    pub fn bound_messages(&mut self) {
        const KEPT: usize = 10_000;
        if self.messages.len() > KEPT { self.messages.drain(..self.messages.len() - KEPT); }
    }

    /// Writes a turn onto the user's message that started it: the message with `id`, or else the
    /// newest message when it is the user's and names no other turn. Returns that message's id.
    pub fn name_turn(&mut self, id: Option<&str>, turn: &str) -> Option<String> {
        let message = match id {
            Some(id) => self.messages.iter_mut().rev().find(|message| message["id"] == id),
            None => self.messages.last_mut(),
        }.filter(|message| message["role"] == "user"
            && (message["turnId"].is_null() || message["turnId"] == turn))?;
        message["turnId"] = json!(turn);
        message["id"].as_str().map(str::to_owned)
    }

    /// Adds or updates an assistant message by item ID; final text replaces deltas.
    fn assistant(&mut self, id: &str, text: &str, complete: bool) {
        if complete { self.frozen.remove(id); } else if self.frozen.contains(id) { return; }
        if let Some(message) =
            self.messages.iter_mut().find(|message| message["id"] == id)
        {
            // Completion carries the full text, not another delta.
            if complete {
                message["text"] = json!(text);
                message["complete"] = json!(true);
            } else if message["complete"] != true {
                let mut current = message["text"].as_str().unwrap_or_default().to_owned();
                if current.len() + text.len() <= 2 * 1024 * 1024 {
                    current.push_str(text);
                    message["text"] = json!(current);
                }
            }
        } else {
            self.messages.push(json!({
                "id": id, "role": "assistant", "text": text,
                "complete": complete, "turnId": self.active,
            }));
            self.bound_messages();
        }
    }

    /// Applies a Codex notification to this chat and queues the resulting browser events.
    pub fn notify(&mut self, method: &str, params: &Value) {
        self.accessed = Instant::now();
        let turn = params["turnId"]
            .as_str()
            .or_else(|| params["turn"]["id"].as_str())
            .unwrap_or_default();
        if self.finished.iter().any(|id| id == turn) {
            return;
        }
        match method {
            "serverRequest/resolved" | "miniclaw/approvalResolved" => {
                if self
                    .approval
                    .as_ref()
                    .is_some_and(|approval| approval.request.id == params["requestId"])
                {
                    self.approval = None;
                    self.emit("snapshot", self.snapshot());
                }
            }
            "item/started" | "item/completed"
                if params["item"]["type"] == "mcpToolCall"
                    && params["item"]["server"] == computer::SERVER
                    && self.summary["computerUse"] == true =>
            {
                let item = &params["item"];
                // Keep tool status here; raw results and screenshots stay out of the UI.
                let mut activity = json!({ "id": item["id"], "tool": item["tool"],
                    "app": item["arguments"]["app"].as_str(), "status": item["status"],
                });
                if item["status"] == "failed" && let Some((error, hint)) = computer::failure(item) {
                    activity["error"] = json!(error);
                    if let Some(hint) = hint { activity["hint"] = json!(hint); }
                }
                if let Some(current) = self
                    .computer_activity
                    .iter_mut()
                    .find(|entry| entry["id"] == item["id"])
                {
                    *current = activity;
                } else {
                    self.computer_activity.push(activity);
                    if self.computer_activity.len() > 20 {
                        self.computer_activity.remove(0);
                    }
                }
                self.emit("snapshot", self.snapshot());
            }
            // Codex reports what a turn cost while it runs; the newest report is the whole turn.
            "thread/tokenUsage/updated" => {
                if let Some(usage) = params["tokenUsage"]["last"].as_object() {
                    self.turn_tokens = Some((turn.to_owned(), json!(usage)));
                }
            }
            "turn/started" => {
                self.active = Some(turn.into());
                self.turn_started = Some(chrono::Utc::now().timestamp_millis());
                // The page places a turn's steps by its messages, also when no reply comes.
                let message = self.name_turn(None, turn);
                self.emit("turn.started", json!({ "turnId": turn, "messageId": message }));
            }
            "item/agentMessage/delta" => {
                if let (Some(id), Some(text)) =
                    (params["itemId"].as_str(), params["delta"].as_str())
                {
                    self.assistant(id, text, false);
                    self.emit(
                        "message.delta",
                        json!({
                            "itemId": id, "turnId": turn, "delta": text,
                        }),
                    );
                }
            }
            "item/completed" if params["item"]["type"] == "agentMessage" => {
                if let (Some(id), Some(text)) = (
                    params["item"]["id"].as_str(),
                    params["item"]["text"].as_str(),
                ) {
                    self.assistant(id, text, true);
                    self.emit(
                        "message.completed",
                        json!({
                            "itemId": id, "turnId": turn, "text": text,
                        }),
                    );
                }
            }
            "item/started" | "item/completed" if params["item"]["type"] == "imageGeneration" => {
                let item = &params["item"];
                if let Some(id) = item["id"].as_str().filter(|id| !id.is_empty()) {
                    let status = if method == "item/started" { "generating" }
                        else { item["status"].as_str().filter(|status| !status.is_empty()).unwrap_or("generating") };
                    let text = if item["storageFailure"] == true { "Could not save generated image." }
                    else if item["failure"]["type"] == "usageLimitExceeded" {
                        "Image generation limit reached."
                    } else if status == "failed" { "Image generation failed." }
                    else { "" };
                    let mut message = json!({ "id": id, "role": "assistant", "text": text,
                        "turnId": turn, "complete": status != "generating", "imageStatus": status });
                    if item["attachment"].is_object() { message["attachments"] = json!([item["attachment"]]); }
                    if let Some(current) = self.messages.iter_mut().find(|message| message["id"] == id) {
                        *current = message;
                    } else { self.messages.push(message); self.bound_messages(); }
                    self.summary["updatedAt"] = json!(chrono::Utc::now().timestamp_millis());
                    self.emit("snapshot", self.snapshot());
                }
            }
            // A final error (one the App Server will not retry) explains the failed turn that follows.
            "error" if params["willRetry"] != true && params["error"].is_object() => {
                self.turn_error = Some((turn.to_owned(), params["error"].clone()));
            }
            "turn/completed" => {
                self.approval = None;
                self.scoped_approval = None;
                // Dropping the question tells its waiting tool call that the turn is over.
                self.question = None;
                // A turn that ends takes its open review and its "apply all" with it.
                self.review = None;
                self.review_apply_all = None;
                self.active = None;
                self.uncertain = false;
                self.finished.push_back(turn.into());
                if self.finished.len() > 128 {
                    self.finished.pop_front();
                }
                let status = params["turn"]["status"].as_str().unwrap_or("failed");
                if status == "completed" && self.messages.iter().any(|message|
                    message["role"] == "assistant" && message["turnId"] == turn
                    && message["complete"] == true
                    && (message["text"].as_str().is_some_and(|text| !text.trim().is_empty())
                        || message["attachments"].as_array().is_some_and(|files| !files.is_empty()))) {
                    self.last_response = Some((turn.to_owned(), chrono::Utc::now().timestamp_millis()));
                }
                let mut activity_changed = false;
                // Interrupted turns can end without a final native-tool notification.
                for activity in &mut self.computer_activity {
                    if activity["status"] == "inProgress" {
                        activity["status"] = json!(if status == "interrupted" {
                            "interrupted"
                        } else {
                            "unknown"
                        });
                        activity_changed = true;
                    }
                }
                let reported = Some(&params["turn"]["error"]).filter(|error| error.is_object()).cloned()
                    .or_else(|| self.turn_error.take().filter(|(id, _)| id == turn).map(|(_, error)| error));
                self.turn_error = None;
                let provider = if self.summary["provider"] == "claude" { "Claude" } else { "Codex" };
                self.emit(
                    "turn.completed",
                    json!({
                        "turnId": turn, "status": status,
                        "error": (status == "failed").then(|| failure_reason(provider, reported.as_ref())),
                    }),
                );
                if activity_changed {
                    self.emit("snapshot", self.snapshot());
                }
            }
            _ => {}
        }
    }
}

/// Checks whether a cached task belongs to the worker that emitted an event.
fn worker_owns(chat: &Chat, worker: &Assignment, primary: &Assignment) -> bool {
    match chat.summary["environment"]["kind"].as_str() {
        Some("host") => matches!(worker, Assignment::Host {}),
        Some("vm") => matches!(worker, Assignment::Vm { vm_id } if
            chat.summary["vmId"].as_str() == Some(vm_id.as_str())),
        _ => worker == primary,
    }
}

impl Data {
    /// Returns the notice owner: the ChatGPT identity, else the Claude identity.
    pub fn owner(&self) -> Option<String> {
        crate::notifications::owner(&self.account)
            .or_else(|| crate::notifications::owner(&self.claude_account))
    }

    /// Frees one least-recently-used safe cache slot, preserving Codex's saved history.
    pub fn make_room(&mut self) -> bool {
        if self.chats.len() < 32 {
            return true;
        }
        let candidate = self.chats.iter()
            .filter(|(_, chat)| chat.evictable())
            .min_by_key(|(_, chat)| chat.accessed)
            .map(|(id, _)| id.clone());
        if let Some(id) = candidate {
            if let Some(chat) = self.chats.remove(&id) {
                for (client, receipt) in chat.receipts { self.kept_receipts.push_back((id.clone(), client, receipt)); }
                while self.kept_receipts.len() > 2048 { self.kept_receipts.pop_front(); }
            }
            true
        } else {
            false
        }
    }
}

impl Backend {
    /// Resolves one model tool call through consent, then returns only a bounded result.
    async fn scoped_tool(self: Arc<Self>, worker: Assignment,
        primary: Assignment, request: crate::rpc::ServerRequest) {
        // The user follows every call live, whatever its outcome. The entry is theirs alone:
        // the model receives only `result`.
        let thread = request.params["threadId"].as_str().unwrap_or_default().to_owned();
        let mut request = request;
        // A conversation with no project and no agent gets its own folder at its first command,
        // write or edit, so a conversation that only talks never makes one.
        if matches!(request.params["arguments"]["action"].as_str(), Some("run_shell" | "write_file" | "replace_in_file"))
            && self.plain_host_chat(&thread, &worker).await
            && crate::seatbelt::available() {
            let title = self.data.lock().await.chats.get(&thread)
                .and_then(|chat| chat.summary["name"].as_str().map(str::to_owned)).unwrap_or_default();
            let folders = self.clone();
            let id = thread.clone();
            match tokio::task::spawn_blocking(move || folders.chat_folders.ensure(&id, &title)).await {
                Ok(Ok(_)) => { tracing::debug!(thread, "conversation folder ready"); self.changed(); }
                Ok(Err(error)) => tracing::warn!(thread, %error, "conversation folder could not be made"),
                Err(_) => {}
            }
        }
        // A command sent without its folder runs in the conversation's working folder, when it has one.
        if request.params["arguments"]["action"] == "run_shell" && request.params["arguments"].get("cwd").is_none()
            && request.params["arguments"].is_object()
            && let Some(folder) = self.working_folder(&thread, &worker).await {
            tracing::debug!(thread, "run_shell without cwd uses the working folder");
            request.params["arguments"]["cwd"] = json!(folder);
        }
        let entry = match (request.params["callId"].as_str(), request.params["turnId"].as_str(),
            Operation::parse(&request.params["arguments"])) {
            (Some(id), Some(turn), Ok(operation)) => {
                let agent = self.data.lock().await.chats.get(&thread)
                    .and_then(|chat| chat.summary["agentName"].as_str().map(str::to_owned));
                Some(crate::activity::started(id, turn, &operation, agent.as_deref()))
            }
            // A call MiniClaw cannot read is still shown, so the user sees what was attempted.
            (Some(id), Some(turn), Err(_)) => Some(crate::activity::malformed(id, turn, &request.params["arguments"])),
            _ => None,
        };
        if let Some(entry) = &entry { self.publish_activity(&thread, entry).await; }
        let mut executed = None;
        let mut why = None;
        let malformed = entry.as_ref().is_some_and(|entry| entry["malformed"] == true);
        let result = self.scoped_tool_result(&worker, &primary, &request.params, &mut executed, &mut why).await
            .unwrap_or_else(|| if let Some(reason) = &why {
                // A refusal says why, so the model can tell the user what to change.
                json!({ "success": false, "contentItems": [{ "type": "inputText", "text": reason }] })
            } else if malformed {
                // Silence made a real model report "no output". It is told what was wrong.
                json!({ "success": false, "contentItems": [{ "type": "inputText", "text":
                    crate::activity::malformed_reason(&request.params["arguments"]) }] })
            } else { json!({ "success": false, "contentItems": [] }) });
        let _ = request.answer.send(result);
        if let Some(entry) = entry {
            let mut done = crate::activity::finished(entry, executed.as_ref());
            // The user reads the same reason in the activity row.
            if done["status"] == "refused" && let Some(reason) = why { done["output"] = json!(reason); }
            else if malformed { done["output"] = json!(crate::activity::malformed_reason(&request.params["arguments"])); }
            self.publish_activity(&thread, &done).await;
        }
    }

    /// The folder a conversation's commands run in when they name none: its project's folder, or for
    /// a VM conversation the VM's shared workspace. `None` when it has neither.
    async fn working_folder(self: &Arc<Self>, thread: &str, worker: &Assignment) -> Option<String> {
        let context = self.agents.permission_context(thread.to_owned()).await.ok()?;
        let project = self.agents.project_context(thread.to_owned(), context.agent(), false).await.ok()?
            .and_then(|project| project.project.definition.workspace);
        project.or_else(|| matches!(worker, Assignment::Vm { .. }).then(|| self.cwd.clone()))
            .or_else(|| self.own_folder(thread, context.agent().is_none()))
    }

    /// A conversation with no project and no agent, on this computer.
    pub async fn plain_host_chat(&self, thread: &str, worker: &Assignment) -> bool {
        matches!(worker, Assignment::Host {}) && self.is_local()
            && self.data.lock().await.chats.get(thread).is_some_and(|chat|
                chat.summary["agentId"].as_str().is_none() && chat.summary["projectId"].as_str().is_none())
    }

    /// The conversation's own folder under Documents/MiniClaw, when it has one that still exists.
    /// Only a conversation with no agent has one; a project's folder is its project's.
    pub fn own_folder(&self, thread: &str, no_agent: bool) -> Option<String> {
        if !no_agent { return None; }
        self.chat_folders.recorded(thread).filter(|folder| std::path::Path::new(folder).is_dir())
    }

    /// Records one activity entry and sends it to the conversation's open pages.
    pub async fn publish_activity(self: &Arc<Self>, thread: &str, entry: &Value) {
        self.activity.record(thread, entry);
        // Finished steps are counted for the Analytics tab: the kind, what it named, and how it went.
        if entry["status"] != "running" && let (Some(id), Some(kind)) = (entry["id"].as_str(), entry["kind"].as_str()) {
            let step = crate::analytics::Step { thread: thread.to_owned(),
                turn: entry["turnId"].as_str().unwrap_or_default().to_owned(), kind: kind.to_owned(),
                // An MCP step is named `server: tool`; the others are named by their kind alone,
                // because a title holds a path, a command or a question the user typed.
                name: if kind == "mcp" { entry["title"].as_str().unwrap_or_default().chars().take(160).collect() } else { String::new() },
                status: entry["status"].as_str().unwrap_or("ok").to_owned(),
                ms: (entry["endedAt"].as_i64().unwrap_or(0) - entry["startedAt"].as_i64().unwrap_or(0)).max(0) };
            let (backend, id) = (self.clone(), id.to_owned());
            tokio::spawn(async move { let _ = backend.agents.record_step(id, step).await; });
        }
        if let Some(chat) = self.data.lock().await.chats.get_mut(thread) { chat.emit("activity", entry.clone()); }
        // What a delegated agent does is also shown in the conversation that asked for it,
        // under the agent's name and its run.
        if let Ok(Some(run)) = self.agents.run_of_thread(thread.to_owned()).await && run.is_delegated() {
            let mut copy = entry.clone();
            copy["runId"] = json!(run.id);
            copy["agent"] = json!(run.agent);
            copy["turnId"] = json!(run.turn_id);
            self.activity.record(&run.parent, &copy);
            if let Some(chat) = self.data.lock().await.chats.get_mut(&run.parent) { chat.emit("activity", copy); }
        }
        // Open pages wait on this signal; without it the entry shows only when something else happens.
        self.changed();
    }

    /// Checks chat identity and every target before asking or executing in the guest.
    async fn scoped_tool_result(self: &Arc<Self>, worker: &Assignment,
        primary: &Assignment, params: &Value, report: &mut Option<Value>, why: &mut Option<String>) -> Option<Value> {
        // A Mac chat executes here under Seatbelt; a VM chat executes in its own VM's
        // worker under the same kind of profile.
        let host = self.is_local() && matches!(worker, Assignment::Host {});
        let machine = match worker {
            Assignment::Host {} if host => "host".to_owned(),
            Assignment::Vm { vm_id } if self.vm_routing => vm_id.clone(),
            _ => return None,
        };
        let thread = params["threadId"].as_str()?;
        let turn = params["turnId"].as_str()?;
        let mut operation = Operation::parse(&params["arguments"]).ok()?;
        // Apple Events and GUI control make an unsandboxed app act, so they are refused
        // on every machine before asking the user anything.
        if !matches!(operation, Operation::ReadFile { .. } | Operation::WriteFile { .. }
                | Operation::RunShell { .. } | Operation::ListFolder { .. }
                | Operation::Search { .. } | Operation::ReplaceInFile { .. }) {
            *why = Some(refusal::UNSUPPORTED.into());
            return None;
        }
        let shell_grant = if host && matches!(operation, Operation::RunShell { .. }) {
            self.shell_grant(thread).await
        } else { None };
        let unrestricted = shell_grant.is_some();
        if host && !crate::scoped::host_execution_available(crate::seatbelt::available(), &operation, unrestricted) {
            return Some(json!({ "success": false, "contentItems": [{ "type": "inputText",
                "text": crate::seatbelt::with_reason("Sandboxed tools are unavailable on this platform. A shell command requires the user to grant timed Unsandboxed shell access in Tools.") }] }));
        }
        let mut targets = operation.targets();
        if unrestricted { targets.retain(|target| !matches!(target, Target::Host { .. })); }
        {
            let data = self.data.lock().await;
            let chat = data.chats.get(thread)?;
            // A host chat with no agent and no project acts only in the sandbox, in its own folder
            // and on what the user approves; outside the sandbox only under a live timed grant.
            let plain = host && !unrestricted && chat.summary["agentId"].as_str().is_none()
                && chat.summary["projectId"].as_str().is_none() && !crate::seatbelt::available();
            if plain { tracing::debug!(thread, "scoped tool refused: plain host chat without the sandbox or a timed shell grant"); *why = Some(refusal::PLAIN.into()); }
            if chat.stopping || chat.active.as_deref() != Some(turn) { *why = Some(refusal::TURN_OVER.into()); }
            if !worker_owns(chat, worker, primary) || plain
                || chat.summary["computerUse"] == true
                || chat.stopping || chat.active.as_deref() != Some(turn) {
                return None;
            }
        }
        let context = self.agents.permission_context(thread.to_owned()).await.ok()?;
        if host && !unrestricted && context.agent().is_none() && context.project().is_none() && !crate::seatbelt::available() {
            *why = Some(refusal::PLAIN.into());
            return None;
        }
        if context.machine() != machine
            || self.agents.admit(thread.to_owned()).await.is_err() {
            return None;
        }
        // A VM chat's operations run in that VM's own worker, never another machine's.
        let remote = match worker {
            Assignment::Vm { vm_id } if vm_id == "master" => Some(self.primary_client()?),
            Assignment::Vm { vm_id } => Some(self.dedicated_client(vm_id)?),
            Assignment::Host {} => None,
        };
        let mut once = Vec::new();
        let mut revision = self.permission_revision.load(Ordering::Acquire);
        // A project's own folder needs no rule; an explicit block inside it still wins.
        let workspace = self.agents.project_context(thread.to_owned(), context.agent(), false)
            .await.ok()?.and_then(|project| project.project.definition.workspace)
            // A host chat with no project and no agent owns its own folder, once it has one.
            .or_else(|| if host && context.project().is_none() { self.own_folder(thread, context.agent().is_none()) } else { None });
        // A VM chat also owns the VM's shared workspace folder.
        let vm_workspace = (!host).then(|| self.cwd.clone());
        let in_workspace = |target: &Target| match target {
            Target::File { path, .. } | Target::Folder { path, .. } =>
                [&workspace, &vm_workspace].into_iter().flatten()
                    .any(|root| crate::platform_fs::path_within(path, root)),
            _ => false,
        };
        // A coding session makes the user review every change: each command is asked about even
        // where a rule allows it, and each write or edit waits with its diff.
        let switch_on = |name: &str| { let context = context.clone(); let name = name.to_owned(); async move {
            matches!(self.agents.permission(&context, &Target::Feature { name }, chrono::Utc::now().timestamp()).await.ok(),
                Some(PermissionDecision::Allow(_))) } };
        // Full access: the conversation's user chose to be asked nothing. What a rule blocks stays
        // blocked and what no rule reaches is refused, not asked; only the asking stops. With a
        // coding session on beside it, file changes alone still wait for their review.
        let full_access = switch_on("full_access").await;
        let reviews_changes = switch_on("coding_session").await;
        let coding = !full_access && reviews_changes;
        // A read-only command from a fixed list runs without asking, unless the conversation asks
        // about every command. The list is in `harmless.rs`; the coding session outranks it.
        let quiet_command = !unrestricted && !coding && matches!(operation, Operation::RunShell { ref text, .. } if crate::harmless::harmless(text))
            && !switch_on("ask_commands").await;
        // A plain line whose every program the agent (or the main agent) allows runs without asking too.
        let plain_programs = match &operation { Operation::RunShell { text, .. } => crate::harmless::programs(text), _ => None };
        let allowed_programs = match (&plain_programs, coding || unrestricted) {
            (Some(programs), false) => {
                let mut all = true;
                for name in programs {
                    let target = Target::Program { name: name.clone() };
                    if !matches!(self.agents.permission(&context, &target, chrono::Utc::now().timestamp()).await.ok(), Some(PermissionDecision::Allow(_))) { all = false; break; }
                }
                all
            }
            _ => false,
        };
        // A sandboxed command starts in its folder, so a folder the sandbox may not enter is refused
        // here with the folder to use, instead of asking and then failing at `cd`. On Windows the
        // sandboxed shell cannot start there at all; the Mac's sandbox can, so it keeps running there.
        if cfg!(windows) && !unrestricted && let Operation::RunShell { cwd, .. } = &operation {
            let grants = self.agents.file_grants(&context, chrono::Utc::now().timestamp()).await.ok()?;
            let within = |root: &str| crate::platform_fs::path_within(cwd, root);
            let granted = ([&workspace, &vm_workspace].into_iter().flatten().any(|root| within(root))
                || grants.iter().any(|grant| grant.folder && !grant.blocked && within(&grant.path)))
                && !grants.iter().any(|grant| grant.blocked && within(&grant.path));
            tracing::debug!(thread, granted, "run_shell folder check");
            if !granted {
                *why = Some(refusal::outside_folder(cwd, workspace.as_deref().or(vm_workspace.as_deref())));
                return None;
            }
        }
        // Network is a switch: a project or agent may turn it on, and the conversation may
        // turn it on or off whatever they say. Off is the default and is never asked about.
        let wants_network = targets.iter().any(|target| matches!(target, Target::Host { .. }));
        if wants_network {
            let switch = Target::Feature { name: "network".into() };
            let on = matches!(self.agents.permission(&context, &switch,
                chrono::Utc::now().timestamp()).await.ok()?, PermissionDecision::Allow(_));
            if !on {
                return Some(json!({ "success": false, "contentItems": [{ "type": "inputText",
                    "text": "Network is off for this conversation, so the command was not run. The user switches it from Tools, beside the message box, once this conversation's agent allows it in Settings, Agents. Without hosts the command runs with no network." }] }));
            }
        }
        // Hosts that still need an answer are decided in the command's own popup. With full
        // access a host the network switch lets through is reached without one.
        let mut pending_hosts = Vec::new();
        for target in &targets {
            if let Target::Host { host } = target
                && self.agents.permission(&context, target, chrono::Utc::now().timestamp())
                    .await.ok()? == PermissionDecision::Ask {
                // Under full access a host no rule names is refused, not asked.
                if full_access {
                    tracing::debug!(thread, host, "scoped tool refused: full access and no rule names the host");
                    *why = Some(refusal::full_access_host(host));
                    return None;
                }
                pending_hosts.push(host.clone());
            }
        }
        for target in &targets {
            let now = chrono::Utc::now().timestamp();
            if once.contains(target) { continue; }
            let decision = self.agents.permission(&context, target, now).await.ok()?;
            tracing::debug!(thread, target = refusal::kind(target), ?decision, inside = in_workspace(target), full_access, "scoped tool target");
            match decision {
                PermissionDecision::Allow(_) if coding && matches!(target, Target::Command { .. }) => {}
                PermissionDecision::Allow(_) => continue,
                PermissionDecision::Block => { *why = Some(refusal::blocked(target)); return None; }
                PermissionDecision::Unavailable => { *why = Some(refusal::UNSUPPORTED.into()); return None; }
                PermissionDecision::Ask if in_workspace(target) => continue,
                // Counted as approved once, so the check after the popups lets it through too. Under
                // full access every command runs; a path no rule names is refused, not asked.
                PermissionDecision::Ask if full_access && matches!(target, Target::Command { .. }) => { once.push(target.clone()); continue; }
                PermissionDecision::Ask if full_access => { *why = Some(refusal::full_access_path(target)); return None; }
                PermissionDecision::Ask if (quiet_command || allowed_programs) && matches!(target, Target::Command { .. }) => { once.push(target.clone()); continue; }
                PermissionDecision::Ask => {}
            }
            let (answer, decision) = oneshot::channel();
            let id = Uuid::new_v4();
            // An unattended run waits as long as its schedule says; a conversation with the
            // user in it waits a minute.
            let unattended = self.unattended.lock().ok().and_then(|runs| runs.get(thread).copied());
            let window = unattended.unwrap_or(Duration::from_secs(60));
            {
                let mut data = self.data.lock().await;
                let chat = data.chats.get_mut(thread)?;
                if chat.stopping || chat.active.as_deref() != Some(turn)
                    || chat.scoped_approval.is_some() {
                    return None;
                }
                let hosts = if matches!(target, Target::Command { .. }) { pending_hosts.clone() }
                    else { Vec::new() };
                // A plain shell line names its programs, so the popup can offer to allow them for good.
                let mut extra = match (target, &plain_programs) {
                    (Target::Command { kind, .. }, Some(programs)) if kind == "shell" => Some(json!({ "programs": programs })),
                    _ => None,
                };
                if unrestricted { extra.get_or_insert_with(|| json!({}))["unrestricted"] = json!(true); }
                chat.scoped_approval = Some(ScopedApproval {
                    id, turn: turn.into(), target: target.clone(), hosts, extra,
                    expires: Instant::now() + window, answer,
                });
                chat.emit("snapshot", chat.snapshot());
            }
            self.changed();
            let answered = tokio::time::timeout(window, decision).await.ok().and_then(Result::ok);
            // Nobody answered an unattended run: its owner terminates it instead of letting it
            // carry on or wait again.
            if answered.is_none() && unattended.is_some() && let Ok(mut expired) = self.unattended_expired.lock() {
                expired.insert(thread.to_owned(), serde_json::to_string(target).unwrap_or_default());
            }
            if answered.is_none() { *why = Some(refusal::unanswered(window)); }
            let choice = answered.unwrap_or(ScopedChoice::Reject);
            {
                let mut data = self.data.lock().await;
                if let Some(chat) = data.chats.get_mut(thread)
                    && chat.scoped_approval.as_ref().is_some_and(|a| a.id == id) {
                    chat.scoped_approval = None;
                    chat.emit("snapshot", chat.snapshot());
                }
            }
            self.changed();
            if self.permission_revision.load(Ordering::Acquire) != revision {
                *why = Some(refusal::CHANGED.into());
                return None;
            }
            // The answer to a command also answers for the hosts shown with it.
            let with_hosts: Vec<Target> = if matches!(target, Target::Command { .. }) {
                pending_hosts.iter().map(|host| Target::Host { host: host.clone() }).collect()
            } else { Vec::new() };
            match choice {
                ScopedChoice::Reject => { why.get_or_insert_with(|| refusal::DECLINED.into()); return None; }
                ScopedChoice::Once => {
                    once.push(target.clone());
                    once.extend(with_hosts);
                }
                // Allowing the programs is a lasting rule on the agent, or on the main agent for a plain
                // conversation; this command runs now, and the rule covers plain lines from then on.
                ScopedChoice::Programs => {
                    let _gate = self.agent_gate.read().await;
                    if self.permission_revision.load(Ordering::Acquire) != revision { return None; }
                    let scope = crate::permissions::Scope::Agent(context.agent().unwrap_or(crate::main_agent::ID));
                    for name in plain_programs.clone().unwrap_or_default() {
                        if self.agents.set_permission(scope, &Target::Program { name }, Some(crate::permissions::Effect::Allow)).await.is_err() { return None; }
                    }
                    self.permission_revision.fetch_add(1, Ordering::AcqRel);
                    revision = self.permission_revision.load(Ordering::Acquire);
                    once.push(target.clone());
                    once.extend(with_hosts);
                }
                ScopedChoice::Session | ScopedChoice::SessionDomains => {
                    let _gate = self.agent_gate.read().await;
                    if self.permission_revision.load(Ordering::Acquire) != revision {
                        return None;
                    }
                    let now = chrono::Utc::now().timestamp();
                    self.agents.approve_session(&context, target, now).await.ok()?;
                    for host in with_hosts {
                        // The wider choice stores the domain above the host where one is reasonable.
                        let granted = match (&choice, &host) {
                            (ScopedChoice::SessionDomains, Target::Host { host: name }) =>
                                crate::permissions::parent_domain(name)
                                    .map(|domain| Target::Domain { domain }).unwrap_or(host.clone()),
                            _ => host.clone(),
                        };
                        self.agents.approve_session(&context, &granted, now).await.ok()?;
                    }
                }
            }
        }
        // A settings writer cannot finish removal while this final dispatch is in flight.
        let mut edited_by_user = false;
        if reviews_changes && matches!(operation, Operation::WriteFile { .. } | Operation::ReplaceInFile { .. }) {
            match self.review_change(thread, turn, &operation, remote.as_ref(), &context, &workspace, &vm_workspace).await {
                Reviewed::Unchanged => {}
                Reviewed::Replaced(updated) => { operation = updated; edited_by_user = true; }
                Reviewed::Refused(reason) => return Some(json!({ "success": false,
                    "contentItems": [{ "type": "inputText", "text": reason }] })),
            }
        }
        let _gate = self.agent_gate.read().await;
        if self.agents.admit(thread.to_owned()).await.is_err()
            || self.agents.permission_context(thread.to_owned()).await.ok()? != context
            || self.permission_revision.load(Ordering::Acquire) != revision {
            *why = Some(refusal::CHANGED.into());
            return None;
        }
        {
            let data = self.data.lock().await;
            let chat = data.chats.get(thread)?;
            if chat.stopping || chat.active.as_deref() != Some(turn) { return None; }
        }
        for target in &targets {
            match self.agents.permission(&context, target,
                chrono::Utc::now().timestamp()).await.ok()? {
                PermissionDecision::Allow(_) => {}
                PermissionDecision::Ask if once.contains(target) || in_workspace(target) => {}
                _ => { *why = Some(refusal::CHANGED.into()); return None; }
            }
        }
        let is_command = matches!(operation, Operation::RunShell { .. }
            | Operation::RunApplescript { .. });
        // The profile is rebuilt per call, so a revoked grant fails the next command.
        let mut grants = self.agents.file_grants(&context,
            chrono::Utc::now().timestamp()).await.ok()?;
        for root in [&workspace, &vm_workspace].into_iter().flatten() {
            // Allows come before blocks in the profile, so blocks inside still hold.
            grants.insert(0, crate::seatbelt::Grant { path: root.clone(), write: true,
                folder: true, blocked: false });
        }
        // A shell command of a Mac conversation with a working folder runs in its shared terminal,
        // in the user's view; anything else, and any failure to start the shell, takes the one-shot path.
        let unrestricted_result = if let Some(grant) = shell_grant {
            drop(_gate);
            if let Operation::RunShell { text, cwd, .. } = &operation {
                Some(self.run_unrestricted(thread, turn, grant, text, cwd).await)
            } else { return None; }
        } else { None };
        let in_terminal = if unrestricted { None } else { match (&operation, &remote, &workspace, &vm_workspace) {
            (Operation::RunShell { text, cwd, hosts }, None, Some(_), _) => self.run_in_terminal(thread, params["callId"].as_str().unwrap_or("call"), text, cwd, hosts.clone(), &grants, None).await,
            (Operation::RunShell { text, cwd, hosts }, Some(worker), _, Some(_)) if self.machine_carries(&Assignment::Vm { vm_id: machine.clone() }, "terminal").await =>
                self.run_in_terminal(thread, params["callId"].as_str().unwrap_or("call"), text, cwd, hosts.clone(), &grants, Some(worker.clone())).await,
            _ => None,
        }
        };
        let executed = match unrestricted_result.or(in_terminal) {
            Some(executed) => executed,
            None => match remote {
                Some(worker) => worker.call("miniclaw/scoped/execute",
                    json!({ "operation": operation, "grants": grants })).await.ok()?,
                None => crate::executor::execute_host(operation, &grants).await,
            },
        };
        *report = Some(executed.clone());
        // An edit that changed nothing tells the model why, so it can correct its request.
        if executed["status"] == "failed" && let Some(reason) = executed["text"].as_str() {
            return Some(json!({ "success": false,
                "contentItems": [{ "type": "inputText", "text": reason }] }));
        }
        if executed["status"] != "ok" {
            // The executor's own words, such as a sandbox that could not start, when it has them.
            *why = Some(executed["text"].as_str().filter(|text| !text.is_empty()).map(str::to_owned)
                .unwrap_or_else(|| refusal::NOT_RUN.into()));
            return None;
        }
        let mut content = Vec::new();
        if is_command {
            let stdout = executed["text"].as_str()?;
            let stderr = executed["stderr"].as_str()?;
            let code = executed["exitCode"].as_i64()
                .map(|code| code.to_string()).unwrap_or_else(|| "signal".into());
            if stdout.len() + stderr.len() > 65_000 { return None; }
            // Naming a refused host lets the model ask for it instead of guessing.
            let refused: Vec<&str> = executed["refusedHosts"].as_array().map(|list|
                list.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            let network = if refused.is_empty() { String::new() } else {
                format!("\nRefused connections (not among the approved hosts, or not HTTPS on port 443): {}",
                    refused.join(", "))
            };
            content.push(json!({ "type": "inputText",
                "text": format!("Exit code: {code}\nstdout:\n{stdout}\nstderr:\n{stderr}{network}") }));
        } else if let Some(text) = executed["text"].as_str() {
            if text.len() > 65_536 { return None; }
            // The agent must not assume its own text is in the file.
            let text = if edited_by_user { format!("{text} The user changed your proposed content before applying it. Read the file again before you edit it further.") }
                else { text.to_owned() };
            content.push(json!({ "type": "inputText", "text": text }));
        }
        if let Some(image) = executed["image"].as_str() {
            if image.len() > 6_000_000 || !image.starts_with("data:image/png;base64,") {
                return None;
            }
            content.push(json!({ "type": "inputImage", "imageUrl": image }));
        }
        Some(json!({ "success": true, "contentItems": content }))
    }

    /// Runs one approved command in the conversation's shared shell and shapes the result like the
    /// one-shot executor's. `None` when the shell could not be used, so the caller falls back.
    async fn run_in_terminal(self: &Arc<Self>, thread: &str, call: &str, text: &str, cwd: &str, hosts: Vec<String>,
        grants: &[crate::seatbelt::Grant], remote: Option<Client>) -> Option<Value> {
        let (root, _, idle_hosts) = self.conversation_grants(thread).await?;
        let (cols, rows) = match self.terminals.get(thread) {
            Some(terminal) => terminal.size().await,
            None => (100, 30),
        };
        let spec = crate::terminal::Spec { cwd: root, grants: grants.to_vec(), hosts: idle_hosts.clone(), cols, rows };
        let terminal = match &remote {
            Some(client) => self.terminals.ensure_remote(thread, client.clone(), spec).await.ok()?,
            None => self.terminals.ensure(thread, spec).await.ok()?,
        };
        // In a VM the worker runs the command in its shell and answers whole.
        if let Some(client) = &remote {
            let answer = client.call("miniclaw/terminal/run", json!({ "thread": thread, "id": call, "text": text, "cwd": cwd, "hosts": hosts })).await.ok()?;
            if answer["error"] == "busy" { return Some(json!({ "status": "failed", "text": "The terminal is busy with what the user is running. Wait for it, or ask the user." })); }
            if answer.get("error").is_some() { return None; }
            let output = answer["output"].as_str().and_then(crate::frames::decode).unwrap_or_default();
            let mut text = String::from_utf8_lossy(&output).into_owned();
            let total = answer["total"].as_u64().unwrap_or(0);
            if answer["endedShell"] == true { text.push_str("\n[this command ended the shared shell; a fresh one starts at the next use]"); }
            if total > output.len() as u64 { text.push_str(&format!("\n[output truncated at 60 KiB ({total} bytes in all); the full output is in the terminal]")); }
            if answer["code"].is_null() { text.push_str("\n[still running in the terminal after 120 s; the user can watch it there and stop it with Ctrl-C]"); }
            let mut value = json!({ "status": "ok", "text": text, "stderr": "", "exitCode": answer["code"] });
            if answer["refused"].as_array().is_some_and(|list| !list.is_empty()) { value["refusedHosts"] = answer["refused"].clone(); }
            return Some(value);
        }
        let (backend, thread_owned, call_owned) = (self.clone(), thread.to_owned(), call.to_owned());
        let shaped = |ran: crate::terminal::Ran| {
            let mut text = String::from_utf8_lossy(&ran.output).into_owned();
            if ran.ended_shell { text.push_str("\n[this command ended the shared shell; a fresh one starts at the next use]"); }
            if ran.total > ran.output.len() as u64 {
                text.push_str(&format!("\n[output truncated at 60 KiB ({} bytes in all); the full output is in the terminal]", ran.total));
            }
            let mut value = json!({ "status": "ok", "text": text, "stderr": "", "exitCode": ran.code });
            if !ran.refused.is_empty() { value["refusedHosts"] = json!(ran.refused); }
            value
        };
        let late = shaped.clone();
        let ran = terminal.run(call, text, cwd, hosts, move |ran| {
            // The command outlived the model's wait: the user's Activity entry is finished now.
            let finished = late(ran);
            tokio::spawn(async move {
                let entry = backend.activity.finish_late(&thread_owned, &call_owned, &finished);
                if let Some(entry) = entry { backend.publish_activity(&thread_owned, &entry).await; }
            });
        }).await;
        // Whatever the shell's idle hosts are, they come back once the command is done.
        if let Some(shell) = terminal.shell.lock().await.as_ref() { shell.set_hosts(idle_hosts); }
        match ran {
            Ok(ran) => {
                let mut value = shaped(ran);
                if value["exitCode"].is_null() {
                    let text = value["text"].as_str().unwrap_or_default().to_owned();
                    value["text"] = json!(format!("{text}\n[still running in the terminal after 120 s; the user can watch it there and stop it with Ctrl-C]"));
                }
                Some(value)
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Some(json!({ "status": "failed",
                "text": "The terminal is busy with what the user is running. Wait for it, or ask the user." })),
            Err(_) => None,
        }
    }

    /// What a Mac conversation's shell runs under: its working folder, the grants its commands
    /// get, and the hosts its commands may reach today. None without a working folder.
    pub async fn conversation_grants(&self, thread: &str) -> Option<(String, Vec<crate::seatbelt::Grant>, Vec<String>)> {
        let context = self.agents.permission_context(thread.to_owned()).await.ok()?;
        // In a VM the shell starts in the VM's shared workspace, as its commands do.
        let in_vm = matches!(self.agents.assignment(thread.to_owned()).await.ok().flatten(), Some(Assignment::Vm { .. }));
        let workspace = if in_vm { self.cwd.clone() } else {
            match self.agents.project_context(thread.to_owned(), context.agent(), false).await.ok()?
                .and_then(|project| project.project.definition.workspace) {
                Some(workspace) => workspace,
                None if context.project().is_none() => self.own_folder(thread, context.agent().is_none())?,
                None => return None,
            }
        };
        let grants = if in_vm { self.grants_for(&context, &None, &Some(workspace.clone())).await? }
            else { self.grants_for(&context, &Some(workspace.clone()), &None).await? };
        let now = chrono::Utc::now().timestamp();
        // The user's typing reaches the network only where a rule allows it, and only with the switch on.
        let network = matches!(self.agents.permission(&context, &crate::permissions::Target::Feature { name: "network".into() }, now).await,
            Ok(crate::permissions::Decision::Allow(_)));
        let hosts = if network { self.agents.host_grants(&context, now).await.unwrap_or_default() } else { Vec::new() };
        Some((workspace, grants, hosts))
    }

    /// Builds the file grants a call runs under: the saved rules plus the conversation's own
    /// workspace folders.
    async fn grants_for(&self, context: &crate::permissions::Context, workspace: &Option<String>,
        vm_workspace: &Option<String>) -> Option<Vec<crate::seatbelt::Grant>> {
        let mut grants = self.agents.file_grants(context, chrono::Utc::now().timestamp()).await.ok()?;
        for root in [workspace, vm_workspace].into_iter().flatten() {
            grants.insert(0, crate::seatbelt::Grant { path: root.clone(), write: true, folder: true, blocked: false });
        }
        Some(grants)
    }

    /// Shows one proposed change to the user and waits for their decision, for at most 30
    /// minutes. Nothing is written here; the caller applies what comes back.
    #[allow(clippy::too_many_arguments)]
    async fn review_change(&self, thread: &str, turn: &str, operation: &Operation, remote: Option<&Client>,
        context: &crate::permissions::Context, workspace: &Option<String>, vm_workspace: &Option<String>) -> Reviewed {
        let (path, kind) = match operation {
            Operation::WriteFile { path, .. } => (path.clone(), "write"),
            Operation::ReplaceInFile { path, .. } => (path.clone(), "edit"),
            _ => return Reviewed::Unchanged,
        };
        // The user already chose to apply the rest of this turn.
        if self.data.lock().await.chats.get(thread).is_some_and(|chat| chat.review_apply_all.as_deref() == Some(turn)) {
            return Reviewed::Unchanged;
        }
        // The current content is read under the same grants the change would run under.
        let Some(grants) = self.grants_for(context, workspace, vm_workspace).await else { return Reviewed::Unchanged };
        let read = Operation::ReadFile { path: path.clone() };
        let current = match remote {
            Some(worker) => worker.call("miniclaw/scoped/execute", json!({ "operation": read, "grants": grants })).await.ok(),
            None => Some(crate::executor::execute_host(read, &grants).await),
        };
        // A file that cannot be read as text cannot be shown, and what cannot be shown is not
        // changed in a coding session. A path that names nothing yet is a new file, reviewed
        // against nothing. The folder's listing tells the two apart, on this Mac and in a VM.
        let readable = current.as_ref().filter(|value| value["status"] == "ok").and_then(|value| value["text"].as_str());
        let (kind, before) = match (readable, operation) {
            (Some(text), _) => (kind, text.to_owned()),
            (None, Operation::WriteFile { .. }) => {
                // A Windows change that cannot be placed or compared is refused, never written unseen.
                let windows = cfg!(windows) && remote.is_none();
                let unseen = || Reviewed::Refused("This file's folder could not be listed, so the change could not be shown for review. Nothing was written.".into());
                let Some((folder, name)) = folder_and_name(&path, windows) else { return unseen() };
                let listing = Operation::ListFolder { path: folder };
                let found = match remote {
                    Some(worker) => worker.call("miniclaw/scoped/execute", json!({ "operation": listing, "grants": grants })).await.ok(),
                    None => Some(crate::executor::execute_host(listing, &grants).await),
                };
                let entries = found.as_ref().filter(|value| value["status"] == "ok").and_then(|value| value["text"].as_str());
                match listed(entries, &name, windows) {
                    Listed::Absent => ("create", String::new()),
                    Listed::Present => return Reviewed::Refused("This file cannot be shown as text, so a coding session will not replace it. Nothing was written.".into()),
                    Listed::Unknown if windows => return unseen(),
                    Listed::Unknown => return Reviewed::Unchanged,
                }
            }
            (None, _) => return Reviewed::Unchanged,
        };
        let before = before.as_str();
        let proposed = match operation {
            Operation::WriteFile { text, .. } => text.clone(),
            // An edit that does not match exactly once changes nothing; the executor explains it.
            Operation::ReplaceInFile { find, text, .. } if before.matches(find.as_str()).count() == 1 =>
                before.replacen(find.as_str(), text, 1),
            _ => return Reviewed::Unchanged,
        };
        let (answer, decision) = oneshot::channel();
        let id = Uuid::new_v4();
        let window = Duration::from_secs(30 * 60);
        {
            let mut data = self.data.lock().await;
            let Some(chat) = data.chats.get_mut(thread) else { return Reviewed::Refused("The conversation is gone.".into()) };
            if chat.stopping || chat.active.as_deref() != Some(turn) || chat.review.is_some() {
                return Reviewed::Refused("Another change is still waiting for the user's review.".into());
            }
            chat.review = Some(Review { id, turn: turn.into(), path: path.clone(), kind,
                diff: crate::executor::diff(before, &proposed), proposed: proposed.clone(),
                expires: Instant::now() + window, answer });
            chat.emit("snapshot", chat.snapshot());
        }
        self.changed();
        let choice = tokio::time::timeout(window, decision).await;
        {
            let mut data = self.data.lock().await;
            if let Some(chat) = data.chats.get_mut(thread) && chat.review.as_ref().is_some_and(|review| review.id == id) {
                chat.review = None;
                chat.emit("snapshot", chat.snapshot());
            }
        }
        self.changed();
        match choice {
            Ok(Ok(ReviewChoice::Apply { text, rest_of_turn })) => {
                if rest_of_turn && let Some(chat) = self.data.lock().await.chats.get_mut(thread) {
                    chat.review_apply_all = Some(turn.to_owned());
                }
                match text.filter(|text| *text != proposed) {
                    Some(text) => Reviewed::Replaced(Operation::WriteFile { path, text }),
                    None => Reviewed::Unchanged,
                }
            }
            Ok(Ok(ReviewChoice::Reject { note })) => Reviewed::Refused(if note.trim().is_empty() {
                "The user reviewed this change and rejected it. Nothing was written.".into()
            } else { format!("The user reviewed this change and rejected it. Nothing was written. Their note: {}", note.trim()) }),
            Ok(Err(_)) => Reviewed::Refused("The review was closed. Nothing was written.".into()),
            Err(_) => {
                let owner = self.data.lock().await.owner();
                if let Some(owner) = owner { let _ = self.agents.review_expired(owner, thread.to_owned()).await; }
                Reviewed::Refused("Nobody reviewed this change within 30 minutes, so it was rejected. Nothing was written.".into())
            }
        }
    }

    /// Returns the Mac worker, including one attached after backend startup.
    /// Copies a VM chat's messages to the Mac off the async threads; a failure only
    /// means the chat shows no history while its VM is off.
    /// Records one finished turn for the Analytics tab: what it ran on and what it cost. It holds
    /// counts and names only, so it outlives the conversation without keeping anything private.
    pub fn record_turn(self: &Arc<Self>, id: &str, chat: &mut Chat, params: &Value) {
        let Some(turn) = params["turn"]["id"].as_str().map(str::to_owned) else { return };
        let count = |usage: &Value, key: &str| usage[key].as_i64().unwrap_or(0);
        let usage = chat.turn_tokens.take().filter(|(known, _)| *known == turn).map(|(_, usage)| usage).unwrap_or_else(|| json!({}));
        let summary = &chat.summary;
        let record = crate::analytics::Turn {
            thread: id.to_owned(), turn,
            provider: if summary["provider"] == "claude" { "claude".into() } else { "codex".into() },
            model: summary["model"].as_str().map(str::to_owned),
            effort: summary["effort"].as_str().map(str::to_owned),
            agent: summary["agentName"].as_str().map(str::to_owned),
            project: summary["projectName"].as_str().map(str::to_owned),
            status: params["turn"]["status"].as_str().unwrap_or("failed").to_owned(),
            seconds: chat.turn_started.take().map(|began| (chrono::Utc::now().timestamp_millis() - began).max(0) as f64 / 1000.0).unwrap_or(0.0),
            input: count(&usage, "inputTokens"), cached: count(&usage, "cachedInputTokens"),
            output: count(&usage, "outputTokens"), reasoning: count(&usage, "reasoningOutputTokens"),
            total: match count(&usage, "totalTokens") { 0 => count(&usage, "inputTokens") + count(&usage, "outputTokens"), known => known },
        };
        let backend = self.clone();
        tokio::spawn(async move { let _ = backend.agents.record_turn(record).await; });
    }

    pub fn keep_transcript(&self, id: &str, chat: &Chat) {
        let Some(dir) = self.transcripts.get().cloned() else { return };
        // A VM conversation is copied so it can be read with its VM off; a project conversation so
        // the project's later conversations can look it up.
        if chat.summary["environment"]["kind"] != "vm" && !chat.summary["projectId"].is_string() { return; }
        let (id, thread, messages) = (id.to_owned(), chat.summary.clone(), json!(chat.messages));
        tokio::task::spawn_blocking(move || {
            let _ = crate::transcripts::save(&dir, &id, &thread, &messages);
        });
    }

    pub fn host_worker(&self) -> Option<Client> {
        self.host.read().ok().and_then(|slot| slot.clone())
            .filter(|client| client.available())
            .or_else(|| self.host_client.clone().filter(|client| client.available()))
    }

    /// Attaches one newly authenticated Mac worker without changing Master.
    pub async fn attach_host(self: &Arc<Self>, client: Client) -> Result<(), Failure> {
        if !self.vm_routing || !client.available() { return Err(Failure::Rejected); }
        let mut slot = self.host.write().map_err(|_| Failure::Unavailable)?;
        if slot.as_ref().is_some_and(|old| old.available())
            || self.host_client.as_ref().is_some_and(|old| old.available())
        { return Err(Failure::Rejected); }
        *slot = Some(client.clone());
        drop(slot);
        let primary = Assignment::Vm { vm_id: "master".into() };
        Self::listen_client(self, client, Assignment::Host {}, primary);
        Ok(())
    }

    /// Returns the assigned Master client, including one attached after backend startup.
    pub fn primary_client(&self) -> Option<Client> {
        self.master.read().ok().and_then(|slot| slot.clone())
            .filter(|client| client.available())
            .or_else(|| self.client.clone().filter(|client| client.available()))
    }

    /// Reports whether a conversation's machine can carry one of MiniClaw's tools that the Mac
    /// answers. This Mac always can. A VM can when its worker names the tool in
    /// `miniclaw/worker/info`; a worker installed before that names none, so its conversations
    /// keep the file tool alone. The answer is kept until the VM's worker connects again.
    pub async fn machine_carries(&self, assignment: &Assignment, tool: &str) -> bool {
        let Assignment::Vm { vm_id } = assignment else { return true; };
        if let Some(known) = self.worker_tools.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(vm_id) {
            return known.iter().any(|name| name == tool);
        }
        let client = if vm_id == "master" { self.primary_client() } else { self.dedicated_client(vm_id) };
        let Some(client) = client else { return false; };
        // Tools and features are one list here: a name is a name, and the worker's lists do not overlap.
        let tools: Vec<String> = client.call("miniclaw/worker/info", json!({})).await.ok()
            .map(|info| ["tools", "features"].iter().flat_map(|key| info[key].as_array().cloned().unwrap_or_default())
                .filter_map(|name| name.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        let carries = tools.iter().any(|name| name == tool);
        self.worker_tools.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(vm_id.clone(), tools);
        carries
    }

    /// Marks a VM-capable backend as running on the Mac, keeping its local features.
    pub fn set_local(&self) { self.local.store(true, Ordering::Release); }

    /// Records the Mac's own helper for a backend whose main helper is the VM's.
    pub fn set_local_helper(&self, helper: String) {
        let _ = self.local_helper.set(computer::Target::Local(helper));
    }

    /// Returns the Computer Use helper for chats that run on this Mac.
    pub fn local_computer(&self) -> Option<&computer::Target> {
        if !self.is_local() { return None; }
        match &self.computer_helper {
            Some(target @ (computer::Target::Local(_) | computer::Target::Installed(_))) => Some(target),
            _ => self.local_helper.get(),
        }
    }

    /// Reports whether Mac-only features (Claude, Seatbelt tools) apply here.
    pub fn is_local(&self) -> bool { self.local.load(Ordering::Acquire) }

    /// Returns the workspace of the Mac's own Codex and Claude chats.
    pub fn local_cwd(&self) -> &str { self.host_cwd.as_deref().unwrap_or(&self.cwd) }

    /// Returns the Claude Code bridge while it is running.
    pub fn claude_client(&self) -> Option<Client> {
        self.claude.read().ok().and_then(|slot| slot.clone())
            .filter(|client| client.available())
    }

    /// Reports whether this task belongs to the Claude bridge instead of a Codex worker.
    pub fn is_claude(&self, thread: &str) -> bool {
        self.claude_threads.read().is_ok_and(|threads| threads.contains(thread))
    }

    /// Records or forgets one Claude task so later requests route to the bridge.
    pub fn track_claude(&self, thread: &str, present: bool) {
        if let Ok(mut threads) = self.claude_threads.write() {
            if present { threads.insert(thread.into()); } else { threads.remove(thread); }
        }
    }

    /// Attaches the host-only Claude bridge and indexes its saved tasks for routing.
    pub async fn attach_claude(self: &Arc<Self>, client: Client) -> Result<(), Failure> {
        if !self.is_local() || !client.available() { return Err(Failure::Rejected); }
        let mut cursor = None;
        for _ in 0..128 {
            let page = crate::history::list_for(&client, self.local_cwd(), cursor.as_deref(),
                crate::claude::PROVIDER).await?;
            for thread in page["threads"].as_array().ok_or(Failure::Unavailable)? {
                if let Some(id) = thread["id"].as_str() { self.track_claude(id, true); }
            }
            cursor = crate::history::cursor(&page["nextCursor"])?;
            if cursor.is_none() { break; }
        }
        *self.claude.write().map_err(|_| Failure::Unavailable)? = Some(client.clone());
        let primary = if self.vm_routing { Assignment::Vm { vm_id: "master".into() } }
            else { Assignment::Host {} };
        Self::listen_client(self, client, Assignment::Host {}, primary);
        Ok(())
    }

    /// Returns only the worker authenticated for this dedicated VM identity.
    pub fn dedicated_client(&self, vm_id: &str) -> Option<Client> {
        self.dedicated.read().ok().and_then(|workers| workers.get(vm_id).cloned())
            .filter(|client| client.available())
    }

    /// Forgets a deleted VM's worker link, so nothing routes to it again.
    pub fn detach_dedicated(&self, vm_id: &str) {
        if let Ok(mut workers) = self.dedicated.write() { workers.remove(vm_id); }
    }

    /// Lists connected dedicated workers for independent task pagination.
    pub fn dedicated_workers(&self) -> Vec<(String, Client)> {
        self.dedicated.read().ok().map(|workers| workers.iter()
            .filter(|(_, client)| client.available())
            .map(|(id, client)| (id.clone(), client.clone())).collect())
            .unwrap_or_default()
    }

    /// Attaches one dedicated worker without changing Host or Master routing.
    pub async fn attach_dedicated(self: &Arc<Self>, vm_id: String, client: Client)
        -> Result<(), Failure> {
        if !self.vm_routing || Uuid::parse_str(&vm_id).is_err() || !client.available() {
            return Err(Failure::Rejected);
        }
        {
            // The lock guard must end before the first await below.
            let mut workers = self.dedicated.write().map_err(|_| Failure::Unavailable)?;
            if workers.get(&vm_id).is_some_and(|old| old.available()) {
                return Err(Failure::Rejected);
            }
            workers.insert(vm_id.clone(), client.clone());
        }
        // A VM that connects again may have been updated, so its worker is asked anew.
        self.worker_tools.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&vm_id);
        // Each VM answers for its own helper; the answer is dropped when its worker goes.
        let ready = client.call("miniclaw/computer/read", json!({})).await
            .ok().is_some_and(|status| status["available"] == true);
        self.set_dedicated_computer_ready(&vm_id, ready);
        let (owner, watched, id) = (Arc::downgrade(self), client.clone(), vm_id.clone());
        tokio::spawn(async move {
            watched.closed().await;
            if let Some(backend) = owner.upgrade() { backend.set_dedicated_computer_ready(&id, false); }
        });
        Self::listen_client(self, client, Assignment::Vm { vm_id },
            Assignment::Vm { vm_id: "master".into() });
        Ok(())
    }

    /// Records whether one project VM has its Computer Use helper.
    fn set_dedicated_computer_ready(&self, vm_id: &str, ready: bool) {
        if let Ok(mut set) = self.dedicated_computer_ready.write() {
            if ready { set.insert(vm_id.to_owned()); } else { set.remove(vm_id); }
        }
        self.changed();
    }

    /// Reports whether the named VM is attached and has its own Computer Use helper.
    pub fn vm_computer_ready(&self, vm_id: &str) -> bool {
        if vm_id == "master" { return self.master_computer_ready.load(Ordering::Acquire); }
        self.dedicated_computer_ready.read().is_ok_and(|set| set.contains(vm_id))
    }

    /// Attaches one newly authenticated Master client after the prior client has stopped.
    pub async fn attach_master(self: &Arc<Self>, client: Client) -> Result<(), Failure> {
        if !self.vm_routing || !client.available() { return Err(Failure::Rejected); }
        let mut slot = self.master.write().map_err(|_| Failure::Unavailable)?;
        if slot.as_ref().is_some_and(|old| old.available())
            || self.client.as_ref().is_some_and(|old| old.available())
        { return Err(Failure::Rejected); }
        *slot = Some(client.clone());
        drop(slot);
        self.worker_tools.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove("master");
        let worker = Assignment::Vm { vm_id: "master".into() };
        Self::listen_client(self, client, worker.clone(), worker);
        Ok(())
    }

    /// Records the attached Master's own helper status without changing Mac access.
    pub fn set_master_computer_ready(&self, ready: bool) {
        self.master_computer_ready.store(ready, Ordering::Release);
        self.changed();
    }

    /// Creates shared backend state and starts approval/notification listeners
    /// when a Codex client is available. Returns the shared backend handle.
    pub fn new(client: Option<Client>, cwd: String, origin: String,
        recovery: PathBuf, agents: Store, computer_helper: Option<computer::Target>,
        vm_routing: bool) -> Arc<Self> {
        Self::build(client, None, cwd, None, origin, recovery, agents,
            computer_helper, vm_routing)
    }

    /// Creates a controller with separate Master and Mac workers and workspaces.
    pub fn new_with_host(vm: Option<Client>, host: Option<Client>, vm_cwd: String,
        host_cwd: String, origin: String, recovery: PathBuf, agents: Store,
        computer_helper: Option<computer::Target>) -> Arc<Self> {
        Self::build(vm, host, vm_cwd, Some(host_cwd), origin, recovery, agents,
            computer_helper, true)
    }

    /// Builds shared state and subscribes to each available worker's events.
    fn build(client: Option<Client>, host_client: Option<Client>, cwd: String,
        host_cwd: Option<String>, origin: String, recovery: PathBuf, agents: Store,
        computer_helper: Option<computer::Target>, vm_routing: bool) -> Arc<Self> {
        let (changes, _) = watch::channel(0);
        let master_computer_ready = vm_routing && client.is_some()
            && matches!(computer_helper, Some(computer::Target::Guest));
        let backend = Arc::new(Self {
            search: Default::default(),
            client,
            master: SyncRwLock::new(None),
            dedicated: SyncRwLock::new(HashMap::new()),
            host_client,
            host: SyncRwLock::new(None),
            claude: SyncRwLock::new(None),
            claude_threads: SyncRwLock::new(HashSet::new()),
            providers: OnceLock::new(),
            transcripts: OnceLock::new(),
            appearance: OnceLock::new(),
            idle: crate::idle::Idle::default(),
            transfers: Default::default(),
            selections: Default::default(),
            picker: OnceLock::new(),
            backups: OnceLock::new(),
            activity: Default::default(),
            chat_folders: Default::default(),
            attachments: Default::default(),
            delegate: OnceLock::new(),
            ask: OnceLock::new(),
            mcp: Default::default(),
            schedule_tick: OnceLock::new(),
            turn_ended: OnceLock::new(),
            terminals: Default::default(),
            worker_tools: Default::default(),
            project_seen: Default::default(),
            unattended: Default::default(),
            unattended_expired: Default::default(),
            delegation_prompts: Mutex::new(HashMap::new()),
            local_helper: OnceLock::new(),
            cwd,
            host_cwd,
            origin,
            changes,
            streams: Arc::new(Semaphore::new(64)),
            data: Mutex::new(Data::default()),
            auth_gate: Mutex::new(()),
            login_gate: Mutex::new(()),
            create_gate: Mutex::new(()),
            load_gate: Mutex::new(()),
            computer_helper,
            master_computer_ready: AtomicBool::new(master_computer_ready),
            dedicated_computer_ready: SyncRwLock::new(HashSet::new()),
            vm_routing,
            local: AtomicBool::new(!vm_routing),
            recovery,
            agents,
            agent_gate: RwLock::new(()),
            vm_start_approvals: Mutex::new(HashMap::new()),
            permission_revision: AtomicU64::new(0),
            shell_grants: Mutex::new(HashMap::new()),
        });
        let primary = if vm_routing { Assignment::Vm { vm_id: "master".into() } }
            else { Assignment::Host {} };
        for (client, worker) in [
            (backend.client.as_ref(), primary.clone()),
            (backend.host_client.as_ref(), Assignment::Host {}),
        ].into_iter().filter_map(|(client, worker)| client.map(|client| (client, worker)))
        {
            Self::listen_client(&backend, client.clone(), worker, primary.clone());
        }
        backend
    }

    /// Subscribes one authenticated worker without observing another worker's tasks.
    pub(crate) fn listen_client(backend: &Arc<Self>, client: Client, worker: Assignment,
        primary: Assignment) {
            if let Some(mut tools) = client.take_tool_requests() {
                let owner = Arc::downgrade(backend);
                let tool_worker = worker.clone();
                let tool_primary = primary.clone();
                tokio::spawn(async move {
                    while let Some(request) = tools.recv().await {
                        let Some(backend) = owner.upgrade() else { break; };
                        let worker = tool_worker.clone();
                        let primary = tool_primary.clone();
                        tokio::spawn(async move {
                            // Delegation needs the chat routes, so the API layer handles it.
                            if request.params["tool"] == "miniclaw_delegate" {
                                match backend.delegate.get() {
                                    Some(handle) => handle(backend.clone(), request).await,
                                    None => { let _ = request.answer.send(json!({ "success": false, "contentItems": [] })); }
                                }
                                return;
                            }
                            // A question for the user waits for their answer and may end the turn.
                            if request.params["tool"] == crate::ask::TOOL {
                                match backend.ask.get() {
                                    Some(handle) => handle(backend.clone(), request).await,
                                    None => { let _ = request.answer.send(json!({ "success": false, "contentItems": [] })); }
                                }
                                return;
                            }
                            // A project's shared memory is MiniClaw's own store.
                            if request.params["tool"] == crate::project_tool::TOOL {
                                crate::project_tool::handle(backend.clone(), request).await;
                                return;
                            }
                            // MCP servers are reached through their own gateway, with its own approvals.
                            if request.params["tool"] == crate::mcp_gateway::TOOL {
                                crate::mcp_gateway::handle(backend.clone(), request).await;
                                return;
                            }
                            backend.scoped_tool(worker, primary, request).await;
                        });
                    }
                });
            }
            if let Some(mut requests) = client.take_requests() {
                let owner = Arc::downgrade(backend);
                let request_worker = worker.clone();
                let request_primary = primary.clone();
                tokio::spawn(async move {
                    while let Some(request) = requests.recv().await {
                        let Some(backend) = owner.upgrade() else {
                            break;
                        };
                        let Some(approval) = Approval::new(request) else {
                            continue;
                        };
                        let params = &approval.request.params;
                        let Some(id) = params["threadId"].as_str() else {
                            continue;
                        };
                        let mut data = backend.data.lock().await;
                        let Some(chat) = data.chats.get_mut(id) else {
                            continue;
                        };
                        if !worker_owns(chat, &request_worker, &request_primary)
                            || chat.summary["computerUse"] != true
                            || chat.computer_changing || chat.computer_error.is_some()
                            || chat.image_changing || chat.image_error.is_some()
                            || chat.stopping
                            || chat.approval.is_some()
                            || chat.active.is_none()
                            || chat.active.as_deref() != params["turnId"].as_str()
                        {
                            continue;
                        }
                        chat.approval = Some(approval);
                        chat.emit("snapshot", chat.snapshot());
                        drop(data);
                        backend.changed();
                    }
                });
            }
            let mut notifications = client.subscribe();
            let completed = client.track_events();
            let mut connection = client.completion();
            // The listener must not own the client, or dropping the backend leaks its child.
            let mut lag = client.lag();
            let notice_session = Uuid::new_v4().to_string();
            let primary = primary.clone();
            // Waiting for notifications must not keep the backend alive by itself.
            let owner = Arc::downgrade(backend);
            tokio::spawn(async move {
                let _completed = completed;
                loop {
                    let notification = tokio::select! {
                        biased;
                        event = notifications.recv() => event,
                        frame = connection.closed() => Ok(frame),
                    };
                    let Some(backend) = owner.upgrade() else {
                        break;
                    };
                    let envelope = match notification {
                        Ok(frame) => frame,
                        Err(_) => {
                            lag.lost().await;
                            connection.closed().await
                        }
                    };
                    let original = envelope.value();
                    let mut data = backend.data.lock().await;
                    if let Some(chat) = original["params"]["threadId"].as_str()
                        .and_then(|id| data.chats.get(id))
                        && !worker_owns(chat, &worker, &primary)
                    { continue; }
                    if matches!(original["method"].as_str(), Some("item/started" | "item/completed"))
                        && original["params"]["item"]["type"] == "imageGeneration"
                        && (!matches!(worker, Assignment::Host {})
                            || original["params"]["threadId"].as_str().is_none_or(|id|
                                !data.chats.contains_key(id) && !data.loading.contains_key(id))) {
                        continue;
                    }
                    let cleaned = clean_image_notification(&backend, &original);
                    let frame = cleaned.as_ref().unwrap_or(&original);
                    let method = frame["method"].as_str().unwrap_or_default();
                    let params = &frame["params"];
                    if let Some(owner) = data.owner() {
                        let failure = if method == "miniclaw/disconnected" && params["expected"] != true {
                            Some(backend.agents.runtime_failure(owner, notice_session.clone()).await)
                        } else if method == "turn/completed" && params["turn"]["status"] == "failed" {
                            match (params["threadId"].as_str(), params["turn"]["id"].as_str()) {
                                (Some(id), Some(turn)) if data.chats.get(id).is_some_and(|chat|
                                    !chat.finished.iter().any(|done| done == turn)
                                    && (chat.active.as_deref() == Some(turn)
                                        || (chat.sending && chat.active.is_none()))) => {
                                    Some(backend.agents.response_failure(owner, id.into(), turn.into()).await)
                                }
                                _ => None,
                            }
                        } else { None };
                        if failure.is_some_and(|result| result.is_err()) { data.notice_error = true; }
                    }
                    if method == "account/login/completed" {
                        if let Some(id) = params["loginId"].as_str() {
                            let success = params["success"] == true;
                            if data.login_results.len() >= 16 {
                                data.login_results.clear();
                            }
                            data.login_results.insert(id.into(), success);
                            if data.login_id.as_deref() == Some(id) {
                                data.login["status"] =
                                    json!(if success { "succeeded" } else { "failed" });
                            }
                        }
                    } else if method == "account/updated"
                        && (worker == Assignment::Host {}
                            || backend.host_worker().is_none())
                    {
                        // Invalidate immediately; the next read confirms the new identity.
                        data.generation += 1;
                        data.account = Value::Null;
                        backend.forget(&mut data);
                        data.usage.clear();
                    } else if method == "miniclaw/disconnected" {
                        if backend.host_worker().is_none() {
                            data.generation += 1;
                            data.account = Value::Null;
                        }
                        for chat in data.chats.values_mut() {
                            if worker_owns(chat, &worker, &primary) {
                                chat.approval = None;
                                chat.scoped_approval = None;
                                chat.uncertain |= chat.active.is_some() || chat.sending;
                            }
                        }
                    } else if let Some(loading) = params["threadId"].as_str()
                        .and_then(|id| data.loading.get_mut(id))
                    {
                        loading.push(&frame);
                    } else if let Some(id) = params["threadId"].as_str()
                        && let Some(chat) = data.chats.get_mut(id)
                    {
                        // A turn of this conversation ends here if one was running before the event.
                        let ending = method == "turn/completed" && (chat.active.is_some() || chat.sending);
                        let stopping = chat.stopping;
                        chat.notify(method, params);
                        if method == "turn/completed" {
                            backend.keep_transcript(id, chat);
                            backend.record_turn(id, chat, params);
                        }
                        if ending && chat.active.is_none() && let Some(ended) = backend.turn_ended.get() {
                            ended(backend.clone(), id.to_owned(), params["turn"]["status"].as_str().unwrap_or("failed").to_owned(), stopping);
                        }
                    } else if method == "turn/started"
                        && let (Some(id), Some(turn)) =
                            (params["threadId"].as_str(), params["turn"]["id"].as_str())
                    {
                        backend.cancel(id, turn);
                    }
                    drop(data);
                    backend.changed();
                    if method == "miniclaw/disconnected" {
                        break;
                    }
                }
            });
    }

    /// Wakes subscribers so they can read updated backend state.
    pub fn changed(&self) {
        self.changes.send_modify(|version| *version += 1);
    }

    /// Drops cached chats and pending history loads, requesting cancellation of active
    /// turns. Codex's saved transcripts remain untouched.
    pub fn forget(&self, data: &mut Data) {
        data.notice_error = false;
        // Receipts belong to the account that sent the messages.
        data.kept_receipts.clear();
        data.loading.clear();
        // Claude chats belong to a separate sign-in and stay cached.
        let codex: Vec<String> = data.chats.iter()
            .filter(|(_, chat)| chat.summary["provider"] != "claude")
            .map(|(id, _)| id.clone()).collect();
        for id in codex {
            if let Some(chat) = data.chats.remove(&id) && let Some(turn) = &chat.active {
                self.cancel(&id, turn);
            }
        }
    }

    /// Requests turn cancellation in the background; stops the owned client if it fails.
    pub fn cancel(&self, id: &str, turn: &str) {
        let agents = self.agents.clone();
        let vm_routing = self.vm_routing;
        let primary = self.primary_client();
        let host = self.host_worker();
        let dedicated = self.dedicated.read().ok().map(|workers| workers.clone())
            .unwrap_or_default();
        let claude = self.is_claude(id).then(|| self.claude_client());
        let id = id.to_owned();
        let params = json!({ "threadId": id, "turnId": turn });
        // Cleanup must continue even if the HTTP request is cancelled.
        tokio::spawn(async move {
            let client = if let Some(claude) = claude { claude } else { match agents.assignment(id).await {
                Ok(Some(Assignment::Host {})) if vm_routing => host,
                Ok(Some(Assignment::Host {})) if !vm_routing => primary,
                Ok(Some(Assignment::Vm { vm_id })) if vm_routing && vm_id == "master" => primary,
                Ok(Some(Assignment::Vm { vm_id })) if vm_routing =>
                    dedicated.get(&vm_id).cloned().filter(|client| client.available()),
                Ok(None) if !vm_routing => primary,
                _ => None,
            } };
            let Some(client) = client else { return; };
            if client.call("turn/interrupt", params).await.is_err() {
                client.stop().await;
            }
        });
    }
}

/// Removes an image's base64 before replay or history buffering and keeps its bytes privately.
fn clean_image_notification(backend: &Backend, frame: &Value) -> Option<Value> {
    if !matches!(frame["method"].as_str(), Some("item/started" | "item/completed"))
        || frame["params"]["item"]["type"] != "imageGeneration" { return None; }
    let mut clean = frame.clone();
    let item = &mut clean["params"]["item"];
    if item["status"] == "completed"
        && let (Some(thread), Some(id), Some(result)) = (
            frame["params"]["threadId"].as_str(), item["id"].as_str(), item["result"].as_str())
        && let Ok(record) = backend.attachments.generated(thread, id, result) {
            item["attachment"] = record.view();
    }
    if item["status"] == "completed" && !item["attachment"].is_object() {
        item["status"] = json!("failed");
        item["storageFailure"] = json!(true);
    }
    item["result"] = json!("");
    item["savedPath"] = Value::Null;
    Some(clean)
}

/// Why a file or command call was refused, in words for the model and the activity row. Each
/// names what the user can change; none repeats file contents, a command's output or secrets.
mod refusal {
    use crate::permissions::Target;
    use std::time::Duration;

    pub const PLAIN: &str = "MiniClaw refused this, and nothing was run: in a conversation with no project and no agent, the file and command tool cannot act on this computer. Tell the user once that they can start a new conversation in a project (its folder becomes the working folder) or with an agent that has folder permissions in Settings, Agents; or, if their main agent allows it, grant timed Unsandboxed shell access from Tools beside the message box.";
    pub const TURN_OVER: &str = "MiniClaw refused this: the turn that asked for it has ended or is being stopped.";
    pub const UNSUPPORTED: &str = "MiniClaw refused this: the operation is not available in this conversation.";
    pub const CHANGED: &str = "MiniClaw refused this: the conversation's permissions changed while it waited, so nothing was run. Ask again if it is still needed.";
    pub const DECLINED: &str = "The user declined this request, so nothing was run.";
    pub const NOT_RUN: &str = "MiniClaw could not run this operation.";

    /// Names a target the way the user reads it.
    fn named(target: &Target) -> String {
        match target {
            Target::File { path, action } | Target::Folder { path, action } =>
                format!("{} {path}", if action == "write" { "writing" } else { "reading" }),
            Target::Command { .. } => "the command".into(),
            Target::Host { host } => format!("the host {host}"),
            _ => "this request".into(),
        }
    }

    /// A target's kind, for the debug log, which never holds a command's text.
    pub fn kind(target: &Target) -> &'static str {
        match target {
            Target::File { .. } => "file", Target::Folder { .. } => "folder",
            Target::Command { .. } => "command", Target::Host { .. } => "host",
            _ => "other",
        }
    }

    pub fn blocked(target: &Target) -> String {
        format!("MiniClaw refused this: a rule in this conversation blocks {}, so nothing was done.", named(target))
    }

    pub fn full_access_path(target: &Target) -> String {
        format!("MiniClaw refused this, and nothing was done: Full access is on, and under Full access a path outside the working folder that no rule covers is refused instead of asked ({}). The user can give this conversation's agent that folder in Settings, Agents, work in a project whose folder contains it, or turn Full access off in Tools to be asked.", named(target))
    }

    pub fn full_access_host(host: &str) -> String {
        format!("MiniClaw refused this, and nothing was run: Full access is on, and under Full access a host that no rule allows is refused instead of asked ({host}). The user can allow it for this conversation's agent in Settings, Agents, or turn Full access off in Tools to be asked.")
    }

    pub fn outside_folder(cwd: &str, working: Option<&str>) -> String {
        match working {
            Some(working) => format!("MiniClaw refused this, and nothing was run: the command's cwd {cwd} is outside the folders this conversation may use, so the sandbox could not start there. Use the conversation's working folder {working} as cwd (or leave cwd out), or a folder inside it."),
            None => format!("MiniClaw refused this, and nothing was run: the command's cwd {cwd} is outside the folders this conversation may use, so the sandbox could not start there. Use a folder this conversation's agent was given in Settings, Agents."),
        }
    }

    pub fn unanswered(window: Duration) -> String {
        format!("Nobody answered the approval within {} seconds, so nothing was run.", window.as_secs())
    }
}

#[cfg(test)]
mod bounds {
    use super::*;

    #[test]
    fn a_long_conversation_keeps_its_newest_ten_thousand_messages() {
        let mut chat = Chat::new(json!({ "id": "t" }));
        chat.messages = (0..10_050).map(|index| json!({ "id": index.to_string(), "role": "user", "text": "x" })).collect();
        chat.bound_messages();
        assert_eq!(chat.messages.len(), 10_000);
        assert_eq!(chat.messages[0]["id"], "50");
        assert_eq!(chat.messages.last().unwrap()["id"], "10049");
    }

    #[test]
    fn only_an_unanswered_receipt_holds_a_cache_slot() {
        let mut chat = Chat::new(json!({ "id": "t" }));
        chat.history_loaded = true;
        let receipt = |turn: Option<&str>| Receipt { text: "hi".into(),
            choice: crate::models::Choice { model: None, effort: None, speed: None }, turn: turn.map(str::to_owned), attachments: Vec::new() };
        chat.receipts.insert("a".into(), receipt(Some("turn-1")));
        assert!(!chat.evictable(), "its turn is not known to have finished");
        chat.finished.push_back("turn-1".into());
        assert!(chat.evictable());
        chat.receipts.insert("b".into(), receipt(None));
        assert!(!chat.evictable(), "a send whose turn is unknown is still protected");
    }
}

#[cfg(test)]
mod turn_names {
    use super::*;

    /// Without a message id, only the newest message is named, and only when it is the user's and
    /// names no other turn: an assistant reply or another turn's message keeps what it has.
    #[test]
    fn a_turn_without_a_message_id_names_only_an_unnamed_user_message() {
        let mut chat = Chat::new(json!({ "id": "t" }));
        chat.messages = vec![json!({ "id": "u1", "role": "user", "text": "First" }),
            json!({ "id": "a1", "role": "assistant", "text": "Answer", "turnId": null })];
        assert_eq!(chat.name_turn(None, "turn-2"), None);
        assert!(chat.messages[0]["turnId"].is_null() && chat.messages[1]["turnId"].is_null());

        chat.messages.push(json!({ "id": "u2", "role": "user", "text": "Second", "turnId": "turn-1" }));
        assert_eq!(chat.name_turn(None, "turn-2"), None);
        assert_eq!(chat.messages[2]["turnId"], "turn-1");
        // The same turn named again is harmless.
        assert_eq!(chat.name_turn(None, "turn-1").as_deref(), Some("u2"));

        chat.messages.push(json!({ "id": "u3", "role": "user", "text": "Third" }));
        assert_eq!(chat.name_turn(None, "turn-3").as_deref(), Some("u3"));
        assert_eq!(chat.messages[3]["turnId"], "turn-3");
        // A named message is never renamed, by id or as the newest.
        assert_eq!(chat.name_turn(Some("u3"), "turn-4"), None);
        assert_eq!(chat.name_turn(None, "turn-4"), None);
        assert_eq!(chat.messages[3]["turnId"], "turn-3");
        // By id, an assistant message is refused and an unknown id names nothing.
        assert_eq!(chat.name_turn(Some("a1"), "turn-4"), None);
        assert_eq!(chat.name_turn(Some("missing"), "turn-4"), None);
        assert_eq!(chat.name_turn(Some("u1"), "turn-0").as_deref(), Some("u1"));
    }
}

#[cfg(test)]
mod review_paths {
    use super::*;

    #[test]
    fn a_mac_path_splits_as_before() {
        assert_eq!(folder_and_name("/Users/a/proj/new.txt", false), Some(("/Users/a/proj".into(), "new.txt".into())));
        assert_eq!(folder_and_name("/new.txt", false), Some(("".into(), "new.txt".into())));
        assert_eq!(folder_and_name("new.txt", false), Some(("/".into(), "new.txt".into())));
        // A backslash is an ordinary character of a Mac file name.
        assert_eq!(folder_and_name("/a/b\\c.txt", false), Some(("/a".into(), "b\\c.txt".into())));
    }

    #[test]
    fn a_windows_path_splits_at_its_last_separator() {
        assert_eq!(folder_and_name("C:\\proj\\new.txt", true), Some(("C:\\proj".into(), "new.txt".into())));
        assert_eq!(folder_and_name("C:\\proj/sub\\r\u{e9}sum\u{e9}.txt", true), Some(("C:\\proj/sub".into(), "r\u{e9}sum\u{e9}.txt".into())));
        assert_eq!(folder_and_name("C:/proj/new.txt", true), Some(("C:/proj".into(), "new.txt".into())));
        // A file in a drive's root is listed from the root, not from the drive's current folder.
        assert_eq!(folder_and_name("C:\\new.txt", true), Some(("C:\\".into(), "new.txt".into())));
        assert_eq!(folder_and_name("\\\\?\\C:\\new.txt", true), Some(("\\\\?\\C:\\".into(), "new.txt".into())));
    }

    #[test]
    fn a_windows_path_without_a_folder_or_a_name_is_not_split() {
        for path in ["new.txt", "C:new.txt", "C:\\proj\\", "C:\\", "", "\\new.txt"] {
            assert_eq!(folder_and_name(path, true), None, "{path}");
        }
    }

    #[test]
    fn a_listing_tells_a_new_file_from_one_that_is_not_text() {
        let entries = "Logo.PNG\tfile\t12\nsrc\tfolder\nR\u{c9}SUM\u{c9}.pdf\tfile\t9";
        assert_eq!(listed(Some(entries), "new.txt", false), Listed::Absent);
        assert_eq!(listed(Some(entries), "Logo.PNG", false), Listed::Present);
        // A Mac folder may hold both spellings; only the exact one is this file.
        assert_eq!(listed(Some(entries), "logo.png", false), Listed::Absent);
        assert_eq!(listed(None, "new.txt", false), Listed::Unknown);
        // Windows names one file by every spelling of its case.
        assert_eq!(listed(Some(entries), "logo.png", true), Listed::Present);
        assert_eq!(listed(Some(entries), "r\u{e9}sum\u{e9}.PDF", true), Listed::Present);
        assert_eq!(listed(Some(entries), "new.txt", true), Listed::Absent);
        assert_eq!(listed(None, "new.txt", true), Listed::Unknown);
    }
}

/// Says why a turn failed. A usage limit gets a short reason with the retry time the provider gave;
/// another reason is shown in the provider's own words only when it is one plain sentence that names
/// no account, address or identifier. Otherwise, or without a reason, the plain notice stays.
fn failure_reason(provider: &str, error: Option<&Value>) -> String {
    let notice = format!("{provider} could not complete this response.");
    let Some(error) = error else { return notice };
    let message = error["message"].as_str().unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ");
    if error["codexErrorInfo"] == "usageLimitExceeded" {
        // "... or try again at Oct 11th, 2026 12:25 PM." as the App Server words it.
        // ASCII lower-casing keeps byte offsets, so the match indexes the message itself.
        let lower = message.to_ascii_lowercase();
        let when = lower.rfind("try again ").map(|start| message[start + "try again ".len()..].trim_end_matches('.').trim())
            .filter(|when| (when.starts_with("at ") || when.starts_with("in ")) && when.len() <= 60
                && when.chars().all(|c| c.is_ascii_alphanumeric() || " ,:-/".contains(c)));
        return match when {
            Some(when) => format!("{provider} usage limit reached. Try again {when}."),
            None => format!("{provider} usage limit reached."),
        };
    }
    let lower = message.to_lowercase();
    let mut run = 0;
    let identifier = message.chars().any(|c| {
        run = if c.is_ascii_alphanumeric() || c == '_' || c == '-' { run + 1 } else { 0 };
        run >= 24
    });
    let private = message.contains(['@', '?', '=', '\\']) || identifier
        || ["bearer", "sk-", "token", "api key", "apikey", "secret", "password", "cookie"].iter().any(|word| lower.contains(word));
    if message.is_empty() || message.len() > 240 || private || !message.chars().any(char::is_alphabetic) { return notice; }
    format!("{} {message}", notice.trim_end_matches('.').to_owned() + ":")
}

#[cfg(test)]
mod failure_reason_tests {
    use super::*;

    /// Letters whose lower case is longer never shift the retry time out of the message.
    #[test]
    fn usage_limit_retry_time_survives_non_ascii_text() {
        let error = json!({ "message": "İİİ limit. Try again at Oct 11th, 2026 12:25 PM.", "codexErrorInfo": "usageLimitExceeded" });
        assert_eq!(failure_reason("Codex", Some(&error)), "Codex usage limit reached. Try again at Oct 11th, 2026 12:25 PM.");
        let odd = json!({ "message": "Try again at <script>", "codexErrorInfo": "usageLimitExceeded" });
        assert_eq!(failure_reason("Codex", Some(&odd)), "Codex usage limit reached.");
        assert_eq!(failure_reason("Claude", None), "Claude could not complete this response.");
        assert_eq!(failure_reason("Codex", Some(&json!({ "message": "x".repeat(300) }))), "Codex could not complete this response.");
    }
}
