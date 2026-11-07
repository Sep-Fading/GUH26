// Reuses the private metadata connection and errors.
use crate::agents::{Issue, Store};
// Interprets the conversation's pinned worker identity.
use crate::environment::Assignment;
// Persists exact rules and session expiry atomically.
use rusqlite::{Connection, OptionalExtension, params};
// Encodes reviewed targets without lossy normalization.
use serde::{Deserialize, Serialize};
// Indexes targets and validates absolute paths.
use std::{collections::BTreeMap, path::Path};
// Identifies agents, projects, and conversations exactly.
use uuid::Uuid;

const SESSION_SECONDS: i64 = 8 * 60 * 60;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct VettedApp {
    pub id: &'static str,
    pub name: &'static str,
    pub code_requirement: &'static str,
}

const VETTED_APPS: [VettedApp; 1] = [VettedApp {
    id: "com.apple.TextEdit",
    name: "TextEdit",
    code_requirement: "anchor apple and identifier = \"com.apple.TextEdit\"",
}];

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AppRequest {
    pub name: String,
    pub status: &'static str,
}

/// Lists reviewed app identities; native dispatch must also verify the running app.
pub fn vetted_applications() -> &'static [VettedApp] { &VETTED_APPS }

/// Matches a bundle identifier only against the reviewed catalog.
fn vetted_application(id: &str) -> bool {
    VETTED_APPS.iter().any(|app| app.id == id)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    conversation: Uuid,
    agent: Option<Uuid>,
    project: Option<Uuid>,
    machine: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "target_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    Application { id: String, action: String },
    File { path: String, action: String },
    // Covers every file beneath the path; an exact file rule still takes precedence.
    Folder { path: String, action: String },
    Command { kind: String, text: String, cwd: String },
    // A program by name. Allowed programs make a plain command line run without asking; the
    // line's shape is judged by `harmless::programs`, so no rule ever covers a redirect or a trick.
    Program { name: String },
    // One exact DNS name a shell command may reach over HTTPS.
    Host { host: String },
    // A domain and every name below it; an exact host rule still takes precedence.
    Domain { domain: String },
    // A switch, not a resource: allow turns a capability on for its scope, a conversation
    // block turns it off again. No rule means off. It is never asked about in a popup.
    Feature { name: String },
    // A switch for one saved MCP server, named by its id so a later server of the same name
    // inherits nothing. No rule means off, and it is never asked about in a popup.
    McpServer { server: String },
    // One tool of one MCP server. It asks, like a command.
    McpTool { server: String, tool: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Agent(Uuid),
    Project(Uuid),
    Conversation(Uuid),
    Session,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Allow,
    Block,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow(Scope),
    Block,
    Ask,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Rule {
    pub target: Target,
    pub effect: Effect,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRule {
    pub target: Target,
    pub expires_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Effective {
    pub target: Target,
    pub decision: &'static str,
    pub source: &'static str,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Sources {
    pub agent: Vec<Rule>,
    pub project: Vec<Rule>,
    pub conversation: Vec<Rule>,
    pub session: Vec<SessionRule>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub sources: Sources,
    pub effective: Vec<Effective>,
}

impl Context {
    /// Returns the verified conversation identifier for settings and prompts.
    pub fn conversation(&self) -> Uuid { self.conversation }

    /// Returns the bound agent, if this conversation has one.
    pub fn agent(&self) -> Option<Uuid> { self.agent }

    /// Returns the bound project, if this conversation has one.
    pub fn project(&self) -> Option<Uuid> { self.project }

    /// Returns the pinned machine identity used by approvals.
    pub fn machine(&self) -> &str { &self.machine }

    /// Rejects missing or oversized machine identities before lookup.
    fn validate(&self) -> Result<(), Issue> {
        if self.machine.is_empty() || self.machine.len() > 128
            || !self.machine.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        {
            return Err(Issue::Invalid);
        }
        Ok(())
    }
}

impl Target {
    /// Checks exact target shape without granting access to its path.
    fn shape_key(&self) -> Result<String, Issue> {
        let valid = match self {
            Self::Application { id, action } => {
                !id.is_empty() && id.len() <= 255 && id.contains('.')
                    && id.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
                    && matches!(action.as_str(), "observe" | "control")
            }
            Self::File { path, action } | Self::Folder { path, action } => {
                plain_absolute_path(path) && path.len() <= 4096
                    && matches!(action.as_str(), "read" | "write")
            }
            Self::Command { kind, text, cwd } => {
                matches!(kind.as_str(), "shell" | "applescript")
                    && !text.is_empty() && text.len() <= 16384 && !text.contains('\0')
                    && plain_absolute_path(cwd) && cwd.len() <= 4096
            }
            Self::Program { name } => !name.is_empty() && name.len() <= 64
                && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-')),
            Self::Host { host } => dns_name(host),
            Self::Domain { domain } => dns_name(domain) && !shared_suffix(domain),
            Self::Feature { name } => matches!(name.as_str(),
                "unsandboxed_shell" | "network" | "web_fetch" | "subagents" | "task_tracking" | "image_generation" | "plan_mode" | "coding_session"
                // Web search is on unless this switch is: an allow turns search OFF for its scope,
                // and a conversation block turns it back on there. That keeps "no rule" as the default.
                | "no_web_search"
                // Asking the user questions is on unless this switch is set, the same way round.
                | "no_questions"
                // A project's shared memory is read and written unless this switch is set.
                | "no_project_memory"
                // A read-only command runs without asking unless this switch is set.
                | "ask_commands"
                // Full access: nothing the agent is allowed is asked about, for one conversation.
                | "full_access"
                // An agent default copied only when creating a conversation.
                | "default_full_access"),
            Self::McpServer { server } => Uuid::parse_str(server).is_ok(),
            Self::McpTool { server, tool } => Uuid::parse_str(server).is_ok()
                && !tool.is_empty() && tool.len() <= 128 && !tool.chars().any(char::is_control),
        };
        if !valid { return Err(Issue::Invalid); }
        serde_json::to_string(self).map_err(Issue::storage)
    }

    /// Canonicalizes Windows aliases before storing or deciding path permissions.
    pub(crate) fn key(&self) -> Result<String, Issue> {
        let raw = self.shape_key()?;
        #[cfg(windows)] {
            let mut normalized = self.clone();
            let path = match &mut normalized {
                Self::File { path, .. } | Self::Folder { path, .. } => Some(path),
                Self::Command { cwd, .. } => Some(cwd),
                _ => None,
            };
            if let Some(path) = path {
                *path = crate::platform_fs::permission_path(Path::new(path)).map_err(Issue::storage)?;
            } else { return Ok(raw); }
            return serde_json::to_string(&normalized).map_err(Issue::storage);
        }
        #[cfg(not(windows))]
        Ok(raw)
    }

    /// Checks a model target without exposing its storage representation.
    pub fn validate(&self) -> Result<(), Issue> { self.key().map(|_| ()) }
}

/// Accepts a plain lowercase DNS name with at least two labels. Addresses, ports, paths,
/// wildcards and single labels such as `localhost` are not host names here.
pub fn dns_name(name: &str) -> bool {
    let labels: Vec<&str> = name.split('.').collect();
    name.len() <= 253 && labels.len() >= 2
        && labels.iter().all(|label| !label.is_empty() && label.len() <= 63
            && !label.starts_with('-') && !label.ends_with('-')
            && label.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'))
        // A name made only of digits and dots is an address, not a host.
        && labels.last().is_some_and(|tld| tld.bytes().any(|c| c.is_ascii_lowercase()))
}

/// Reports a name under which unrelated parties publish, so a rule for it would cover
/// strangers: a public suffix shape such as `co.uk`, or a shared hosting domain. This is a
/// short built-in list, not the full Public Suffix List; the popup always shows the exact
/// domain, so the user sees what a rule would cover.
fn shared_suffix(domain: &str) -> bool {
    const HOSTING: [&str; 15] = ["github.io", "amazonaws.com",
        "cloudfront.net", "azurewebsites.net", "windows.net", "pages.dev", "workers.dev",
        "vercel.app", "netlify.app", "herokuapp.com", "web.app", "firebaseapp.com",
        "appspot.com", "blogspot.com", "fly.dev"];
    let labels: Vec<&str> = domain.split('.').collect();
    let country_shape = labels.len() == 2 && labels[1].len() == 2
        && matches!(labels[0], "co" | "com" | "org" | "net" | "ac" | "gov" | "edu" | "or" | "ne");
    labels.len() < 2 || country_shape || HOSTING.contains(&domain)
}

/// Names the domain one level above a host, when a rule for it would be reasonable.
pub fn parent_domain(host: &str) -> Option<String> {
    let parent = host.split_once('.')?.1;
    (dns_name(parent) && !shared_suffix(parent)).then(|| parent.to_owned())
}

/// Whether `path` names an existing folder in exactly the spelling it resolves to, which is
/// the spelling the sandbox and the file tool match. On Windows `std::fs::canonicalize`
/// answers the verbatim `\\?\C:\...` form, so the comparison uses the policy normalizer
/// that the Windows sandbox applies to grants: the plain drive form of the final path, with
/// links refused. The match stays exact, so case, 8.3, slash and link aliases are refused.
pub fn exact_folder(path: &str) -> bool {
    #[cfg(windows)]
    let real = crate::platform_fs::permission_path(Path::new(path)).ok();
    #[cfg(not(windows))]
    let real = std::fs::canonicalize(path).ok().and_then(|real| real.into_os_string().into_string().ok());
    real.is_some_and(|real| real == path && Path::new(&real).is_dir())
}

/// Rejects lexical path escapes before a native worker resolves the file.
pub(crate) fn plain_absolute_path(path: &str) -> bool {
    #[cfg(windows)] { crate::platform_fs::plain_windows_path(path) }
    #[cfg(not(windows))] {
        Path::new(path).is_absolute() && !path.contains('\0')
            && !path.split('/').any(|part| matches!(part, "." | ".."))
    }
}

impl Scope {
    /// Returns a stable kind and identifier for a lasting rule.
    fn stored(self) -> Result<(&'static str, String), Issue> {
        match self {
            Self::Agent(id) => Ok(("agent", id.to_string())),
            Self::Project(id) => Ok(("project", id.to_string())),
            Self::Conversation(id) => Ok(("conversation", id.to_string())),
            Self::Session => Err(Issue::Invalid),
        }
    }
}

/// Adds one schema generation; absent version-four tables stop reopening.
pub(crate) fn migrate(db: &mut Connection) -> Result<(), Issue> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(Issue::storage)?;
    if version == 5 {
        for table in ["permission_rules", "permission_sessions", "permission_app_requests"] {
            if !db.table_exists(Some("main"), table).map_err(Issue::storage)? {
                return Err(Issue::Storage);
            }
        }
        return retire_web_tools(db);
    }
    if version == 3 {
        let tx = db.transaction().map_err(Issue::storage)?;
        tx.execute_batch("CREATE TABLE permission_rules (
        scope_kind TEXT NOT NULL CHECK(scope_kind IN ('agent', 'project', 'conversation')),
        scope_id TEXT NOT NULL, target TEXT NOT NULL,
        effect TEXT NOT NULL CHECK(effect IN ('allow', 'block')),
        PRIMARY KEY(scope_kind, scope_id, target)
    );
    CREATE TABLE permission_sessions (
        conversation TEXT NOT NULL, agent TEXT NOT NULL,
        project TEXT NOT NULL, machine TEXT NOT NULL,
        target TEXT NOT NULL, expires_at INTEGER NOT NULL,
        PRIMARY KEY(conversation, agent, project, machine, target)
    );
    PRAGMA user_version = 4;") .map_err(Issue::storage)?;
        tx.commit().map_err(Issue::storage)?;
    } else if version == 4 {
        for table in ["permission_rules", "permission_sessions"] {
            if !db.table_exists(Some("main"), table).map_err(Issue::storage)? {
                return Err(Issue::Storage);
            }
        }
    } else { return Err(Issue::Storage); }
    let tx = db.transaction().map_err(Issue::storage)?;
    tx.execute_batch("CREATE TABLE permission_app_requests (
        scope_kind TEXT NOT NULL CHECK(scope_kind IN ('agent', 'project', 'conversation')),
        scope_id TEXT NOT NULL, name TEXT NOT NULL COLLATE NOCASE,
        PRIMARY KEY(scope_kind, scope_id, name)
    );
    PRAGMA user_version = 5;").map_err(Issue::storage)?;
    tx.commit().map_err(Issue::storage)?;
    retire_web_tools(db)
}

/// Removes the retired bundle's saved rules without granting any replacement capability.
fn retire_web_tools(db: &mut Connection) -> Result<(), Issue> {
    let target = serde_json::to_string(&Target::Feature { name: "web_tools".into() })
        .map_err(Issue::storage)?;
    let tx = db.transaction().map_err(Issue::storage)?;
    for table in ["permission_rules", "permission_sessions"] {
        tx.execute(&format!("DELETE FROM {table} WHERE target = ?1"), [&target])
            .map_err(Issue::storage)?;
    }
    tx.commit().map_err(Issue::storage)
}

impl Store {
    /// Derives an authorization context from saved chat and machine bindings.
    pub async fn permission_context(&self, thread: String) -> Result<Context, Issue> {
        let conversation = Uuid::parse_str(&thread).map_err(|_| Issue::Invalid)?;
        self.check_creation(thread.clone()).await?;
        self.run(move |db| {
            if crate::agents::removed(db, "chat", &thread)? { return Err(Issue::Missing); }
            let row: Option<(Option<String>, Option<String>, String)> = db.query_row(
                "SELECT a.agent_id, p.project_id, e.assignment
                FROM chat_environments e
                LEFT JOIN agent_chats a ON a.thread_id = e.thread_id
                LEFT JOIN project_chats p ON p.thread_id = e.thread_id
                WHERE e.thread_id = ?1", [&thread],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            ).optional().map_err(Issue::storage)?;
            let Some((agent, project, machine)) = row else { return Err(Issue::Missing); };
            let assignment: Assignment = serde_json::from_str(&machine).map_err(Issue::storage)?;
            if !assignment.valid() { return Err(Issue::Storage); }
            let machine = match assignment {
                Assignment::Host {} => "host".to_owned(),
                Assignment::Vm { vm_id } => vm_id,
            };
            Ok(Context {
                conversation,
                agent: agent.map(|id| Uuid::parse_str(&id)).transpose()
                    .map_err(Issue::storage)?,
                project: project.map(|id| Uuid::parse_str(&id)).transpose()
                    .map_err(Issue::storage)?,
                machine,
            })
        }).await
    }

    /// Confirms a settings scope exists before allowing its rules to change.
    pub async fn permission_scope(&self, scope: Scope) -> Result<(), Issue> {
        let (kind, id) = scope.stored()?;
        if let Scope::Conversation(_) = scope {
            self.permission_context(id).await?;
            return Ok(());
        }
        // The main agent is not a saved agent; its scope always exists.
        if scope == Scope::Agent(crate::main_agent::ID) { return Ok(()); }
        self.run(move |db| {
            if kind == "agent" && crate::agents::removed(db, "agent", &id)? {
                return Err(Issue::Missing);
            }
            let table = if kind == "agent" { "agents" } else { "projects" };
            let exists: bool = db.query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id = ?1)"),
                [&id], |row| row.get(0)).map_err(Issue::storage)?;
            if !exists { return Err(Issue::Missing); }
            Ok(())
        }).await
    }

    /// Lists lasting rules for one verified settings scope.
    pub async fn permission_rules(&self, scope: Scope) -> Result<Vec<Rule>, Issue> {
        self.permission_scope(scope).await?;
        self.run(move |db| rules(db, scope)).await
    }

    /// Shows the effective decision and every lasting source for this chat.
    pub async fn permission_summary(&self, context: &Context, now: i64)
        -> Result<Summary, Issue> {
        context.validate()?;
        if now < 0 { return Err(Issue::Invalid); }
        let context = context.clone();
        self.run(move |db| {
            let sources = Sources {
                agent: rules(db, Scope::Agent(context.agent.unwrap_or(crate::main_agent::ID)))?,
                project: context.project.map(|id| rules(db, Scope::Project(id)))
                    .transpose()?.unwrap_or_default(),
                conversation: rules(db, Scope::Conversation(context.conversation))?,
                session: session_rules(db, &context, now)?,
            };
            let mut targets = BTreeMap::<String, Target>::new();
            for rule in sources.agent.iter().chain(&sources.project)
                .chain(&sources.conversation) {
                targets.insert(rule.target.key()?, rule.target.clone());
            }
            for grant in &sources.session {
                targets.insert(grant.target.key()?, grant.target.clone());
            }
            let mut effective = Vec::new();
            for (key, target) in targets {
                let (decision, source) = match decide(db, &context, &key, now)? {
                    Decision::Allow(Scope::Agent(_)) => ("allow", "agent"),
                    Decision::Allow(Scope::Project(_)) => ("allow", "project"),
                    Decision::Allow(Scope::Conversation(_)) => ("allow", "conversation"),
                    Decision::Allow(Scope::Session) => ("allow", "session"),
                    Decision::Block => ("block", "conversation"),
                    Decision::Ask => ("ask", "none"),
                    Decision::Unavailable => ("unavailable", "catalog"),
                };
                effective.push(Effective { target, decision, source });
            }
            Ok(Summary { sources, effective })
        }).await
    }

    /// Lists the file and folder decisions a host sandbox must enforce right now:
    /// every effective allow, plus every block so it holds inside an allowed folder.
    pub async fn file_grants(&self, context: &Context, now: i64)
        -> Result<Vec<crate::seatbelt::Grant>, Issue> {
        let summary = self.permission_summary(context, now).await?;
        let mut grants: Vec<crate::seatbelt::Grant> = Vec::new();
        for entry in summary.effective {
            let (path, action, folder) = match entry.target {
                Target::File { path, action } => (path, action, false),
                Target::Folder { path, action } => (path, action, true),
                _ => continue,
            };
            let blocked = entry.decision == "block";
            if entry.decision != "allow" && !blocked { continue; }
            let write = action == "write";
            // Read and write rules for one path merge into a single profile entry.
            match grants.iter_mut().find(|grant| grant.path == path
                && grant.folder == folder && grant.blocked == blocked) {
                Some(grant) => grant.write |= write,
                None => grants.push(crate::seatbelt::Grant { path, write, folder, blocked }),
            }
        }
        Ok(grants)
    }

    /// Returns the hosts allowed by rule for a conversation, for a shell the user types into.
    /// Domains are not expanded: a rule for a whole domain still needs a command's own request.
    pub async fn host_grants(&self, context: &Context, now: i64) -> Result<Vec<String>, Issue> {
        let summary = self.permission_summary(context, now).await?;
        Ok(summary.effective.into_iter().filter(|entry| entry.decision == "allow")
            .filter_map(|entry| match entry.target { Target::Host { host } => Some(host), _ => None }).take(64).collect())
    }

    /// Resolves a current exact grant or asks before any scoped operation.
    pub async fn permission(&self, context: &Context, target: &Target,
        now: i64) -> Result<Decision, Issue> {
        context.validate()?;
        let key = target.key()?;
        if now < 0 { return Err(Issue::Invalid); }
        let context = context.clone();
        self.run(move |db| decide(db, &context, &key, now)).await
    }

    /// Reports whether a switch is on by default for a chat that does not exist yet: its
    /// project or its agent allows it. A new chat has no rule of its own.
    pub async fn feature_default(&self, agent: Option<Uuid>, project: Option<Uuid>,
        name: &str) -> Result<bool, Issue> {
        let key = Target::Feature { name: name.to_owned() }.key()?;
        self.run(move |db| {
            let agent = Some(agent.unwrap_or(crate::main_agent::ID));
            for scope in [project.map(Scope::Project), agent.map(Scope::Agent)].into_iter().flatten() {
                if rule(db, scope, &key)? == Some(Effect::Allow) { return Ok(true); }
            }
            Ok(false)
        }).await
    }

    /// Reports whether a switch is set by the configuration a new conversation would have: its
    /// project or its agent, where no agent means the main agent.
    pub async fn configured_switch(&self, agent: Option<Uuid>, project: Option<Uuid>, target: &Target) -> Result<bool, Issue> {
        let key = target.key()?;
        self.run(move |db| Ok(configured(db, agent, project, &key)?.is_some())).await
    }

    /// Saves or removes a lasting settings rule and invalidates matching sessions.
    pub async fn set_permission(&self, scope: Scope, target: &Target,
        effect: Option<Effect>) -> Result<(), Issue> {
        if matches!(target, Target::Feature { name } if name == "default_full_access")
            && !matches!(scope, Scope::Agent(_)) {
            return Err(Issue::Invalid);
        }
        if effect == Some(Effect::Block) && !matches!(scope, Scope::Conversation(_)) {
            return Err(Issue::Invalid);
        }
        // The main agent holds capabilities only: switches and MCP servers, no resource rules.
        if scope == Scope::Agent(crate::main_agent::ID)
            && !matches!(target, Target::Feature { .. } | Target::McpServer { .. } | Target::McpTool { .. }) {
            return Err(Issue::Invalid);
        }
        // A conversation switches a bounded capability on and off by itself; it cannot allow what
        // its agent and project do not. "Off" is the absence of its allow, so a block is stored as none.
        let mut effect = effect;
        if let (Scope::Conversation(conversation), Some(bound)) = (scope, ceiling(target)) {
            let context = self.permission_context(conversation.to_string()).await?;
            let key = target.key()?;
            let set = self.run(move |db| configured(db, context.agent, context.project, &key)).await?;
            effect = match (bound, effect, set) {
                (Ceiling::Needs, Some(Effect::Allow), None) => return Err(Issue::NotAllowed),
                (Ceiling::Needs, Some(Effect::Block), _) => None,
                (Ceiling::Unless, Some(Effect::Block), Some(_)) => return Err(Issue::NotAllowed),
                (Ceiling::Unless, Some(Effect::Block), None) => None,
                (_, other, _) => other,
            };
        }
        let (kind, id) = scope.stored()?;
        let key = if effect.is_none() {
            target.key().or_else(|_| target.shape_key())?
        } else { target.key()? };
        if effect == Some(Effect::Allow)
            && matches!(target, Target::Application { id, .. } if !vetted_application(id)) {
            return Err(Issue::Invalid);
        }
        self.run(move |db| {
            let tx = db.transaction().map_err(Issue::storage)?;
            tx.execute("DELETE FROM permission_rules
                WHERE scope_kind = ?1 AND scope_id = ?2 AND target = ?3",
                params![kind, id, key]).map_err(Issue::storage)?;
            if let Some(effect) = effect {
                let value = if effect == Effect::Allow { "allow" } else { "block" };
                tx.execute("INSERT INTO permission_rules VALUES (?1, ?2, ?3, ?4)",
                    params![kind, id, key, value]).map_err(Issue::storage)?;
            }
            let column = match scope {
                Scope::Agent(_) => "agent", Scope::Project(_) => "project",
                Scope::Conversation(_) => "conversation", Scope::Session => unreachable!(),
            };
            tx.execute(&format!("DELETE FROM permission_sessions WHERE {column} = ?1 AND target = ?2"),
                params![id, key]).map_err(Issue::storage)?;
            tx.commit().map_err(Issue::storage)
        }).await
    }

    /// Removes a settings rule and matching temporary grants in one transaction.
    pub async fn revoke_permission(&self, scope: Scope, target: &Target) -> Result<(), Issue> {
        self.set_permission(scope, target, None).await
    }

    /// Removes every lasting rule and session grant that names one MCP server, when it is deleted.
    pub async fn forget_mcp_server(&self, server: String) -> Result<(), Issue> {
        if Uuid::parse_str(&server).is_err() { return Err(Issue::Invalid); }
        self.run(move |db| {
            // A rule's key is its JSON, and an id is 36 characters no other field can hold by chance.
            let like = format!("%\"server\":\"{server}\"%");
            for table in ["permission_rules", "permission_sessions"] {
                db.execute(&format!("DELETE FROM {table} WHERE target LIKE ?1 AND target LIKE '%\"target_type\":\"mcp_%'"), [&like])
                    .map_err(Issue::storage)?;
            }
            Ok(())
        }).await
    }

    /// Stores one eight-hour grant for a trusted approval response.
    pub async fn approve_session(&self, context: &Context, target: &Target,
        now: i64) -> Result<(), Issue> {
        context.validate()?;
        let key = target.key()?;
        if matches!(target, Target::Application { id, .. } if !vetted_application(id)) {
            return Err(Issue::Invalid);
        }
        let expires = now.checked_add(SESSION_SECONDS).ok_or(Issue::Invalid)?;
        if now < 0 { return Err(Issue::Invalid); }
        let context = context.clone();
        self.run(move |db| {
            db.execute("INSERT INTO permission_sessions
                (conversation, agent, project, machine, target, expires_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(conversation, agent, project, machine, target)
                DO UPDATE SET expires_at = excluded.expires_at",
                params![context.conversation.to_string(),
                    context.agent.map(|id| id.to_string()).unwrap_or_default(),
                    context.project.map(|id| id.to_string()).unwrap_or_default(),
                    context.machine, key, expires]).map_err(Issue::storage)?;
            Ok(())
        }).await
    }

    /// Saves a request for catalog review without creating any permission grant.
    pub async fn request_application(&self, scope: Scope, name: &str) -> Result<(), Issue> {
        self.permission_scope(scope).await?;
        let (kind, id) = scope.stored()?;
        let name = name.trim();
        if name.len() < 2 || name.len() > 120 || name.chars().any(char::is_control) {
            return Err(Issue::Invalid);
        }
        let name = name.to_owned();
        self.run(move |db| {
            let tx = db.transaction().map_err(Issue::storage)?;
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM permission_app_requests
                WHERE scope_kind = ?1 AND scope_id = ?2 AND name = ?3)",
                params![kind, id, name], |row| row.get(0)).map_err(Issue::storage)?;
            if exists { return Ok(()); }
            let count: i64 = tx.query_row("SELECT count(*) FROM permission_app_requests
                WHERE scope_kind = ?1 AND scope_id = ?2", params![kind, id],
                |row| row.get(0)).map_err(Issue::storage)?;
            if count >= 256 { return Err(Issue::Limit); }
            tx.execute("INSERT INTO permission_app_requests VALUES (?1, ?2, ?3)",
                params![kind, id, name]).map_err(Issue::storage)?;
            tx.commit().map_err(Issue::storage)
        }).await
    }

    /// Lists pending app requests for one saved settings scope.
    pub async fn application_requests(&self, scope: Scope) -> Result<Vec<AppRequest>, Issue> {
        self.permission_scope(scope).await?;
        let (kind, id) = scope.stored()?;
        self.run(move |db| {
            let mut query = db.prepare("SELECT name FROM permission_app_requests
                WHERE scope_kind = ?1 AND scope_id = ?2 ORDER BY name LIMIT 257")
                .map_err(Issue::storage)?;
            let rows = query.query_map(params![kind, id], |row| row.get::<_, String>(0))
                .map_err(Issue::storage)?;
            let mut requests = Vec::new();
            for row in rows {
                requests.push(AppRequest { name: row.map_err(Issue::storage)?, status: "pending" });
            }
            if requests.len() > 256 { return Err(Issue::Limit); }
            Ok(requests)
        }).await
    }
}

/// How an agent's or project's configuration bounds a switch. The configuration says whether a
/// capability is allowed at all; the conversation only switches what is allowed on or off.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ceiling {
    /// Allowed only where the agent or the project allows it, and on only where the conversation
    /// itself switches it on: the configuration is the bound, never the switch.
    Needs,
    /// The switch that turns web search off: once the agent or project sets it, the
    /// conversation cannot bring search back.
    Unless,
}

