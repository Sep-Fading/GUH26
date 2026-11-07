//! Swarms: one shared goal, a lead agent that splits it into tasks, and development and testing
//! agents that claim those tasks from a shared ledger. Agents never talk to each other, only to the
//! ledger, so a dropped link costs the swarm nothing but that agent's voice:
//!
//! - an agent holds a task with a lease and renews it with each heartbeat;
//! - when its link is down, it keeps working; its heartbeats stop reaching the ledger and its result
//!   waits in a local queue;
//! - when the lease runs out, the task is open again and another agent takes it;
//! - when the link comes back, the queue is replayed. A result counts only while the agent still
//!   holds the task at the version it claimed, so a late result never overwrites finished work.
//!
//! Every agent is a real MiniClaw conversation (Codex or Claude, chosen by its model) working in one
//! shared folder through MiniClaw's sandboxed tool, with Full access so no command waits on a popup.
//! Swarms live in memory: a restart of the backend ends them.

// Reuses chat creation, sending and interruption exactly as delegation does.
use super::*;
// Grants the shared folder to each agent's conversation.
use crate::permissions::{Effect, Scope, Target};
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
/// A task that fails this often is given up.
const MAX_ATTEMPTS: u32 = 3;
/// How many rounds of "test failed, fix it, test again" one task may cause.
const MAX_FIX_ROUNDS: u32 = 2;

/// Milliseconds since the epoch, the clock of the ledger.
fn now() -> i64 { chrono::Utc::now().timestamp_millis() }

/// What an agent does in the swarm.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Role { Lead, Dev, Review, Test }

/// One ledger entry.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Task {
    id: u32,
    title: String,
    detail: String,
    kind: Role,
    /// open, claimed, done or failed.
    state: &'static str,
    owner: Option<String>,
    lease_until: Option<i64>,
    /// Tasks that must be done before this one may be claimed.
    after: Vec<u32>,
    attempts: u32,
    result: Option<String>,
    /// Bumped by every claim and reopening; a result must name the version it was claimed at.
    version: u64,
    fix_round: u32,
}

/// A result an agent could not deliver while its link was down.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Queued { task: u32, version: u64, ok: bool, text: String }

/// One agent of the swarm.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Member {
    name: String,
    role: Role,
    model: String,
    /// The simulated link between this agent and the ledger.
    link_up: bool,
    last_seen: i64,
    status: String,
    task: Option<u32>,
    thread: Option<String>,
    queued: Vec<Queued>,
    done: u32,
    joined: i64,
    unreachable_ms: i64,
}

/// One line of the swarm's history.
#[derive(Clone, Serialize)]
struct Event { at: i64, kind: &'static str, text: String }

/// Everything that changes, behind one lock that is never held across an await.
struct Ledger { status: &'static str, tasks: Vec<Task>, members: Vec<Member>, events: Vec<Event>, next_id: u32, finished: Option<i64>,
    /// Whether the swarm has code review agents; each finished development task is then reviewed.
    reviewers: bool }

/// One swarm and its fixed facts.
pub(super) struct Swarm {
    id: String,
    goal: String,
    workspace: String,
    /// The user started it with unsandboxed shell on: agents' commands run as the user, outside the
    /// sandbox, with the user's files and network, for as long as the swarm runs.
    unsandboxed: bool,
    /// The MiniClaw project this swarm is: its folder is the project's workspace.
    project: Option<Uuid>,
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

/// Every conversation a swarm agent worked in. They are kept out of the sidebar; the swarm view
/// shows their work.
fn threads() -> &'static StdMutex<std::collections::HashSet<String>> {
    static ALL: OnceLock<StdMutex<std::collections::HashSet<String>>> = OnceLock::new();
    ALL.get_or_init(|| StdMutex::new(std::collections::HashSet::new()))
}

/// Says whether a conversation belongs to a swarm agent: one of this run's, or an earlier run's,
/// known by how every swarm prompt begins.
pub(super) fn hides(thread: &str) -> bool {
    threads().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).contains(thread)
}

/// Recognises a swarm agent's conversation from an earlier backend run by its opening words.
pub(super) fn looks_like_swarm(summary: &Value) -> bool {
    ["name", "preview"].iter().filter_map(|key| summary[*key].as_str()).any(|text| {
        let text = text.trim_start();
        text.starts_with("You lead a swarm of ") || (text.starts_with("You are ") && text.contains(", one agent in a swarm."))
    })
}

/// Finds a swarm by id.
fn find(id: &str) -> Option<Arc<Swarm>> {
    swarms().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).iter().find(|swarm| swarm.id == id).cloned()
}

/// Shortens text for a ledger field or an event.
fn clip(text: &str, most: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= most { return text.to_owned(); }
    format!("{}…", text.chars().take(most).collect::<String>())
}

impl Ledger {
    /// Adds a line to the history.
    fn event(&mut self, kind: &'static str, text: String) {
        self.events.push(Event { at: now(), kind, text });
        if self.events.len() > 400 { self.events.remove(0); }
    }

