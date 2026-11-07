//! One shared shell per conversation. It lives on a pseudo-terminal under the same Seatbelt
//! profile as the conversation's commands, in a private home with a fixed PATH, and keeps its
//! state between uses: the user types into it from the page, and the agent's approved commands
//! run in it too. Output goes into a ring the page reads from, so a page that arrives late
//! still sees the screen. Nothing here decides permissions; it is given grants and hosts.

// Grants and the sandboxed command builder.
use crate::seatbelt::{self, Grant};
// The private temp folder each shell gets.
#[cfg(unix)]
use crate::executor::Scratch;
// One proxy per shell, its hosts set per command.
use crate::egress;
// Keeps the shells and shares one across tasks.
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
// The pseudo-terminal, the shell child and its output.
#[cfg(unix)]
use std::os::fd::{AsRawFd, OwnedFd};
#[cfg(unix)]
use tokio::io::unix::AsyncFd;
#[cfg(unix)]
use tokio::io::Interest;
// Tells readers that new bytes exist.
use tokio::sync::watch;
// Times idle shells.
use std::time::{Duration, Instant};

/// How much output a late page can still see.
pub const RING_BYTES: usize = 256 * 1024;
/// Most bytes one input request may carry.
pub const INPUT_BYTES: usize = 4 * 1024;
/// A shell nobody used for this long is closed.
const IDLE: Duration = Duration::from_secs(30 * 60);

/// What a shell is built from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec { pub cwd: String, pub grants: Vec<Grant>, pub hosts: Vec<String>, pub cols: u16, pub rows: u16 }

/// Output kept for late readers: the newest `RING_BYTES`, and how many bytes were ever written.
#[derive(Default)]
pub struct Ring { bytes: VecDeque<u8>, pub end: u64 }

impl Ring {
    /// Appends output, dropping the oldest bytes past the size.
    pub fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend(chunk);
        self.end += chunk.len() as u64;
        let over = self.bytes.len().saturating_sub(RING_BYTES);
        if over > 0 { self.bytes.drain(..over); }
    }

    /// Returns the bytes after an offset a reader has seen. A reader behind the ring gets
    /// everything kept and is told its view was reset; one ahead of the end is treated as caught up.
    pub fn since(&self, after: u64) -> (Vec<u8>, bool) {
        let start = self.end - self.bytes.len() as u64;
        if after < start { return (self.bytes.iter().copied().collect(), true); }
        if after > self.end { return (self.bytes.iter().copied().collect(), true); }
        if after == self.end { return (Vec::new(), false); }
        (self.bytes.iter().skip((after - start) as usize).copied().collect(), false)
    }
}

/// The running shell process and its terminal.
pub struct Shell {
    #[cfg(unix)]
    child: tokio::process::Child,
    #[cfg(windows)]
    native: crate::windows_process::PtyProcess,
    #[cfg(windows)]
    input: std::sync::mpsc::SyncSender<Vec<u8>>,
    #[cfg(windows)]
    _sandbox: crate::windows_sandbox::Sandbox,
    #[cfg(windows)]
    _network: Option<crate::windows_network_broker::ClientLease>,
    #[cfg(unix)]
    master: Arc<AsyncFd<OwnedFd>>,
    #[cfg(unix)]
    _scratch: Scratch,
    proxy: egress::Proxy,
    pub spec: Spec,
    /// On Linux the sandbox binds each granted path itself, not its name: the identity of each
    /// at start, so a folder deleted and made again leads to a fresh shell that sees the new one.
    #[cfg(target_os = "linux")]
    identities: Vec<Option<(u64, u64)>>,
}

/// The device and inode of each granted path, `None` for one that does not exist.
#[cfg(target_os = "linux")]
fn identities(grants: &[Grant]) -> Vec<Option<(u64, u64)>> {
    use std::os::unix::fs::MetadataExt;
    grants.iter().map(|grant| std::fs::metadata(&grant.path).ok().map(|meta| (meta.dev(), meta.ino()))).collect()
}

/// One conversation's terminal.
pub struct Terminal {
    pub shell: tokio::sync::Mutex<Option<Shell>>,
    /// Set when the shell runs inside a VM: the worker that has it, and the pump that reads it.
    pub remote: Mutex<Option<Remote>>,
    pub ring: Mutex<Ring>,
    /// Bumped whenever the ring changes or the shell ends; readers wait on it.
    pub seq: watch::Sender<u64>,
    /// Set once the shell exited, with its status, until it is started again.
    pub exited: Mutex<Option<i32>>,
    /// Words the page shows once, such as why the shell was restarted.
    pub notices: Mutex<Vec<String>>,
    pub last_used: Mutex<Instant>,
}

/// Every conversation's terminal, by conversation id.
#[derive(Default)]
pub struct Terminals { all: Mutex<HashMap<String, Arc<Terminal>>> }

/// A shell that lives in a VM, reached through its worker. The Mac keeps the ring; a pump task
/// long-polls the worker for output so the page and the agent path are the same as for a Mac shell.
pub struct Remote { pub client: crate::rpc::Client, pub thread: String, pub spec: Spec, pump: tokio::task::JoinHandle<()> }

impl Drop for Remote {
    /// Stops reading when the terminal is closed.
    fn drop(&mut self) { self.pump.abort(); }
}

