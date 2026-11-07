//! Swarms: one shared goal worked on by a lead and teams of development, code review and testing
//! agents. Each swarm is a MiniClaw project, and its work is the project's issues (`issues.rs`):
//!
//! - the lead files the goal as features and bugs in the Backlog;
//! - development agents claim issues in Backlog or In Dev; testing agents claim issues in QA; code
//!   review agents claim issues in Agent review. Idle agents keep looking for an issue they can take;
//! - finished development moves an issue to QA, a passing test to Agent review, an approving review
//!   finishes it (closed, or waiting for the user's review by severity). A failing test or a review
//!   that asks for changes sends it back to the Backlog with the reason as the handover;
//! - agents never talk to each other, only to the issues, so a dropped link costs nothing but that
//!   agent's voice. Each claim holds a lease renewed by heartbeats; when the link is down the agent
//!   keeps working and queues its result, the lease runs out, MiniClaw releases the issue for
//!   another agent, and when the link comes back the queue is replayed. A result counts only while
//!   its agent still holds the claim it was made under, so a late result never moves an issue twice.
//!
//! Every agent is a real MiniClaw conversation in the project (Codex or Claude, by its model), with
//! its role's skills and Full access, so no command waits on a popup. The swarm's agents, leases and
//! history live in memory and are saved to `<data>/swarms/<id>.json`; the issues are in the database.

// Reuses chat creation, sending, naming and interruption exactly as the page does.
use super::*;
// The project's issues, who acts on them, and how a plan is filed.
use crate::issues::{Actor, Draft, Record};
// Holds the swarms and stops them.
use std::sync::{Mutex as StdMutex, OnceLock, atomic::{AtomicBool, Ordering}};

/// How often an agent's heartbeat renews its lease.
const HEARTBEAT_MS: i64 = 5_000;
/// Three missed heartbeats make an agent unreachable.
const UNREACHABLE_MS: i64 = 15_000;
/// How long a claim holds without a heartbeat.
const LEASE_MS: i64 = 40_000;
/// The longest one agent turn may take.
const TURN_LIMIT: Duration = Duration::from_secs(15 * 60);
/// How often one issue may come back from testing or review before it goes to the user.
const MAX_ROUNDS: u32 = 2;
/// How often an agent's turn may fail on one issue before it goes to the user.
const MAX_ATTEMPTS: u32 = 3;
/// How long an agent waits after a failed turn before it runs the same issue again.
const RETRY_DELAY_SECS: u64 = 5;

/// Milliseconds since the epoch, the clock of the leases.
fn now() -> i64 { chrono::Utc::now().timestamp_millis() }

/// What an agent does in the swarm.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Role { Lead, Dev, Review, Test }

impl Role {
    /// Whether an agent of this role may take an issue in `status`.
    fn takes(self, status: &str) -> bool {
        match self {
            Role::Dev => status == "backlog",
            Role::Test => status == "qa",
            Role::Review => status == "agent_review",
            Role::Lead => false,
        }
    }
}

/// A claim the swarm holds on one issue for one agent.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Lease { owner: String, until: i64, version: u64 }

/// A result an agent could not deliver while its link was down.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Queued { issue: i64, version: u64, ok: bool, text: String }

/// One agent of the swarm.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Member {
    name: String,
    role: Role,
    model: String,
    /// The simulated link between this agent and the issues.
    link_up: bool,
    last_seen: i64,
    status: String,
    /// Its last turn failed, and it is waiting to run the same issue again or running it again.
    retrying: bool,
    issue: Option<i64>,
    thread: Option<String>,
    queued: Vec<Queued>,
    done: u32,
    joined: i64,
    unreachable_ms: i64,
    /// When MiniClaw will try to reconnect this agent's cut link by itself.
    reconnect_at: Option<i64>,
}

/// One line of the swarm's history.
#[derive(Clone, Serialize)]
struct Event { at: i64, kind: &'static str, text: String }

/// Everything that changes, behind one lock that is never held across an await.
struct Ledger {
    status: &'static str,
    members: Vec<Member>,
    events: Vec<Event>,
    leases: std::collections::BTreeMap<i64, Lease>,
    /// How often each issue came back from testing or review.
    rounds: std::collections::BTreeMap<i64, u32>,
    /// How often an agent's turn failed on each issue.
    attempts: std::collections::BTreeMap<i64, u32>,
    next_version: u64,
    finished: Option<i64>,
}

/// One swarm and its fixed facts.
pub(super) struct Swarm {
    id: String,
    goal: String,
    workspace: String,
    /// The user started it with unsandboxed shell on: agents' commands run as the user, outside the
    /// sandbox, with the user's files and network, for as long as the swarm runs.
    unsandboxed: bool,
    /// The MiniClaw project this swarm is: its folder is the project's workspace, its issues the work.
    project: Uuid,
    /// The skills each role's conversations get.
    skills: Vec<(Role, Uuid)>,
    created: i64,
    state: StdMutex<Ledger>,
    stopped: AtomicBool,
}

/// Every swarm of this backend run.
fn swarms() -> &'static StdMutex<Vec<Arc<Swarm>>> {
    static ALL: OnceLock<StdMutex<Vec<Arc<Swarm>>>> = OnceLock::new();
    ALL.get_or_init(|| StdMutex::new(Vec::new()))
}

/// Every conversation a swarm agent worked in, with its swarm and agent.
type Threads = std::collections::HashMap<String, (Arc<Swarm>, String)>;
fn threads() -> &'static StdMutex<Threads> {
    static ALL: OnceLock<StdMutex<Threads>> = OnceLock::new();
    ALL.get_or_init(|| StdMutex::new(Threads::new()))
}

/// Says whether a conversation belongs to a swarm agent of this backend run.
pub(super) fn hides(thread: &str) -> bool {
    threads().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).contains_key(thread)
}

/// The issue actor of a swarm agent's conversation: its agent, under an id no saved agent has. The
/// issue tool uses it, so an agent's own comments and new issues carry its name.
pub fn actor(thread: &str) -> Option<Actor> {
    threads().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(thread).map(|(swarm, name)| swarm.actor(name))
}

/// Recognises a swarm agent's conversation from an earlier backend run by its opening words.
pub(super) fn looks_like_swarm(summary: &Value) -> bool {
    ["name", "preview"].iter().filter_map(|key| summary[*key].as_str()).any(|text| {
        let text = text.trim_start();
        text.starts_with("You lead a swarm of ") || (text.starts_with("You are ") && text.contains(", one agent in a swarm."))
    })
}

/// Finds a running swarm of this backend by id.
fn find(id: &str) -> Option<Arc<Swarm>> {
    swarms().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).iter().find(|swarm| swarm.id == id).cloned()
}

/// Shortens text for a title, a handover or an event.
fn clip(text: &str, most: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= most { return text.to_owned(); }
    format!("{}…", text.chars().take(most).collect::<String>())
}

/// The verdict word a reply starts with, with leading Markdown (`**FAIL:**`) skipped.
fn verdict(text: &str) -> String {
    text.trim_start().trim_start_matches(|c: char| !c.is_ascii_alphanumeric()).chars().take(8).collect::<String>().to_ascii_uppercase()
}

/// The first line of a reply after its verdict word, for a handover.
fn reason(text: &str) -> String {
    let line = text.trim_start().trim_start_matches(|c: char| !c.is_ascii_alphanumeric()).lines().next().unwrap_or("");
    let line = line.trim_start_matches(|c: char| c.is_ascii_alphabetic()).trim_start_matches(|c: char| c == ':' || c == '*').trim();
    clip(if line.is_empty() { text } else { line }, 300)
}

impl Ledger {
    /// Adds a line to the history.
    fn event(&mut self, kind: &'static str, text: String) {
        self.events.push(Event { at: now(), kind, text });
        if self.events.len() > 400 { self.events.remove(0); }
    }

    /// The agent named `name`.
    fn member(&mut self, name: &str) -> Option<&mut Member> { self.members.iter_mut().find(|member| member.name == name) }

    /// A heartbeat that reached the issues: the agent is seen and its leases renewed.
    fn beat(&mut self, name: &str) {
        let Some(member) = self.member(name) else { return };
        if !member.link_up { return; }
        member.last_seen = now();
        for lease in self.leases.values_mut().filter(|lease| lease.owner == name) { lease.until = now() + LEASE_MS; }
    }