    /// The agent named `name`.
    fn member(&mut self, name: &str) -> Option<&mut Member> { self.members.iter_mut().find(|member| member.name == name) }

    /// Adds a task and returns its id.
    fn add(&mut self, title: String, detail: String, kind: Role, after: Vec<u32>, fix_round: u32) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.tasks.push(Task { id, title, detail, kind, state: "open", owner: None, lease_until: None, after, attempts: 0,
            result: None, version: 0, fix_round });
        id
    }

    /// Gives `name` the first open task of its role whose prerequisites are done; returns it with
    /// the version the result must name.
    fn claim(&mut self, name: &str, role: Role) -> Option<Task> {
        let done: Vec<u32> = self.tasks.iter().filter(|task| task.state == "done" || task.state == "failed").map(|task| task.id).collect();
        let task = self.tasks.iter_mut().find(|task| task.state == "open" && task.kind == role
            && task.after.iter().all(|id| done.contains(id)))?;
        task.state = "claimed";
        task.owner = Some(name.to_owned());
        task.lease_until = Some(now() + LEASE_MS);
        task.version += 1;
        let task = task.clone();
        if let Some(member) = self.member(name) { member.task = Some(task.id); member.status = format!("Working on #{} {}", task.id, task.title); }
        self.event("info", format!("{name} claimed #{} {}.", task.id, task.title));
        Some(task)
    }

    /// A heartbeat that reached the ledger: the agent is seen and its lease renewed.
    fn beat(&mut self, name: &str) {
        let Some(member) = self.member(name) else { return };
        if !member.link_up { return; }
        member.last_seen = now();
        for task in self.tasks.iter_mut().filter(|task| task.state == "claimed" && task.owner.as_deref() == Some(name)) {
            task.lease_until = Some(now() + LEASE_MS);
        }
    }

    /// Applies a result if `name` still holds the task at `version`. Returns whether it counted.
    fn finish(&mut self, name: &str, id: u32, version: u64, ok: bool, text: &str) -> bool {
        let Some(task) = self.tasks.iter_mut().find(|task| task.id == id) else { return false };
        if task.owner.as_deref() != Some(name) || task.state != "claimed" || task.version != version { return false; }
        task.lease_until = None;
        if ok {
            task.state = "done";
            task.result = Some(clip(text, 2000));
            let (title, kind, round) = (task.title.clone(), task.kind, task.fix_round);
            if let Some(member) = self.member(name) { member.done += 1; }
            // Models often bold the verdict ("**FAIL:**"), so leading Markdown is skipped.
            let verdict = text.trim_start().trim_start_matches(|c: char| !c.is_ascii_alphanumeric());
            let first = verdict.to_ascii_uppercase();
            let failed = kind == Role::Test && first.starts_with("FAIL");
            let changes = kind == Role::Review && first.starts_with("CHANGES");
            if changes && round < MAX_FIX_ROUNDS {
                let reason = verdict.lines().next().unwrap_or("").trim_start_matches(|c: char| c.is_ascii_alphabetic() || c == ':' || c == '*').trim().to_owned();
                let reason = if reason.is_empty() { title.clone() } else { reason };
                let fix = self.add(format!("Fix: {}", clip(&reason, 70)), clip(text, 1500), Role::Dev, Vec::new(), round + 1);
                self.event("warn", format!("{name}: the review of #{id} asks for changes. Filed #{fix} to make them."));
            } else if kind == Role::Review {
                self.event("ok", format!("{name} approved #{id} {title}."));
            } else if kind == Role::Dev && self.reviewers {
                let review = self.add(format!("Review: {}", clip(&title, 70)), format!("Review the change made for #{id} {title}. The developer reported: {}", clip(text, 1200)),
                    Role::Review, Vec::new(), round);
                self.event("ok", format!("{name} finished #{id} {title}. Filed #{review} to review it."));
            } else if failed && round < MAX_FIX_ROUNDS {
                let reason = verdict.lines().next().unwrap_or("").trim_start_matches(|c: char| c.is_ascii_alphabetic() || c == ':' || c == '*').trim().to_owned();
                let reason = if reason.is_empty() { title.clone() } else { reason };
                let fix = self.add(format!("Fix: {}", clip(&reason, 70)), clip(text, 1500), Role::Dev, Vec::new(), round + 1);
                self.add(format!("Re-test: {}", clip(title.trim_start_matches("Re-test: "), 60)), format!("Test again after #{fix}. {}", clip(text, 600)),
                    Role::Test, vec![fix], round + 1);
                self.event("warn", format!("{name}: #{id} found a problem. Filed #{fix} to fix it."));
            } else {
                self.event("ok", format!("{name} finished #{id} {title}."));
            }
        } else {
            task.attempts += 1;
            task.owner = None;
            task.version += 1;
            let (title, attempts) = (task.title.clone(), task.attempts);
            if attempts >= MAX_ATTEMPTS {
                task.state = "failed";
                task.result = Some(clip(text, 2000));
                self.event("fail", format!("#{id} {title} failed {attempts} times and was given up: {}", clip(text, 160)));
            } else {
                task.state = "open";
                self.event("warn", format!("{name} could not finish #{id}: {}. It is open again.", clip(text, 160)));
            }
        }
        true
    }

    /// Reopens every claim whose lease ran out while its owner was unreachable, and counts each
    /// agent's unreachable time.
    fn sweep(&mut self, elapsed: i64) {
        let time = now();
        for member in self.members.iter_mut().filter(|member| member.role != Role::Lead) {
            if !member.link_up || time - member.last_seen > UNREACHABLE_MS { member.unreachable_ms += elapsed; }
        }
        let expired: Vec<(u32, String)> = self.tasks.iter()
            .filter(|task| task.state == "claimed" && task.lease_until.is_some_and(|until| until < time))
            .map(|task| (task.id, task.owner.clone().unwrap_or_default())).collect();
        for (id, owner) in expired {
            if let Some(task) = self.tasks.iter_mut().find(|task| task.id == id) {
                task.state = "open";
                task.owner = None;
                task.lease_until = None;
                task.version += 1;
            }
            self.event("warn", format!("{owner}'s lease on #{id} ran out while it was unreachable. #{id} is open for another agent."));
        }
    }

    /// Delivers what `name` queued while its link was down.
    fn replay(&mut self, name: &str) {
        let queued = self.member(name).map(|member| std::mem::take(&mut member.queued)).unwrap_or_default();
        for item in queued {
            if self.finish(name, item.task, item.version, item.ok, &item.text) {
                self.event("ok", format!("{name} replayed its queued result for #{}. The ledger accepted it.", item.task));
            } else {
                self.event("ok", format!("{name}'s late result for #{} was not applied: the task had moved on without it. Nothing was duplicated.", item.task));
            }
        }
    }

    /// True when no task is waiting or running.
    fn settled(&self) -> bool {
        !self.tasks.is_empty() && self.tasks.iter().all(|task| task.state == "done" || task.state == "failed")
            && self.members.iter().all(|member| member.task.is_none() && member.queued.is_empty())
    }
}