/// Writes the private `.zshrc`: a plain prompt, history in the private home, and a hook that marks
/// each prompt so an agent command can wait for one. The `__mc` function runs an agent's command
/// between markers the page never shows (an unknown OSC number is invisible in xterm).
const ZSHRC: &str = r#"
# The prompt names the folder under the working folder, so it stays short however deep that folder is.
__mc_prompt() { local here="$PWD"; case "$here" in "$MC_ROOT") here="${MC_ROOT:t}";; "$MC_ROOT"/*) here="${MC_ROOT:t}/${here#$MC_ROOT/}";; esac; PROMPT="%F{blue}${here}%f %# "; }
precmd_functions+=(__mc_prompt)
HISTFILE="$HOME/.zsh_history"
HISTSIZE=2000
SAVEHIST=2000
unsetopt beep
bindkey -e
precmd() { printf '\e]7331;prompt\a'; }
__mc() {
  local run="$MC_RUN/$1.zsh"
  [ -r "$run" ] || { printf 'MiniClaw: nothing to run\n'; return 1; }
  local dir; IFS= read -r dir < "$run"; dir="${dir#\# }"
  printf '\e[1A\e[2K\r'
  printf '\e[2m%s $ %s\e[0m\n' "${dir/#$HOME/~}" "$(tail -n +2 "$run")"
  cd -- "$dir" 2>/dev/null
  printf '\e]7331;start;%s\a' "$1"
  source "$run"
  local code=$?
  printf '\e]7331;end;%s;%s\a' "$1" "$code"
  return $code
}
"#;

/// The same prompt and command markers for bash, on a Linux without zsh.
const BASHRC: &str = r#"
__mc_prompt() { printf '\e]7331;prompt\a'; local here="$PWD"; case "$here" in "$MC_ROOT") here="${MC_ROOT##*/}";; "$MC_ROOT"/*) here="${MC_ROOT##*/}/${here#$MC_ROOT/}";; esac; PS1="\[\e[34m\]${here}\[\e[0m\] \$ "; }
PROMPT_COMMAND=__mc_prompt
HISTFILE="$HOME/.bash_history"
HISTSIZE=2000
set -o emacs
bind 'set bell-style none' 2>/dev/null
__mc() {
  local run="$MC_RUN/$1.zsh"
  [ -r "$run" ] || { printf 'MiniClaw: nothing to run\n'; return 1; }
  local dir; IFS= read -r dir < "$run"; dir="${dir#\# }"
  printf '\e[1A\e[2K\r'
  printf '\e[2m%s $ %s\e[0m\n' "${dir/#$HOME/\~}" "$(tail -n +2 "$run")"
  cd -- "$dir" 2>/dev/null
  printf '\e]7331;start;%s\a' "$1"
  source "$run"
  local code=$?
  printf '\e]7331;end;%s;%s\a' "$1" "$code"
  return $code
}
"#;

impl Shell {
    /// Starts a shell for a spec: a pseudo-terminal, a private home and temp folder, a proxy for
    /// its life, and `zsh -i` under the profile of the grants. `-i` and not `-l`: a login shell
    /// runs `/etc/zprofile`, which widens PATH beyond the sandbox's fixed one.
    #[cfg(unix)]
    pub async fn spawn(spec: Spec) -> std::io::Result<Self> {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let scratch = Scratch::create()?;
        let home = format!("{}/home", scratch.0);
        std::fs::DirBuilder::new().mode(0o700).create(&home)?;
        std::fs::DirBuilder::new().mode(0o700).create(format!("{}/run", scratch.0))?;
        let program = crate::executor::shell();
        let bash = program.ends_with("/bash");
        if program.ends_with("/sh") {
            return Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "the shared shell needs zsh or bash"));
        }
        let (rc, text) = if bash { (".bashrc", BASHRC) } else { (".zshrc", ZSHRC) };
        std::fs::OpenOptions::new().create(true).write(true).mode(0o600).open(format!("{home}/{rc}"))?.write_all(text.as_bytes())?;
        let proxy = egress::Proxy::start(spec.hosts.clone(), egress::system_resolver()).await?;
        let winsize = rustix::termios::Winsize { ws_col: spec.cols, ws_row: spec.rows, ws_xpixel: 0, ws_ypixel: 0 };
        let pty = rustix_openpty::openpty(None, Some(&winsize)).map_err(std::io::Error::from)?;
        let tty = rustix::termios::ttyname(&pty.user, Vec::new()).map_err(std::io::Error::from)?;
        let tty = tty.to_string_lossy().into_owned();
        // The scratch folder is a grant of its own, so the shell may use its temp and run files.
        let mut grants = spec.grants.clone();
        grants.push(Grant { path: scratch.0.clone(), write: true, folder: true, blocked: false });
        let cwd = crate::executor::open_folder(&spec.cwd)?;
        let mut command = seatbelt::command_terminal(&grants, program, proxy.port, &tty);
        // bash reads only MiniClaw's rc file, never the system's or the user's start-up files.
        if bash { command.arg("--noprofile").arg("--rcfile").arg(format!("{home}/.bashrc")); }
        command.arg("-i");
        let proxy_url = format!("http://127.0.0.1:{}", proxy.port);
        command.env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", &home).env("ZDOTDIR", &home)
            .env("TMPDIR", &scratch.0).env("TMPPREFIX", format!("{}/zsh", scratch.0))
            .env("MC_RUN", format!("{}/run", scratch.0)).env("MC_ROOT", &spec.cwd)
            .env("TERM", "xterm-256color").env("LANG", "en_US.UTF-8")
            .env("HTTPS_PROXY", &proxy_url).env("https_proxy", &proxy_url)
            .env("ALL_PROXY", &proxy_url).env("all_proxy", &proxy_url);
        let slave = pty.user;
        command.stdin(std::process::Stdio::from(slave.try_clone()?))
            .stdout(std::process::Stdio::from(slave.try_clone()?))
            .stderr(std::process::Stdio::from(slave));
        let cwd_fd = cwd.as_raw_fd();
        // The child becomes the terminal's session leader and starts in the pinned folder.
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() < 0 { return Err(std::io::Error::last_os_error()); }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 { return Err(std::io::Error::last_os_error()); }
                if libc::fchdir(cwd_fd) != 0 { return Err(std::io::Error::last_os_error()); }
                Ok(())
            });
        }
        command.kill_on_drop(true);
        let child = command.spawn()?;
        drop(cwd);
        let master = Arc::new(AsyncFd::with_interest(set_nonblocking(pty.controller)?, Interest::READABLE)?);
        #[cfg(target_os = "linux")]
        let identities = identities(&spec.grants);
        Ok(Self { child, master, _scratch: scratch, proxy, spec, #[cfg(target_os = "linux")] identities })
    }

    /// Writes bytes to the shell's terminal, as typed.
    #[cfg(unix)]
    pub fn write(&self, bytes: &[u8]) -> std::io::Result<()> {
        let fd = self.master.as_raw_fd();
        let mut sent = 0;
        while sent < bytes.len() {
            let wrote = unsafe { libc::write(fd, bytes[sent..].as_ptr().cast(), bytes.len() - sent) };
            if wrote < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::WouldBlock { std::thread::sleep(Duration::from_millis(5)); continue; }
                return Err(error);
            }
            sent += wrote as usize;
        }
        Ok(())
    }

    /// Tells the terminal its new size; the shell learns of it through SIGWINCH.
    #[cfg(unix)]
    pub fn resize(&self, cols: u16, rows: u16) -> std::io::Result<()> {
        let winsize = rustix::termios::Winsize { ws_col: cols, ws_row: rows, ws_xpixel: 0, ws_ypixel: 0 };
        rustix::termios::tcsetwinsize(self.master.get_ref(), winsize).map_err(std::io::Error::from)
    }

    /// Replaces the hosts the shell's next connections may reach.
    pub fn set_hosts(&self, hosts: Vec<String>) { self.proxy.set_hosts(hosts); }

    /// Returns and forgets the hosts the proxy refused.
    pub fn take_refused(&self) -> Vec<String> { self.proxy.take_refused() }

    /// Where an agent's command file goes.
    #[cfg(unix)]
    pub fn run_dir(&self) -> String { format!("{}/run", self._scratch.0) }
}