/// Returns the ceiling that applies to a switch. Restrictions (plan mode, coding session) and
/// resource rules have none.
fn ceiling(target: &Target) -> Option<Ceiling> {
    match target {
        Target::Feature { name } if matches!(name.as_str(), "unsandboxed_shell" | "network" | "web_fetch" | "subagents" | "task_tracking" | "image_generation") => Some(Ceiling::Needs),
        Target::McpServer { .. } => Some(Ceiling::Needs),
        Target::Feature { name } if name == "no_web_search" => Some(Ceiling::Unless),
        _ => None,
    }
}

/// Finds the configuration scope that sets a switch: the project first, then the agent, where
/// a conversation with no agent belongs to the main agent.
fn configured(db: &Connection, agent: Option<Uuid>, project: Option<Uuid>, key: &str) -> Result<Option<Scope>, Issue> {
    let agent = agent.unwrap_or(crate::main_agent::ID);
    for scope in [project.map(Scope::Project), Some(Scope::Agent(agent))].into_iter().flatten() {
        if rule(db, scope, key)? == Some(Effect::Allow) { return Ok(Some(scope)); }
    }
    Ok(None)
}

/// Decides one target. A file with no exact decision inherits the nearest enclosing
/// folder rule for the same action.
fn decide(db: &Connection, context: &Context, key: &str, now: i64) -> Result<Decision, Issue> {
    let decision = decide_exact(db, context, key, now)?;
    let target: Target = serde_json::from_str(key).map_err(Issue::storage)?;
    if decision != Decision::Ask { return Ok(decision); }
    // A host inherits from a rule for its own name as a domain, then from each domain above.
    if let Target::Host { host } = &target {
        let mut name = Some(host.as_str());
        while let Some(domain) = name.filter(|domain| dns_name(domain)) {
            let rule = Target::Domain { domain: domain.to_owned() };
            if let Ok(key) = rule.key() {
                let inherited = decide_exact(db, context, &key, now)?;
                if inherited != Decision::Ask { return Ok(inherited); }
            }
            name = domain.split_once('.').map(|(_, parent)| parent);
        }
        return Ok(Decision::Ask);
    }
    let Target::File { path, action } = target else { return Ok(decision); };
    let mut folder = std::path::Path::new(&path).parent();
    while let Some(parent) = folder.filter(|parent| parent.parent().is_some()) {
        let target = Target::Folder { path: parent.to_string_lossy().into_owned(),
            action: action.clone() };
        let inherited = decide_exact(db, context, &target.key()?, now)?;
        if inherited != Decision::Ask { return Ok(inherited); }
        folder = parent.parent();
    }
    Ok(Decision::Ask)
}