    /// Records a new claim and returns the version a result must name.
    fn lease(&mut self, issue: i64, owner: &str) -> u64 {
        self.next_version += 1;
        self.leases.insert(issue, Lease { owner: owner.to_owned(), until: now() + LEASE_MS, version: self.next_version });
        self.next_version
    }

    /// Takes the claim if `owner` still holds it at `version`; a late result finds nothing.
    fn take(&mut self, issue: i64, owner: &str, version: u64) -> bool {
        let held = self.leases.get(&issue).is_some_and(|lease| lease.owner == owner && lease.version == version);
        if held { self.leases.remove(&issue); }
        held
    }

    /// Counts a failed turn on `issue` and answers the next attempt's number if `owner` may run it
    /// again: it still holds the claim at `version`, and the issue has failed fewer than
    /// MAX_ATTEMPTS times.
    fn failed(&mut self, issue: i64, owner: &str, version: u64) -> Option<u32> {
        let count = self.attempts.entry(issue).or_default();
        *count += 1;
        let count = *count;
        let held = self.leases.get(&issue).is_some_and(|lease| lease.owner == owner && lease.version == version);
        (held && count < MAX_ATTEMPTS).then_some(count + 1)
    }

    /// Removes every lease that ran out and returns them, and counts unreachable time.
    fn expired(&mut self, elapsed: i64) -> Vec<(i64, String)> {
        let time = now();
        for member in self.members.iter_mut().filter(|member| member.role != Role::Lead) {
            if !member.link_up || time - member.last_seen > UNREACHABLE_MS { member.unreachable_ms += elapsed; }
        }
        let gone: Vec<(i64, String)> = self.leases.iter().filter(|(_, lease)| lease.until < time)
            .map(|(issue, lease)| (*issue, lease.owner.clone())).collect();
        for (issue, _) in &gone { self.leases.remove(issue); }
        gone
    }
}