/// Puts a descriptor into non-blocking mode.
#[cfg(unix)]
fn set_nonblocking(fd: OwnedFd) -> std::io::Result<OwnedFd> {
    let raw = fd.as_raw_fd();
    let flags = unsafe { libc::fcntl(raw, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(fd)
}

/// A marker the shell printed for MiniClaw, parsed out of its output.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Marker { Prompt, Start(String), End(String, i32) }

/// Finds the markers in a stream of output, across chunk boundaries. Everything else passes through
/// unchanged, markers included: xterm does not show them, and the capture strips them itself.
#[derive(Default)]
pub struct Markers { held: Vec<u8> }

impl Markers {
    /// Feeds one chunk and returns the markers completed by it. A partial marker at the end is held.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<Marker> {
        const HEAD: &[u8] = b"\x1b]7331;";
        self.held.extend_from_slice(chunk);
        let mut found = Vec::new();
        let mut from = 0;
        while let Some(at) = find(&self.held[from..], HEAD).map(|offset| offset + from) {
            let body = at + HEAD.len();
            let Some(bell) = self.held[body..].iter().position(|byte| *byte == 0x07) else { break; };
            let text = String::from_utf8_lossy(&self.held[body..body + bell]).into_owned();
            let mut parts = text.split(';');
            match (parts.next(), parts.next(), parts.next()) {
                (Some("prompt"), None, None) => found.push(Marker::Prompt),
                (Some("start"), Some(call), None) if valid(call) => found.push(Marker::Start(call.into())),
                (Some("end"), Some(call), Some(code)) if valid(call) => if let Ok(code) = code.parse() { found.push(Marker::End(call.into(), code)); },
                _ => {}
            }
            from = body + bell + 1;
        }
        // Keep only what could still be the start of a marker.
        let keep = self.held.len().saturating_sub(HEAD.len() + 80);
        let keep = keep.max(from);
        self.held.drain(..keep.min(self.held.len()));
        found
    }
}

/// Reports whether a call id in a marker is one MiniClaw could have written.
fn valid(call: &str) -> bool { !call.is_empty() && call.len() <= 64 && call.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') }

/// Finds a byte pattern.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> { haystack.windows(needle.len()).position(|window| window == needle) }

impl Terminal {
    /// Makes an empty terminal; the shell is started by `Terminals::ensure`.
    fn new() -> Self {
        Self { shell: tokio::sync::Mutex::new(None), remote: Mutex::new(None), ring: Mutex::new(Ring::default()), seq: watch::channel(0).0,
            exited: Mutex::new(None), notices: Mutex::new(Vec::new()), last_used: Mutex::new(Instant::now()) }
    }

    /// Wakes readers.
    fn bump(&self) { self.seq.send_modify(|seq| *seq += 1); }

    /// The size the shell was last given, on the Mac or in the VM.
    pub async fn size(&self) -> (u16, u16) {
        if let Some(remote) = self.remote.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref() { return (remote.spec.cols, remote.spec.rows); }
        self.shell.lock().await.as_ref().map(|shell| (shell.spec.cols, shell.spec.rows)).unwrap_or((100, 30))
    }

    /// Types into the shell, wherever it runs.
    pub async fn type_in(&self, text: &str) -> std::io::Result<()> {
        let remote = self.remote.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref().map(|remote| (remote.client.clone(), remote.thread.clone()));
        if let Some((client, thread)) = remote {
            let answer = client.call("miniclaw/terminal/input", serde_json::json!({ "thread": thread, "text": text })).await.map_err(|_| std::io::Error::other("worker"))?;
            return if answer.get("error").is_some() { Err(std::io::Error::other("ended")) } else { Ok(()) };
        }
        match self.shell.lock().await.as_ref() { Some(shell) => shell.write(text.as_bytes()), None => Err(std::io::Error::other("ended")) }
    }

    /// Resizes the shell, wherever it runs.
    pub async fn resize(&self, cols: u16, rows: u16) -> std::io::Result<()> {
        let remote = self.remote.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref().map(|remote| (remote.client.clone(), remote.thread.clone()));
        if let Some((client, thread)) = remote {
            let _ = client.call("miniclaw/terminal/resize", serde_json::json!({ "thread": thread, "cols": cols, "rows": rows })).await;
            return Ok(());
        }
        match self.shell.lock().await.as_ref() { Some(shell) => shell.resize(cols, rows), None => Ok(()) }
    }

    /// Notes use, so the idle sweep leaves the shell alone.
    pub fn touch(&self) { if let Ok(mut used) = self.last_used.lock() { *used = Instant::now(); } }
}

/// What one agent command produced in the shared shell.
pub struct Ran { pub output: Vec<u8>, pub total: u64, pub code: Option<i32>, pub refused: Vec<String>, pub ended_shell: bool }

/// Most output the model is given from one command; the terminal keeps showing the rest. Under
/// the tool result's own 65 000-byte bound with room for the truncation note.
pub const CAPTURE_BYTES: usize = 60 * 1024;
/// How long a command may take before the model is told it is still running.
pub const RUN_LIMIT: Duration = Duration::from_secs(120);
/// How long a run waits for the shell to show a prompt before giving up.
const PROMPT_WAIT: Duration = Duration::from_secs(60);

impl Terminal {
    /// Reads the run limit, which a test build may shorten.
    fn run_limit() -> Duration {
        #[cfg(debug_assertions)]
        if let Some(ms) = std::env::var("MINICLAW_TEST_TERMINAL_TIMEOUT_MS").ok().and_then(|text| text.parse().ok()) { return Duration::from_millis(ms); }
        RUN_LIMIT
    }

    /// Runs one approved command in the shell and returns what it printed, its exit code and the
    /// hosts the proxy refused. The command is written to a file only the shell reads, typed as
    /// `__mc <call>`, and its output is everything between the start and end markers, kept up to
    /// `CAPTURE_BYTES`. Past the run limit the output so far is returned with no exit code; the
    /// command keeps running in view, and `finished` is called with the rest when it ends.
    pub async fn run(self: &Arc<Self>, call: &str, text: &str, cwd: &str, hosts: Vec<String>,
        finished: impl FnOnce(Ran) + Send + 'static) -> std::io::Result<Ran> {
        use std::io::Write;
        #[cfg(unix)]
        use std::os::unix::fs::OpenOptionsExt;
        self.touch();
        // Wait for a prompt: the shell must be idle before a line is typed into it.
        let mut seq = self.seq.subscribe();
        let ring_start = self.ring.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).end;
        let started = Instant::now();
        loop {
            if self.at_prompt(ring_start) { break; }
            if started.elapsed() > PROMPT_WAIT { return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "busy")); }
            let _ = tokio::time::timeout(Duration::from_millis(500), seq.changed()).await;
        }
        let guard = self.shell.lock().await;
        let shell = guard.as_ref().ok_or_else(|| std::io::Error::other("no shell"))?;
        #[cfg(windows)]
        if call.is_empty() || call.len() > 128 || !call.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')) {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        let from = self.ring.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).end;
        #[cfg(unix)]
        {
        let path = format!("{}/{call}.zsh", shell.run_dir());
        let mut file = std::fs::OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(&path)?;
        // The first line is the approved folder, as a comment so `source` skips it.
        file.write_all(format!("# {cwd}\n{text}\n").as_bytes())?;
        drop(file);
        shell.set_hosts(hosts);
        let _ = shell.take_refused();
        // Ctrl-U clears anything half typed; the line is typed as the user would type it.
        shell.write(format!("\x15__mc {call}\r").as_bytes())?;
        }
        #[cfg(windows)]
        {
            if !hosts.is_empty() && shell._network.is_none() { return Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "Restart the terminal to apply its network permission")); }
            shell.set_hosts(hosts);
            let _ = shell.take_refused();
            let path = shell._sandbox.scratch().join(format!("{call}.sh"));
            let source = crate::windows_shell::command_source(text, cwd)?;
            std::fs::OpenOptions::new().write(true).create_new(true).open(&path)?.write_all(source.as_bytes())?;
            let command = format!("\x15__mc {call}\r");
            shell.write(command.as_bytes())?;
        }
        // The lock goes now: the capture and the page both need the shell while the command runs.
        drop(guard);
        let capture = Capture::new(self.clone(), call.to_owned(), from);
        let deadline = tokio::time::sleep(Self::run_limit());
        tokio::pin!(deadline);
        // Subscribed before the read, so a bump between the two is never missed.
        let mut seq = self.seq.subscribe();
        seq.mark_changed();
        loop {
            tokio::select! {
                _ = &mut deadline => break,
                changed = seq.changed() => if changed.is_err() { break; },
            }
            if let Some(ran) = capture.finished().await { return Ok(ran); }
            // `exit` inside the command ends the shell itself; what it printed comes back with the shell's code.
            let exited = *self.exited.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(code) = exited {
                let mut ran = capture.so_far().await;
                ran.code = Some(code);
                ran.ended_shell = true;
                return Ok(ran);
            }
        }
        // Still running: hand the rest to the caller's continuation and return what there is.
        let partial = capture.so_far().await;
        let terminal = self.clone();
        tokio::spawn(async move {
            let mut seq = terminal.seq.subscribe();
            seq.mark_changed();
            loop {
                if seq.changed().await.is_err() { return; }
                if let Some(ran) = capture.finished().await { finished(ran); return; }
                let exited = *terminal.exited.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(code) = exited {
                    let mut ran = capture.so_far().await;
                    ran.code = Some(code);
                    ran.ended_shell = true;
                    finished(ran);
                    return;
                }
            }
        });
        Ok(partial)
    }

    /// Reports whether the shell is waiting at a prompt: the last marker in its output is the
    /// prompt marker, so no command is running and nothing was typed after it.
    fn at_prompt(&self, _since: u64) -> bool {
        let ring = self.ring.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (all, _) = ring.since(0);
        let found = Markers::default().feed(&all);
        let prompt_last = matches!(found.last(), Some(Marker::Prompt));
        // zsh switches bracketed paste off (`ESC [?2004l`) the moment a line is accepted, so that
        // sequence after the last prompt means the user sent a command that has not finished.
        let after_prompt = all.rsplit(|byte| *byte == 0x07).next().unwrap_or_default();
        prompt_last && find(after_prompt, b"\x1b[?2004l").is_none()
    }
}

