//! Identity-bound file operations and isolated shell behind native acceptance gates.
// Reuses exact operation and grant contracts.
use crate::{platform_fs, scoped::Operation, seatbelt::Grant};
// Shapes tool results without exposing state.
use serde_json::{Value, json};
// Holds checked files while reading or writing.
use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
    time::Duration,
};

/// Pins a validated current directory while the approved shell is running.
pub(crate) fn open_folder(path: &str) -> io::Result<File> {
    platform_fs::open_existing(Path::new(path), false, true)
}

/// Produces the same bounded unified diff as the Mac executor.
pub fn diff(before: &str, after: &str) -> String {
    if before == after {
        return String::new();
    }
    let text = similar::TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(3)
        .to_string();
    if text.len() <= 65_536 {
        return text;
    }
    let mut end = 65_536;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let cut = text[..end].rfind('\n').unwrap_or(end);
    format!(
        "{}\n... the rest of this change is not shown ({} bytes).\n",
        &text[..cut],
        text.len() - cut
    )
}

/// The one answer to "can Windows run sandboxed work on this installation?". The Changes panel,
/// the briefing's survey, the terminal and the MCP gateway all ask here before they create a
/// sandbox, so nothing stamps permissions on the user's folders while a precondition is missing.
/// `seatbelt::unavailable_reason` says which one.
pub fn sandbox_available() -> bool {
    crate::seatbelt::available()
}

/// Whether the product's own Git can run: the sandbox, and the private Git that setup installs.
/// Without it Changes shows the conversation view and discard is refused.
pub fn git_available() -> bool {
    crate::windows_gate::git().is_ok()
}

/// Says why the product's own Git cannot run, or `None` when it can.
pub fn git_unavailable_reason() -> Option<String> {
    crate::windows_gate::git().err().map(|missing| missing.reason())
}

/// Whether shutdown began, and how many one-shot sandbox runs are still in flight.
static RUNS: std::sync::LazyLock<tokio::sync::watch::Sender<(bool, usize)>> =
    std::sync::LazyLock::new(|| tokio::sync::watch::channel((false, 0)).0);

/// Counts one sandbox run from before its sandbox exists until after it was dropped.
struct Run;

impl Run {
    /// Registers a run, or refuses it once shutdown began.
    fn begin() -> io::Result<Self> {
        let mut admitted = false;
        RUNS.send_modify(|(closing, count)| {
            if !*closing {
                *count += 1;
                admitted = true;
            }
        });
        if admitted { Ok(Self) } else { Err(io::Error::other("MiniClaw is shutting down")) }
    }
}

impl Drop for Run {
    /// Tells a waiting shutdown that this run's sandbox was dropped and queued for cleanup.
    fn drop(&mut self) {
        RUNS.send_modify(|(_, count)| *count -= 1);
    }
}

/// Runs one sandboxed command, and drops it with its sandbox as soon as shutdown begins.
async fn until_shutdown<T>(work: impl std::future::Future<Output = io::Result<T>>) -> io::Result<T> {
    let _run = Run::begin()?;
    let mut runs = RUNS.subscribe();
    tokio::select! {
        biased;
        _ = runs.wait_for(|(closing, _)| *closing) => Err(io::Error::other("MiniClaw is shutting down")),
        result = work => result,
    }
}

/// Begins shutdown for one-shot sandbox runs: no new run starts, and each run in flight is
/// dropped, which ends its process tree and queues its sandbox for cleanup. Returns once all
/// of them are gone, so `drain_cleanup` afterwards sees every journal; a timeout is an error.
pub async fn end_runs(timeout: Duration) -> io::Result<()> {
    RUNS.send_modify(|(closing, _)| *closing = true);
    let mut runs = RUNS.subscribe();
    tokio::time::timeout(timeout, runs.wait_for(|(_, count)| *count == 0))
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
        .map(|_| ())
        .map_err(io::Error::other)
}