impl Swarm {
    /// Runs `change` on the state.
    fn with<T>(&self, change: impl FnOnce(&mut Ledger) -> T) -> T {
        change(&mut self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    /// The issue actor of one agent; ids are unique to the swarm, so no saved agent shares one.
    fn actor(&self, name: &str) -> Actor { Actor { id: format!("swarm-{}-{name}", &self.id[..6]), name: name.to_owned() } }

    /// The roles this swarm has agents for.
    fn has(&self, role: Role) -> bool { self.with(|state| state.members.iter().any(|member| member.role == role)) }

    /// The swarm as the page and other tools read it, with its issues counted.
    async fn snapshot(&self, backend: &Backend) -> Value {
        let issues = backend.agents.issues(self.project, None, None).await.unwrap_or_default();
        let done = issues.iter().filter(|issue| !crate::issues::active(&issue.status)).count();
        let time = now();
        self.with(|state| {
            let members: Vec<Value> = state.members.iter().map(|member| {
                let reachable = member.role == Role::Lead || (member.link_up && time - member.last_seen <= UNREACHABLE_MS);
                let status = if !member.link_up && let Some(issue) = member.issue {
                    format!("Link down · still working on #{issue} offline")
                } else if !member.link_up && !member.queued.is_empty() {
                    format!("Link down · {} result{} queued", member.queued.len(), if member.queued.len() == 1 { "" } else { "s" })
                } else if !member.link_up { "Link down · waiting to reconnect".into() } else { member.status.clone() };
                let status = match member.reconnect_at.filter(|_| !member.link_up) {
                    Some(at) => format!("{status} · reconnecting in {} s", ((at - time).max(0) + 999) / 1000),
                    None => status,
                };
                let lived = (state.finished.unwrap_or(time) - member.joined).max(1);
                json!({ "name": member.name, "role": member.role, "model": member.model, "linkUp": member.link_up,
                    "reachable": reachable, "lastSeen": member.last_seen, "status": status, "retrying": member.retrying, "issue": member.issue,
                    "thread": member.thread, "queued": member.queued.len(), "done": member.done,
                    "uptime": ((lived - member.unreachable_ms.min(lived)) as f64 / lived as f64 * 1000.0).round() / 10.0 })
            }).collect();
            json!({ "id": self.id, "goal": self.goal, "workspace": self.workspace, "created": self.created, "unsandboxed": self.unsandboxed,
                "projectId": self.project, "finished": state.finished, "status": state.status, "members": members,
                "leadThread": state.members.iter().find(|member| member.role == Role::Lead).and_then(|member| member.thread.clone()),
                "leases": state.leases, "events": state.events, "counts": { "done": done, "total": issues.len() } })
        })
    }
}

/// Opens one agent conversation in the swarm's project, titled `title`, sends `prompt` and waits for
/// its answer, renewing `name`'s leases while it works.
async fn turn(backend: &Shared, swarm: &Arc<Swarm>, name: &str, title: &str, model: &str, prompt: &str) -> std::result::Result<String, String> {
    let words = |error: Error| error.2.to_owned();
    let mut capabilities = std::collections::BTreeMap::new();
    capabilities.insert("full_access".to_owned(), true);
    // No agent may stop the swarm to wait on a question popup; the Lead talks to the user in its chat.
    capabilities.insert("questions".to_owned(), false);
    let role = swarm.with(|state| state.member(name).map(|member| member.role)).unwrap_or(Role::Dev);
    let skills = swarm.skills.iter().filter(|(owner, _)| *owner == role).map(|(_, id)| *id).collect::<Vec<_>>();
    let skill_selection = (!skills.is_empty()).then(|| skill_groups::Selection { skill_ids: skills, skill_group_ids: Vec::new(), revision: 0 });
    let created = create_chat(backend.clone(), Create { capabilities, skill_selection, project_id: Some(swarm.project), agent_id: None,
        model: Some(model.to_owned()), effort: None, speed: None, computer_use: false,
        // An ordinary project conversation, so it gets the project tool with the issue actions.
        environment: Some(Assignment::Host {}.choice()), delegated: false }).await.map_err(words)?;
    let thread = created.1.0["thread"]["id"].as_str().ok_or("The agent's conversation could not be created.")?.to_owned();
    threads().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(thread.clone(), (swarm.clone(), name.to_owned()));
    swarm.with(|state| if let Some(member) = state.member(name) { member.thread = Some(thread.clone()); });
    // Titled by its agent, so the swarm's list reads "Dev-1 · #2"; the agent's name is the
    // conversation's agent wherever turns are counted or shown (the Agents page, the running box).
    let _ = name_chat(State(backend.clone()), Path(thread.clone()), Ok(Json(Named { name: clip(title, 80) }))).await;
    if let Some(chat) = backend.data.lock().await.chats.get_mut(&thread) { chat.summary["agentName"] = json!(name); }
    if swarm.unsandboxed {
        let context = backend.agents.permission_context(thread.clone()).await.map_err(|_| "The agent's permissions could not be read.")?;
        backend.shell_grants.lock().await.insert(thread.clone(), crate::shell_access::Grant { id: Uuid::new_v4(), context,
            expires_at: i64::MAX, swarm: true });
    }
    let outcome = answer(backend, swarm, name, &thread, prompt).await;
    backend.shell_grants.lock().await.remove(&thread);
    outcome
}

/// Sends the prompt in the agent's conversation and waits for the answer, renewing the leases.
async fn answer(backend: &Shared, swarm: &Arc<Swarm>, name: &str, thread: &str, prompt: &str) -> std::result::Result<String, String> {
    let words = |error: Error| error.2.to_owned();
    let thread = thread.to_owned();
    let sent = send(State(backend.clone()), Path(thread.clone()), Ok(Json(Send { attachments: Vec::new(), text: prompt.to_owned(),
        client_message_id: Uuid::new_v4(), model: None, effort: None, speed: None }))).await.map_err(words)?;
    let turn = sent.1.0["turnId"].as_str().ok_or("The agent's turn did not start.")?.to_owned();
    let deadline = tokio::time::Instant::now() + TURN_LIMIT;
    let mut beat = tokio::time::Instant::now();
    loop {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if beat.elapsed() >= Duration::from_millis(HEARTBEAT_MS as u64) {
            beat = tokio::time::Instant::now();
            swarm.with(|state| state.beat(name));
        }
        let ended = swarm.stopped.load(Ordering::SeqCst).then_some("The swarm was stopped.")
            .or((tokio::time::Instant::now() >= deadline).then_some("The agent ran out of time."));
        if let Some(reason) = ended {
            let _ = interrupt(State(backend.clone()), Path(thread.clone()), Ok(Json(Interrupt { turn_id: turn.clone() }))).await;
            return Err(reason.into());
        }
        let data = backend.data.lock().await;
        let chat = data.chats.get(&thread).ok_or("The agent's conversation was lost.")?;
        if !chat.finished.contains(&turn) { continue; }
        let status = chat.events.iter().rev().filter(|event| event.name == "turn.completed")
            .filter_map(|event| serde_json::from_str::<Value>(&event.data).ok())
            .find(|event| event["turnId"] == turn.as_str())
            .and_then(|event| event["status"].as_str().map(str::to_owned)).unwrap_or_else(|| "unknown".into());
        let answer = chat.messages.iter().rev().find(|message| message["role"] == "assistant" && message["turnId"] == turn.as_str())
            .and_then(|message| message["text"].as_str()).map(str::to_owned).filter(|text| !text.trim().is_empty());
        return match (status.as_str(), answer) {
            ("completed", Some(answer)) => Ok(answer),
            ("completed", None) => Err("the agent ended without an answer".into()),
            (status, _) => Err(format!("the provider ended the turn as {status}")),
        };
    }
}

/// The project's standing instructions: who is who in the swarm, and each role's rules. Every agent
/// conversation reads them, so each agent's own message only has to carry its story.
fn rules(goal: &str, workspace: &str) -> String {
    format!("This project is a swarm working towards one goal: {goal}\n\n\
Every conversation in it belongs to one agent, named in its title: the Lead, development agents (Dev-1, Dev-2...), code review agents (Review-1...) and testing agents (Test-1...). Agents never talk to each other; they work through this project's issues (the stories) in one shared folder, {workspace}. Use that folder as the cwd for every command. Others work there at the same time, so change only what your story needs.\n\n\
Stories always start in the Backlog. Developers take them from the Backlog; testing and code review follow. MiniClaw gives each agent its story, moves it on and finishes it, so agents never call issue_claim, issue_stage, issue_done or issue_release. Anyone may file a separate problem with issue_create and add a note with issue_comment.\n\n\
- The Lead talks with the user and never changes files. Every story action is an action of the miniclaw_project tool; the Task tracking switch has nothing to do with stories. It plans by filing stories with issue_create (each a feature or bug, with a severity), answers questions about the swarm with issue_list and issue_read, gives a story to a named agent with issue_assign, and changes what agents may close without the user with issue_review_level, the last two only when the user asks. It answers in plain words, briefly.\n\
- A development agent builds or fixes its story, then replies in two or three sentences: what changed and which files.\n\
- A testing agent writes and runs tests for its story. Its reply's first line starts with PASS: or FAIL: and one sentence.\n\
- A code review agent reads the files its story touched without changing them. Its reply's first line starts with APPROVE: or CHANGES: and one sentence, then at most five specific fixes with file names.")
}

/// What the lead is asked to plan.
fn plan_prompt(swarm: &Swarm, devs: usize, reviews: usize, tests: usize) -> String {
    let _ = &swarm.workspace;
    format!("You are the Lead of this swarm: {devs} development, {reviews} code review and {tests} testing agents.\n\nGoal: {goal}\n\nAdd 2 to 8 stories to the Backlog: call your miniclaw_project tool with action issue_create once per story (the Task tracking switch is something else and does not matter here). Stories are features to build and bugs to fix, each with a severity and enough text that an agent who sees only that story can do it, naming the files it owns. Every story is tested and reviewed by itself, so do not add testing or review stories. Then tell me in a few lines what you added.",
        goal = swarm.goal)
}

/// What an agent is sent for one story: the story, what earlier agents left on it, and its role.
fn issue_prompt(swarm: &Swarm, name: &str, role: Role, issue: &Record) -> String {
    let notes: Vec<String> = issue.events.iter().flatten().filter(|event| matches!(event.kind.as_str(), "handover" | "comment") && !event.text.is_empty())
        .map(|event| format!("- {}: {}", event.author, clip(&event.text, 600))).collect();
    let recent = notes[notes.len().saturating_sub(4)..].join("\n");
    let history = if recent.is_empty() { String::new() } else { format!("\n\nNotes from earlier agents:\n{recent}") };
    let shell = if swarm.unsandboxed { " run_shell runs outside the sandbox as the user; if the file actions are unavailable, change files with run_shell." } else { "" };
    let ask = match role {
        Role::Test => "Test it, and start your reply with PASS: or FAIL:.",
        Role::Review => "Review the change without editing files, and start your reply with APPROVE: or CHANGES:.",
        _ => "Build it, then say in two or three sentences what you changed.",
    };
    let role = match role { Role::Test => "testing agent", Role::Review => "code reviewer", _ => "development agent" };
    format!("Story #{number} ({kind}, {severity}): {title}\n\n{text}{history}\n\nYou are {name}, a {role}. {ask}{shell}",
        number = issue.number, kind = issue.kind, severity = issue.severity, title = issue.title, text = clip(&issue.text, 3000))
}

/// Reads the lead's plan as drafts; a reply without one becomes one feature for the whole goal.
fn read_plan(answer: &str, goal: &str) -> Vec<Draft> {
    let parsed = answer.find('{').zip(answer.rfind('}')).filter(|(start, end)| start < end)
        .and_then(|(start, end)| serde_json::from_str::<Value>(&answer[start..=end]).ok());
    let mut plan: Vec<Draft> = parsed.as_ref().and_then(|value| value["issues"].as_array().or(value["tasks"].as_array())).map(|issues| issues.iter().take(8)
        .filter_map(|issue| {
            let title = clip(issue["title"].as_str()?.lines().next()?, 110);
            let kind = if issue["kind"] == "bug" { "bug" } else { "feature" };
            let severity = match issue["severity"].as_str() { Some(level @ ("low" | "high")) => level, _ => "medium" };
            let text = issue["text"].as_str().or(issue["detail"].as_str()).unwrap_or("").trim();
            (!title.is_empty()).then(|| Draft { kind: kind.into(), title: title.clone(), severity: severity.into(), tags: Vec::new(),
                text: clip(if text.is_empty() { &title } else { text }, 6000) })
        }).collect()).unwrap_or_default();
    if plan.is_empty() {
        plan.push(Draft { kind: "feature".into(), title: clip(goal.lines().next().unwrap_or(goal), 110), text: clip(goal, 6000),
            severity: "medium".into(), tags: Vec::new() });
    }
    plan
}

/// Plans the swarm, starts its agents, and runs the sweeper until every issue is closed or waiting
/// for the user.
async fn run(backend: Shared, swarm: Arc<Swarm>, lead_model: String, team: Vec<(String, Role, String)>) {
    let count = |role| team.iter().filter(|member| member.1 == role).count();
    let prompt = plan_prompt(&swarm, count(Role::Dev), count(Role::Review), count(Role::Test));
    // The Lead's conversation lasts: the user talks to it there for the rest of the swarm.
    let answer = turn(&backend, &swarm, "Lead", "Lead", &lead_model, &prompt).await;
    let mut filed = backend.agents.issues(swarm.project, None, None).await.map(|issues| issues.len()).unwrap_or(0);
    if filed == 0 {
        // A Lead that answered without filing anything still leaves one story for the whole goal.
        let reason = answer.as_ref().err().cloned().unwrap_or_else(|| "it filed no stories".into());
        swarm.with(|state| state.event("warn", format!("The Lead could not plan ({reason}). The whole goal is one story.")));
        for draft in read_plan("", &swarm.goal) {
            if backend.agents.create_issue(swarm.project, draft, "Lead".into()).await.is_ok() { filed += 1; }
        }
    }
    swarm.with(|state| {
        state.status = "running";
        if let Some(lead) = state.member("Lead") { lead.status = "Ready to talk".into(); }
        state.event("info", format!("The Lead filed {filed} stories in the Backlog."));
    });
    supervise(backend, swarm, team).await;
}

/// Starts the agents and watches the swarm until it is stopped: releases leases that ran out, saves
/// its record, and moves it between running and idle as stories close and arrive.
async fn supervise(backend: Shared, swarm: Arc<Swarm>, team: Vec<(String, Role, String)>) {
    for (name, role, model) in team {
        tokio::spawn(worker(backend.clone(), swarm.clone(), name, role, model));
    }
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut written = String::new();
    loop {
        tick.tick().await;
        // Leases that ran out are released for the next agent, in the name of the agent that held them.
        for (issue, owner) in swarm.with(|state| state.expired(1000)) {
            let handover = format!("{owner}'s lease ran out while it was unreachable. MiniClaw released #{issue} for another agent.");
            let _ = backend.agents.release_issue(swarm.project, issue, swarm.actor(&owner), handover.clone()).await;
            swarm.with(|state| state.event("warn", handover));
        }
        let snapshot = swarm.snapshot(&backend).await;
        let key = format!("{}:{}:{}", snapshot["status"], snapshot["events"].as_array().map_or(0, Vec::len), snapshot["counts"]["done"]);
        if key != written { save(&snapshot); written = key; }
        let issues = backend.agents.issues(swarm.project, None, None).await.unwrap_or_default();
        let over = swarm.with(|state| {
            if swarm.stopped.load(Ordering::SeqCst) {
                state.status = "stopped";
                state.finished.get_or_insert(now());
                return true;
            }
            let quiet = state.members.iter().all(|member| member.issue.is_none() && member.queued.is_empty());
            let settled = !issues.is_empty() && issues.iter().all(|issue| !crate::issues::active(&issue.status));
            if quiet && settled && state.status == "running" {
                let waiting = issues.iter().filter(|issue| issue.status == "review").count();
                state.status = "idle";
                state.event("done", if waiting == 0 { format!("All {} stories are closed. The agents wait for new ones: tell the Lead what to do next.", issues.len()) }
                    else { format!("The agents are done: {} stories closed, {waiting} waiting for your review. Tell the Lead what to do next.", issues.len() - waiting) });
            } else if state.status == "idle" && !settled {
                state.status = "running";
                state.event("info", "New stories arrived. The agents are back at work.".into());
            }
            false
        });
        if over { save(&swarm.snapshot(&backend).await); return; }
    }
}

/// One development, testing or review agent: takes an issue its role may work on, works it,
/// reports, and queues what it cannot report while its link is down.
async fn worker(backend: Shared, swarm: Arc<Swarm>, name: String, role: Role, model: String) {
    let actor = swarm.actor(&name);
    loop {
        if swarm.stopped.load(Ordering::SeqCst) {
            swarm.with(|state| if let Some(member) = state.member(&name) { member.status = "Stopped".into(); member.issue = None; member.retrying = false; });
            return;
        }
        let up = swarm.with(|state| state.member(&name).is_some_and(|member| member.link_up));
        if !up { tokio::time::sleep(Duration::from_secs(2)).await; continue; }
        swarm.with(|state| state.beat(&name));
        let Some((issue, version)) = claim(&backend, &swarm, &actor, role).await else {
            swarm.with(|state| if let Some(member) = state.member(&name) && member.issue.is_none() { member.status = "Looking for an issue".into(); });
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        };
        swarm.with(|state| {
            if let Some(member) = state.member(&name) { member.issue = Some(issue.number); member.status = format!("Working on #{} {}", issue.number, issue.title); }
            state.event("info", format!("{name} took #{} {}.", issue.number, issue.title));
        });
        let prompt = issue_prompt(&swarm, &name, role, &issue);
        // Short enough for the sidebar: the agent and its issue; the issue's title is on the Board.
        let title = format!("{name} · #{}", issue.number);
        let mut outcome = turn(&backend, &swarm, &name, &title, &model, &prompt).await;
        // A failed turn waits a few seconds and runs again, by the same agent on the same issue, while
        // the agent still holds the claim and the issue has failures left. Its tag says so meanwhile.
        while let Err(reason) = &outcome {
            if swarm.stopped.load(Ordering::SeqCst) { break; }
            let Some(attempt) = swarm.with(|state| state.failed(issue.number, &name, version)) else { break };
            swarm.with(|state| state.event("warn", format!("{name}'s turn on #{} failed: {}. It retries in {RETRY_DELAY_SECS} s (attempt {attempt} of {MAX_ATTEMPTS}).",
                issue.number, clip(reason, 160))));
            for left in (1..=RETRY_DELAY_SECS).rev() {
                if swarm.stopped.load(Ordering::SeqCst) { break; }
                swarm.with(|state| {
                    // Waiting still counts as holding the issue: the heartbeat keeps the lease.
                    state.beat(&name);
                    if let Some(member) = state.member(&name) {
                        member.retrying = true;
                        member.status = format!("Failed · retrying #{} in {left} s (attempt {attempt} of {MAX_ATTEMPTS})", issue.number);
                    }
                });
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            if swarm.stopped.load(Ordering::SeqCst) { break; }
            swarm.with(|state| if let Some(member) = state.member(&name) {
                member.status = format!("Retrying #{} {} (attempt {attempt} of {MAX_ATTEMPTS})", issue.number, issue.title);
            });
            outcome = turn(&backend, &swarm, &name, &format!("{title} · try {attempt}"), &model, &prompt).await;
        }
        let (ok, text) = match outcome { Ok(text) => (true, text), Err(reason) => (false, reason) };
        let up = swarm.with(|state| {
            let up = state.member(&name).is_some_and(|member| member.link_up);
            if let Some(member) = state.member(&name) {
                member.issue = None;
                member.retrying = false;
                if !up { member.queued.push(Queued { issue: issue.number, version, ok, text: text.clone() }); }
            }
            if !up { state.event("warn", format!("{name} finished #{} while its link was down. The result is queued until it reconnects.", issue.number)); }
            up
        });
        if up { deliver(&backend, &swarm, &name, role, issue.number, version, ok, &text).await; }
    }
}

/// Claims the most severe issue this role may take that nobody owns; returns it with its history and
/// the version its result must name.
async fn claim(backend: &Backend, swarm: &Swarm, actor: &Actor, role: Role) -> Option<(Record, u64)> {
    let mine = |issue: &Record| issue.owner_id.as_deref() == Some(actor.id.as_str());
    let leased = swarm.with(|state| state.leases.keys().copied().collect::<Vec<_>>());
    let mut open: Vec<Record> = backend.agents.issues(swarm.project, None, None).await.ok()?.into_iter()
        .filter(|issue| role.takes(&issue.status) && !leased.contains(&issue.number) && (issue.owner_id.is_none() || mine(issue))).collect();
    open.sort_by_key(|issue| (!mine(issue), std::cmp::Reverse(crate::issues::rank(&issue.severity)), issue.number));
    for candidate in open {
        if backend.agents.claim_issue(swarm.project, candidate.number, actor.clone()).await.is_err() { continue; }
        // Taking a story out of the Backlog is what moves it to In Dev, a given one included.
        if role == Role::Dev { let _ = backend.agents.set_issue_stage(swarm.project, candidate.number, actor.clone(), "dev".into()).await; }
        let version = swarm.with(|state| state.lease(candidate.number, &actor.name));
        let issue = backend.agents.issue(swarm.project, candidate.number).await.ok().flatten().unwrap_or(candidate);
        return Some((issue, version));
    }
    None
}

/// Applies a result if its agent still holds the claim it was made under, and moves the issue on:
/// development to QA, a pass to Agent review, an approval to done; a failing test or requested
/// changes back to the Backlog, until the issue has come back too often and goes to the user.
#[allow(clippy::too_many_arguments)]
async fn deliver(backend: &Backend, swarm: &Swarm, name: &str, role: Role, number: i64, version: u64, ok: bool, text: &str) {
    if !swarm.with(|state| state.take(number, name, version)) {
        swarm.with(|state| state.event("ok", format!("{name}'s late result for #{number} was not applied: the issue had moved on without it. Nothing was done twice.")));
        return;
    }
    let (project, actor) = (swarm.project, swarm.actor(name));
    let store = &backend.agents;
    let summary = clip(text, 4000);
    // The agent's turn failed and its own retries ran out (the worker counted each failure): the
    // issue goes back for another agent, or to the user once it has failed MAX_ATTEMPTS times.
    if !ok {
        let attempts = swarm.with(|state| state.attempts.get(&number).copied().unwrap_or(0));
        if attempts >= MAX_ATTEMPTS {
            hand_to_user(backend, swarm, &actor, number, &format!("{name} could not finish #{number} {attempts} times: {}", clip(text, 300))).await;
        } else {
            let _ = store.release_issue(project, number, actor, format!("{name} could not finish: {}", clip(text, 300))).await;
            swarm.with(|state| state.event("warn", format!("{name} could not finish #{number}: {}. It is open again.", clip(text, 160))));
        }
        return;
    }
    swarm.with(|state| if let Some(member) = state.member(name) { member.done += 1; });
    let word = verdict(text);
    let sent_back = (role == Role::Test && word.starts_with("FAIL")) || (role == Role::Review && word.starts_with("CHANGES"));
    if sent_back {
        let rounds = swarm.with(|state| { let count = state.rounds.entry(number).or_default(); *count += 1; *count });
        let why = reason(text);
        if rounds > MAX_ROUNDS {
            hand_to_user(backend, swarm, &actor, number, &format!("#{number} came back {rounds} times. Last: {why}")).await;
            return;
        }
        let _ = store.comment_issue(project, number, name.to_owned(), summary).await;
        let _ = store.set_issue_stage(project, number, actor.clone(), "dev".into()).await;
        let _ = store.release_issue(project, number, actor, format!("{}: {why}", if role == Role::Test { "Tests failed" } else { "Review asks for changes" })).await;
        swarm.with(|state| state.event("warn", format!("{name} sent #{number} back to development: {}", clip(&why, 140))));
        return;
    }
    let next = match role {
        Role::Dev if swarm.has(Role::Test) => Some("qa"),
        Role::Dev | Role::Test if swarm.has(Role::Review) => Some("agent_review"),
        _ => None,
    };
    match next {
        Some(stage) => {
            let _ = store.set_issue_stage(project, number, actor.clone(), stage.into()).await;
            let _ = store.release_issue(project, number, actor, summary).await;
            let place = if stage == "qa" { "QA" } else { "Agent review" };
            swarm.with(|state| state.event("ok", format!("{name} finished its part of #{number}. It moves to {place}.")));
        }
        None => {
            let status = store.finish_issue(project, number, actor, summary).await;
            let words = match status.as_deref() { Ok("closed") => "closed", Ok(_) => "waiting for your review", Err(_) => "already moved on" };
            swarm.with(|state| state.event("ok", format!("{name} approved #{number}. It is {words}.")));
        }
    }
}

/// Gives an issue to the user: raised to high severity and put in Human Review, whatever the level.
async fn hand_to_user(backend: &Backend, swarm: &Swarm, actor: &Actor, number: i64, why: &str) {
    let _ = backend.agents.set_issue_severity(swarm.project, number, "high".into(), Some(actor.clone())).await;
    let _ = backend.agents.await_review(swarm.project, number, actor.clone(), why.to_owned()).await;
    swarm.with(|state| state.event("fail", format!("{} It waits for you.", clip(why, 200))));
}

/// A swarm Lead's own issue actions, refused for anyone else: `issue_assign` gives a story to one of
/// the swarm's agents by name; `issue_review_level` changes what agents may close without the user.
/// Answers the Activity title and the words for the model.
pub async fn lead_action(backend: &Backend, thread: &str, project: Uuid, action: &str, arguments: &Value) -> (String, std::result::Result<String, String>) {
    let lead = threads().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(thread)
        .filter(|(swarm, name)| name == "Lead" && swarm.project == project).map(|(swarm, _)| swarm.clone());
    let title = if action == "issue_assign" { "Gave a story to an agent".to_owned() } else { "Changed the review level".to_owned() };
    let Some(swarm) = lead else { return (title, Err("Only a swarm's Lead may do this. Ask the user to do it in Issues.".into())) };
    if action == "issue_review_level" {
        let level = arguments["level"].as_str().unwrap_or_default().to_owned();
        if !crate::issues::valid_level(&level) { return (title, Err("level is none, low, medium or high.".into())); }
        return match backend.agents.set_review_level(project, level.clone()).await {
            Ok(()) => { swarm.with(|state| state.event("info", format!("The Lead set the review level: {}", crate::issues::level_sentence(&level))));
                (title, Ok(format!("Done: {}", crate::issues::level_sentence(&level)))) }
            Err(_) => (title, Err("MiniClaw could not save the review level.".into())),
        };
    }
    let number = arguments["issue"].as_i64().unwrap_or(0);
    let agent = arguments["agent"].as_str().unwrap_or_default().trim().to_owned();
    let title = format!("Gave #{number} to {agent}");
    let Some(role) = swarm.with(|state| state.member(&agent).map(|member| member.role)).filter(|role| *role != Role::Lead) else {
        let names = swarm.with(|state| state.members.iter().filter(|member| member.role != Role::Lead).map(|member| member.name.clone()).collect::<Vec<_>>());
        return (title, Err(format!("There is no agent {agent} in this swarm. Its agents are {}.", names.join(", "))));
    };
    let Ok(Some(issue)) = backend.agents.issue(project, number).await else { return (title, Err(format!("There is no story #{number}."))) };
    if swarm.with(|state| state.leases.contains_key(&number)) {
        return (title, Err(format!("#{number} is being worked on right now; give it once that agent is done.")));
    }
    if !role.takes(&issue.status) {
        return (title, Err(format!("#{number} is in {}, which {agent} does not work on.", crate::issues::stage_word(&issue.status))));
    }
    match backend.agents.assign_issue_as(project, number, Some(swarm.actor(&agent)), "Lead".into()).await {
        Ok(()) => { swarm.with(|state| state.event("info", format!("The Lead gave #{number} to {agent}.")));
            (title, Ok(format!("#{number} is {agent}'s now; {agent} takes it next.")) ) }
        Err(_) => (title, Err("MiniClaw could not change the story.".into())),
    }
}

/// Where swarms are kept, so the sidebar still lists them after a restart.
fn saved_folder() -> std::path::PathBuf { crate::platform_fs::data_directory().join("swarms") }

/// Writes a swarm's snapshot beside the other saved swarms. A failure only costs the history.
fn save(snapshot: &Value) {
    let Some(id) = snapshot["id"].as_str() else { return };
    let folder = saved_folder();
    if std::fs::create_dir_all(&folder).is_err() { return; }
    let temporary = folder.join(format!("{id}.json.tmp"));
    if std::fs::write(&temporary, snapshot.to_string()).is_ok() { let _ = std::fs::rename(&temporary, folder.join(format!("{id}.json"))); }
}

/// Reads the swarms saved by earlier backend runs. One that was still running is shown as
/// interrupted: its agents ended with the backend; its issues stay where they were.
fn saved() -> Vec<Value> {
    saved_records().into_iter().map(|mut snapshot| {
        if matches!(snapshot["status"].as_str(), Some("planning" | "running" | "idle")) { snapshot["status"] = json!("interrupted"); }
        snapshot
    }).collect()
}

/// The saved records exactly as written, newest first.
fn saved_records() -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(saved_folder()) else { return Vec::new() };
    let mut all: Vec<Value> = entries.flatten().filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| serde_json::from_slice::<Value>(&std::fs::read(entry.path()).ok()?).ok())
        .filter(|snapshot| snapshot["id"].as_str().is_some_and(|id| id.chars().all(|c| c.is_ascii_hexdigit())))
        .collect();
    all.sort_by_key(|snapshot| std::cmp::Reverse(snapshot["created"].as_i64().unwrap_or(0)));
    all
}

/// Maps a saved event's kind back to the fixed words the history uses.
fn kind_of(text: &str) -> &'static str {
    match text { "ok" => "ok", "warn" => "warn", "fail" => "fail", "done" => "done", _ => "info" }
}