/// Reads one command's output out of the ring, between its markers.
struct Capture { terminal: Arc<Terminal>, call: String, from: u64 }

impl Capture {
    fn new(terminal: Arc<Terminal>, call: String, from: u64) -> Self { Self { terminal, call, from } }

    /// Parses the ring from the command's start and returns the run once its end marker is there.
    async fn finished(&self) -> Option<Ran> {
        let (bytes, code) = self.read();
        let code = code?;
        Some(self.ran(bytes, Some(code)).await)
    }

    /// What the command printed so far, with no exit code.
    async fn so_far(&self) -> Ran { let (bytes, _) = self.read(); self.ran(bytes, None).await }

    /// Splits the ring after the start marker: the output between the markers, and the exit code
    /// when the end marker has arrived.
    fn read(&self) -> (Vec<u8>, Option<i32>) {
        let ring = self.terminal.ring.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (bytes, _) = ring.since(self.from.min(ring.end));
        let start = format!("\x1b]7331;start;{}\x07", self.call);
        let end = format!("\x1b]7331;end;{};", self.call);
        let Some(at) = find(&bytes, start.as_bytes()) else { return (Vec::new(), None); };
        let body = &bytes[at + start.len()..];
        match find(body, end.as_bytes()) {
            Some(stop) => {
                let rest = &body[stop + end.len()..];
                let code = rest.iter().position(|byte| *byte == 0x07)
                    .and_then(|bell| String::from_utf8_lossy(&rest[..bell]).parse().ok());
                (body[..stop].to_vec(), code)
            }
            None => (body.to_vec(), None),
        }
    }

