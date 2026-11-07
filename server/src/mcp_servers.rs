//! The list of MCP servers. MiniClaw is the MCP client: a provider never receives a server's
//! command, address or token, only the tools MiniClaw offers and relays. A saved server is
//! switched on nowhere; permissions switch it on for an agent, a project or a conversation.
//! Secrets are not stored here. This table holds the names of a server's environment values
//! and whether it has a token; the values live in the Keychain (`secrets.rs`).

// Reuses the private metadata database.
use crate::agents::{Issue, Store};
// Runs the list's statements and optional lookups.
use rusqlite::{Connection, OptionalExtension, params};
// Reads requests and writes list entries.
use serde::{Deserialize, Deserializer, Serialize};
// Keeps environment names in a stable order.
use std::collections::BTreeMap;
// Gives each saved server an opaque identifier.
use uuid::Uuid;

/// The most servers the list holds.
const LIMIT: i64 = 32;

/// Which provider's conversations may use a server. One server normally serves both, since
/// MiniClaw is the client; the choice is the user's way to keep a server to one of them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider { #[default] Both, Codex, Claude }

impl Provider {
    /// Reports whether a conversation of this provider may use the server.
    pub fn serves(self, claude: bool) -> bool {
        match self { Self::Both => true, Self::Claude => claude, Self::Codex => !claude }
    }

    /// Names the provider a server is kept to, for a refusal in words.
    pub fn only(self) -> &'static str {
        match self { Self::Both => "", Self::Claude => "Claude conversations only", Self::Codex => "Codex conversations only" }
    }
}

/// The start and call limits a server has when it sets none, in seconds.
pub const STARTUP_SECONDS: u16 = 15;
pub const TOOL_SECONDS: u16 = 300;

fn startup_default() -> u16 { STARTUP_SECONDS }
fn tool_default() -> u16 { TOOL_SECONDS }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A command on this Mac that speaks MCP over its standard input and output.
    Stdio,
    /// A remote address that speaks Streamable HTTP.
    Http,
}

/// What the user sends to create or change a server.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Draft {
    pub name: String,
    pub kind: Kind,
    #[serde(default)] pub command: Option<String>,
    #[serde(default)] pub args: Vec<String>,
    #[serde(default)] pub url: Option<String>,
    /// The one folder a sandboxed local server may read and write; also its working folder.
    #[serde(default)] pub folder: Option<String>,
    /// Explicit read-only runtime directories approved by the user.
    #[serde(default)] pub runtime_paths: Vec<String>,
    /// Hosts a sandboxed local server may reach, over HTTPS only.
    #[serde(default)] pub hosts: Vec<String>,
    /// Ports on this Mac a sandboxed local server may connect to, for a server that drives an app
    /// through the app's own local socket, as Blender's does.
    #[serde(default)] pub ports: Vec<u16>,
    /// Ports a sandboxed local server may listen on, loopback only, for a server that an app on
    /// this Mac connects to, as Roblox Studio's does.
    #[serde(default)] pub listen: Vec<u16>,
    /// Runs a local server outside the sandbox. The list and every approval say so.
    #[serde(default)] pub unrestricted: bool,
    /// Whose conversations may use it.
    #[serde(default)] pub provider: Provider,
    /// How long the server may take to start and answer `initialize`, and how long one tool call
    /// may take. Both providers' own configurations have these; MiniClaw applies them itself.
    #[serde(default = "startup_default")] pub startup_seconds: u16,
    #[serde(default = "tool_default")] pub tool_seconds: u16,
    /// When not empty, only these tools are offered. Tools listed as disabled are never offered.
    #[serde(default)] pub enabled_tools: Vec<String>,
    #[serde(default)] pub disabled_tools: Vec<String>,
    /// Extra HTTP headers of a remote server, with the same rules as `env`: a value sets the
    /// header, `null` keeps its saved value, a name left out is removed.
    #[serde(default)] pub headers: Option<BTreeMap<String, Option<String>>>,
    #[serde(default)] pub revision: Option<u64>,
    /// A value sets that name, `null` keeps its saved value, and a name left out is removed.
    /// Leaving the whole field out keeps everything.
    #[serde(default)] pub env: Option<BTreeMap<String, Option<String>>>,
    /// Names of values a new server needs and the user has not entered yet. Only read on creation.
    #[serde(default)] pub needs: Vec<String>,
    /// Left out keeps the saved token, `null` removes it, a value replaces it.
    #[serde(default, deserialize_with = "present")] pub token: Option<Option<String>>,
}