/// Brings back every swarm that was working when the backend stopped. It waits for a signed-in
/// provider, then for each saved swarm that was planning, running or idle: rebuilds its agents from
/// the record, gives back to the Backlog every story an agent of that swarm held (the agent that
/// held it is gone), and starts the agents again. Stories, conversations and files are as they were.
pub async fn resume_all(backend: Shared) {
    for _ in 0..120 {
        if authorized(&backend).await.is_ok() { break; }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    for record in saved_records() {
        if !matches!(record["status"].as_str(), Some("planning" | "running" | "idle")) { continue; }
        let Some(id) = record["id"].as_str().map(str::to_owned) else { continue };
        if find(&id).is_some() { continue; }
        let Some(project) = record["projectId"].as_str().and_then(|text| Uuid::parse_str(text).ok()) else { continue };
        if !backend.agents.projects().await.is_ok_and(|all| all.iter().any(|item| item.id == project)) { continue; }
        let time = now();
        let members: Vec<Member> = record["members"].as_array().into_iter().flatten().filter_map(|member| Some(Member {
            name: member["name"].as_str()?.to_owned(), role: serde_json::from_value(member["role"].clone()).ok()?,
            model: member["model"].as_str()?.to_owned(), link_up: true, last_seen: time, status: "Resuming".into(), issue: None,
            thread: member["thread"].as_str().map(str::to_owned), queued: Vec::new(), done: member["done"].as_u64().unwrap_or(0) as u32,
            joined: time, unreachable_ms: 0, reconnect_at: None })).collect();
        let Some(lead) = members.iter().find(|member| member.role == Role::Lead).cloned() else { continue };
        let team: Vec<(String, Role, String)> = members.iter().filter(|member| member.role != Role::Lead)
            .map(|member| (member.name.clone(), member.role, member.model.clone())).collect();
        let mut events: Vec<Event> = record["events"].as_array().into_iter().flatten().filter_map(|event| Some(Event {
            at: event["at"].as_i64()?, kind: kind_of(event["kind"].as_str()?), text: event["text"].as_str()?.to_owned() })).collect();
        events.push(Event { at: time, kind: "warn", text: "The backend restarted. MiniClaw is resuming the swarm: stories held by agents go back to the Backlog.".into() });
        let swarm = Arc::new(Swarm { id: id.clone(), goal: record["goal"].as_str().unwrap_or_default().to_owned(),
            workspace: record["workspace"].as_str().unwrap_or_default().to_owned(), unsandboxed: record["unsandboxed"] == true, project,
            skills: role_skills(&backend).await, created: record["created"].as_i64().unwrap_or(time), stopped: AtomicBool::new(false),
            state: StdMutex::new(Ledger { status: "running", members, events, leases: Default::default(), rounds: Default::default(),
                attempts: Default::default(), next_version: 0, finished: None }) });
        // Stories an agent of this swarm held go back: a developer's to the Backlog, the others stay in their stage unowned.
        let prefix = format!("swarm-{}-", &id[..6]);
        let held: Vec<Record> = backend.agents.issues(project, None, None).await.unwrap_or_default().into_iter()
            .filter(|issue| crate::issues::active(&issue.status) && issue.status != "backlog" && issue.owner_id.as_deref().is_some_and(|owner| owner.starts_with(&prefix))).collect();
        for issue in held {
            let (Some(owner_id), Some(owner)) = (issue.owner_id.clone(), issue.owner.clone()) else { continue };
            let handover = format!("The backend restarted while {owner} worked on this. MiniClaw released it for another agent.");
            let _ = backend.agents.release_issue(project, issue.number, Actor { id: owner_id, name: owner }, handover).await;
        }
        if backend.agents.issues(project, None, None).await.map_or(true, |issues| issues.is_empty()) {
            for draft in read_plan("", &swarm.goal) { let _ = backend.agents.create_issue(project, draft, "Lead".into()).await; }
        }
        if let Some(thread) = lead.thread { threads().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(thread, (swarm.clone(), "Lead".into())); }
        swarms().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(swarm.clone());
        tokio::spawn(supervise(backend.clone(), swarm, team));
    }
}

/// The Anthropic skills each role gets (server/swarm-skills, Apache 2.0; see its NOTICE.md).
const SKILLS: &[(Role, &str, &str)] = &[
    (Role::Lead, "code-architect", include_str!("../../swarm-skills/code-architect.md")),
    (Role::Dev, "code-explorer", include_str!("../../swarm-skills/code-explorer.md")),
    (Role::Dev, "code-simplifier", include_str!("../../swarm-skills/code-simplifier.md")),
    (Role::Dev, "frontend-design", include_str!("../../swarm-skills/frontend-design.md")),
    (Role::Review, "code-reviewer", include_str!("../../swarm-skills/code-reviewer.md")),
    (Role::Review, "silent-failure-hunter", include_str!("../../swarm-skills/silent-failure-hunter.md")),
    (Role::Test, "pr-test-analyzer", include_str!("../../swarm-skills/pr-test-analyzer.md")),
    (Role::Test, "webapp-testing", include_str!("../../swarm-skills/webapp-testing.md")),
];

/// The helper script webapp-testing names, copied into each swarm's folder.
const WEBAPP_SCRIPT: &str = include_str!("../../swarm-skills/webapp-testing/scripts/with_server.py");

/// The `description:` line of a skill file's front matter, cut to what MiniClaw stores.
fn skill_description(text: &str) -> String {
    let front = text.strip_prefix("---").and_then(|rest| rest.split("\n---").next()).unwrap_or("");
    let line = front.lines().find_map(|line| line.strip_prefix("description:")).unwrap_or("").trim().trim_matches('"');
    clip(line, 480)
}

/// Saves the role skills as MiniClaw skills the first time, and finds them by name after that, so
/// a user's edit of one is kept. Returns each skill's role and id; one that cannot be saved is left out.
async fn role_skills(backend: &Backend) -> Vec<(Role, Uuid)> {
    let Ok(existing) = backend.agents.skills().await else { return Vec::new() };
    let mut found = Vec::new();
    for (role, name, text) in SKILLS {
        let title = format!("Swarm · {name}");
        let id = match existing.iter().find(|skill| skill.definition.name == title) {
            Some(skill) => Some(skill.id),
            None => backend.agents.save_skill(None, crate::skills::Definition { name: title, description: skill_description(text),
                instructions: (*text).to_owned(), active: true, slash: false, revision: None }).await.ok().map(|skill| skill.id),
        };
        if let Some(id) = id { found.push((*role, id)); }
    }
    found
}

/// One team of agents in a start request.
#[derive(Deserialize)]
pub(super) struct Team { count: usize, model: String }

/// A request to start a swarm.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Start { goal: String, lead_model: Option<String>, dev: Team, test: Team, #[serde(default)] review: Option<Team>,
    #[serde(default)] unsandboxed: bool, #[serde(default)] review_level: Option<String> }

/// Starts a swarm and answers with its first snapshot.
pub(super) async fn start(State(backend): State<Shared>, input: Body<Start>) -> Result<(StatusCode, Json<Value>)> {
    authorized(&backend).await?;
    let input = body(input)?;
    let goal = input.goal.trim().to_owned();
    let model_ok = |model: &str| !model.is_empty() && model.len() <= 100 && model.chars().all(|c| c.is_ascii_alphanumeric() || "-._".contains(c));
    let review = input.review.as_ref().filter(|team| team.count > 0);
    let level = input.review_level.clone().unwrap_or_else(|| "medium".into());
    if goal.is_empty() || goal.chars().count() > 4000 || !(1..=6).contains(&input.dev.count) || input.test.count > 4
        || review.is_some_and(|team| team.count > 4 || !model_ok(&team.model)) || !crate::issues::valid_level(&level)
        || !model_ok(&input.dev.model) || !model_ok(&input.test.model) || input.lead_model.as_deref().is_some_and(|model| !model_ok(model)) {
        return Err(invalid());
    }
    let id = Uuid::new_v4().simple().to_string();
    let slug: String = goal.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect::<String>()
        .split('-').filter(|part| !part.is_empty()).take(5).collect::<Vec<_>>().join("-");
    // The user's own folder on every platform: HOME on Linux and macOS, USERPROFILE on Windows.
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let workspace = home.join("swarms").join(format!("{}-{}", if slug.is_empty() { "swarm" } else { &slug }, &id[..6]));
    std::fs::create_dir_all(&workspace).map_err(|_| invalid())?;
    let workspace = std::fs::canonicalize(&workspace).map_err(|_| invalid())?.to_string_lossy().into_owned();
    // The helper script the webapp-testing skill names lives in the folder the agents can reach.
    let scripts = std::path::Path::new(&workspace).join(".swarm-skills").join("webapp-testing").join("scripts");
    if std::fs::create_dir_all(&scripts).is_ok() { let _ = std::fs::write(scripts.join("with_server.py"), WEBAPP_SCRIPT); }
    // Each swarm is a project: the folder is its workspace, its issues are the work, and every agent
    // conversation is in it.
    let saved = super::save_project(&backend, None, crate::projects::Definition { name: clip(&goal, 60), agent_ids: Vec::new(),
        skill_ids: Vec::new(), skill_group_ids: Vec::new(), environment: Some(crate::environment::ProjectChoice::Host {}),
        workspace: Some(workspace.clone()), instructions: rules(&goal, &workspace), revision: None }).await?;
    let project = saved.0["project"]["id"].as_str().and_then(|id| Uuid::parse_str(id).ok()).ok_or_else(unavailable)?;
    backend.agents.set_review_level(project, level).await?;
    let skills = role_skills(&backend).await;
    let lead_model = input.lead_model.clone().unwrap_or_else(|| input.dev.model.clone());
    let time = now();
    let member = |name: String, role: Role, model: String| Member { name, role, model, link_up: true, last_seen: time,
        status: if role == Role::Lead { "Filing the goal as issues".into() } else { "Waiting for the plan".into() },
        issue: None, thread: None, queued: Vec::new(), done: 0, joined: time, unreachable_ms: 0, reconnect_at: None };
   let mut team = Vec::new();
    for n in 1..=input.dev.count { team.push((format!("Dev-{n}"), Role::Dev, input.dev.model.clone())); }
    if let Some(review) = review {
        for n in 1..=review.count { team.push((format!("Review-{n}"), Role::Review, review.model.clone())); }
    }
    for n in 1..=input.test.count { team.push((format!("Test-{n}"), Role::Test, input.test.model.clone())); }
    let mut members = vec![member("Lead".into(), Role::Lead, lead_model.clone())];
    members.extend(team.iter().map(|(name, role, model)| member(name.clone(), *role, model.clone())));
    let swarm = Arc::new(Swarm { id, goal: goal.clone(), workspace, unsandboxed: input.unsandboxed, project, skills, created: time,
        stopped: AtomicBool::new(false), state: StdMutex::new(Ledger { status: "planning", members, events: Vec::new(),
            leases: Default::default(), rounds: Default::default(), attempts: Default::default(), next_version: 0, finished: None }) });
    swarm.with(|state| state.event("info", format!("Swarm started with {} agents in {}.", team.len() + 1, swarm.workspace)));
    if swarm.unsandboxed {
        swarm.with(|state| state.event("warn", "Unsandboxed shell is on: agents' commands run as you, outside the sandbox, with your files and network.".into()));
    }
    swarms().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(swarm.clone());
    tokio::spawn(run(backend.clone(), swarm.clone(), lead_model, team));
    Ok((StatusCode::CREATED, Json(swarm.snapshot(&backend).await)))
}

/// The running swarms with their agents, for the running box beside every page.
pub(super) async fn live(backend: &Backend) -> Value {
    let running: Vec<Arc<Swarm>> = swarms().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).iter()
        .filter(|swarm| swarm.with(|state| matches!(state.status, "planning" | "running" | "idle"))).cloned().collect();
    let mut all = Vec::new();
    for swarm in running { all.push(swarm.snapshot(backend).await); }
    json!(all)
}

