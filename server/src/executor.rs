// Keeps guest file descriptors and command paths closed to model shortcuts.
use crate::scoped::Operation;
// Opens each path component without following links.
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, open, openat, statat};
// Encodes one bounded worker result.
use serde_json::{Value, json};
// Reads, edits, and owns kernel file descriptors.
use std::{fs::{self, File}, io::{self, Read, Write},
    os::unix::fs::MetadataExt,
    path::{Component, Path}, process::Stdio, time::Duration};
// Runs approved commands with a bounded lifetime.
use tokio::{io::AsyncReadExt, process::Command};

/// Runs a host shell without Seatbelt; callers must hold a live timed grant.
pub async fn execute_unrestricted(text: &str, cwd: &str) -> Value {
    command(shell(), text, cwd, None, Vec::new(), crate::egress::system_resolver()).await
        .unwrap_or_else(|error| json!({ "status": "failed", "text": format!("Shell command failed: {error}") }))
}

struct ProcessGroup(i32);
impl Drop for ProcessGroup {
    /// Stops descendants in this command's process group, including on cancellation.
    fn drop(&mut self) { unsafe { libc::kill(-self.0, libc::SIGKILL); } }
}

const MAX_FILE: u64 = 1_048_576;
const MAX_TEXT: usize = 32_768;

/// Runs one already-authorized operation on this machine, the Mac or a VM guest. Shell
/// commands run inside a Seatbelt profile built from the conversation's grants. Apple
/// Events and GUI operations are refused: they make another, unsandboxed app act.
pub async fn execute_host(operation: Operation, grants: &[crate::seatbelt::Grant]) -> Value {
    execute_host_resolving(operation, grants, crate::egress::system_resolver()).await
}

/// As `execute_host`, with the way approved host names are turned into addresses supplied
/// by the caller. Production resolves through the system and keeps public addresses only.
pub async fn execute_host_resolving(operation: Operation, grants: &[crate::seatbelt::Grant],
    resolve: crate::egress::Resolver) -> Value {
    // File operations run outside the sandbox, so the protected roots are checked here.
    let roots = crate::seatbelt::protected();
    if let Operation::ReadFile { path } | Operation::WriteFile { path, .. }
        | Operation::ListFolder { path } | Operation::Search { path, .. }
        | Operation::ReplaceInFile { path, .. } = &operation
        && (crate::seatbelt::is_protected(path, &roots) || std::fs::canonicalize(path)
            .is_ok_and(|real| crate::seatbelt::is_protected(&real.to_string_lossy(), &roots))) {
        return json!({ "status": "denied" });
    }
    let result = match operation {
        Operation::ReadFile { path } => read_file(&path).map(|text| json!({
            "status": "ok", "text": text,
        })),
        Operation::WriteFile { path, text } => {
            // The old content is read first so the page can show what changed. A path that
            // names nothing yet becomes a new file.
            let before = read_file(&path).ok();
            let created = before.is_none() && !exists(&path);
            let written = if created { create_file(&path, &text) } else { write_file(&path, &text) };
            written.map(|_| json!({
                "status": "ok", "text": if created { "File created." } else { "File updated." },
                "diff": diff(before.as_deref().unwrap_or_default(), &text),
            }))
        }
        Operation::ListFolder { path } => list_folder(&path).map(|text| json!({
            "status": "ok", "text": text,
        })),
        Operation::Search { path, text } => search(&path, &text, &roots).map(|text| json!({
            "status": "ok", "text": text,
        })),
        Operation::ReplaceInFile { path, find, text } => replace_in_file(&path, &find, &text),
        Operation::RunShell { text, cwd, hosts } if crate::seatbelt::available() =>
            command(shell(), &text, &cwd, Some(grants), hosts, resolve).await,
        _ => Err(io::ErrorKind::Unsupported.into()),
    };
    result.unwrap_or_else(|_| json!({ "status": "denied" }))
}

/// The shell commands run in: zsh on the Mac, and on Linux where it is installed; otherwise
/// bash, or `sh` where bash is missing too.
pub fn shell() -> &'static str {
    if cfg!(target_os = "linux") {
        ["/bin/zsh", "/bin/bash"].into_iter().find(|path| std::path::Path::new(path).is_file()).unwrap_or("/bin/sh")
    } else { "/bin/zsh" }
}