/// Runs an operation whose permission was decided by the backend; no shell fallback exists.
pub async fn execute_host(operation: Operation, grants: &[Grant]) -> Value {
    execute_host_resolving(operation, grants, crate::egress::system_resolver()).await
}

/// Executes bounded file operations through handles while retaining all ancestor guards.
pub async fn execute_host_resolving(
    operation: Operation,
    grants: &[Grant],
    resolve: crate::egress::Resolver,
) -> Value {
    if let Operation::RunShell { text, cwd, hosts } = &operation {
        // No sandbox is created, and nothing runs instead, while a precondition is missing.
        if let Some(reason) = crate::seatbelt::unavailable_reason() {
            return json!({ "status": "denied", "text": format!("Sandboxed command refused: {reason}") });
        }
        return until_shutdown(sandboxed_shell(text, cwd, hosts, grants, resolve)).await.unwrap_or_else(|error|
            json!({ "status": "denied", "text": format!("Sandboxed command refused: {error}") }));
    }
    let result = (|| {
        let policy = compile_policy(grants)?;
        match operation {
            Operation::ReadFile { path } => {
                let guard = checked(&policy, &path, false, false)?;
                Ok(json!({ "status": "ok", "text": read(&guard.file)? }))
            }
            Operation::WriteFile { path, text } => write(&policy, &path, &text),
            Operation::ReplaceInFile { path, find, text } => {
                if find.is_empty() {
                    return Err(io::ErrorKind::InvalidInput.into());
                }
                let mut guard = checked(&policy, &path, true, false)?;
                let before = read(&guard.file)?;
                // The same two answers as the Mac executor, so the model learns which way it missed.
                let count = before.matches(&find).count();
                if count != 1 {
                    return Ok(json!({ "status": "failed", "text": if count == 0 {
                        "The passage was not found; nothing was changed.".to_owned()
                    } else {
                        format!("The passage occurs {count} times; nothing was changed. Include more surrounding text.")
                    } }));
                }
                let after = before.replacen(&find, &text, 1);
                write_handle(&mut guard.file, &after)?;
                Ok(
                    json!({ "status": "ok", "text": "File updated.", "diff": diff(&before, &after) }),
                )
            }
            Operation::ListFolder { path } => list(&policy, &path),
            Operation::Search { path, text } => {
                if text.is_empty() || text.len() > 512 {
                    return Err(io::ErrorKind::InvalidInput.into());
                }
                let mut search = Search {
                    policy: &policy,
                    needle: &text,
                    lines: Vec::new(),
                    visited: 0,
                    more: false,
                };
                search.folder(&path, 0)?;
                if search.more {
                    search
                        .lines
                        .push("More matches may exist; narrow the search.".into());
                }
                let text = if search.lines.is_empty() {
                    "No matches.".into()
                } else {
                    search.lines.join("\n")
                };
                Ok(json!({ "status": "ok", "text": text }))
            }
            _ => Err(io::ErrorKind::Unsupported.into()),
        }
    })();
    result.unwrap_or_else(|error: io::Error| json!({ "status": "denied", "text": format!("Operation refused: {error}") }))
}