/// Tells a field that was sent as `null` from one that was not sent.
fn present<'de, D: Deserializer<'de>>(input: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(input).map(Some)
}

/// One saved server as the page sees it. It carries no secret.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Server {
    #[serde(default)] pub id: String,
    pub name: String,
    pub kind: Kind,
    #[serde(default)] pub command: Option<String>,
    #[serde(default)] pub args: Vec<String>,
    #[serde(default)] pub url: Option<String>,
    #[serde(default)] pub folder: Option<String>,
    /// Explicit read-only runtime directories approved by the user.
    #[serde(default)] pub runtime_paths: Vec<String>,
    #[serde(default)] pub hosts: Vec<String>,
    #[serde(default)] pub ports: Vec<u16>,
    #[serde(default)] pub listen: Vec<u16>,
    #[serde(default)] pub unrestricted: bool,
    #[serde(default)] pub provider: Provider,
    #[serde(default = "startup_default")] pub startup_seconds: u16,
    #[serde(default = "tool_default")] pub tool_seconds: u16,
    #[serde(default)] pub enabled_tools: Vec<String>,
    #[serde(default)] pub disabled_tools: Vec<String>,
    /// Names of the extra headers whose values are saved in the Keychain.
    #[serde(default)] pub headers: Vec<String>,
    /// Names of the environment values that are saved in the Keychain.
    #[serde(default)] pub env: Vec<String>,
    #[serde(default)] pub has_token: bool,
    /// Names an import found but whose values the user still has to enter. `token` means the token.
    #[serde(default)] pub needs: Vec<String>,
    #[serde(default)] pub revision: u64,
}

/// A change to the Keychain that belongs to one save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretChange {
    Set(String, String),
    Remove(String),
}

/// Accepts a short lowercase name that is safe inside a tool's name and is not MiniClaw's own.
pub fn valid_name(name: &str) -> bool {
    let mut letters = name.chars();
    letters.next().is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && name.len() <= 40
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        && !name.starts_with("miniclaw")
}

/// Accepts an environment name a shell would accept.
pub fn valid_env_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Accepts a single line of printable text up to a length.
fn line(text: &str, most: usize) -> bool {
    !text.is_empty() && text.len() <= most && !text.chars().any(char::is_control)
}

