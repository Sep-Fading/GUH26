//! Reads MCP server definitions from Codex's `config.toml` and Claude Code's `.claude.json`.
//! Only the `mcp_servers` / `mcpServers` entries are looked at. Values of environment
//! variables and headers are dropped as soon as the file is parsed, so a provider's secret is
//! never listed or saved; only the names survive, as a note of what the user still has to enter.
//! The provider's file is opened for reading and never changed.

// Shares validation and the list's entry types.
use crate::mcp_servers::{Draft, Kind, valid_env_name, valid_name, valid_url};
// Shares the provider choice and storage failures.
use crate::{agents::Issue, skills::Provider};
// Walks the parsed configuration.
use serde_json::Value;
// Locates the configuration files and commands.
use std::path::{Path, PathBuf};

/// The largest configuration file that is read.
const MOST: u64 = 4 * 1024 * 1024;

/// One importable server, as listed to the user.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub name: String,
    pub kind: Kind,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    /// Names of environment values the provider's entry sets.
    pub env: Vec<String>,
    /// Whether the entry carries a token or a header the user has to enter again.
    pub needs_token: bool,
}

/// Finds the file that holds the provider's server entries.
fn file(provider: Provider) -> Result<PathBuf, Issue> {
    let home = || std::env::var_os("HOME")
        .or_else(|| cfg!(windows).then(|| std::env::var_os("USERPROFILE")).flatten())
        .map(PathBuf::from).ok_or(Issue::Storage);
    Ok(match provider {
        Provider::Codex => std::env::var_os("CODEX_HOME").map(PathBuf::from)
            .map_or_else(|| home().map(|home| home.join(".codex")), Ok)?.join("config.toml"),
        // Claude Code keeps `.claude.json` in its config folder when one is set, otherwise in the home folder.
        Provider::Claude => std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from)
            .map_or_else(home, Ok)?.join(".claude.json"),
    })
}

/// Reads a bounded regular file; a missing file means nothing to import.
fn text(path: &Path) -> Result<Option<String>, Issue> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Issue::Storage),
    };
    if !meta.is_file() || meta.len() > MOST { return Err(Issue::Limit); }
    std::fs::read_to_string(path).map(Some).map_err(|_| Issue::Storage)
}

/// The names of a table's keys, which is all that is kept of values that may be secret.
fn names(table: Option<&Value>) -> Vec<String> {
    table.and_then(Value::as_object).map(|table| table.keys().filter(|name| valid_env_name(name)).cloned().collect())
        .unwrap_or_default()
}