    /// Bounds the output for the model and adds what the proxy refused. The terminal's line
    /// endings become plain newlines, as a pipe would have given.
    async fn ran(&self, output: Vec<u8>, code: Option<i32>) -> Ran {
        let mut output: Vec<u8> = { let mut out = Vec::with_capacity(output.len()); let mut i = 0;
            while i < output.len() { if output[i] == b'\r' && output.get(i + 1) == Some(&b'\n') { i += 1; continue; } out.push(output[i]); i += 1; } out };
        let total = output.len() as u64;
        if output.len() > CAPTURE_BYTES {
            output.truncate(CAPTURE_BYTES);
            while !std::str::from_utf8(&output).is_ok() && !output.is_empty() { output.pop(); }
        }
        let refused = match self.terminal.shell.lock().await.as_ref() { Some(shell) => shell.take_refused(), None => Vec::new() };
        Ran { output, total, code, refused, ended_shell: false }
    }
}

impl Terminals {
    /// Returns the conversation's terminal, making one.
    fn entry(&self, thread: &str) -> Arc<Terminal> {
        self.all.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).entry(thread.to_owned()).or_insert_with(|| Arc::new(Terminal::new())).clone()
    }

    /// Returns the conversation's terminal only if it exists.
    pub fn get(&self, thread: &str) -> Option<Arc<Terminal>> {
        self.all.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(thread).cloned()
    }

    /// Returns the terminal with a running shell for this spec. A shell that ended, or one started
    /// with other grants or another folder, is replaced, and the page is told why.
    pub async fn ensure(&self, thread: &str, spec: Spec) -> std::io::Result<Arc<Terminal>> {
        let terminal = self.entry(thread);
        terminal.touch();
        let mut slot = terminal.shell.lock().await;
        let ended = terminal.exited.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).is_some();
        let changed = slot.as_ref().is_some_and(|shell| shell.spec.grants != spec.grants || shell.spec.cwd != spec.cwd);
        #[cfg(windows)]
        let changed = changed || slot.as_ref().is_some_and(|shell| !spec.hosts.is_empty() && shell._network.is_none());
        #[cfg(target_os = "linux")]
        let replaced = !changed && slot.as_ref().is_some_and(|shell| shell.identities != identities(&spec.grants));
        #[cfg(not(target_os = "linux"))]
        let replaced = false;
        if slot.is_some() && !ended && !changed && !replaced {
            if let Some(shell) = slot.as_ref() && shell.spec.hosts != spec.hosts { shell.set_hosts(spec.hosts.clone()); }
            return Ok(terminal.clone());
        }
        if changed {
            terminal.notices.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
                .push("MiniClaw restarted the shell: this conversation's permissions changed.".into());
        } else if replaced {
            terminal.notices.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
                .push("MiniClaw restarted the shell: a folder it works in was replaced.".into());
        }
        *slot = None;
        drop(slot);
        // A new shell starts with an empty screen; a page that was reading is reset to it.
        *terminal.ring.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Ring::default();
        let started = Shell::spawn_into(&terminal, spec).await?;
        *terminal.shell.lock().await = Some(started);
        Ok(terminal)
    }

    /// Returns the terminal of a conversation whose shell runs in a VM, opening it there and
    /// starting the pump that copies its output into the Mac's ring. The worker is asked for a
    /// fresh shell when the grants or folder changed, as on the Mac.
    pub async fn ensure_remote(&self, thread: &str, client: crate::rpc::Client, spec: Spec) -> std::io::Result<Arc<Terminal>> {
        use serde_json::json;
        let terminal = self.entry(thread);
        terminal.touch();
        let same = terminal.remote.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref()
            .is_some_and(|remote| remote.spec.grants == spec.grants && remote.spec.cwd == spec.cwd && remote.client.available());
        let ended = terminal.exited.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).is_some();
        if same && !ended {
            let _ = client.call("miniclaw/terminal/resize", json!({ "thread": thread, "cols": spec.cols, "rows": spec.rows })).await;
            return Ok(terminal);
        }
        let changed = terminal.remote.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref().is_some_and(|remote| remote.spec.grants != spec.grants || remote.spec.cwd != spec.cwd);
        if changed {
            terminal.notices.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push("MiniClaw restarted the shell: this conversation's permissions changed.".into());
        }
        let method = if changed || ended { "miniclaw/terminal/restart" } else { "miniclaw/terminal/open" };
        let opened = client.call(method, json!({ "thread": thread, "cwd": spec.cwd, "grants": spec.grants, "hosts": spec.hosts, "cols": spec.cols, "rows": spec.rows })).await
            .map_err(|_| std::io::Error::other("worker"))?;
        if opened.get("error").is_some() { return Err(std::io::Error::other("refused")); }
        *terminal.ring.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Ring::default();
        *terminal.exited.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        let (reader, pumped, id) = (client.clone(), terminal.clone(), thread.to_owned());
        let pump = tokio::spawn(async move {
            let mut after = 0u64;
            while reader.available() {
                let Ok(read) = reader.call("miniclaw/terminal/read", json!({ "thread": id, "after": after, "wait": 1000 })).await else { break; };
                if read.get("error").is_some() { break; }
                if read["reset"] == true { *pumped.ring.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Ring::default(); }
                let chunk = read["chunk"].as_str().and_then(crate::frames::decode).unwrap_or_default();
                if !chunk.is_empty() { pumped.ring.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(&chunk); }
                for notice in read["notices"].as_array().into_iter().flatten().filter_map(|text| text.as_str()) {
                    pumped.notices.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(notice.to_owned());
                }
                after = read["seq"].as_u64().unwrap_or(after);
                if let Some(code) = read["exited"].as_i64() {
                    *pumped.exited.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(code as i32);
                    pumped.bump();
                    break;
                }
                if !chunk.is_empty() || read["reset"] == true { pumped.bump(); }
            }
        });
        *terminal.remote.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Remote { client, thread: thread.to_owned(), spec, pump });
        terminal.bump();
        Ok(terminal)
    }

    /// Ends and forgets a conversation's shell, in the VM too.
    #[cfg(windows)]
    pub async fn close_all(&self) {
        let threads: Vec<_> = self.all.lock().unwrap_or_else(|e| e.into_inner()).keys().cloned().collect();
        for thread in threads { self.close(&thread).await; }
    }

    /// Ends and forgets a conversation's shell, in the VM too.
    pub async fn close(&self, thread: &str) {
        let removed = self.all.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(thread);
        if let Some(terminal) = removed {
            *terminal.shell.lock().await = None;
            let remote = terminal.remote.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
            if let Some(remote) = remote {
                let _ = remote.client.call("miniclaw/terminal/close", serde_json::json!({ "thread": thread })).await;
            }
            terminal.bump();
        }
    }

    /// Closes shells nobody used for a while.
    pub async fn sweep(&self) {
        let idle: Vec<String> = self.all.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).iter()
            .filter(|(_, terminal)| terminal.last_used.lock().map(|used| used.elapsed() > IDLE).unwrap_or(true))
            .map(|(thread, _)| thread.clone()).collect();
        for thread in idle { self.close(&thread).await; }
    }
}