/// Lists this run's swarms and the saved ones, newest first.
pub(super) async fn list(State(backend): State<Shared>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    let live: Vec<Arc<Swarm>> = swarms().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).iter().rev().cloned().collect();
    let mut all = Vec::new();
    for swarm in &live {
        let snapshot = swarm.snapshot(&backend).await;
        all.push(json!({ "id": swarm.id, "goal": swarm.goal, "status": snapshot["status"], "created": swarm.created,
            "projectId": swarm.project, "counts": snapshot["counts"] }));
    }
    for snapshot in saved() {
        if live.iter().any(|swarm| snapshot["id"] == swarm.id.as_str()) { continue; }
        all.push(json!({ "id": snapshot["id"], "goal": snapshot["goal"], "status": snapshot["status"], "created": snapshot["created"],
            "projectId": snapshot["projectId"], "counts": snapshot["counts"] }));
    }
    Ok(Json(json!({ "swarms": all })))
}

/// Answers one swarm's snapshot, live or saved.
pub(super) async fn view(State(backend): State<Shared>, Path(id): Path<String>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    if let Some(swarm) = find(&id) { return Ok(Json(swarm.snapshot(&backend).await)); }
    saved().into_iter().find(|snapshot| snapshot["id"] == id.as_str()).map(Json).ok_or_else(missing)
}