/// The most entries a listing names, matches a search reports, files it opens and folders
/// deep it goes. A model can narrow its request; an unbounded answer helps nobody.
const MAX_ENTRIES: usize = 500;
const MAX_MATCHES: usize = 200;
const MAX_SEARCHED: usize = 5_000;
const MAX_DEPTH: usize = 24;

/// Names and kinds of one open folder's entries, sorted. Links are reported, never followed.
fn entries(folder: &File) -> io::Result<Vec<(String, FileType)>> {
    let mut found = Vec::new();
    for entry in Dir::read_from(folder)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "." || name == ".." { continue; }
        found.push((name, entry.file_type()));
    }
    found.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(found)
}

/// Lists the direct entries of one real folder as `name<TAB>kind[<TAB>bytes]` lines.
pub fn list_folder(path: &str) -> io::Result<String> {
    let folder = safe_open(path, false, true)?;
    let mut lines = Vec::new();
    for (name, kind) in entries(&folder)?.into_iter().take(MAX_ENTRIES + 1) {
        if lines.len() == MAX_ENTRIES { lines.push("More entries exist.".to_owned()); break; }
        lines.push(match kind {
            FileType::Directory => format!("{name}\tfolder"),
            FileType::Symlink => format!("{name}\tlink"),
            FileType::RegularFile => {
                let size = statat(&folder, name.as_str(), AtFlags::SYMLINK_NOFOLLOW)?.st_size;
                format!("{name}\tfile\t{size}")
            }
            _ => format!("{name}\tother"),
        });
    }
    Ok(if lines.is_empty() { "The folder is empty.".into() } else { lines.join("\n") })
}

/// Searches the regular text files below one folder for literal text. It opens every entry
/// without following links, skips protected roots, and stops at its limits.
pub fn search(path: &str, text: &str, protected: &[String]) -> io::Result<String> {
    let mut state = Search { text, protected, matches: Vec::new(), searched: 0, more: false };
    state.folder(&safe_open(path, false, true)?, path, 0)?;
    if state.matches.is_empty() { return Ok("No matches.".into()); }
    if state.more { state.matches.push("More matches exist; narrow the search.".into()); }
    Ok(state.matches.join("\n"))
}

struct Search<'a> {
    text: &'a str,
    protected: &'a [String],
    matches: Vec<String>,
    searched: usize,
    more: bool,
}