impl Swarm {
    /// Runs `change` on the state.
    fn with<T>(&self, change: impl FnOnce(&mut Ledger) -> T) -> T {
        change(&mut self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    /// The swarm as the page and other tools read it.
    fn snapshot(&self) -> Value {
        let time = now();
        self.with(|state| {
            let members: Vec<Value> = state.members.iter().map(|member| {
                let reachable = member.role == Role::Lead || (member.link_up && time - member.last_seen <= UNREACHABLE_MS);
                let status = if !member.link_up && member.task.is_some() {
                    format!("Link down · still working on #{} offline", member.task.unwrap_or_default())
                } else if !member.link_up && !member.queued.is_empty() {
                    format!("Link down · {} result{} queued", member.queued.len(), if member.queued.len() == 1 { "" } else { "s" })
                } else if !member.link_up { "Link down · waiting to reconnect".into() } else { member.status.clone() };
                let lived = (state.finished.unwrap_or(time) - member.joined).max(1);
                json!({ "name": member.name, "role": member.role, "model": member.model, "linkUp": member.link_up,
                    "reachable": reachable, "lastSeen": member.last_seen, "status": status, "task": member.task,
                    "thread": member.thread, "queued": member.queued.len(), "done": member.done,
                    "uptime": ((lived - member.unreachable_ms.min(lived)) as f64 / lived as f64 * 1000.0).round() / 10.0 })
            }).collect();
            let done = state.tasks.iter().filter(|task| task.state == "done").count();
            json!({ "id": self.id, "goal": self.goal, "workspace": self.workspace, "created": self.created, "unsandboxed": self.unsandboxed,
                "projectId": self.project,
                "finished": state.finished, "status": state.status, "tasks": state.tasks, "members": members,
                "events": state.events, "counts": { "done": done, "total": state.tasks.len() } })
        })
    }
}

/// Opens one agent conversation in the shared folder, sends `prompt` and waits for its answer,
/// renewing `name`'s lease while it works.
async fn turn(backend: &Shared, swarm: &Arc<Swarm>, name: &str, title: &str, model: &str, prompt: &str) -> std::result::Result<String, String> {
    let words = |error: Error| error.2.to_owned();
    let mut capabilities = std::collections::BTreeMap::new();
    capabilities.insert("full_access".to_owned(), true);
    let role = swarm.with(|state| state.member(name).map(|member| member.role)).unwrap_or(Role::Dev);
    let skills = swarm.skills.iter().filter(|(owner, _)| *owner == role).map(|(_, id)| *id).collect::<Vec<_>>();
    let skill_selection = (!skills.is_empty()).then(|| skill_groups::Selection { skill_ids: skills, skill_group_ids: Vec::new(), revision: 0 });
    let created = create_chat(backend.clone(), Create { capabilities, skill_selection, project_id: swarm.project, agent_id: None,
        model: Some(model.to_owned()), effort: None, speed: None, computer_use: false,
        environment: Some(Assignment::Host {}.choice()), delegated: true }).await.map_err(words)?;
    let thread = created.1.0["thread"]["id"].as_str().ok_or("The agent's conversation could not be created.")?.to_owned();
    threads().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(thread.clone());
    // The conversation is titled by its agent, so the swarm's list reads "Dev-1 · #2 Web page".
    let _ = name_chat(State(backend.clone()), Path(thread.clone()), Ok(Json(Named { name: clip(title, 80) }))).await;
    swarm.with(|state| if let Some(member) = state.member(name) { member.thread = Some(thread.clone()); });
    // The swarm's project owns the folder. Without a project (it could not be made), the agent's
    // own conversation is granted it.
    if swarm.project.is_none() && let Ok(id) = Uuid::parse_str(&thread) {
        for action in ["read", "write"] {
            backend.agents.set_permission(Scope::Conversation(id), &Target::Folder { path: swarm.workspace.clone(), action: action.into() },
                Some(Effect::Allow)).await.map_err(|_| "The shared folder could not be granted.")?;
        }
    }
    if swarm.unsandboxed {
        let context = backend.agents.permission_context(thread.clone()).await.map_err(|_| "The agent's permissions could not be read.")?;
        backend.shell_grants.lock().await.insert(thread.clone(), crate::shell_access::Grant { id: Uuid::new_v4(), context,
            expires_at: i64::MAX, swarm: true });
    }
    let outcome = answer(backend, swarm, name, &thread, prompt).await;
    backend.shell_grants.lock().await.remove(&thread);
    outcome
}

/// Sends the prompt in the agent's conversation and waits for the answer, renewing the lease.
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
            ("completed", None) => Err("The agent ended without an answer".into()),
            (status, _) => Err(format!("the provider ended the turn as {status}")),
        };
    }
}