/// Cuts or restores one agent's link to the issues. A cut link is retried by MiniClaw after
/// `reconnectSeconds` (45 if not given; 0 leaves it cut until someone restores it).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Link { agent: String, up: bool, #[serde(default)] reconnect_seconds: Option<u64> }

/// How long a cut link waits before MiniClaw tries to reconnect it.
const RECONNECT_SECONDS: u64 = 45;

/// Changes one agent's link. Returns what the agent queued while it was cut (to deliver now) and
/// when the next automatic reconnect is due, or `None` if there is no such agent.
fn apply_link(swarm: &Swarm, agent: &str, up: bool, after: u64, automatic: bool) -> Option<(Vec<(Role, Queued)>, Option<i64>)> {
    swarm.with(|state| {
        let member = state.member(agent).filter(|member| member.role != Role::Lead)?;
        if member.link_up == up { return Some((Vec::new(), None)); }
        member.link_up = up;
        let role = member.role;
        let queued = if up { member.last_seen = now(); member.reconnect_at = None; std::mem::take(&mut member.queued) } else { Vec::new() };
        let due = (!up && after > 0).then(|| now() + after as i64 * 1000);
        member.reconnect_at = due;
        if up {
            state.beat(agent);
            state.event("ok", if automatic { format!("Reconnect attempt succeeded: the link to {agent} is back.") } else { format!("Link to {agent} restored.") });
        } else {
            let retry = if after > 0 { format!(" MiniClaw tries to reconnect in {after} s.") } else { String::new() };
            state.event("warn", format!("Link to {agent} cut. It keeps working offline; no one waits for it.{retry}"));
        }
        Some((queued.into_iter().map(|item| (role, item)).collect(), due))
    })
}