/// Applies conversation precedence, then session, project, and agent defaults.
fn decide_exact(db: &Connection, context: &Context, key: &str, now: i64)
    -> Result<Decision, Issue> {
    let target: Target = serde_json::from_str(key).map_err(Issue::storage)?;
    if matches!(&target, Target::Application { id, .. } if !vetted_application(id)) {
        return Ok(Decision::Unavailable);
    }
    if let Some(ceiling) = ceiling(&target) {
        let set = configured(db, context.agent, context.project, key)?;
        let own = rule(db, Scope::Conversation(context.conversation), key)?;
        return Ok(match (ceiling, set, own) {
            (Ceiling::Needs, None, _) => Decision::Ask,
            (Ceiling::Needs, Some(_), Some(Effect::Allow)) => Decision::Allow(Scope::Conversation(context.conversation)),
            (Ceiling::Needs, Some(_), _) => Decision::Ask,
            (Ceiling::Unless, Some(scope), _) => Decision::Allow(scope),
            (Ceiling::Unless, None, Some(Effect::Allow)) => Decision::Allow(Scope::Conversation(context.conversation)),
            (Ceiling::Unless, None, _) => Decision::Ask,
        });
    }
    if let Some(effect) = rule(db, Scope::Conversation(context.conversation), key)? {
        return Ok(if effect == Effect::Block { Decision::Block }
            else { Decision::Allow(Scope::Conversation(context.conversation)) });
    }
    let expires: Option<i64> = db.query_row("SELECT expires_at FROM permission_sessions
        WHERE conversation = ?1 AND agent = ?2 AND project = ?3
        AND machine = ?4 AND target = ?5",
        params![context.conversation.to_string(),
            context.agent.map(|id| id.to_string()).unwrap_or_default(),
            context.project.map(|id| id.to_string()).unwrap_or_default(),
            context.machine, key], |row| row.get(0))
        .optional().map_err(Issue::storage)?;
    if expires.is_some_and(|expiry| now < expiry) {
        return Ok(Decision::Allow(Scope::Session));
    }
    if let Some(project) = context.project
        && rule(db, Scope::Project(project), key)? == Some(Effect::Allow) {
        return Ok(Decision::Allow(Scope::Project(project)));
    }
    // A conversation with no agent belongs to the main agent.
    let agent = context.agent.unwrap_or(crate::main_agent::ID);
    if rule(db, Scope::Agent(agent), key)? == Some(Effect::Allow) {
        return Ok(Decision::Allow(Scope::Agent(agent)));
    }
    Ok(Decision::Ask)
}

