//! The right-hand panel's Files and Changes: a read-only look at a conversation's working
//! folder, and its git working tree.
//!
//! Every read and every git call goes through the same sandboxed executor as an agent's own
//! tool calls, with a grant for the working folder only. That matters for git: a repository's
//! configuration can make git run programs (`core.fsmonitor`, `diff.external`, filters), and an
//! agent can write to this folder. Inside the sandbox such a program can reach nothing but the
//! folder, and the options below switch the known hooks off as well. It also means a VM
//! conversation works through the VM's existing worker, with no code of its own.

// Shares the conversation checks and the executor.
use super::*;
use crate::scoped::Operation;
use crate::seatbelt::Grant;

/// The most text the viewer shows, and the most files Changes lists.
const VIEW: usize = 512 * 1024;
const FILES: usize = 500;

/// Where a conversation's files live, and how to reach them.
struct Place { root: String, remote: Option<Client>, grants: Vec<Grant> }

impl Place {
    /// Runs one executor operation where the conversation runs: on this Mac, or in its VM.
    async fn run(&self, operation: Operation) -> Result<Value> {
        Ok(match &self.remote {
            Some(worker) => worker.call("miniclaw/scoped/execute", json!({ "operation": operation, "grants": self.grants })).await?,
            None => crate::executor::execute_host(operation, &self.grants).await,
        })
    }

    /// Runs one git command in the working folder and returns its output, or `None` when git failed.
    async fn git(&self, arguments: &[&str]) -> Result<Option<String>> {
        #[cfg(windows)]
        let done = if self.remote.is_none() {
            // While Windows cannot enforce the sandbox, nothing is stamped on the folder for
            // Git: the folder is treated as one without a repository.
            let Some(done) = crate::executor::workspace_git(arguments, &self.root, &self.grants).await
                else { return Ok(None) };
            done
        } else {
            self.run(Operation::RunShell { text: crate::workspace_commands::git_script(arguments), cwd: self.root.clone(), hosts: Vec::new() }).await?
        };
        #[cfg(not(windows))]
        let done = self.run(Operation::RunShell { text: crate::workspace_commands::git_script(arguments), cwd: self.root.clone(), hosts: Vec::new() }).await?;
        Ok((done["status"] == "ok" && done["exitCode"] == 0).then(|| done["text"].as_str().unwrap_or_default().to_owned()))
    }
}

/// Reads Git's answer to `ls-files --error-unmatch` for one path: tracked, not tracked, or
/// unknown. Exit 1 is Git saying the path is not tracked. Any other failure says nothing about
/// the file unless Git names the cause as a folder without a repository; in particular Git
/// refuses a repository whose owner it does not trust ("dubious ownership"), and a tracked
/// file there must not be taken for a new one.
#[cfg(any(windows, test))]
fn tracked(answer: Option<&Value>) -> Option<bool> {
    let answer = answer.filter(|answer| answer["status"] == "ok")?;
    match answer["exitCode"].as_i64()? {
        0 => Some(true),
        1 => Some(false),
        _ => answer["stderr"].as_str().is_some_and(|words| words.contains("not a git repository")).then_some(false),
    }
}

fn no_folder() -> Error {
    Error(StatusCode::CONFLICT, "no_working_folder",
        "This conversation has no working folder. Give its project a workspace folder to see files and changes.", false)
}

/// Finds the working folder: the project's workspace on this Mac, or the VM's Workspace.
async fn place(backend: &Shared, thread: &str) -> Result<Place> {
    authorized_thread(backend, thread).await?;
    let (root, remote) = match backend.agents.assignment(thread.to_owned()).await? {
        Some(Assignment::Vm { vm_id }) => {
            let client = if vm_id == "master" { backend.primary_client() } else { backend.dedicated_client(&vm_id) };
            (backend.cwd.clone(), Some(client.ok_or_else(vm_control_unavailable)?))
        }
        _ => (backend.agents.project_context(thread.to_owned(), None, false).await?
            .and_then(|context| context.project.definition.workspace).ok_or_else(no_folder)?, None),
    };
    let mut grants = vec![Grant { path: root.clone(), write: true, folder: true, blocked: false }];
    if remote.is_some() {
        grants.push(Grant { path: "/Users/Shared/MiniClaw/Transfers".into(), write: true, folder: true, blocked: false });
    }
    Ok(Place { root, remote, grants })
}