/// What the lead is asked to plan.
fn plan_prompt(goal: &str, workspace: &str, devs: usize, reviews: usize, tests: usize) -> String {
    let review = if reviews > 0 { format!(" {reviews} code review agents review every finished development task by themselves, so do not plan review tasks.") } else { String::new() };
    format!("You lead a swarm of {devs} development agents and {tests} testing agents.{review} They work in parallel in one shared folder, {workspace}, and never talk to each other: each sees only the goal and its own task.\n\nShared goal: {goal}\n\nDo not change any files yourself. Split the goal into 2 to 6 tasks. Development tasks (kind \"dev\") build things; testing tasks (kind \"test\") write and run tests for finished development tasks. Make the tasks independent where you can so agents can work at the same time, and name the files each task owns so agents do not overwrite each other.\n\nReply with JSON only, no other text, in exactly this shape:\n{{\"tasks\":[{{\"title\":\"short title\",\"kind\":\"dev\",\"detail\":\"what to do, which files\",\"after\":[]}}]}}\n`after` lists the 1-based numbers of tasks that must be finished first.")
}

/// What a worker is asked to do for one task.
fn task_prompt(swarm: &Swarm, name: &str, task: &Task) -> String {
    let tools = if swarm.unsandboxed {
        "Work with your miniclaw tool. run_shell runs outside the sandbox as the user. If the file actions are unavailable, create and change files with run_shell."
    } else { "Work with your miniclaw tool; there is no network." };
    let base = format!("You are {name}, one agent in a swarm. Shared goal: {}\n\nShared folder: {} . Use it as the cwd for every command. Other agents work in the same folder at the same time, so change only what your task needs. {tools}\n\nYour task #{}: {}\n{}\n\n",
        swarm.goal, swarm.workspace, task.id, task.title, task.detail);
    if task.kind == Role::Test {
        format!("{base}Write or run the tests this task needs, and run them. Your reply's first line must start with PASS: or FAIL: followed by one sentence; then a short summary.")
    } else if task.kind == Role::Review {
        format!("{base}You are a code reviewer: read the files this change touched with list_folder, search and read_file, and do not change any file. Look for bugs, unhandled errors and anything that breaks the goal. Your reply's first line must start with APPROVE: or CHANGES: followed by one sentence; for CHANGES, then list at most five specific fixes with file names.")
    } else {
        format!("{base}When the task is done, reply with two or three sentences on what you changed.")
    }
}