/// Turns one provider entry into a source, or nothing when it is not a kind MiniClaw supports.
fn source(name: &str, entry: &Value) -> Option<Source> {
    if !valid_name(name) { return None; }
    let words = |key: &str| entry[key].as_str().map(str::to_owned);
    let kind = entry["type"].as_str();
    if let Some(url) = words("url") {
        if !matches!(kind, None | Some("http" | "streamable-http")) || !valid_url(&url) { return None; }
        let needs_token = ["bearer_token_env_var", "headers", "http_headers", "env_http_headers"].iter()
            .any(|key| !entry[*key].is_null());
        return Some(Source { name: name.to_owned(), kind: Kind::Http, command: None, args: Vec::new(), url: Some(url),
            env: Vec::new(), needs_token });
    }
    if !matches!(kind, None | Some("stdio")) { return None; }
    let args = entry["args"].as_array().map(|args| args.iter().filter_map(|arg| arg.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    let mut env = names(entry.get("env"));
    env.extend(entry["env_vars"].as_array().into_iter().flatten().filter_map(Value::as_str)
        .filter(|name| valid_env_name(name)).map(str::to_owned));
    env.sort();
    env.dedup();
    Some(Source { name: name.to_owned(), kind: Kind::Stdio, command: Some(words("command")?), args, url: None, env,
        needs_token: false })
}

/// Lists what can be imported from one provider, in the order of its names.
pub fn sources(provider: Provider) -> Result<Vec<Source>, Issue> {
    let Some(text) = text(&file(provider)?)? else { return Ok(Vec::new()) };
    let mut tables: Vec<Value> = Vec::new();
    match provider {
        Provider::Codex => {
            let config: toml::Value = toml::from_str(&text).map_err(|_| Issue::Invalid)?;
            if let Some(servers) = config.get("mcp_servers") {
                tables.push(serde_json::to_value(servers).map_err(|_| Issue::Invalid)?);
            }
        }
        Provider::Claude => {
            let config: Value = serde_json::from_str(&text).map_err(|_| Issue::Invalid)?;
            tables.push(config["mcpServers"].clone());
            // Servers saved for one folder are offered too; the first entry of a name wins.
            tables.extend(config["projects"].as_object().into_iter().flatten().map(|(_, project)| project["mcpServers"].clone()));
        }
    }
    let mut found: Vec<Source> = Vec::new();
    for table in tables.iter().filter_map(Value::as_object) {
        for (name, entry) in table {
            if found.len() < 128 && !found.iter().any(|known| &known.name == name)
                && let Some(source) = source(name, entry) { found.push(source); }
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(found)
}

/// Finds a bare command such as `npx` on the PATH, since a saved server names an absolute path.
fn resolve(command: &str) -> Option<String> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    resolve_in(command, &path)
}

/// Saves npm's launcher on Windows as its bare name (`npx` or `npm`), whether the provider named
/// it bare or by the path of `npx.cmd`: MiniClaw starts it with its own Node and npm, never the
/// batch file. Relative paths stay refused; elsewhere the launcher is found like any command.
fn npm_launcher(command: &str, windows: bool) -> Option<String> {
    let named = !command.contains(['/', '\\', ':']) || Path::new(command).is_absolute() || crate::platform_fs::plain_windows_path(command);
    (windows && named).then(|| crate::npm_launch::shim(command)).flatten().map(|shim| shim.name().to_owned())
}

/// Uses native PATH parsing; batch files require a user-selected explicit native launcher.
fn resolve_in(command: &str, path: &std::ffi::OsStr) -> Option<String> {
    if let Some(launcher) = npm_launcher(command, cfg!(windows)) { return Some(launcher); }
    let native = |path: &Path| !cfg!(windows) || path.extension().and_then(|v| v.to_str())
        .is_some_and(|v| v.eq_ignore_ascii_case("exe") || v.eq_ignore_ascii_case("com"));
    let command_path = Path::new(command);
    if command_path.is_absolute() { return native(command_path).then(|| command.to_owned()); }
    if command.is_empty() || command.contains(['/', '\\', ':']) { return None; }
    let mut folders: Vec<PathBuf> = std::env::split_paths(path).filter(|p| p.is_absolute()).collect();
    if !cfg!(windows) { folders.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"].map(PathBuf::from)); }
    let names = if cfg!(windows) && command_path.extension().is_none() {
        vec![format!("{command}.exe"), format!("{command}.com")]
    } else { vec![command.to_owned()] };
    folders.into_iter().flat_map(|folder| names.iter().map(move |name| folder.join(name)))
        .find(|candidate| native(candidate) && candidate.is_file())
        .and_then(|found| found.to_str().map(str::to_owned))
}

/// Builds the draft of one named source and the names the user still has to fill in.
pub fn draft(provider: Provider, name: &str) -> Result<(Draft, Vec<String>), Issue> {
    let source = sources(provider)?.into_iter().find(|source| source.name == name).ok_or(Issue::Missing)?;
    let command = match &source.command {
        Some(command) => Some(resolve(command).ok_or(Issue::Invalid)?),
        None => None,
    };
    let mut needs = source.env.clone();
    if source.needs_token { needs.push("token".to_owned()); }
    Ok((Draft { name: source.name, kind: source.kind, command, args: source.args, url: source.url, folder: None, runtime_paths: Vec::new(), hosts: Vec::new(), ports: Vec::new(), listen: Vec::new(), provider: Default::default(),
        startup_seconds: crate::mcp_servers::STARTUP_SECONDS, tool_seconds: crate::mcp_servers::TOOL_SECONDS,
        enabled_tools: Vec::new(), disabled_tools: Vec::new(), headers: None,
        unrestricted: false,
        revision: None, env: None, needs: Vec::new(), token: None }, needs))
}

#[cfg(test)]
mod platform_tests {
    use super::*;
    #[test]
    fn fixture_search_uses_native_path_entries_without_relative_fallbacks() {
        let root = std::env::temp_dir().join(format!("mc-import-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let name = if cfg!(windows) { "fixture.exe" } else { "fixture" };
        std::fs::write(root.join(name), b"fixture").unwrap();
        let paths = std::env::join_paths([root.clone()]).unwrap();
        assert_eq!(resolve_in("fixture", &paths), Some(root.join(name).to_string_lossy().into_owned()));
        assert!(resolve_in("../fixture", &paths).is_none());
        assert!(resolve_in("nested\\fixture", &paths).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn npm_launchers_are_saved_by_name_on_windows_only() {
        for command in ["npx", "npx.cmd", r"C:\Program Files\nodejs\npx.cmd", "C:/nodejs/NPX.CMD", r"D:\Søren Tools\npx"] {
            assert_eq!(npm_launcher(command, true).as_deref(), Some("npx"), "{command}");
        }
        assert_eq!(npm_launcher(r"C:\nodejs\npm.cmd", true).as_deref(), Some("npm"));
        // Not npm's launcher, a relative path, or a Mac: the ordinary rules decide.
        for command in [r"C:\Tools\server.cmd", r"C:\Tools\npx.cmd.exe", "npx .cmd", r"tools\npx.cmd", "./npx", r"C:\Tools\node.exe"] {
            assert_eq!(npm_launcher(command, true), None, "{command}");
        }
        assert_eq!(npm_launcher("npx", false), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_import_rejects_batch_shims_without_explicit_native_launcher() {
        assert!(resolve_in(r"C:\Tools\server.cmd", std::ffi::OsStr::new("")).is_none());
        assert_eq!(resolve_in(r"C:\Tools\npx.cmd", std::ffi::OsStr::new("")).as_deref(), Some("npx"));
        assert_eq!(resolve_in(r"C:\Tools\node.exe", std::ffi::OsStr::new("")), Some(r"C:\Tools\node.exe".into()));
        assert_eq!(resolve_in(r"C:\Tools\npx.cmd.exe", std::ffi::OsStr::new("")), Some(r"C:\Tools\npx.cmd.exe".into()));
    }
}