impl Shell {
    /// Starts a shell and its pump for a terminal, without holding the terminal's shell lock.
    async fn spawn_into(terminal: &Arc<Terminal>, spec: Spec) -> std::io::Result<Shell> {
        let shell = Shell::spawn(spec).await?;
        terminal.pump(&shell);
        Ok(shell)
    }
}

impl Terminal {
    /// Copies the shell's output into the ring until it ends, then records the exit code.
    #[cfg(unix)]
    fn pump(self: &Arc<Self>, shell: &Shell) {
        let master = shell.master.clone();
        let terminal = self.clone();
        let pid = shell.child.id();
        *self.exited.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.bump();
        tokio::spawn(async move {
            let mut buffer = [0u8; 8192];
            loop {
                let Ok(mut ready) = master.readable().await else { break; };
                match ready.try_io(|fd| {
                    let read = unsafe { libc::read(fd.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
                    if read < 0 { Err(std::io::Error::last_os_error()) } else { Ok(read as usize) }
                }) {
                    Ok(Ok(0)) => break,
                    Ok(Ok(count)) => {
                        terminal.ring.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(&buffer[..count]);
                        terminal.bump();
                    }
                    // EIO is how macOS reports the shell side closing: the shell is gone.
                    Ok(Err(error)) => if error.kind() != std::io::ErrorKind::WouldBlock { break; },
                    Err(_) => continue,
                }
            }
            // The exit status is read from the process, whose handle the shell slot still owns.
            let code = pid.map(|pid| {
                let mut status = 0;
                let waited = unsafe { libc::waitpid(pid as i32, &mut status, 0) };
                if waited == pid as i32 && libc::WIFEXITED(status) { libc::WEXITSTATUS(status) } else { -1 }
            }).unwrap_or(-1);
            *terminal.exited.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(code);
            terminal.bump();
        });
    }
}

/// The worker's own shells, one per conversation the Mac names.
fn guest_terminals() -> &'static Terminals {
    static ALL: std::sync::OnceLock<Terminals> = std::sync::OnceLock::new();
    ALL.get_or_init(Terminals::default)
}

/// Answers one `miniclaw/terminal/*` call inside a VM: the same shells, driven by the Mac. The
/// Mac sends the folder and grants it decided on, as it does for a command; the VM's shell is
/// confined by them the same way.
pub async fn guest_call(method: &str, params: &serde_json::Value) -> serde_json::Value {
    use serde_json::json;
    let Some(thread) = params["thread"].as_str().filter(|id| uuid::Uuid::parse_str(id).is_ok()) else { return json!({ "error": "thread" }); };
    let terminals = guest_terminals();
    let size = (params["cols"].as_u64().unwrap_or(100) as u16, params["rows"].as_u64().unwrap_or(30) as u16);
    match method {
        "open" | "restart" => {
            let Ok(grants) = serde_json::from_value::<Vec<Grant>>(params["grants"].clone()) else { return json!({ "error": "grants" }); };
            let Some(cwd) = params["cwd"].as_str() else { return json!({ "error": "cwd" }); };
            if grants.len() > 512 { return json!({ "error": "grants" }); }
            if method == "restart" { terminals.close(thread).await; }
            let hosts: Vec<String> = params["hosts"].as_array().into_iter().flatten().filter_map(|host| host.as_str().map(str::to_owned)).collect();
            match terminals.ensure(thread, Spec { cwd: cwd.to_owned(), grants, hosts, cols: size.0, rows: size.1 }).await {
                Ok(terminal) => {
                    if let Some(shell) = terminal.shell.lock().await.as_ref() { let _ = shell.resize(size.0, size.1); }
                    let exited = *terminal.exited.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    json!({ "seq": terminal.ring.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).end, "status": if exited.is_some() { "ended" } else { "live" } })
                }
                Err(_) => json!({ "error": "failed" }),
            }
        }
        "input" => {
            let Some(terminal) = terminals.get(thread) else { return json!({ "error": "closed" }); };
            let Some(text) = params["text"].as_str().filter(|text| !text.is_empty() && text.len() <= INPUT_BYTES) else { return json!({ "error": "text" }); };
            terminal.touch();
            match terminal.shell.lock().await.as_ref() { Some(shell) if shell.write(text.as_bytes()).is_ok() => json!({}), _ => json!({ "error": "ended" }) }
        }
        "resize" => {
            let Some(terminal) = terminals.get(thread) else { return json!({ "error": "closed" }); };
            match terminal.shell.lock().await.as_ref() { Some(shell) if shell.resize(size.0, size.1).is_ok() => json!({}), _ => json!({ "error": "ended" }) }
        }
        "close" => { terminals.close(thread).await; json!({}) }
        // Waits up to a second for output past `after`, then answers with what there is.
        "read" => {
            let Some(terminal) = terminals.get(thread) else { return json!({ "error": "closed" }); };
            let after = params["after"].as_u64().unwrap_or(0);
            let mut seq = terminal.seq.subscribe();
            seq.mark_changed();
            let deadline = tokio::time::sleep(Duration::from_millis(params["wait"].as_u64().unwrap_or(1000).min(1000)));
            tokio::pin!(deadline);
            loop {
                let (bytes, reset, end) = { let ring = terminal.ring.lock().unwrap_or_else(|poisoned| poisoned.into_inner()); let (bytes, reset) = ring.since(after); (bytes, reset, ring.end) };
                let exited = *terminal.exited.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let notices: Vec<String> = std::mem::take(&mut *terminal.notices.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
                if !bytes.is_empty() || reset || exited.is_some() || !notices.is_empty() {
                    return json!({ "seq": end, "chunk": crate::frames::encode(&bytes), "reset": reset, "exited": exited, "notices": notices });
                }
                tokio::select! { _ = &mut deadline => return json!({ "seq": end, "chunk": "", "reset": false, "exited": null, "notices": [] }),
                    changed = seq.changed() => if changed.is_err() { return json!({ "error": "closed" }); } }
            }
        }
        // Runs an approved command in the VM's shell and answers like `scoped/execute` would.
        "run" => {
            let Some(terminal) = terminals.get(thread) else { return json!({ "error": "closed" }); };
            let (Some(call), Some(text), Some(cwd)) = (params["id"].as_str(), params["text"].as_str(), params["cwd"].as_str()) else { return json!({ "error": "call" }); };
            let hosts: Vec<String> = params["hosts"].as_array().into_iter().flatten().filter_map(|host| host.as_str().map(str::to_owned)).collect();
            // A late finish inside the VM has nobody to tell; the Mac reports the partial result.
            match terminal.run(call, text, cwd, hosts, |_| {}).await {
                Ok(ran) => json!({ "output": crate::frames::encode(&ran.output), "total": ran.total, "code": ran.code, "refused": ran.refused, "endedShell": ran.ended_shell }),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => json!({ "error": "busy" }),
                Err(_) => json!({ "error": "failed" }),
            }
        }
        _ => json!({ "error": "method" }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_answers_by_offset_and_resets_a_reader_it_dropped() {
        let mut ring = Ring::default();
        ring.push(b"hello ");
        ring.push(b"world");
        assert_eq!(ring.since(0), (b"hello world".to_vec(), false));
        assert_eq!(ring.since(6), (b"world".to_vec(), false));
        assert_eq!(ring.since(11), (Vec::new(), false));
        assert_eq!(ring.since(99), (b"hello world".to_vec(), true), "a reader from before a restart is reset");
        ring.push(&vec![b'x'; RING_BYTES]);
        let (bytes, reset) = ring.since(3);
        assert!(reset && bytes.len() == RING_BYTES);
    }

    #[test]
    fn markers_are_found_across_chunks_and_bad_ones_are_ignored() {
        let mut markers = Markers::default();
        assert_eq!(markers.feed(b"text \x1b]7331;pro"), vec![]);
        assert_eq!(markers.feed(b"mpt\x07 more \x1b]7331;start;call-1\x07"), vec![Marker::Prompt, Marker::Start("call-1".into())]);
        assert_eq!(markers.feed(b"\x1b]7331;end;call-1;3\x07\x1b]7331;end;bad id;x\x07\x1b]7331;other\x07"), vec![Marker::End("call-1".into(), 3)]);
        assert_eq!(markers.feed(&vec![b'y'; 5000]), vec![]);
        assert!(markers.held.len() < 200, "held output stays bounded");
    }
}

#[cfg(test)]
mod capture_tests {
    use super::*;

    #[tokio::test]
    async fn a_capture_finds_its_output_between_the_markers() {
        let terminal = Arc::new(Terminal::new());
        terminal.ring.lock().unwrap().push(b"prompt\x1b]7331;prompt\x07 % __mc c1\r\n");
        let from = terminal.ring.lock().unwrap().end;
        let capture = Capture::new(terminal.clone(), "c1".into(), from);
        terminal.ring.lock().unwrap().push(b"\x1b[2m$ echo RAN\x1b[0m\r\n\x1b]7331;start;c1\x07RAN\r\n");
        assert!(capture.finished().await.is_none());
        terminal.ring.lock().unwrap().push(b"\x1b]7331;end;c1;0\x07\x1b]7331;prompt\x07");
        let ran = capture.finished().await.expect("finished");
        assert_eq!((String::from_utf8_lossy(&ran.output).into_owned(), ran.code), ("RAN\n".into(), Some(0)), "terminal line endings become plain newlines");
    }
}

#[cfg(windows)]
impl Shell {
    /// Starts the pinned ConPTY shell with the conversation's explicit file grants.
    pub async fn spawn(spec: Spec) -> std::io::Result<Self> {
        use std::io::Write;
        let proxy = egress::Proxy::start(spec.hosts.clone(), egress::system_resolver()).await?;
        let mut sandbox = crate::windows_sandbox::Sandbox::new_shell(&spec.grants, std::path::Path::new(&spec.cwd))?;
        // `npm` and `npx` typed in the terminal run MiniClaw's npm with the private Node.
        sandbox.add_private_npm()?;
        // Only a terminal with approved hosts gets the capabilities Winsock needs.
        if !spec.hosts.is_empty() { sandbox.enable_network()?; }
        let runtime = crate::windows_shell::runtime()?;
        let startup = sandbox.scratch().join("startup.sh");
        std::fs::write(&startup, crate::windows_shell::STARTUP)?;
        let args = vec!["sh".to_owned(), "-i".to_owned()];
        let mut env = sandbox.environment().into_iter().map(|(name, value)| Ok((name.into_string().map_err(|_| std::io::ErrorKind::InvalidInput)?,
            value.into_string().map_err(|_| std::io::ErrorKind::InvalidInput)?))).collect::<std::io::Result<Vec<_>>>()?;
        env.extend([
            ("HOSTNAME".into(), "miniclaw".into()),
            ("ENV".into(), crate::windows_shell::native_path(&startup.to_string_lossy())),
            ("MC_RUN".into(), crate::windows_shell::native_path(&sandbox.scratch().to_string_lossy())),
            ("TERM".into(), "xterm-256color".into()),
        ]);
        let proxy_url = format!("http://127.0.0.1:{}", proxy.port);
        env.extend([("HTTP_PROXY".into(), proxy_url.clone()), ("HTTPS_PROXY".into(), proxy_url.clone()), ("ALL_PROXY".into(), proxy_url), ("NO_PROXY".into(), String::new()), ("no_proxy".into(), String::new()),
            ("NODE_USE_ENV_PROXY".into(), "1".into())]);
        let mut native = crate::windows_process::conpty_pending(&runtime.program, &args, std::path::Path::new(&spec.cwd), &env,
            spec.cols, spec.rows, Some(&sandbox.security()))?;
        sandbox.bind_process(&native.process)?;
        let network = if spec.hosts.is_empty() { None } else {
            Some(crate::windows_network_broker::connect(&native.process, &sandbox.profile_name(), proxy.port).await?)
        };
        native.resume()?;
        let mut writer = native.input.try_clone()?;
        let (input, receiver) = std::sync::mpsc::sync_channel::<Vec<u8>>(32);
        std::thread::spawn(move || { while let Ok(bytes) = receiver.recv() { if writer.write_all(&bytes).is_err() { break; } } });
        Ok(Self { native, input, _sandbox: sandbox, _network: network, proxy, spec })
    }

    /// Queues bounded input without blocking the async executor on a synchronous ConPTY pipe.
    pub fn write(&self, bytes: &[u8]) -> std::io::Result<()> {
        if bytes.len() > INPUT_BYTES { return Err(std::io::ErrorKind::InvalidInput.into()); }
        self.input.try_send(bytes.to_vec()).map_err(|error| match error {
            std::sync::mpsc::TrySendError::Full(_) => std::io::ErrorKind::WouldBlock.into(),
            std::sync::mpsc::TrySendError::Disconnected(_) => std::io::ErrorKind::BrokenPipe.into(),
        })
    }

    /// Applies a checked ConPTY window-size update.
    pub fn resize(&self, cols: u16, rows: u16) -> std::io::Result<()> { self.native.console.resize(cols, rows) }
}

#[cfg(windows)]
impl Terminal {
    /// Drains ConPTY independently of input and closes it after the contained process exits.
    fn pump(self: &Arc<Self>, shell: &Shell) {
        use std::io::Read;
        let Some(output) = shell.native.output.as_ref() else { return; };
        let Ok(mut output) = output.try_clone() else { return; };
        let terminal = self.clone();
        *self.exited.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let drained = tokio::task::spawn_blocking(move || {
            let mut buffer = [0u8; 8192];
            while let Ok(count) = output.read(&mut buffer) {
                if count == 0 { break; }
                terminal.ring.lock().unwrap_or_else(|e| e.into_inner()).push(&buffer[..count]);
                terminal.bump();
            }
        });
        let status = shell.native.process.watch_exit();
        let console = shell.native.console.clone();
        let terminal = self.clone();
        tokio::spawn(async move {
            let code = status.await.ok().and_then(Result::ok).map(|code| code as i32).unwrap_or(-1);
            console.close();
            let _ = drained.await;
            *terminal.exited.lock().unwrap_or_else(|e| e.into_inner()) = Some(code);
            terminal.bump();
        });
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    /// Approved commands share state while an ungranted sibling file remains inaccessible.
    #[tokio::test]
    async fn terminal_keeps_state_and_enforces_file_grants() {
        let root = std::env::temp_dir().join(format!("mc-terminal-{}", uuid::Uuid::new_v4()));
        let allowed = root.join("allowed");
        std::fs::create_dir_all(&allowed).unwrap();
        let forbidden = root.join("forbidden.txt");
        std::fs::write(&forbidden, "PRIVATE_SENTINEL").unwrap();
        let terminal = Arc::new(Terminal {
            shell: tokio::sync::Mutex::new(None), remote: Mutex::new(None), ring: Mutex::new(Ring::default()),
            seq: watch::channel(0).0, exited: Mutex::new(None), notices: Mutex::new(Vec::new()),
            last_used: Mutex::new(Instant::now()),
        });
        let cwd = allowed.to_string_lossy().into_owned();
        let spec = Spec { cwd: cwd.clone(), grants: vec![Grant { path: cwd.clone(), write: true, folder: true, blocked: false }],
            hosts: Vec::new(), cols: 100, rows: 30 };
        let shell = Shell::spawn_into(&terminal, spec).await.unwrap();
        let (job, _) = shell.native.process.job_descriptor();
        let verified = crate::windows_process::TreeGuard::open_verified(&job, &shell._sandbox.profile_name()).unwrap();
        drop(verified);
        *terminal.shell.lock().await = Some(shell);
        let first = terminal.run("first", "mcState='saved'; printf 'ready\\n'", &cwd, Vec::new(), |_| {}).await.unwrap();
        assert_eq!(first.code, Some(0));
        let second = terminal.run("second", "printf '%s\\n' \"$mcState\"", &cwd, Vec::new(), |_| {}).await.unwrap();
        assert!(String::from_utf8_lossy(&second.output).contains("saved"));
        let attempt = format!("cat {}", crate::windows_shell::quote_path(&forbidden.to_string_lossy()));
        let denied = terminal.run("denied", &attempt, &cwd, Vec::new(), |_| {}).await.unwrap();
        assert_ne!(denied.code, Some(0));
        assert!(!String::from_utf8_lossy(&denied.output).contains("PRIVATE_SENTINEL"));
        terminal.shell.lock().await.take();
        crate::windows_sandbox::drain_cleanup(std::time::Duration::from_secs(35)).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