/// Reads one exact lasting rule, treating missing rows as no decision.
fn rule(db: &Connection, scope: Scope, key: &str) -> Result<Option<Effect>, Issue> {
    let (kind, id) = scope.stored()?;
    let value: Option<String> = db.query_row("SELECT effect FROM permission_rules
        WHERE scope_kind = ?1 AND scope_id = ?2 AND target = ?3",
        params![kind, id, key], |row| row.get(0)).optional().map_err(Issue::storage)?;
    match value.as_deref() {
        Some("allow") => Ok(Some(Effect::Allow)),
        Some("block") => Ok(Some(Effect::Block)),
        None => Ok(None),
        _ => Err(Issue::Storage),
    }
}

/// Decodes bounded exact targets from one settings source.
fn rules(db: &Connection, scope: Scope) -> Result<Vec<Rule>, Issue> {
    let (kind, id) = scope.stored()?;
    let mut query = db.prepare("SELECT target, effect FROM permission_rules
        WHERE scope_kind = ?1 AND scope_id = ?2 ORDER BY target LIMIT 257")
        .map_err(Issue::storage)?;
    let rows = query.query_map(params![kind, id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }).map_err(Issue::storage)?;
    let mut result = Vec::new();
    for row in rows {
        let (text, effect) = row.map_err(Issue::storage)?;
        let target: Target = serde_json::from_str(&text).map_err(Issue::storage)?;
        target.shape_key().map_err(|_| Issue::Storage)?;
        let effect = match effect.as_str() {
            "allow" => Effect::Allow, "block" => Effect::Block,
            _ => return Err(Issue::Storage),
        };
        result.push(Rule { target, effect });
    }
    if result.len() > 256 { return Err(Issue::Limit); }
    Ok(result)
}

/// Lists active session grants for exactly this saved conversation binding.
fn session_rules(db: &Connection, context: &Context, now: i64)
    -> Result<Vec<SessionRule>, Issue> {
    let mut query = db.prepare("SELECT target, expires_at FROM permission_sessions
        WHERE conversation = ?1 AND agent = ?2 AND project = ?3
        AND machine = ?4 AND expires_at > ?5 ORDER BY target LIMIT 257")
        .map_err(Issue::storage)?;
    let rows = query.query_map(params![context.conversation.to_string(),
        context.agent.map(|id| id.to_string()).unwrap_or_default(),
        context.project.map(|id| id.to_string()).unwrap_or_default(),
        context.machine, now], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        }).map_err(Issue::storage)?;
    let mut result = Vec::new();
    for row in rows {
        let (text, expires_at) = row.map_err(Issue::storage)?;
        let target: Target = serde_json::from_str(&text).map_err(Issue::storage)?;
        target.shape_key().map_err(|_| Issue::Storage)?;
        result.push(SessionRule { target, expires_at });
    }
    if result.len() > 256 { return Err(Issue::Limit); }
    Ok(result)
}