/// Reads the lead's plan; anything unreadable becomes one task per role.
fn read_plan(answer: &str, goal: &str, tests: usize) -> Vec<(String, String, Role, Vec<u32>)> {
    let parsed = answer.find('{').zip(answer.rfind('}')).filter(|(start, end)| start < end)
        .and_then(|(start, end)| serde_json::from_str::<Value>(&answer[start..=end]).ok());
    let mut plan: Vec<(String, String, Role, Vec<u32>)> = parsed.as_ref().and_then(|value| value["tasks"].as_array()).map(|tasks| tasks.iter().take(8)
        .filter_map(|task| {
            let title = clip(task["title"].as_str()?, 80);
            let kind = if task["kind"] == "test" && tests > 0 { Role::Test } else { Role::Dev };
            let after = task["after"].as_array().map(|list| list.iter().filter_map(|n| n.as_u64()).map(|n| n as u32).collect()).unwrap_or_default();
            (!title.is_empty()).then(|| (title, clip(task["detail"].as_str().unwrap_or(""), 1500), kind, after))
        }).collect()).unwrap_or_default();
    if plan.is_empty() {
        plan.push(("Build it".into(), goal.to_owned(), Role::Dev, Vec::new()));
        if tests > 0 { plan.push(("Test it".into(), format!("Test the work for: {goal}"), Role::Test, vec![1])); }
    }
    plan
}

/// Plans the swarm, starts its agents and the sweeper.
async fn run(backend: Shared, swarm: Arc<Swarm>, lead_model: String, team: Vec<(String, Role, String)>) {
    let count = |role| team.iter().filter(|member| member.1 == role).count();
    let (devs, reviews, tests) = (count(Role::Dev), count(Role::Review), count(Role::Test));
    let prompt = plan_prompt(&swarm.goal, &swarm.workspace, devs, reviews, tests);
    let answer = turn(&backend, &swarm, "Lead", "Lead · plan", &lead_model, &prompt).await;
    let plan = match &answer {
        Ok(text) => read_plan(text, &swarm.goal, tests),
        Err(reason) => {
            swarm.with(|state| state.event("warn", format!("The lead could not plan ({reason}). Starting with one task per role.")));
            read_plan("", &swarm.goal, tests)
        }
    };
    swarm.with(|state| {
        let first = state.next_id;
        for (title, detail, kind, after) in plan {
            let after = after.into_iter().filter(|n| *n >= 1).map(|n| first + n - 1).collect();
            state.add(title, detail, kind, after, 0);
        }
        state.status = "running";
        if let Some(lead) = state.member("Lead") { lead.status = "Keeping the ledger".into(); }
        let count = state.tasks.len();
        state.event("info", format!("The lead split the goal into {count} tasks on the shared ledger."));
    });
    for (name, role, model) in team {
        tokio::spawn(worker(backend.clone(), swarm.clone(), name, role, model));
    }
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut written = String::new();
    loop {
        tick.tick().await;
        let snapshot = swarm.snapshot();
        let key = format!("{}:{}:{}", snapshot["status"], snapshot["events"].as_array().map_or(0, Vec::len), snapshot["counts"]["done"]);
        if key != written { save(&snapshot); written = key; }
        let over = swarm.with(|state| {
            state.sweep(1000);
            if swarm.stopped.load(Ordering::SeqCst) {
                state.status = "stopped";
                state.finished.get_or_insert(now());
                return true;
            }
            if state.settled() {
                let failed = state.tasks.iter().filter(|task| task.state == "failed").count();
                state.status = if failed == 0 { "done" } else { "failed" };
                state.finished = Some(now());
                for member in &mut state.members { member.status = "Done".into(); }
                let done = state.tasks.len() - failed;
                state.event("done", if failed == 0 { format!("Goal met: all {done} tasks are done.") }
                    else { format!("Finished with {failed} task{} given up and {done} done.", if failed == 1 { "" } else { "s" }) });
                return true;
            }
            false
        });
        if over { save(&swarm.snapshot()); return; }
    }
}

/// Where finished and running swarms are kept, so the sidebar still lists them after a restart.
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
/// interrupted: its agents ended with the backend.
fn saved() -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(saved_folder()) else { return Vec::new() };
    let mut all: Vec<Value> = entries.flatten().filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| serde_json::from_slice::<Value>(&std::fs::read(entry.path()).ok()?).ok())
        .filter(|snapshot| snapshot["id"].as_str().is_some_and(|id| id.chars().all(|c| c.is_ascii_hexdigit())))
        .map(|mut snapshot| {
            if matches!(snapshot["status"].as_str(), Some("planning" | "running")) { snapshot["status"] = json!("interrupted"); }
            snapshot
        }).collect();
    all.sort_by_key(|snapshot| std::cmp::Reverse(snapshot["created"].as_i64().unwrap_or(0)));
    all
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
/// a user's edit of one is kept. Returns each skill's role and id; a skill that cannot be saved is
/// left out.
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