/// Delivers what an agent queued while its link was down.
async fn replay(backend: &Backend, swarm: &Swarm, agent: &str, queued: Vec<(Role, Queued)>) {
    for (role, item) in queued {
        swarm.with(|state| state.event("info", format!("{agent} replays its queued result for #{}.", item.issue)));
        deliver(backend, swarm, agent, role, item.issue, item.version, item.ok, &item.text).await;
    }
}

/// Cuts or restores an agent's link; a restored agent replays what it queued. A cut starts the timer
/// that reconnects it.
pub(super) async fn link(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Link>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    let input = body(input)?;
    let swarm = find(&id).ok_or_else(missing)?;
    let after = input.reconnect_seconds.unwrap_or(RECONNECT_SECONDS).min(3600);
    let (queued, due) = apply_link(&swarm, &input.agent, input.up, after, false).ok_or_else(invalid)?;
    replay(&backend, &swarm, &input.agent, queued).await;
    if let Some(at) = due {
        let (backend, swarm, agent) = (backend.clone(), swarm.clone(), input.agent.clone());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis((at - now()).max(0) as u64)).await;
            // Only the cut this timer was set for: a restore, or a newer cut, replaces it.
            let current = swarm.with(|state| state.member(&agent).is_some_and(|member| !member.link_up && member.reconnect_at == Some(at)));
            if !current || swarm.stopped.load(Ordering::SeqCst) { return; }
            if let Some((queued, _)) = apply_link(&swarm, &agent, true, 0, true) { replay(&backend, &swarm, &agent, queued).await; }
        });
    }
    Ok(Json(swarm.snapshot(&backend).await))
}

/// A task the user adds while the swarm runs.
#[derive(Deserialize)]
pub(super) struct Add { text: String }