/// Accepts HTTPS anywhere and plain HTTP only for this Mac. An address never carries a sign-in.
pub fn valid_url(url: &str) -> bool {
    if !line(url, 2048) || url.contains(char::is_whitespace) { return false; }
    let (rest, secure) = match (url.strip_prefix("https://"), url.strip_prefix("http://")) {
        (Some(rest), _) => (rest, true),
        (_, Some(rest)) => (rest, false),
        _ => return false,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') { return false; }
    let host = if let Some(end) = authority.find(']') { &authority[..=end] }
        else { authority.split(':').next().unwrap_or_default() };
    secure || matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

/// Accepts a header name a server may be sent. The ones MiniClaw sets itself, and the ones that
/// frame the request, are not the user's to set.
pub fn valid_header_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
        && !matches!(name.to_ascii_lowercase().as_str(), "host" | "content-length" | "content-type" | "accept" | "connection"
            | "transfer-encoding" | "mcp-session-id" | "mcp-protocol-version" | "user-agent")
}

impl Server {
    /// Reports whether a tool is offered: it is enabled, or nothing is, and it is not disabled.
    pub fn offers(&self, tool: &str) -> bool {
        (self.enabled_tools.is_empty() || self.enabled_tools.iter().any(|name| name == tool))
            && !self.disabled_tools.iter().any(|name| name == tool)
    }
}

/// MiniClaw's own port. A server is never let in there: the page's requests come from the user,
/// and nothing a model can start should be able to make them.
fn own_port() -> u16 {
    std::env::var("MINICLAW_PORT").ok().and_then(|text| text.parse().ok()).unwrap_or(8787)
}

/// Accepts an absolute folder that is not the whole disk and that MiniClaw does not protect.
/// The path is judged as written and as it resolves, since `/var` and a link lead elsewhere.
#[cfg(not(windows))]
fn valid_folder(folder: &str) -> bool {
    let roots = crate::seatbelt::protected();
    let open = |path: &str| path.trim_end_matches('/').len() > 1 && !crate::seatbelt::is_protected(path.trim_end_matches('/'), &roots)
        && !roots.iter().any(|root| std::path::Path::new(root).starts_with(path.trim_end_matches('/')));
    folder.starts_with('/') && line(folder, 1024) && !folder.split('/').any(|part| part == "..") && open(folder)
        && std::fs::canonicalize(folder).ok().and_then(|real| real.to_str().map(|real| open(real))).unwrap_or(true)
}

#[cfg(windows)]
fn valid_folder(folder: &str) -> bool {
    if !line(folder, 1024) { return false; }
    let Ok(path) = crate::platform_fs::permission_path(std::path::Path::new(folder)) else { return false; };
    let normalized = path.replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase();
    if normalized.len() <= 2 { return false; }
    crate::seatbelt::protected().iter().all(|root| {
        let root = root.replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase();
        normalized != root && !normalized.starts_with(&(root.clone() + "\\")) && !root.starts_with(&(normalized.clone() + "\\"))
    })
}


/// Accepts npm's launcher saved by name on Windows (`npx`, `npm`), only with arguments MiniClaw
/// can start: npx options it knows and a registry package. MiniClaw's own npm and Node run it.
fn npm_launcher(command: &str, args: &[String], windows: bool) -> bool {
    windows && crate::npm_launch::bare(command).is_some_and(|shim| crate::npm_launch::plan(shim, args).is_ok())
}

impl Draft {
    /// Checks the fields that belong to the server's kind and refuses those of the other kind.
    pub fn valid(&self) -> bool {
        let shared = valid_name(&self.name)
            && self.env.iter().flatten().all(|(name, value)| valid_env_name(name)
                && value.as_deref().is_none_or(|value| value.len() <= 4096 && !value.contains('\0')))
            && self.env.as_ref().is_none_or(|env| env.len() <= 32)
            && self.needs.len() <= 32 && self.needs.iter().all(|name| valid_env_name(name))
            && self.token.iter().flatten().all(|token| line(token, 4096))
            && (1..=120).contains(&self.startup_seconds) && (1..=3600).contains(&self.tool_seconds)
            && [&self.enabled_tools, &self.disabled_tools].iter().all(|tools| tools.len() <= 128
                && tools.iter().all(|tool| line(tool, 128)));
        shared && match self.kind {
            Kind::Stdio => self.url.is_none() && self.token.is_none() && self.headers.is_none()
                && self.command.as_deref().is_some_and(|command| line(command, 1024)
                    && (std::path::Path::new(command).is_absolute() || npm_launcher(command, &self.args, cfg!(windows))))
                && self.args.len() <= 64 && self.args.iter().all(|arg| arg.len() <= 4096 && !arg.chars().any(char::is_control))
                && self.folder.as_deref().is_none_or(valid_folder)
                && self.runtime_paths.len() <= 8 && self.runtime_paths.iter().all(|path| valid_folder(path))
                && self.hosts.len() <= 16 && self.hosts.iter().all(|host| crate::permissions::dns_name(host))
                && self.ports.len() <= 8 && self.ports.iter().all(|port| *port >= 1024 && *port != own_port())
                && self.ports.iter().collect::<std::collections::HashSet<_>>().len() == self.ports.len()
                && self.listen.len() <= 8 && self.listen.iter().all(|port| *port >= 1024 && *port != own_port())
                && self.listen.iter().collect::<std::collections::HashSet<_>>().len() == self.listen.len()
                // A port is one or the other: a server does not connect to what it serves.
                && !self.listen.iter().any(|port| self.ports.contains(port)),
            Kind::Http => self.command.is_none() && self.args.is_empty() && !self.unrestricted
                && self.folder.is_none() && self.runtime_paths.is_empty() && self.hosts.is_empty() && self.ports.is_empty() && self.listen.is_empty()
                && self.env.as_ref().is_none_or(BTreeMap::is_empty)
                && self.url.as_deref().is_some_and(valid_url)
                && self.headers.as_ref().is_none_or(|headers| headers.len() <= 16 && headers.iter().all(|(name, value)|
                    valid_header_name(name) && value.as_deref().is_none_or(|value| line(value, 4096)))),
        }
    }

    /// Works out the saved entry and the Keychain changes, given what was saved before.
    pub fn apply(self, id: &str, before: Option<&Server>) -> Result<(Server, Vec<SecretChange>), Issue> {
        let mut changes = Vec::new();
        let old_env = before.map(|server| server.env.clone()).unwrap_or_default();
        let mut needs = before.map(|server| server.needs.clone()).unwrap_or_default();
        let env = match self.env {
            None => old_env,
            Some(env) => {
                for (name, value) in &env {
                    match value {
                        Some(value) => changes.push(SecretChange::Set(crate::secrets::env_key(id, name), value.clone())),
                        // Keeping a value needs one to keep.
                        None if !old_env.contains(name) => return Err(Issue::Invalid),
                        None => {}
                    }
                }
                for name in old_env.iter().filter(|name| !env.contains_key(*name)) {
                    changes.push(SecretChange::Remove(crate::secrets::env_key(id, name)));
                }
                needs.retain(|name| name == "token" || !env.contains_key(name));
                env.into_keys().collect()
            }
        };
        let old_headers = before.map(|server| server.headers.clone()).unwrap_or_default();
        let headers = match self.headers {
            None => old_headers,
            Some(headers) => {
                for (name, value) in &headers {
                    match value {
                        Some(value) => changes.push(SecretChange::Set(crate::secrets::header_key(id, name), value.clone())),
                        None if !old_headers.contains(name) => return Err(Issue::Invalid),
                        None => {}
                    }
                }
                for name in old_headers.iter().filter(|name| !headers.contains_key(*name)) {
                    changes.push(SecretChange::Remove(crate::secrets::header_key(id, name)));
                }
                headers.into_keys().collect()
            }
        };
        let has_token = match self.token {
            None => before.is_some_and(|server| server.has_token),
            Some(None) => { changes.push(SecretChange::Remove(crate::secrets::token_key(id))); false }
            Some(Some(token)) => {
                changes.push(SecretChange::Set(crate::secrets::token_key(id), token));
                needs.retain(|name| name != "token");
                true
            }
        };
        Ok((Server { id: id.to_owned(), name: self.name, kind: self.kind, command: self.command, args: self.args,
            url: self.url, folder: self.folder, runtime_paths: self.runtime_paths, hosts: self.hosts, ports: self.ports, listen: self.listen, unrestricted: self.unrestricted,
            provider: self.provider, startup_seconds: self.startup_seconds, tool_seconds: self.tool_seconds,
            enabled_tools: self.enabled_tools, disabled_tools: self.disabled_tools, headers, env, has_token, needs, revision: 0 }, changes))
    }
}

/// Creates the server table inside the existing private metadata database.
pub fn migrate(db: &Connection) -> Result<(), Issue> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS mcp_servers (
        id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, definition TEXT NOT NULL, revision INTEGER NOT NULL
    );").map_err(Issue::storage)
}