/// One development or testing agent: claims, works, reports, and queues what it cannot report.
async fn worker(backend: Shared, swarm: Arc<Swarm>, name: String, role: Role, model: String) {
    loop {
        if swarm.stopped.load(Ordering::SeqCst) {
            swarm.with(|state| if let Some(member) = state.member(&name) { member.status = "Stopped".into(); member.task = None; });
            return;
        }
        let claimed = swarm.with(|state| {
            if state.status == "done" || state.status == "failed" { return Err(()); }
            let up = state.member(&name).is_some_and(|member| member.link_up);
            if !up { return Ok(None); }
            state.beat(&name);
            let task = state.claim(&name, role);
            if task.is_none() && let Some(member) = state.member(&name) { member.status = "Waiting for a task".into(); }
            Ok(task)
        });
        let task = match claimed {
            Err(()) => {
                swarm.with(|state| if let Some(member) = state.member(&name) { member.status = "Done".into(); });
                return;
            }
            Ok(None) => { tokio::time::sleep(Duration::from_secs(2)).await; continue; }
            Ok(Some(task)) => task,
        };
        let prompt = task_prompt(&swarm, &name, &task);
        let outcome = turn(&backend, &swarm, &name, &format!("{name} · #{} {}", task.id, task.title), &model, &prompt).await;
        let (ok, text) = match outcome { Ok(text) => (true, text), Err(reason) => (false, reason) };
        swarm.with(|state| {
            let up = state.member(&name).is_some_and(|member| member.link_up);
            if up {
                if !state.finish(&name, task.id, task.version, ok, &text) {
                    state.event("ok", format!("{name}'s result for #{} came too late: the task had moved on. Nothing was duplicated.", task.id));
                }
            } else if let Some(member) = state.member(&name) {
                member.queued.push(Queued { task: task.id, version: task.version, ok, text });
                state.event("warn", format!("{name} finished #{} while its link was down. The result is queued until it reconnects.", task.id));
            }
            if let Some(member) = state.member(&name) { member.task = None; }
        });
    }
}

/// One team of agents in a start request.
#[derive(Deserialize)]
pub(super) struct Team { count: usize, model: String }

/// A request to start a swarm.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Start { goal: String, lead_model: Option<String>, dev: Team, test: Team, #[serde(default)] review: Option<Team>,
    #[serde(default)] unsandboxed: bool }

/// Starts a swarm and answers with its first snapshot.
pub(super) async fn start(State(backend): State<Shared>, input: Body<Start>) -> Result<(StatusCode, Json<Value>)> {
    authorized(&backend).await?;
    let input = body(input)?;
    let goal = input.goal.trim().to_owned();
    let model_ok = |model: &str| !model.is_empty() && model.len() <= 100 && model.chars().all(|c| c.is_ascii_alphanumeric() || "-._".contains(c));
    let review = input.review.as_ref().filter(|team| team.count > 0);
    if goal.is_empty() || goal.chars().count() > 4000 || !(1..=6).contains(&input.dev.count) || input.test.count > 4
        || review.is_some_and(|team| team.count > 4 || !model_ok(&team.model))
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
    // Each swarm is a project: the folder is its workspace, and every agent conversation is in it.
    let project = super::save_project(&backend, None, crate::projects::Definition { name: clip(&goal, 60), agent_ids: Vec::new(),
        skill_ids: Vec::new(), skill_group_ids: Vec::new(), environment: Some(crate::environment::ProjectChoice::Host {}),
        workspace: Some(workspace.clone()), instructions: format!("Swarm goal: {goal}"), revision: None }).await.ok()
        .and_then(|saved| saved.0["project"]["id"].as_str().and_then(|id| Uuid::parse_str(id).ok()));
    let skills = role_skills(&backend).await;
    let lead_model = input.lead_model.clone().unwrap_or_else(|| input.dev.model.clone());
    let time = now();
    let member = |name: String, role: Role, model: String| Member { name, role, model, link_up: true, last_seen: time,
        status: if role == Role::Lead { "Planning the tasks".into() } else { "Waiting for the plan".into() },
        task: None, thread: None, queued: Vec::new(), done: 0, joined: time, unreachable_ms: 0 };
    let mut team = Vec::new();
    for n in 1..=input.dev.count { team.push((format!("Dev-{n}"), Role::Dev, input.dev.model.clone())); }
    if let Some(review) = review {
        for n in 1..=review.count { team.push((format!("Review-{n}"), Role::Review, review.model.clone())); }
    }
    for n in 1..=input.test.count { team.push((format!("Test-{n}"), Role::Test, input.test.model.clone())); }
    let mut members = vec![member("Lead".into(), Role::Lead, lead_model.clone())];
    members.extend(team.iter().map(|(name, role, model)| member(name.clone(), *role, model.clone())));
    let swarm = Arc::new(Swarm { id, goal: goal.clone(), workspace, unsandboxed: input.unsandboxed, project, skills, created: time, stopped: AtomicBool::new(false),
        state: StdMutex::new(Ledger { status: "planning", tasks: Vec::new(), members, events: Vec::new(), next_id: 1, finished: None, reviewers: review.is_some() }) });
    swarm.with(|state| state.event("info", format!("Swarm started with {} agents in {}.", team.len() + 1, swarm.workspace)));
    if swarm.project.is_none() {
        swarm.with(|state| state.event("warn", "The swarm's project could not be made; its agents are granted the folder one by one.".into()));
    }
    if swarm.unsandboxed {
        swarm.with(|state| state.event("warn", "Unsandboxed shell is on: agents' commands run as you, outside the sandbox, with your files and network.".into()));
    }
    swarms().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(swarm.clone());
    tokio::spawn(run(backend.clone(), swarm.clone(), lead_model, team));
    Ok((StatusCode::CREATED, Json(swarm.snapshot())))
}