/// Files the user's task as a Backlog feature, where the next free development agent takes it.
pub(super) async fn add(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Add>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    let text = body(input)?.text.trim().to_owned();
    if text.is_empty() || text.chars().count() > 2000 { return Err(invalid()); }
    let swarm = find(&id).ok_or_else(missing)?;
    if swarm.with(|state| !matches!(state.status, "running" | "idle")) { return Err(invalid()); }
    let draft = Draft { kind: "feature".into(), title: clip(text.lines().next().unwrap_or(&text), 110), text: text.clone(),
        severity: "medium".into(), tags: Vec::new() };
    let issue = backend.agents.create_issue(swarm.project, draft, crate::issues::USER.into()).await.map_err(|_| invalid())?;
    swarm.with(|state| state.event("info", format!("You filed #{} {}.", issue.number, issue.title)));
    Ok(Json(swarm.snapshot(&backend).await))
}

/// Stops a swarm: running turns are interrupted and no issue is taken again.
pub(super) async fn stop(State(backend): State<Shared>, Path(id): Path<String>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    let swarm = find(&id).ok_or_else(missing)?;
    if !swarm.stopped.swap(true, Ordering::SeqCst) { swarm.with(|state| state.event("warn", "You stopped the swarm.".into())); }
    Ok(Json(swarm.snapshot(&backend).await))
}

/// Deletes a swarm for good: it is stopped, then its project goes with every agent conversation,
/// its stories and their histories, and its saved record. The files in its folder stay there.
pub(super) async fn delete(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Value>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    let _ = body(input)?;
    if !id.chars().all(|c| c.is_ascii_hexdigit()) || id.is_empty() { return Err(invalid()); }
    let saved_one = saved().into_iter().find(|snapshot| snapshot["id"] == id.as_str());
    let swarm = find(&id);
    let project = match (&swarm, &saved_one) {
        (Some(swarm), _) => Some(swarm.project),
        (None, Some(snapshot)) => snapshot["projectId"].as_str().and_then(|text| Uuid::parse_str(text).ok()),
        (None, None) => return Err(missing()),
    };
    if let Some(swarm) = &swarm {
        swarm.stopped.store(true, Ordering::SeqCst);
        // Running turns end first: a conversation still answering cannot be deleted.
        let mine: Vec<String> = threads().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).iter()
            .filter(|(_, (owner, _))| Arc::ptr_eq(owner, swarm)).map(|(thread, _)| thread.clone()).collect();
        for _ in 0..40 {
            let running: Vec<(String, String)> = { let data = backend.data.lock().await;
                mine.iter().filter_map(|thread| data.chats.get(thread).and_then(|chat| chat.active.clone()).map(|turn| (thread.clone(), turn))).collect() };
            if running.is_empty() { break; }
            for (thread, turn) in running {
                let _ = interrupt(State(backend.clone()), Path(thread), Ok(Json(Interrupt { turn_id: turn }))).await;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    if let Some(project) = project {
        let revision = backend.agents.projects().await?.into_iter().find(|item| item.id == project).and_then(|item| item.definition.revision);
        if let Some(revision) = revision {
            let _ = super::delete_project(State(backend.clone()), Path(project.to_string()),
                Ok(Json(super::ProjectDelete { revision, delete_vm: false, confirm_name: None }))).await?;
        }
        backend.agents.delete_project_issues(project).await?;
    }
    swarms().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).retain(|item| item.id != id);
    threads().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).retain(|_, (owner, _)| owner.id != id);
    let _ = std::fs::remove_file(saved_folder().join(format!("{id}.json")));
    backend.changed();
    Ok(Json(json!({})))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ledger with two agents.
    fn ledger() -> Ledger {
        let time = now();
        let member = |name: &str, role| Member { name: name.into(), role, model: "m".into(), link_up: true, last_seen: time, status: String::new(),
<<<<<<< HEAD
            issue: None, thread: None, queued: Vec::new(), done: 0, joined: time, unreachable_ms: 0, reconnect_at: None };
=======
            retrying: false, issue: None, thread: None, queued: Vec::new(), done: 0, joined: time, unreachable_ms: 0 };
>>>>>>> fa3be5121548b0cb539466b87d4aa7702287edf2
        Ledger { status: "running", members: vec![member("Dev-1", Role::Dev), member("Dev-2", Role::Dev)], events: Vec::new(),
            leases: Default::default(), rounds: Default::default(), attempts: Default::default(), next_version: 0, finished: None }
    }

    /// Each role takes issues only in its own stage.
    #[test]
    fn roles_take_issues_in_their_stage() {
        assert!(Role::Dev.takes("backlog") && !Role::Dev.takes("dev") && !Role::Dev.takes("qa"));
        assert!(Role::Test.takes("qa") && !Role::Test.takes("agent_review"));
        assert!(Role::Review.takes("agent_review") && !Role::Review.takes("review"));
        assert!(!Role::Lead.takes("backlog"));
    }

    /// A lease that runs out is handed back; the old owner's result then finds nothing, and a new
    /// claim's result counts once.
    #[test]
    fn an_expired_lease_is_released_and_a_late_result_is_not_applied() {
        let mut state = ledger();
        let first = state.lease(7, "Dev-1");
        state.member("Dev-1").unwrap().link_up = false;
        state.leases.get_mut(&7).unwrap().until = now() - 1;
        assert_eq!(state.expired(1000), vec![(7, "Dev-1".to_owned())]);
        let second = state.lease(7, "Dev-2");
        assert!(!state.take(7, "Dev-1", first));
        assert!(state.take(7, "Dev-2", second));
        assert!(!state.take(7, "Dev-2", second));
    }

    /// Heartbeats renew only a reachable agent's leases.
    #[test]
    fn heartbeats_renew_only_reachable_agents() {
        let mut state = ledger();
        state.lease(1, "Dev-1");
        state.leases.get_mut(&1).unwrap().until = now() - 1;
        state.member("Dev-1").unwrap().link_up = false;
        state.beat("Dev-1");
        assert!(state.leases[&1].until < now());
        state.member("Dev-1").unwrap().link_up = true;
        state.beat("Dev-1");
        assert!(state.leases[&1].until > now());
    }

    /// A failed turn is retried by the agent that holds the claim, until the issue has failed
    /// MAX_ATTEMPTS times; an agent that lost its claim does not retry.
    #[test]
    fn a_failed_turn_is_retried_while_the_claim_holds() {
        let mut state = ledger();
        let version = state.lease(3, "Dev-1");
        assert_eq!(state.failed(3, "Dev-1", version), Some(2));
        assert_eq!(state.failed(3, "Dev-1", version), Some(3));
        assert_eq!(state.failed(3, "Dev-1", version), None);
        assert_eq!(state.attempts[&3], MAX_ATTEMPTS);
        let other = state.lease(4, "Dev-1");
        state.leases.get_mut(&4).unwrap().owner = "Dev-2".into();
        assert_eq!(state.failed(4, "Dev-1", other), None);
    }

    /// Verdicts are read through Markdown, and the reason is the rest of the first line.
    #[test]
    fn verdicts_and_reasons_are_read_through_markdown() {
        assert!(verdict("**FAIL:** the copy button does nothing").starts_with("FAIL"));
        assert!(verdict("  CHANGES: rename x").starts_with("CHANGES"));
        assert_eq!(reason("**CHANGES:** the error is swallowed\n- main.py"), "the error is swallowed");
    }

    /// The lead's JSON becomes issues; a reply without it still yields one.
    #[test]
    fn the_plan_is_read_as_issues() {
        let plan = read_plan("{\"issues\":[{\"title\":\"API\",\"kind\":\"feature\",\"severity\":\"high\",\"text\":\"x\"},{\"title\":\"Crash\",\"kind\":\"bug\"}]}", "goal");
        assert_eq!(plan.len(), 2);
        assert_eq!((plan[0].severity.as_str(), plan[1].kind.as_str(), plan[1].severity.as_str()), ("high", "bug", "medium"));
        assert_eq!(read_plan("no json", "Build it\nwell").len(), 1);
    }

    /// A skill's description comes from its front matter.
    #[test]
    fn a_skill_description_is_read_from_its_front_matter() {
        assert_eq!(skill_description("---\nname: x\ndescription: Reviews code.\nmodel: opus\n---\nBody"), "Reviews code.");
        assert!(SKILLS.iter().all(|(_, _, text)| !skill_description(text).is_empty() && text.len() <= 16384));
    }
}