/// Accepts a plain relative path inside the folder; git's own directory is never offered.
fn inside(path: &str) -> Result<&str> {
    if crate::workspace_commands::relative_path(path, cfg!(windows)) { Ok(path) } else { Err(invalid()) }
}

/// Quotes one path for the shell.
fn quoted(path: &str) -> String { crate::workspace_commands::posix_argument(path) }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PathQuery { #[serde(default)] path: String }

/// Lists one folder of the working folder: folders first, then files, by name.
pub(super) async fn files(State(backend): State<Shared>, Path(thread): Path<String>, Query(query): Query<PathQuery>)
    -> Result<Json<Value>> {
    let place = place(&backend, &thread).await?;
    let relative = inside(&query.path)?;
    let folder = if relative.is_empty() { place.root.clone() } else { format!("{}/{relative}", place.root) };
    let listed = place.run(Operation::ListFolder { path: folder }).await?;
    if listed["status"] != "ok" { return Err(missing()); }
    let mut entries: Vec<(bool, String, u64)> = listed["text"].as_str().unwrap_or_default().lines().filter_map(|line| {
        let mut fields = line.split('\t');
        // A folder's line has no size; a link is never offered.
        let (name, kind, size) = (fields.next()?, fields.next()?, fields.next().unwrap_or("0"));
        (name != ".git" && matches!(kind, "file" | "folder")).then(|| (kind == "file", name.to_owned(), size.parse().unwrap_or(0)))
    }).collect();
    entries.sort();
    Ok(Json(json!({ "root": place.root, "path": relative, "entries": entries.into_iter().map(|(file, name, size)|
        json!({ "name": name, "kind": if file { "file" } else { "folder" }, "size": size })).collect::<Vec<_>>() })))
}

/// Reads one file for the viewer. A file the executor cannot read as text is reported, not shown.
pub(super) async fn file(State(backend): State<Shared>, Path(thread): Path<String>, Query(query): Query<PathQuery>)
    -> Result<Json<Value>> {
    let place = place(&backend, &thread).await?;
    let relative = inside(&query.path)?;
    if relative.is_empty() { return Err(invalid()); }
    let read = place.run(Operation::ReadFile { path: format!("{}/{relative}", place.root) }).await?;
    match read["text"].as_str().filter(|_| read["status"] == "ok") {
        Some(text) if text.len() <= VIEW => Ok(Json(json!({ "path": relative, "text": text, "size": text.len() }))),
        Some(text) => Ok(Json(json!({ "path": relative, "text": null, "size": text.len(), "reason": "This file is too large to show here." }))),
        None => {
            // The folder listing tells a missing file from one that is not text.
            let exists = place.run(Operation::ListFolder { path: match relative.rsplit_once('/') {
                Some((folder, _)) => format!("{}/{folder}", place.root), None => place.root.clone() } }).await?;
            let name = relative.rsplit('/').next().unwrap_or_default();
            let listed = exists["text"].as_str().unwrap_or_default().lines()
                .any(|line| line.split('\t').next() == Some(name) && line.split('\t').nth(1) == Some("file"));
            if !listed { return Err(missing()); }
            Ok(Json(json!({ "path": relative, "text": null, "reason": "This file is not text, or is too large to show here." })))
        }
    }
}

/// One changed file for the list.
fn changed(path: &str, status: &str, added: u64, removed: u64) -> Value {
    json!({ "path": path, "status": status, "added": added, "removed": removed })
}

/// Reads what the agent changed in this conversation from MiniClaw's own record.
fn recorded(backend: &Backend, thread: &str, root: &str) -> Vec<(String, String)> {
    backend.activity.list(thread).into_iter().filter_map(|entry| {
        let path = crate::workspace_commands::activity_path(root, entry["title"].as_str()?, cfg!(windows))?;
        Some((path, entry["diff"].as_str().filter(|diff| !diff.is_empty())?.to_owned()))
    }).collect()
}

/// Counts added and removed lines of a unified diff.
fn counts(diff: &str) -> (u64, u64) {
    diff.lines().fold((0, 0), |(added, removed), line| (
        added + u64::from(line.starts_with('+') && !line.starts_with("+++")),
        removed + u64::from(line.starts_with('-') && !line.starts_with("---"))))
}

/// Lists the changed files: the git working tree against the last commit, or, where the folder
/// is no repository, what the agent changed in this conversation.
pub(super) async fn changes(State(backend): State<Shared>, Path(thread): Path<String>) -> Result<Json<Value>> {
    let place = place(&backend, &thread).await?;
    if place.git(&["rev-parse", "--is-inside-work-tree"]).await?.is_none_or(|answer| answer.trim() != "true") {
        let mut files: Vec<Value> = Vec::new();
        for (path, diff) in recorded(&backend, &thread, &place.root) {
            let (added, removed) = counts(&diff);
            match files.iter_mut().find(|file| file["path"] == path.as_str()) {
                Some(file) => { file["added"] = json!(file["added"].as_u64().unwrap_or(0) + added);
                    file["removed"] = json!(file["removed"].as_u64().unwrap_or(0) + removed); }
                None => files.push(changed(&path, "modified", added, removed)),
            }
        }
        return Ok(Json(json!({ "source": "conversation", "root": place.root, "files": files })));
    }
    let status = place.git(&["status", "--porcelain=v1", "--untracked-files=all", "--no-renames"]).await?.unwrap_or_default();
    // A repository without a commit has no HEAD to compare with; every file is new there.
    let numstat = place.git(&["diff", "HEAD", "--numstat", "--no-ext-diff", "--no-textconv", "--no-renames"]).await?.unwrap_or_default();
    let mut files = Vec::new();
    for line in status.lines().take(FILES) {
        let (Some(code), Some(path)) = (line.get(..2), line.get(3..)) else { continue };
        let path = path.trim_matches('"');
        if inside(path).is_err() { continue; }
        let (added, removed) = numstat.lines().find_map(|row| {
            let mut fields = row.splitn(3, '\t');
            let (added, removed, name) = (fields.next()?, fields.next()?, fields.next()?);
            (name == path).then(|| (added.parse().unwrap_or(0), removed.parse().unwrap_or(0)))
        }).unwrap_or((0, 0));
        let kind = if code == "??" || code.contains('A') { "new" } else if code.contains('D') { "deleted" } else { "modified" };
        let added = if kind == "new" && added == 0 {
            place.run(Operation::ReadFile { path: format!("{}/{path}", place.root) }).await?["text"].as_str()
                .map_or(0, |text| text.lines().count() as u64)
        } else { added };
        files.push(changed(path, kind, added, removed));
    }
    files.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    Ok(Json(json!({ "source": "git", "root": place.root, "files": files })))
}

/// Returns one file's unified diff against the last commit, or against nothing for a new file.
pub(super) async fn diff(State(backend): State<Shared>, Path(thread): Path<String>, Query(query): Query<PathQuery>)
    -> Result<Json<Value>> {
    let place = place(&backend, &thread).await?;
    let relative = inside(&query.path)?;
    if relative.is_empty() { return Err(invalid()); }
    if place.git(&["rev-parse", "--is-inside-work-tree"]).await?.is_none() {
        let parts: Vec<String> = recorded(&backend, &thread, &place.root).into_iter()
            .filter(|(path, _)| path == relative).map(|(_, diff)| diff).collect();
        return Ok(Json(json!({ "path": relative, "diff": parts.join("\n") })));
    }
    let tracked = place.git(&["ls-files", "--error-unmatch", "--", relative]).await?.is_some();
    let text = if tracked {
        place.git(&["diff", "HEAD", "--no-ext-diff", "--no-textconv", "--no-color", "--", relative]).await?.unwrap_or_default()
    } else {
        let read = place.run(Operation::ReadFile { path: format!("{}/{relative}", place.root) }).await?;
        crate::executor::diff("", read["text"].as_str().unwrap_or_default())
    };
    // Only the hunks are shown; the file is named by the panel.
    let hunks = text.find("\n@@").map_or(text.as_str(), |start| &text[start + 1..]);
    Ok(Json(json!({ "path": relative, "diff": hunks })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Discard { path: String }

/// Discards one file's changes: a tracked file returns to the last commit, a new file is moved
/// away where it can be recovered (the Trash on this Mac, `Transfers/discarded` in a VM).
pub(super) async fn discard(State(backend): State<Shared>, Path(thread): Path<String>, input: Body<Discard>)
    -> Result<Json<Value>> {
    let input = body(input)?;
    let place = place(&backend, &thread).await?;
    let relative = inside(&input.path)?;
    if relative.is_empty() { return Err(invalid()); }
    let failed = || Error(StatusCode::CONFLICT, "discard_failed", "The change could not be discarded.", false);
    // Without Git the page offers no discard: a tracked file could not be told from a new one,
    // and would be moved away instead of restored.
    #[cfg(windows)]
    if place.remote.is_none() && let Some(reason) = crate::executor::git_unavailable_reason() {
        let message = crate::seatbelt::kept_refusal("Changes cannot be discarded while the Windows sandbox that runs Git is unavailable.", Some(&reason));
        return Err(Error(StatusCode::CONFLICT, "discard_unavailable", message, false));
    }
    // On Windows Git may not have answered at all: the sandbox can refuse the folder before Git
    // starts (a file with a second name outside it, too many files), and Git refuses a repository
    // whose owner it does not trust. A tracked file must then not be taken for a new one and moved away.
    #[cfg(windows)]
    if place.remote.is_none() {
        let listed = crate::executor::workspace_git(&["ls-files", "--error-unmatch", "--", relative], &place.root, &place.grants).await;
        if tracked(listed.as_ref()).is_none() {
            let untrusted = listed.is_some_and(|done| done["stderr"].as_str().is_some_and(|words| words.contains("dubious ownership")));
            return Err(if untrusted { Error(StatusCode::CONFLICT, "discard_failed",
                "The change could not be discarded: Git does not trust the owner of this folder, so it cannot say what the file was.", false) } else { failed() });
        }
    }
    if place.git(&["ls-files", "--error-unmatch", "--", relative]).await?.is_some() {
        place.git(&["restore", "--source=HEAD", "--staged", "--worktree", "--", relative]).await?.ok_or_else(failed)?;
        return Ok(Json(json!({})));
    }
    let name = relative.rsplit('/').next().unwrap_or_default();
    if place.remote.is_some() {
        let moved = place.run(Operation::RunShell { cwd: place.root.clone(), hosts: Vec::new(), text: format!(
            "mkdir -p /Users/Shared/MiniClaw/Transfers/discarded && mv -n -- {} /Users/Shared/MiniClaw/Transfers/discarded/", quoted(relative)) }).await?;
        if moved["exitCode"] != 0 { return Err(failed()); }
        return Ok(Json(json!({})));
    }
    let trash = super::trash_dir(&backend).ok_or_else(failed)?;
    #[cfg(windows)]
    {
        let target = crate::executor::recover_workspace_file(std::path::Path::new(&place.root), relative, &trash)
            .map_err(|_| failed())?;
        return Ok(Json(json!({"recoveryPath": target})));
    }
    #[cfg(unix)]
    {
    // On this Mac the file goes to the Trash. It must be a plain file inside the folder.
    let source = std::path::Path::new(&place.root).join(relative);
    let real = std::fs::canonicalize(&source).map_err(|_| missing())?;
    if !real.starts_with(&place.root) || !std::fs::symlink_metadata(&source).is_ok_and(|found| found.is_file()) { return Err(invalid()); }
    std::fs::create_dir_all(&trash).map_err(|_| failed())?;
    let mut target = trash.join(name);
    if target.exists() { target = trash.join(format!("{name} {}", chrono::Utc::now().format("%H.%M.%S"))); }
    std::fs::rename(&source, &target).map_err(|_| failed())?;
    Ok(Json(json!({"recoveryPath": target})))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only Git's own "not tracked" and "no repository" make a file new; a Git that could not
    /// run, or that refuses the repository, leaves the question open.
    #[test]
    fn a_file_is_new_only_when_git_says_so() {
        let answer = |status: &str, code: i64, stderr: &str| json!({ "status": status, "exitCode": code, "text": "", "stderr": stderr });
        assert_eq!(tracked(Some(&answer("ok", 0, ""))), Some(true));
        assert_eq!(tracked(Some(&answer("ok", 1, "error: pathspec 'new.txt' did not match any file(s) known to git"))), Some(false));
        assert_eq!(tracked(Some(&answer("ok", 128, "fatal: not a git repository (or any of the parent directories): .git"))), Some(false));
        assert_eq!(tracked(Some(&answer("ok", 128, "fatal: detected dubious ownership in repository at 'C:/work'"))), None);
        assert_eq!(tracked(Some(&answer("ok", 128, ""))), None);
        assert_eq!(tracked(Some(&answer("denied", 0, ""))), None);
        assert_eq!(tracked(Some(&json!({ "status": "denied", "text": "Sandboxed Git refused: Permission denied" }))), None);
        assert_eq!(tracked(None), None);
    }
}