impl Search<'_> {
    /// Walks one open folder; `path` is only used to report where a match is.
    fn folder(&mut self, folder: &File, path: &str, depth: usize) -> io::Result<()> {
        for (name, kind) in entries(folder)? {
            if self.more { return Ok(()); }
            let child = format!("{path}/{name}");
            if crate::seatbelt::is_protected(&child, self.protected) { continue; }
            let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            match kind {
                FileType::Directory if depth < MAX_DEPTH => {
                    // A folder that cannot be opened is skipped, not fatal.
                    if let Ok(fd) = openat(folder, name.as_str(), flags | OFlags::DIRECTORY, Mode::empty()) {
                        self.folder(&File::from(fd), &child, depth + 1)?;
                    }
                }
                FileType::RegularFile => {
                    if self.searched == MAX_SEARCHED { self.more = true; return Ok(()); }
                    self.searched += 1;
                    if let Ok(fd) = openat(folder, name.as_str(), flags, Mode::empty()) {
                        self.file(File::from(fd), &child);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Records the matching lines of one small UTF-8 file; anything else is skipped.
    fn file(&mut self, file: File, path: &str) {
        let Ok(info) = file.metadata() else { return };
        if !info.is_file() || info.len() > MAX_FILE { return; }
        let mut bytes = Vec::new();
        if file.take(MAX_FILE).read_to_end(&mut bytes).is_err() { return; }
        let Ok(content) = String::from_utf8(bytes) else { return };
        for (number, line) in content.lines().enumerate() {
            if !line.contains(self.text) { continue; }
            if self.matches.len() == MAX_MATCHES { self.more = true; return; }
            let shown: String = line.trim().chars().take(200).collect();
            self.matches.push(format!("{path}:{}: {shown}", number + 1));
        }
    }
}

/// The most diff text one change reports to the page.
const MAX_DIFF: usize = 64 * 1024;

/// Describes a change as a unified diff for the user. The model never receives it. A diff
/// beyond the limit is cut at a line and says so.
pub fn diff(before: &str, after: &str) -> String {
    if before == after { return String::new(); }
    let text = similar::TextDiff::from_lines(before, after).unified_diff().context_radius(3).to_string();
    if text.len() <= MAX_DIFF { return text; }
    let mut end = MAX_DIFF;
    while !text.is_char_boundary(end) { end -= 1; }
    let cut = text[..end].rfind('\n').unwrap_or(end);
    format!("{}\n... the rest of this change is not shown ({} bytes).\n", &text[..cut], text.len() - cut)
}

/// Replaces one passage that occurs exactly once. Anything else changes nothing and tells
/// the model why, so it can send a longer, unambiguous passage.
fn replace_in_file(path: &str, find: &str, text: &str) -> io::Result<Value> {
    let content = read_file(path)?;
    let count = content.matches(find).count();
    if count != 1 {
        return Ok(json!({ "status": "failed", "text": if count == 0 {
            "The passage was not found; nothing was changed.".to_owned()
        } else {
            format!("The passage occurs {count} times; nothing was changed. Include more surrounding text.")
        } }));
    }
    let updated = content.replacen(find, text, 1);
    if updated.len() as u64 > MAX_FILE { return Err(io::ErrorKind::InvalidInput.into()); }
    write_bytes(path, &updated)?;
    Ok(json!({ "status": "ok", "text": "File updated.", "diff": diff(&content, &updated) }))
}

/// Opens a folder for a shell to start in, the way a command's working directory is opened:
/// component by component, following no link. The descriptor keeps the folder pinned.
pub(crate) fn open_folder(path: &str) -> io::Result<File> { safe_open(path, false, true) }

/// Opens one absolute path component by component without link traversal.
fn safe_open(path: &str, write: bool, directory: bool) -> io::Result<File> {
    if !path.starts_with('/') || path[1..].split('/').any(|part|
        part.is_empty() || matches!(part, "." | "..")) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let path = Path::new(path);
    if !path.is_absolute() || path.as_os_str().len() > 4096 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let parts = path.components().collect::<Vec<_>>();
    if parts.len() < 2 || parts.iter().skip(1).any(|part| !matches!(part, Component::Normal(_))) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut fd = open("/", OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty())?;
    for (index, part) in parts.iter().enumerate().skip(1) {
        let last = index + 1 == parts.len();
        let flags = if !last || directory {
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
        } else if write {
            OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC
        } else {
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC
        };
        let Component::Normal(name) = part else { unreachable!() };
        fd = openat(&fd, *name, flags, Mode::empty())?;
    }
    Ok(File::from(fd))
}

/// Reads one exact regular UTF-8 file through a bounded descriptor.
pub fn read_file(path: &str) -> io::Result<String> {
    let file = safe_open(path, false, false)?;
    let info = file.metadata()?;
    if !info.is_file() || info.nlink() != 1 || info.len() > MAX_FILE {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE { return Err(io::ErrorKind::InvalidData.into()); }
    String::from_utf8(bytes).map_err(|_| io::ErrorKind::InvalidData.into())
}

/// Reports whether anything has this name, without following a link.
pub fn exists(path: &str) -> bool { std::fs::symlink_metadata(path).is_ok() }

/// Creates a new regular file in an existing folder. Every folder on the way is opened without
/// following links, and the file must not exist yet, so nothing is ever replaced here.
pub fn create_file(path: &str, text: &str) -> io::Result<()> {
    if text.len() > MAX_TEXT || text.contains('\0') { return Err(io::ErrorKind::InvalidInput.into()); }
    let (folder, name) = path.rsplit_once('/').filter(|(folder, name)| !folder.is_empty() && !name.is_empty()
        && !matches!(*name, "." | "..")).ok_or(io::ErrorKind::InvalidInput)?;
    let parent = safe_open(folder, false, true)?;
    let fd = openat(&parent, name, OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o644))?;
    let mut file = File::from(fd);
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

/// Replaces an existing exact regular file after descriptor and size checks.
pub fn write_file(path: &str, text: &str) -> io::Result<()> {
    if text.len() > MAX_TEXT || text.contains('\0') {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    write_bytes(path, text)
}

/// Replaces the content of one existing regular file that has no other hard link.
fn write_bytes(path: &str, text: &str) -> io::Result<()> {
    let mut file = safe_open(path, true, false)?;
    let info = file.metadata()?;
    if !info.is_file() || info.nlink() != 1 || info.len() > MAX_FILE {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    file.set_len(0)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

/// One command's private temp directory under its resolved path; removed on drop.
pub(crate) struct Scratch(pub(crate) String);

impl Scratch {
    /// Creates a fresh 0700 directory that the sandbox profile can name exactly.
    pub(crate) fn create() -> io::Result<Self> {
        use std::os::unix::fs::DirBuilderExt;
        let base = std::env::temp_dir().canonicalize()?;
        let path = base.join(format!("mc-run-{}", uuid::Uuid::new_v4().simple()));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path.to_string_lossy().into_owned()))
    }
}

impl Drop for Scratch {
    /// Removes whatever the command left behind.
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

/// Runs only the exact approved script from an already-open working directory,
/// inside a Seatbelt profile when grants are supplied.
async fn command(program: &str, text: &str, cwd: &str,
    sandbox: Option<&[crate::seatbelt::Grant]>, hosts: Vec<String>,
    resolve: crate::egress::Resolver) -> io::Result<Value> {
    if text.is_empty() || text.len() > 16_384 || text.contains('\0') {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let directory = safe_open(cwd, false, true)?;
    // A sandboxed command gets one private temp directory, removed when it ends.
    let scratch = match sandbox {
        Some(_) => Some(Scratch::create()?),
        None => None,
    };
    // Approved hosts get one proxy for this command; without any, nothing listens.
    let proxy = match (sandbox, hosts.is_empty()) {
        (Some(_), false) => Some(crate::egress::Proxy::start(hosts, resolve).await?),
        _ => None,
    };
    let mut child = match (sandbox, &scratch) {
        (Some(grants), Some(scratch)) => {
            let mut grants = grants.to_vec();
            grants.insert(0, crate::seatbelt::Grant { path: scratch.0.clone(), write: true,
                folder: true, blocked: false });
            crate::seatbelt::command_with(&grants, program, proxy.as_ref().map(|proxy| proxy.port))
        }
        _ => Command::new(program),
    };
    if program == shell() { child.args(["-c", text]); }
    else { child.args(["-e", text]); }
    child.env_clear().env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(proxy) = &proxy {
        let address = format!("http://127.0.0.1:{}", proxy.port);
        for name in ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] { child.env(name, &address); }
    }
    if let Some(scratch) = &scratch {
        // zsh writes here-documents under TMPPREFIX, which ignores TMPDIR.
        child.env("TMPDIR", &scratch.0).env("TMPPREFIX", format!("{}/zsh", scratch.0));
    }
    // The open descriptor pins cwd even if its path is replaced before spawn.
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    let fd = directory.as_raw_fd();
    unsafe {
        child.as_std_mut().pre_exec(move || {
            if libc::fchdir(fd) != 0 { return Err(io::Error::last_os_error()); }
            Ok(())
        });
    }
    if sandbox.is_none() {
        child.process_group(0);
        child.env("PATH", "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin");
        for name in ["HOME", "USER", "TMPDIR", "LANG"] {
            if let Some(value) = std::env::var_os(name) { child.env(name, value); }
        }
    }
    let mut child = child.spawn()?;
    let _group = if sandbox.is_none() { child.id().map(|id| ProcessGroup(id as i32)) } else { None };
    let stdout = child.stdout.take().ok_or(io::ErrorKind::BrokenPipe)?;
    let stderr = child.stderr.take().ok_or(io::ErrorKind::BrokenPipe)?;
    let limit = Duration::from_secs(if sandbox.is_none() { 86400 } else if proxy.is_some() { 120 } else { 10 });
    let result = tokio::time::timeout(limit, async {
        let output = async {
            let mut bytes = Vec::new();
            stdout.take(65_537).read_to_end(&mut bytes).await?;
            Ok::<_, io::Error>(bytes)
        };
        let errors = async {
            let mut bytes = Vec::new();
            stderr.take(65_537).read_to_end(&mut bytes).await?;
            Ok::<_, io::Error>(bytes)
        };
        let (output, errors, status) = tokio::try_join!(output, errors, child.wait())?;
        Ok::<_, io::Error>((output, errors, status))
    }).await;
    let (output, errors, status) = match result {
        Ok(result) => result?,
        Err(_) => {
            let _ = child.kill().await;
            return Err(io::ErrorKind::TimedOut.into());
        }
    };
    if output.len() + errors.len() > 65_536 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut result = json!({ "status": "ok", "text": String::from_utf8_lossy(&output),
        "stderr": String::from_utf8_lossy(&errors), "exitCode": status.code() });
    if let Some(proxy) = &proxy { result["refusedHosts"] = json!(proxy.refused()); }
    Ok(result)
}