/// Reads one row; a row that no longer parses is left out instead of failing the list.
fn read(row: &rusqlite::Row) -> rusqlite::Result<Option<Server>> {
    let Ok(mut server) = serde_json::from_str::<Server>(&row.get::<_, String>(1)?) else { return Ok(None) };
    server.id = row.get(0)?;
    server.revision = row.get::<_, i64>(2)?.max(0) as u64;
    Ok(Some(server))
}

impl Store {
    /// Lists every saved server, oldest first.
    pub async fn mcp_servers(&self) -> Result<Vec<Server>, Issue> {
        self.run(|db| {
            let mut query = db.prepare("SELECT id, definition, revision FROM mcp_servers ORDER BY rowid LIMIT 64")
                .map_err(Issue::storage)?;
            let rows = query.query_map([], read).map_err(Issue::storage)?;
            Ok(rows.collect::<Result<Vec<_>, _>>().map_err(Issue::storage)?.into_iter().flatten().collect())
        }).await
    }

    /// Creates a server, or replaces one whose revision the caller has seen. `plan` receives the
    /// saved entry, if any, and answers the new entry with the Keychain changes. The row is
    /// written in the same transaction the old entry was read in; the caller then applies the
    /// Keychain changes.
    pub async fn save_mcp_server(&self, id: Option<String>, draft: Draft, imported_needs: Vec<String>)
        -> Result<(Server, Vec<SecretChange>), Issue> {
        if !draft.valid() { return Err(Issue::Invalid); }
        self.run(move |db| {
            let tx = db.unchecked_transaction().map_err(Issue::storage)?;
            let before = match &id {
                Some(id) => Some(tx.query_row("SELECT id, definition, revision FROM mcp_servers WHERE id = ?1", [id], read)
                    .optional().map_err(Issue::storage)?.flatten().ok_or(Issue::Missing)?),
                None => None,
            };
            if let Some(before) = &before {
                if draft.revision != Some(before.revision) { return Err(Issue::Conflict); }
                // A server does not change its kind; its secrets would no longer fit.
                if draft.kind != before.kind { return Err(Issue::Invalid); }
            } else if draft.revision.is_some() { return Err(Issue::Invalid); }
            let taken: Option<String> = tx.query_row("SELECT id FROM mcp_servers WHERE name = ?1", [&draft.name], |row| row.get(0))
                .optional().map_err(Issue::storage)?;
            if taken.is_some() && taken != id { return Err(Issue::NameTaken); }
            let id = id.unwrap_or_else(|| Uuid::new_v4().to_string());
            let mut needs = imported_needs;
            needs.extend(draft.needs.iter().filter(|name| draft.env.as_ref().is_none_or(|env| !env.contains_key(*name))).cloned());
            let (mut server, changes) = draft.apply(&id, before.as_ref())?;
            if before.is_none() { needs.dedup(); server.needs = needs; }
            server.revision = before.as_ref().map_or(1, |before| before.revision + 1);
            let mut stored = serde_json::to_value(&server).map_err(Issue::storage)?;
            if let Some(fields) = stored.as_object_mut() { fields.remove("id"); fields.remove("revision"); }
            if before.is_some() {
                tx.execute("UPDATE mcp_servers SET name = ?2, definition = ?3, revision = ?4 WHERE id = ?1",
                    params![id, server.name, stored.to_string(), server.revision as i64]).map_err(Issue::storage)?;
            } else {
                let count: i64 = tx.query_row("SELECT count(*) FROM mcp_servers", [], |row| row.get(0)).map_err(Issue::storage)?;
                if count >= LIMIT { return Err(Issue::Limit); }
                tx.execute("INSERT INTO mcp_servers (id, name, definition, revision) VALUES (?1, ?2, ?3, ?4)",
                    params![id, server.name, stored.to_string(), server.revision as i64]).map_err(Issue::storage)?;
            }
            tx.commit().map_err(Issue::storage)?;
            Ok((server, changes))
        }).await
    }