/// Lists this run's swarms, newest first.
pub(super) async fn list(State(backend): State<Shared>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    let live: Vec<Arc<Swarm>> = swarms().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).iter().rev().cloned().collect();
    let mut all: Vec<Value> = live.iter().map(|swarm| {
        let (status, done, total) = swarm.with(|state| (state.status, state.tasks.iter().filter(|task| task.state == "done").count(), state.tasks.len()));
        json!({ "id": swarm.id, "goal": swarm.goal, "status": status, "created": swarm.created, "projectId": swarm.project,
            "counts": { "done": done, "total": total } })
    }).collect();
    for snapshot in saved() {
        if live.iter().any(|swarm| snapshot["id"] == swarm.id.as_str()) { continue; }
        all.push(json!({ "id": snapshot["id"], "goal": snapshot["goal"], "status": snapshot["status"], "created": snapshot["created"],
            "projectId": snapshot["projectId"], "counts": snapshot["counts"] }));
    }
    Ok(Json(json!({ "swarms": all })))
}

/// Answers one swarm's full snapshot.
pub(super) async fn view(State(backend): State<Shared>, Path(id): Path<String>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    if let Some(swarm) = find(&id) { return Ok(Json(swarm.snapshot())); }
    saved().into_iter().find(|snapshot| snapshot["id"] == id.as_str()).map(Json).ok_or_else(missing)
}

/// Cuts or restores one agent's link to the ledger.
#[derive(Deserialize)]
pub(super) struct Link { agent: String, up: bool }

/// Cuts or restores an agent's link; a restored agent replays what it queued.
pub(super) async fn link(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Link>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    let input = body(input)?;
    let swarm = find(&id).ok_or_else(missing)?;
    let known = swarm.with(|state| {
        let Some(member) = state.member(&input.agent).filter(|member| member.role != Role::Lead) else { return false };
        if member.link_up == input.up { return true; }
        member.link_up = input.up;
        if input.up {
            member.last_seen = now();
            let name = input.agent.clone();
            state.event("ok", format!("Link to {name} restored."));
            state.beat(&name);
            state.replay(&name);
        } else {
            state.event("warn", format!("Link to {} cut. It keeps working offline; no one waits for it.", input.agent));
        }
        true
    });
    if !known { return Err(invalid()); }
    Ok(Json(swarm.snapshot()))
}