/// Normalizes every rule, including mandatory protected roots, before touching content.
fn compile_policy(grants: &[Grant]) -> io::Result<BoundPolicy> {
    let protected = crate::seatbelt::protected().into_iter().map(|path| Grant {
        path,
        write: false,
        folder: true,
        blocked: true,
    });
    let mut normalized = Vec::new();
    for mut grant in grants.iter().cloned().chain(protected) {
        grant.path = platform_fs::permission_path(Path::new(&grant.path))?;
        normalized.push(grant);
    }
    let mut bound = Vec::new();
    for grant in &normalized {
        let directory = fs::symlink_metadata(&grant.path)
            .map(|metadata| metadata.is_dir())
            .unwrap_or(grant.folder);
        let guard = match platform_fs::guarded_acl_path(Path::new(&grant.path), directory) {
            Ok(guard) => Some(guard),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        bound.push(guard);
    }
    Ok(BoundPolicy {
        grants: normalized,
        bound,
    })
}

/// Pins each existing permission resource while authorizing actual object identities.
struct BoundPolicy {
    grants: Vec<Grant>,
    bound: Vec<Option<platform_fs::GuardedPath>>,
}

impl BoundPolicy {
    /// Authorizes a new leaf against its pinned parent's identity and conservative blocks.
    fn allows_new(&self, parent: &platform_fs::GuardedPath, key: &str) -> io::Result<bool> {
        let ids = parent.identities()?;
        let mut allowed = false;
        for (grant, bound) in self.grants.iter().zip(&self.bound) {
            let covered_parent = if grant.folder {
                bound
                    .as_ref()
                    .map(|guard| platform_fs::file_identity(&guard.file))
                    .transpose()?
                    .is_some_and(|id| ids.contains(&id))
            } else {
                false
            };
            if grant.blocked && (covered_parent || blocked_name(grant, key)) {
                return Ok(false);
            }
            if !grant.blocked
                && grant.write
                && (covered_parent || !grant.folder && bound.is_none() && grant.path == key)
            {
                allowed = true;
            }
        }
        Ok(allowed)
    }

    /// Matches grants by native object identity; blocks additionally retain conservative names.
    fn allows(&self, target: &platform_fs::GuardedPath, write: bool) -> io::Result<bool> {
        let ids = target.identities()?;
        let target_id = platform_fs::file_identity(&target.file)?;
        let mut allowed = false;
        for (grant, bound) in self.grants.iter().zip(&self.bound) {
            let covers = if let Some(bound) = bound {
                let id = platform_fs::file_identity(&bound.file)?;
                if grant.folder {
                    ids.contains(&id)
                } else {
                    target_id == id
                }
            } else {
                false
            };
            if grant.blocked {
                if covers || blocked_name(grant, &target.key) {
                    return Ok(false);
                }
            } else if covers && (!write || grant.write) {
                allowed = true;
            }
        }
        Ok(allowed)
    }
}

/// Applies deny names conservatively even if directory case sensitivity changes concurrently.
/// Names in any language are compared as Windows may fold them, erring towards a block.
fn blocked_name(grant: &Grant, key: &str) -> bool {
    if platform_fs::windows_names_may_match(key, &grant.path) {
        return true;
    }
    // A blocked folder covers a key that continues it at a component boundary.
    grant.folder
        && key.char_indices().filter(|(_, c)| *c == '\\').any(|(index, _)| {
            let end = if grant.path.ends_with('\\') { index + 1 } else { index };
            platform_fs::windows_names_may_match(&key[..end], &grant.path)
        })
}

/// Authorizes the canonical object after opening without traversing any reparse points.
fn checked(
    policy: &BoundPolicy,
    path: &str,
    write: bool,
    directory: bool,
) -> io::Result<platform_fs::GuardedPath> {
    let guard = platform_fs::guarded_path(Path::new(path), write, directory)?;
    if !policy.allows(&guard, write)? {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(guard)
}

/// Reads at most one MiB of UTF-8 content from the already-authorized handle.
fn read(file: &File) -> io::Result<String> {
    const MAX: u64 = 1_048_576;
    if file.metadata()?.len() > MAX {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut bytes = Vec::new();
    file.take(MAX + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX {
        return Err(io::ErrorKind::InvalidData.into());
    }
    String::from_utf8(bytes).map_err(|_| io::ErrorKind::InvalidData.into())
}

/// Replaces content through the same handle used for policy and diff checks.
fn write_handle(file: &mut File, text: &str) -> io::Result<()> {
    if text.len() > 32_768 || text.contains('\0') {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    file.seek(SeekFrom::Start(0))?;
    file.set_len(0)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

/// Writes an existing file or creates only an approved absent child of a pinned parent.
fn write(policy: &BoundPolicy, path: &str, text: &str) -> io::Result<Value> {
    if text.len() > 32_768 || text.contains('\0') {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let (mut guard, before, created) = match checked(policy, path, true, false) {
        Ok(guard) => {
            let before = read(&guard.file)?;
            (guard, before, false)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let path = Path::new(path);
            let parent = platform_fs::guarded_path(
                path.parent().ok_or(io::ErrorKind::InvalidInput)?,
                false,
                true,
            )?;
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or(io::ErrorKind::InvalidInput)?;
            let key = format!("{}\\{}", parent.key.trim_end_matches('\\'), name.to_owned());
            if !policy.allows_new(&parent, &key)? {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            (
                platform_fs::guarded_create(&parent, name.as_ref())?,
                String::new(),
                true,
            )
        }
        Err(error) => return Err(error),
    };
    write_handle(&mut guard.file, text)?;
    let said = if created { "File created." } else { "File updated." };
    Ok(json!({ "status": "ok", "text": said, "diff": diff(&before, text) }))
}

/// Enumerates a pinned directory, omitting blocked and unsupported entries.
fn list(policy: &BoundPolicy, path: &str) -> io::Result<Value> {
    let folder = checked(policy, path, false, true)?;
    let mut lines = Vec::new();
    for entry in fs::read_dir(&folder.path)?.take(5_001) {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if platform_fs::is_link(&metadata) {
            continue;
        }
        let Ok(child) = checked(
            policy,
            &entry.path().to_string_lossy(),
            false,
            metadata.is_dir(),
        ) else {
            continue;
        };
        if lines.len() == 500 {
            lines.push("More entries exist.".into());
            break;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        lines.push(if metadata.is_dir() {
            format!("{name}\tfolder")
        } else {
            format!("{name}\tfile\t{}", child.file.metadata()?.len())
        });
    }
    lines.sort();
    Ok(
        json!({ "status": "ok", "text": if lines.is_empty() { "The folder is empty.".into() } else { lines.join("\n") } }),
    )
}

struct Search<'a> {
    policy: &'a BoundPolicy,
    needle: &'a str,
    lines: Vec<String>,
    visited: usize,
    more: bool,
}

impl Search<'_> {
    /// Visits a bounded set of authorized descendants while the directory remains pinned.
    fn folder(&mut self, path: &str, depth: usize) -> io::Result<()> {
        if depth > 24 {
            self.more = true;
            return Ok(());
        }
        let folder = checked(self.policy, path, false, true)?;
        for entry in fs::read_dir(&folder.path)? {
            if self.visited >= 5_000 || self.lines.len() >= 200 {
                self.more = true;
                break;
            }
            self.visited += 1;
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if platform_fs::is_link(&metadata) {
                continue;
            }
            let path = entry.path().to_string_lossy().into_owned();
            if metadata.is_dir() {
                let _ = self.folder(&path, depth + 1);
                continue;
            }
            let Ok(child) = checked(self.policy, &path, false, false) else {
                continue;
            };
            let Ok(content) = read(&child.file) else {
                continue;
            };
            for (index, line) in content.lines().enumerate() {
                if !line.contains(self.needle) {
                    continue;
                }
                if self.lines.len() >= 200 {
                    self.more = true;
                    break;
                }
                self.lines.push(format!(
                    "{}:{}: {}",
                    child.path.display(),
                    index + 1,
                    line.trim().chars().take(200).collect::<String>()
                ));
            }
        }
        Ok(())
    }
}

/// Runs the pinned shell inside a fresh LPAC; network grants remain broker-bound.
async fn sandboxed_shell(
    text: &str,
    cwd: &str,
    hosts: &[String],
    grants: &[Grant],
    resolve: crate::egress::Resolver,
) -> io::Result<Value> {
    crate::windows_shell::validate(text)?;
    let runtime = crate::windows_shell::runtime()?;
    let mut sandbox = crate::windows_sandbox::Sandbox::new_shell(grants, Path::new(cwd))?;
    // `npm` and `npx` typed in the command run MiniClaw's npm with the private Node.
    sandbox.add_private_npm()?;
    let proxy = if hosts.is_empty() {
        None
    } else {
        // Only a run with approved hosts gets the capabilities Winsock needs.
        sandbox.enable_network()?;
        Some(crate::egress::Proxy::start(hosts.to_vec(), resolve).await?)
    };
    let mut env = sandbox
        .environment()
        .into_iter()
        .map(|(key, value)| {
            Ok((
                key.into_string().map_err(|_| io::ErrorKind::InvalidInput)?,
                value
                    .into_string()
                    .map_err(|_| io::ErrorKind::InvalidInput)?,
            ))
        })
        .collect::<io::Result<Vec<_>>>()?;
    if let Some(proxy) = &proxy {
        let url = format!("http://127.0.0.1:{}", proxy.port);
        for name in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            env.push((name.into(), url.clone()));
        }
        env.push(("NO_PROXY".into(), String::new()));
        env.push(("no_proxy".into(), String::new()));
        // Node uses the proxy variables only when told to (Node 24, "Built-in proxy support").
        env.push(("NODE_USE_ENV_PROXY".into(), "1".into()));
    }
    env.push(("HOSTNAME".into(), "miniclaw".into()));
    let path = sandbox.scratch().join("command.sh");
    fs::write(&path, text)?;
    let args = vec!["sh".into(), crate::windows_shell::native_path(&path.to_string_lossy())];
    let pending = crate::windows_process::pending_stdio_restricted(
        &runtime.program,
        &args,
        Path::new(cwd),
        &env,
        &sandbox.security(),
    )?;
    sandbox.bind_process(&pending.server.process)?;
    let _lease = if let Some(proxy) = &proxy {
        Some(
            crate::windows_network_broker::connect(
                &pending.server.process,
                &sandbox.profile_name(),
                proxy.port,
            )
            .await?,
        )
    } else {
        None
    };
    let server = pending.resume()?;
    let (output, errors, code) = tokio::time::timeout(
        Duration::from_secs(120),
        crate::windows_process::capture_stdio(server),
    )
    .await
    .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
    let mut result = json!({ "status": "ok", "text": String::from_utf8_lossy(&output), "stderr": String::from_utf8_lossy(&errors), "exitCode": code });
    if let Some(proxy) = proxy {
        result["refusedHosts"] = json!(proxy.refused());
    }
    Ok(result)
}

/// Git for the product's own views of a working folder. `None` while the sandbox or the private
/// Git is unavailable: no sandbox is created and the folder's permissions are left untouched, so
/// callers treat the folder as one without a repository.
pub async fn workspace_git(arguments: &[&str], cwd: &str, grants: &[Grant]) -> Option<Value> {
    if !git_available() {
        return None;
    }
    Some(execute_git(arguments, cwd, grants).await)
}

/// Runs the installer-owned Git executable with literal argv inside the workspace sandbox.
pub async fn execute_git(arguments: &[&str], cwd: &str, grants: &[Grant]) -> Value {
    if let Some(reason) = crate::seatbelt::unavailable_reason() {
        return json!({"status":"denied", "text":format!("Sandboxed Git refused: {reason}")});
    }
    let result = until_shutdown(async {
        let runtime = crate::windows_git::load()?.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Embedded Git is not installed"))?;
        let mut sandbox = crate::windows_sandbox::Sandbox::new_shell(grants, Path::new(cwd))?;
        let mut env = sandbox.environment().into_iter().map(|(name, value)| {
            Ok((name.into_string().map_err(|_| io::ErrorKind::InvalidInput)?,
                value.into_string().map_err(|_| io::ErrorKind::InvalidInput)?))
        }).collect::<io::Result<Vec<_>>>()?;
        env.extend([
            ("GIT_TERMINAL_PROMPT".into(), "0".into()),
            ("GIT_OPTIONAL_LOCKS".into(), "0".into()),
            ("GIT_LITERAL_PATHSPECS".into(), "1".into()),
        ]);
        let empty_config = env.iter().find(|(name, _)| name == "GIT_CONFIG_GLOBAL")
            .ok_or(io::ErrorKind::InvalidData)?.1.as_str();
        let arguments = crate::workspace_commands::git_arguments(arguments, empty_config);
        let pending = crate::windows_process::pending_stdio_restricted(
            &runtime.root.join(&runtime.program), &arguments, Path::new(cwd), &env, &sandbox.security())?;
        sandbox.bind_process(&pending.server.process)?;
        let (output, errors, code) = tokio::time::timeout(Duration::from_secs(120),
            crate::windows_process::capture_stdio(pending.resume()?)).await
            .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
        Ok::<_, io::Error>(json!({ "status": "ok", "text": String::from_utf8_lossy(&output),
            "stderr": String::from_utf8_lossy(&errors), "exitCode": code }))
    }).await;
    result.unwrap_or_else(|error| {
        // The Changes panel only learns that Git did not answer; the reason belongs in the log.
        tracing::warn!(%error, "Sandboxed Git could not run");
        json!({"status":"denied", "text":format!("Sandboxed Git refused: {error}")})
    })
}

/// Runs exact PowerShell only after the caller validates a live unrestricted-shell grant.
pub async fn execute_unrestricted(text: &str, cwd: &str) -> Value {
    let result = async {
        let args = crate::powershell::arguments(text)?;
        let _directory = open_folder(cwd)?;
        let root = std::env::var_os("SystemRoot").ok_or(io::ErrorKind::NotFound)?;
        let program = Path::new(&root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let (output, errors, code) = tokio::time::timeout(
            Duration::from_secs(86_400),
            crate::windows_process::capture(&program, &args, Path::new(cwd)),
        )
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
        Ok::<Value, io::Error>(
            json!({ "status": "ok", "text": String::from_utf8_lossy(&output),
            "stderr": String::from_utf8_lossy(&errors), "exitCode": code }),
        )
    }
    .await;
    result.unwrap_or_else(
        |error| json!({ "status": "failed", "text": format!("Shell command failed: {error}") }),
    )
}

/// Copies a scoped file into private recovery storage before deleting its pinned source object.
pub fn recover_workspace_file(
    root: &Path,
    relative: &str,
    recovery: &Path,
) -> io::Result<std::path::PathBuf> {
    if relative.is_empty() || !crate::workspace_commands::relative_path(relative, true) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let grants = [Grant {
        path: root.to_string_lossy().into_owned(),
        folder: true,
        write: true,
        blocked: false,
    }];
    let policy = compile_policy(&grants)?;
    let mut source = platform_fs::guarded_delete_path(&root.join(relative))?;
    if !policy.allows(&source, true)? {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    platform_fs::private_dir(recovery, true)?;
    let folder = recovery.join(uuid::Uuid::new_v4().to_string());
    platform_fs::private_dir(&folder, false)?;
    let name = source.path.file_name().ok_or(io::ErrorKind::InvalidInput)?;
    let target = folder.join(name);
    let mut saved = platform_fs::private_file(&target, true)?;
    // Source sharing excludes writers and renames for the entire copy/delete sequence.
    std::io::copy(&mut source.file, &mut saved)?;
    saved.sync_all()?;
    platform_fs::delete_open_file(&source.file)?;
    Ok(target)
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    /// The open sandbox journals of this run's isolated data folder.
    fn journals() -> Vec<String> {
        let data = std::path::PathBuf::from(std::env::var_os("MINICLAW_DATA_DIR").expect("Isolated native test data root"));
        let mut names: Vec<String> = fs::read_dir(data.join("sandbox-journals")).into_iter().flatten().flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned()).filter(|name| name.ends_with(".jsonl")).collect();
        names.sort();
        names
    }

    /// Removes a fixture once the cleanup worker has let go of it: a finished run's sandbox is
    /// revoked there, and holds the folder until it is.
    async fn remove(root: std::path::PathBuf) {
        tokio::task::spawn_blocking(|| crate::windows_sandbox::drain_cleanup(Duration::from_secs(35))).await.unwrap().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    /// A folder with a file, its grant, and a reader of the access lists a sandbox would stamp.
    fn fixture() -> (std::path::PathBuf, String, [Grant; 1], impl Fn(&Path) -> String) {
        use std::os::windows::process::CommandExt;
        let root = std::env::temp_dir().join(format!("mc-gate-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("file.txt"), "content").unwrap();
        let cwd = root.to_string_lossy().into_owned();
        let grants = [Grant { path: cwd.clone(), write: true, folder: true, blocked: false }];
        let acl = |path: &Path| {
            let output = std::process::Command::new(Path::new(&std::env::var_os("SystemRoot").unwrap()).join("System32/icacls.exe"))
                .arg(path).creation_flags(0x0800_0000).output().unwrap();
            String::from_utf8_lossy(&output.stdout).into_owned()
        };
        (root, cwd, grants, acl)
    }

    /// The product's Git runs exactly when the gate and the private Git allow it; otherwise it
    /// creates no sandbox and changes no permission. Asserts whichever state this run is in.
    #[tokio::test]
    async fn workspace_git_follows_the_gate() {
        let (root, cwd, grants, acl) = fixture();
        let before = (journals(), acl(&root), acl(&root.join("file.txt")));
        let answer = workspace_git(&["rev-parse", "--is-inside-work-tree"], &cwd, &grants).await;
        println!("GATE sandbox={} reason={:?} git={} git_reason={:?}", sandbox_available(), crate::seatbelt::unavailable_reason(),
            git_available(), git_unavailable_reason());
        if git_available() {
            assert!(sandbox_available(), "Git is never available without the sandbox");
            let answer = answer.expect("an open gate runs Git");
            // The folder is no repository: Git itself ran inside the sandbox and said so.
            assert_eq!(answer["status"], "ok", "{answer}");
            assert_ne!(answer["exitCode"], 0, "{answer}");
            assert!(answer["stderr"].as_str().unwrap().contains("not a git repository"), "{answer}");
        } else {
            assert!(answer.is_none(), "a closed gate answers without Git: {answer:?}");
            assert!(git_unavailable_reason().is_some_and(|reason| reason.ends_with('.')));
            assert_eq!((journals(), acl(&root), acl(&root.join("file.txt"))), before, "nothing was stamped or journaled");
        }
        remove(root).await;
    }

    /// The shell primitive follows the gate as well: open, the command runs inside the sandbox;
    /// closed, it is refused with the gate's reason and nothing is stamped or journaled.
    #[tokio::test]
    async fn shell_follows_the_gate() {
        let (root, cwd, grants, acl) = fixture();
        let before = (journals(), acl(&root), acl(&root.join("file.txt")));
        let answer = execute_host(Operation::RunShell { text: "echo inside > made.txt; cat file.txt".into(), cwd, hosts: vec![] }, &grants).await;
        match crate::seatbelt::unavailable_reason() {
            None => {
                assert!(sandbox_available());
                assert_eq!(answer["status"], "ok", "{answer}");
                assert_eq!(answer["exitCode"], 0, "{answer}");
                assert_eq!(answer["text"], "content", "{answer}");
                assert_eq!(fs::read_to_string(root.join("made.txt")).unwrap().trim(), "inside");
                tokio::task::spawn_blocking(|| crate::windows_sandbox::drain_cleanup(Duration::from_secs(35))).await.unwrap().unwrap();
                assert_eq!((journals(), acl(&root), acl(&root.join("file.txt"))), before, "the finished run left no journal and no entry on the folder");
            }
            Some(reason) => {
                assert!(!sandbox_available());
                assert_eq!(answer["status"], "denied", "{answer}");
                assert_eq!(answer["text"], format!("Sandboxed command refused: {reason}"));
                assert!(!root.join("made.txt").exists(), "nothing ran");
                assert_eq!((journals(), acl(&root), acl(&root.join("file.txt"))), before, "nothing was stamped or journaled");
            }
        }
        remove(root).await;
    }

    /// With `MINICLAW_DISABLE_SANDBOX` set for the whole test process the gate is closed whatever
    /// the installation offers, every sandboxed entry point refuses with that reason, and the
    /// file tools, which need no sandbox, still work.
    #[tokio::test]
    #[ignore = "run in a process started with MINICLAW_DISABLE_SANDBOX=1"]
    async fn the_switch_closes_every_sandboxed_entry_point() {
        assert!(std::env::var_os(crate::windows_gate::SWITCH_OFF).is_some_and(|value| !value.is_empty()), "start this test with the switch set");
        let reason = crate::windows_gate::Missing::SwitchedOff.reason();
        assert!(!sandbox_available() && !git_available());
        assert_eq!(crate::seatbelt::unavailable_reason().as_deref(), Some(reason.as_str()));
        assert_eq!(git_unavailable_reason().as_deref(), Some(reason.as_str()));
        let (root, cwd, grants, acl) = fixture();
        let before = (journals(), acl(&root), acl(&root.join("file.txt")));
        let shell = execute_host(Operation::RunShell { text: "echo x > made.txt".into(), cwd: cwd.clone(), hosts: vec![] }, &grants).await;
        assert_eq!(shell["text"], format!("Sandboxed command refused: {reason}"), "{shell}");
        assert!(workspace_git(&["status"], &cwd, &grants).await.is_none());
        let git = execute_git(&["status"], &cwd, &grants).await;
        assert_eq!(git["text"], format!("Sandboxed Git refused: {reason}"), "{git}");
        assert!(!root.join("made.txt").exists());
        assert_eq!((journals(), acl(&root), acl(&root.join("file.txt"))), before, "nothing was stamped or journaled");
        let read = execute_host(Operation::ReadFile { path: root.join("file.txt").to_string_lossy().into_owned() }, &grants).await;
        assert_eq!(read["text"], "content", "{read}");
        remove(root).await;
    }

    /// Shutdown drops a sandboxed command in flight, so its journal reaches the cleanup queue.
    #[tokio::test]
    #[ignore = "ends every sandbox run of this process; run it alone, with the pinned shell staged"]
    async fn shutdown_ends_a_command_in_flight() {
        let root = std::env::temp_dir().join(format!("mc-run-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let cwd = root.to_string_lossy().into_owned();
        let grants = vec![Grant { path: cwd.clone(), write: true, folder: true, blocked: false }];
        let before = journals();
        let (run_cwd, run_grants) = (cwd.clone(), grants.clone());
        let run = tokio::spawn(async move {
            execute_host(Operation::RunShell { text: "sleep 100".into(), cwd: run_cwd, hosts: vec![] }, &run_grants).await
        });
        // The command is in flight once its sandbox has a journal.
        for _ in 0..200 {
            if journals() != before { break; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(journals().len(), before.len() + 1, "the running command has an open journal");
        tokio::time::sleep(Duration::from_secs(2)).await;
        end_runs(Duration::from_secs(10)).await.unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(5), run).await.unwrap().unwrap();
        assert_eq!(ended["status"], "denied", "{ended}");
        assert!(ended["text"].as_str().unwrap().contains("shutting down"), "{ended}");
        tokio::task::spawn_blocking(|| crate::windows_sandbox::drain_cleanup(Duration::from_secs(35))).await.unwrap().unwrap();
        assert_eq!(journals(), before, "the command's journal was resolved");
        // Nothing starts once shutdown began.
        let late = execute_host(Operation::RunShell { text: "true".into(), cwd, hosts: vec![] }, &grants).await;
        assert_eq!(late["status"], "denied", "{late}");
        assert_eq!(journals(), before);
        tokio::task::spawn_blocking(crate::windows_sandbox::recover).await.unwrap().unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