    /// Deletes a server whose revision the caller has seen and answers what it was, so its
    /// secrets can be removed from the Keychain.
    pub async fn remove_mcp_server(&self, id: String, revision: u64) -> Result<Server, Issue> {
        self.run(move |db| {
            let server = db.query_row("SELECT id, definition, revision FROM mcp_servers WHERE id = ?1", [&id], read)
                .optional().map_err(Issue::storage)?.flatten().ok_or(Issue::Missing)?;
            if server.revision != revision { return Err(Issue::Conflict); }
            db.execute("DELETE FROM mcp_servers WHERE id = ?1", [&id]).map_err(Issue::storage)?;
            Ok(server)
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_https_or_this_mac() {
        for good in ["https://mcp.example.com/mcp", "https://a.example:8443", "http://127.0.0.1:8931/mcp",
            "http://localhost/mcp", "http://[::1]:9000/"] { assert!(valid_url(good), "{good}"); }
        for bad in ["http://mcp.example.com", "https://user:pw@a.example/", "ftp://a.example", "https://", "https://a b.example",
            "http://127.0.0.1.evil.example/", "http://localhost.evil.example/", "https://a.example/\n"] { assert!(!valid_url(bad), "{bad}"); }
    }

    #[test]
    fn npm_launchers_are_saved_by_name_on_windows_only() {
        let args = |items: &[&str]| items.iter().map(|item| (*item).to_owned()).collect::<Vec<_>>();
        assert!(npm_launcher("npx", &args(&["-y", "@upstash/context7-mcp"]), true));
        assert!(npm_launcher("npm", &args(&["exec", "--", "pkg", "--stdio"]), true));
        // Arguments MiniClaw would refuse to start are refused when saving.
        assert!(!npm_launcher("npx", &args(&["-c", "echo hi"]), true));
        assert!(!npm_launcher("npm", &args(&["run", "start"]), true));
        assert!(!npm_launcher("npx.cmd", &args(&["pkg"]), true));
        assert!(!npm_launcher("node", &args(&["pkg"]), true));
        // macOS saves the absolute path of the user's npx, as before.
        assert!(!npm_launcher("npx", &args(&["-y", "pkg"]), false));
        let draft: Draft = serde_json::from_value(serde_json::json!({"name": "context7", "kind": "stdio", "command": "npx", "args": ["-y", "@upstash/context7-mcp"]})).unwrap();
        assert_eq!(draft.valid(), cfg!(windows));
    }

    #[test]
    fn names_fit_inside_a_tool_name() {
        for good in ["notes", "context7", "my-server_2"] { assert!(valid_name(good), "{good}"); }
        for bad in ["", "Has Space", "UPPER", "miniclaw", "miniclaw_computer", "-lead", "a.b"] { assert!(!valid_name(bad), "{bad}"); }
    }
}

#[cfg(test)]
mod runtime_path_tests {
    use super::*;
    #[cfg(not(windows))]
    #[test]
    fn runtime_paths_are_explicit_bounded_stdio_resources() {
        let mut value = serde_json::json!({"name":"python_tool","kind":"stdio","command":"/opt/python/python",
            "runtimePaths":["/opt/python/runtime"]});
        let draft: Draft = serde_json::from_value(value.clone()).unwrap();
        assert!(draft.valid());
        value["runtimePaths"] = serde_json::json!(["relative"]);
        assert!(!serde_json::from_value::<Draft>(value.clone()).unwrap().valid());
        value["runtimePaths"] = serde_json::json!(vec!["/opt/python/runtime"; 9]);
        assert!(!serde_json::from_value::<Draft>(value).unwrap().valid());
        let http = serde_json::json!({"name":"remote","kind":"http","url":"https://example.com/mcp","runtimePaths":["/opt/python/runtime"]});
        assert!(!serde_json::from_value::<Draft>(http).unwrap().valid());
    }
    #[cfg(windows)]
    #[test]
    fn windows_local_server_accepts_native_paths() {
        let value = serde_json::json!({"name":"python_tool","kind":"stdio","command":r"C:\Tools\python.exe",
            "folder":r"C:\Tools\work","runtimePaths":[r"C:\Tools\Python312"]});
        assert!(serde_json::from_value::<Draft>(value).unwrap().valid());
    }
}