/// A task the user adds while the swarm runs.
#[derive(Deserialize)]
pub(super) struct Add { text: String, #[serde(default)] kind: Option<Role> }

/// Adds the user's task to the ledger, where the next free agent claims it.
pub(super) async fn add(State(backend): State<Shared>, Path(id): Path<String>, input: Body<Add>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    let input = body(input)?;
    let text = input.text.trim().to_owned();
    if text.is_empty() || text.chars().count() > 2000 || input.kind == Some(Role::Lead) { return Err(invalid()); }
    let swarm = find(&id).ok_or_else(missing)?;
    let added = swarm.with(|state| {
        if state.status != "running" { return None; }
        let kind = input.kind.unwrap_or(Role::Dev);
        let id = state.add(clip(&text, 80), text.clone(), kind, Vec::new(), 0);
        state.event("info", format!("You added #{id} {}.", clip(&text, 80)));
        Some(id)
    });
    if added.is_none() { return Err(invalid()); }
    Ok(Json(swarm.snapshot()))
}

/// Stops a swarm: running turns are interrupted and no task is claimed again.
pub(super) async fn stop(State(backend): State<Shared>, Path(id): Path<String>) -> Result<Json<Value>> {
    authorized(&backend).await?;
    let swarm = find(&id).ok_or_else(missing)?;
    if !swarm.stopped.swap(true, Ordering::SeqCst) { swarm.with(|state| state.event("warn", "You stopped the swarm.".into())); }
    Ok(Json(swarm.snapshot()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ledger with one dev task, one test task after it, and two agents.
    fn ledger() -> Ledger {
        let time = now();
        let member = |name: &str, role| Member { name: name.into(), role, model: "m".into(), link_up: true, last_seen: time, status: String::new(),
            task: None, thread: None, queued: Vec::new(), done: 0, joined: time, unreachable_ms: 0 };
        let mut state = Ledger { status: "running", tasks: Vec::new(), members: vec![member("Dev-1", Role::Dev), member("Dev-2", Role::Dev), member("Test-1", Role::Test)],
            events: Vec::new(), next_id: 1, finished: None, reviewers: false };
        state.add("Build".into(), String::new(), Role::Dev, Vec::new(), 0);
        state.add("Test".into(), String::new(), Role::Test, vec![1], 0);
        state
    }

    /// A test task waits for its prerequisite, and a task is never claimed twice.
    #[test]
    fn claims_follow_roles_order_and_ownership() {
        let mut state = ledger();
        assert!(state.claim("Test-1", Role::Test).is_none());
        let task = state.claim("Dev-1", Role::Dev).unwrap();
        assert!(state.claim("Dev-2", Role::Dev).is_none());
        assert!(state.finish("Dev-1", task.id, task.version, true, "done"));
        assert_eq!(state.claim("Test-1", Role::Test).unwrap().id, 2);
    }

    /// A lease that runs out while the owner is cut off reopens the task; the owner's queued result
    /// for the old claim is then refused, and the new owner's result counts.
    #[test]
    fn an_expired_lease_moves_the_task_and_a_late_result_is_not_applied() {
        let mut state = ledger();
        let first = state.claim("Dev-1", Role::Dev).unwrap();
        state.member("Dev-1").unwrap().link_up = false;
        state.tasks[0].lease_until = Some(now() - 1);
        state.sweep(1000);
        assert_eq!(state.tasks[0].state, "open");
        let second = state.claim("Dev-2", Role::Dev).unwrap();
        state.member("Dev-1").unwrap().queued.push(Queued { task: first.id, version: first.version, ok: true, text: "late".into() });
        state.member("Dev-1").unwrap().link_up = true;
        state.replay("Dev-1");
        assert_eq!(state.tasks[0].state, "claimed");
        assert!(state.finish("Dev-2", second.id, second.version, true, "done"));
        assert_eq!(state.tasks[0].result.as_deref(), Some("done"));
    }

    /// A queued result whose lease still held when the link came back is applied on replay.
    #[test]
    fn a_result_queued_within_the_lease_is_applied_on_replay() {
        let mut state = ledger();
        let task = state.claim("Dev-1", Role::Dev).unwrap();
        state.member("Dev-1").unwrap().queued.push(Queued { task: task.id, version: task.version, ok: true, text: "built".into() });
        state.replay("Dev-1");
        assert_eq!(state.tasks[0].state, "done");
    }

    /// A failing test files a fix and a re-test after it.
    #[test]
    fn a_failed_test_files_a_fix_and_a_retest() {
        let mut state = ledger();
        let build = state.claim("Dev-1", Role::Dev).unwrap();
        state.finish("Dev-1", build.id, build.version, true, "ok");
        let test = state.claim("Test-1", Role::Test).unwrap();
        state.finish("Test-1", test.id, test.version, true, "FAIL: the copy button does nothing\nmore");
        assert_eq!(state.tasks.len(), 4);
        assert_eq!(state.tasks[2].title, "Fix: the copy button does nothing");
        assert_eq!(state.tasks[3].after, vec![3]);
    }

    /// With reviewers, a finished development task files a review; a review that asks for changes
    /// files a fix, whose own completion is reviewed again.
    #[test]
    fn a_finished_change_is_reviewed_and_requested_changes_are_fixed() {
        let mut state = ledger();
        state.reviewers = true;
        let build = state.claim("Dev-1", Role::Dev).unwrap();
        state.finish("Dev-1", build.id, build.version, true, "built it");
        assert_eq!((state.tasks[2].kind, state.tasks[2].title.as_str()), (Role::Review, "Review: Build"));
        state.members.push(Member { name: "Review-1".into(), role: Role::Review, model: "m".into(), link_up: true, last_seen: now(), status: String::new(),
            task: None, thread: None, queued: Vec::new(), done: 0, joined: now(), unreachable_ms: 0 });
        let review = state.claim("Review-1", Role::Review).unwrap();
        state.finish("Review-1", review.id, review.version, true, "**CHANGES:** the error is swallowed\n- main.py: raise it");
        let fix = state.tasks.last().unwrap().clone();
        assert_eq!((fix.kind, fix.title.as_str(), fix.fix_round), (Role::Dev, "Fix: the error is swallowed", 1));
        let fixing = state.claim("Dev-2", Role::Dev).unwrap();
        state.finish("Dev-2", fixing.id, fixing.version, true, "fixed");
        assert_eq!(state.tasks.last().unwrap().kind, Role::Review);
    }

    /// A skill's description comes from its front matter.
    #[test]
    fn a_skill_description_is_read_from_its_front_matter() {
        assert_eq!(skill_description("---\nname: x\ndescription: Reviews code.\nmodel: opus\n---\nBody"), "Reviews code.");
        assert!(SKILLS.iter().all(|(_, _, text)| !skill_description(text).is_empty() && text.len() <= 16384));
    }

    /// The lead's JSON is read from its reply; a reply without it still yields work.
    #[test]
    fn the_plan_is_read_from_the_leads_reply() {
        let plan = read_plan("Here:\n{\"tasks\":[{\"title\":\"API\",\"kind\":\"dev\",\"detail\":\"x\",\"after\":[]},{\"title\":\"Tests\",\"kind\":\"test\",\"after\":[1]}]}", "goal", 1);
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[1].2, Role::Test);
        assert_eq!(plan[1].3, vec![1]);
        assert_eq!(read_plan("no json", "goal", 0).len(), 1);
    }
}
