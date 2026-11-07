//! The Linux sandbox: bubblewrap builds each command a filesystem view from an empty root, with
//! the system folders read-only, the granted paths, and nothing else. The protected roots are
//! masked last, so no grant can open them. Every namespace is new, so the command has no network
//! but its own loopback device; approved hosts are reached through the egress proxy, whose Unix
//! socket is bound in and relayed to a loopback port inside the sandbox.

// Grants arrive in the shape the macOS profile uses.
use crate::seatbelt::Grant;
// Paths for the mount list and the probe.
use std::path::{Path, PathBuf};
// The cached probe result and the bridge registries.
use std::sync::{Mutex, OnceLock};
// The returned command is spawned by the caller.
use tokio::process::Command;

/// Where bubblewrap is looked for. Never `PATH`: a working folder must not supply it.
const BWRAP: [&str; 2] = ["/usr/bin/bwrap", "/bin/bwrap"];
/// Where the backend binary appears inside the sandbox when it relays proxy connections.
const RELAY: &str = "/run/miniclaw/relay";
/// The argument that turns the backend binary into that relay.
pub const RELAY_MODE: &str = "--sandbox-relay";

/// Returns the bubblewrap binary when the sandbox works on this machine, or the reason it does
/// not, in words for the user. Probed once per process.
pub fn state() -> &'static Result<PathBuf, String> {
    static STATE: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    STATE.get_or_init(|| {
        let found = BWRAP.iter().map(Path::new).find(|path| path.is_file());
        match found {
            Some(path) => probe(path).map(|()| path.to_path_buf()),
            None => Err("bubblewrap is not installed. Install the bubblewrap package (for example \
                `sudo pacman -S bubblewrap` or `sudo apt install bubblewrap`) and restart MiniClaw.".into()),
        }
    })
}

/// Runs `true` in a sandbox shaped like a real one; reports why that failed, if it did.
pub fn probe(bwrap: &Path) -> Result<(), String> {
    let mut command = std::process::Command::new(bwrap);
    command.args(["--unshare-all", "--unshare-user", "--disable-userns", "--die-with-parent", "--new-session", "--cap-drop", "ALL"])
        .args(system_mounts()).arg("--").arg("/usr/bin/true")
        .env_clear().stdin(std::process::Stdio::null());
    match command.output() {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(format!("bubblewrap could not create a sandbox ({}). Unprivileged user \
            namespaces may be turned off on this system.",
            String::from_utf8_lossy(&output.stderr).trim())),
        Err(error) => Err(format!("bubblewrap could not be started: {error}.")),
    }
}

/// The read-only system view every command gets: the toolchain, configuration, `/proc`, a
/// private `/dev` and an empty `/tmp`. The merged-`/usr` links are recreated as links.
fn system_mounts() -> Vec<String> {
    let mut args = Vec::new();
    for path in ["/usr", "/etc", "/opt"] {
        if Path::new(path).is_dir() { args.extend(["--ro-bind".into(), path.into(), path.into()]); }
    }
    for path in ["/bin", "/sbin", "/lib", "/lib64", "/lib32"] {
        match std::fs::read_link(path) {
            Ok(target) => args.extend(["--symlink".into(), target.to_string_lossy().into_owned(), path.into()]),
            Err(_) if Path::new(path).is_dir() => args.extend(["--ro-bind".into(), path.into(), path.into()]),
            Err(_) => {}
        }
    }
    args.extend(["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp"].map(String::from));
    args
}

/// Reports whether `path` is `root` or lies below it.
fn within(path: &str, root: &str) -> bool {
    let root = root.trim_end_matches('/');
    path == root || path.strip_prefix(root).is_some_and(|rest| rest.starts_with('/'))
}

/// Builds the bubblewrap arguments for these grants, ending just before the program. Grants are
/// mounted shallowest first, so a deeper grant replaces its part of a wider one; blocked paths
/// and `protected` come after every grant. `session` gives the command its own session, which
/// keeps it away from the caller's terminal; a terminal shell needs its own tty instead.
pub fn arguments(grants: &[Grant], protected: &[String], session: bool) -> Vec<String> {
    let mut args: Vec<String> = ["--unshare-all", "--unshare-user", "--disable-userns", "--die-with-parent", "--cap-drop", "ALL"]
        .map(String::from).into();
    if session { args.push("--new-session".into()); }
    args.extend(system_mounts());
    let mut open: Vec<&Grant> = grants.iter().filter(|grant| !grant.blocked).collect();
    open.sort_by_key(|grant| grant.path.matches('/').count());
    for grant in &open {
        let kind = if grant.write { "--bind-try" } else { "--ro-bind-try" };
        args.extend([kind.into(), grant.path.clone(), grant.path.clone()]);
    }
    let visible = |path: &str| open.iter().any(|grant| within(path, &grant.path))
        || ["/usr", "/etc", "/opt"].iter().any(|root| within(path, root));
    let closed = grants.iter().filter(|grant| grant.blocked).map(|grant| grant.path.as_str())
        .chain(protected.iter().map(String::as_str));
    for path in closed {
        // A path that is not in the view needs no mask; a missing one cannot be masked.
        if !visible(path) { continue; }
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => args.extend(["--tmpfs".into(), path.into(),
                "--remount-ro".into(), path.into()]),
            Ok(_) => args.extend(["--ro-bind".into(), "/dev/null".into(), path.into()]),
            Err(_) => {}
        }
    }
    args
}

/// Each outbound bridge's host socket by loopback port: an egress proxy's, or a `Bridges`
/// forwarder to a local service. The sandbox binds it in for that port.
static OUTBOUND: Mutex<Vec<(u16, PathBuf)>> = Mutex::new(Vec::new());
/// Each listening port's host folder, where the relay inside creates the socket the host's
/// listener forwards to.
static INBOUND: Mutex<Vec<(u16, PathBuf)>> = Mutex::new(Vec::new());

/// Makes `socket` the way into this machine's loopback `port` for sandboxes started from now on.
pub fn register(port: u16, socket: PathBuf) {
    if let Ok(mut known) = OUTBOUND.lock() { known.push((port, socket)); }
}

/// Forgets a socket `register` named, when its bridge or proxy stops.
pub fn unregister(socket: &Path) {
    if let Ok(mut known) = OUTBOUND.lock() { known.retain(|(_, path)| path != socket); }
}

/// Returns the newest host socket that leads to loopback `port`, if one is running.
fn outbound(port: u16) -> Option<PathBuf> {
    OUTBOUND.lock().ok()?.iter().rev().find(|(known, _)| *known == port).map(|(_, path)| path.clone())
}

/// Returns the host folder of the bridge that serves listening `port`, if one is running.
fn inbound(port: u16) -> Option<PathBuf> {
    INBOUND.lock().ok()?.iter().rev().find(|(known, _)| *known == port).map(|(_, path)| path.clone())
}

/// The bridges one sandboxed server needs across its network namespace: to each local port it
/// may reach, and from each port it listens on to this machine's loopback. They stop on drop.
pub struct Bridges {
    tasks: Vec<tokio::task::JoinHandle<()>>,
    sockets: Vec<PathBuf>,
    listen: Vec<(u16, PathBuf)>,
    _folder: crate::executor::Scratch,
}

impl Bridges {
    /// Opens a forwarder for each of `ports` that no proxy already serves, and a loopback
    /// listener for each of `listen`; fails when a listening port is taken on this machine.
    pub async fn open(ports: &[u16], listen: &[u16]) -> std::io::Result<Self> {
        let folder = crate::executor::Scratch::create()?;
        let mut bridges = Self { tasks: Vec::new(), sockets: Vec::new(), listen: Vec::new(), _folder: folder };
        let base = PathBuf::from(&bridges._folder.0);
        for &port in ports {
            if outbound(port).is_some() { continue; }
            let path = base.join(format!("out-{port}.sock"));
            let listener = tokio::net::UnixListener::bind(&path)?;
            bridges.tasks.push(tokio::spawn(async move {
                while let Ok((mut inside, _)) = listener.accept().await {
                    tokio::spawn(async move {
                        if let Ok(mut local) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
                            let _ = tokio::io::copy_bidirectional(&mut inside, &mut local).await;
                        }
                    });
                }
            }));
            register(port, path.clone());
            bridges.sockets.push(path);
        }
        for &port in listen {
            let room = base.join(format!("in-{port}"));
            std::fs::create_dir(&room)?;
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
            let socket = room.join("s.sock");
            bridges.tasks.push(tokio::spawn(async move {
                while let Ok((mut outside, _)) = listener.accept().await {
                    let socket = socket.clone();
                    tokio::spawn(async move {
                        if let Ok(mut server) = tokio::net::UnixStream::connect(&socket).await {
                            let _ = tokio::io::copy_bidirectional(&mut outside, &mut server).await;
                        }
                    });
                }
            }));
            if let Ok(mut known) = INBOUND.lock() { known.push((port, room.clone())); }
            bridges.listen.push((port, room));
        }
        Ok(bridges)
    }
}

impl Drop for Bridges {
    /// Stops every forwarder and forgets its sockets and folders.
    fn drop(&mut self) {
        for task in &self.tasks { task.abort(); }
        for socket in &self.sockets { unregister(socket); }
        if let Ok(mut known) = INBOUND.lock() {
            known.retain(|(_, room)| !self.listen.iter().any(|(_, mine)| mine == room));
        }
    }
}

/// Returns a command that runs `program` in the sandbox. Each of `ports` is a loopback port the
/// program may reach: an egress proxy's, or a local service a running `Bridges` forwards to;
/// inside, the same port leads there through the relay. Each of `listen` is a port the program
/// serves, reachable on this machine's loopback through its `Bridges`. A port without its
/// bridge refuses the command; nothing is ever opened to the host network.
pub fn command(grants: &[Grant], protected: &[String], program: &str, ports: &[u16],
    listen: &[u16], session: bool) -> Command {
    let Ok(bwrap) = state() else { return refused("The Linux sandbox is unavailable.") };
    let mut command = Command::new(bwrap);
    command.args(arguments(grants, protected, session));
    if ports.is_empty() && listen.is_empty() {
        command.arg("--").arg(program);
        return command;
    }
    let Some(exe) = relay_binary() else { return refused("The sandbox relay is unavailable.") };
    command.arg("--ro-bind").arg(exe).arg(RELAY);
    let mut relay = vec![RELAY_MODE.to_owned()];
    for port in ports {
        let Some(socket) = outbound(*port) else { return refused("A local port's bridge is not running.") };
        let inside = format!("/run/miniclaw/out-{port}.sock");
        command.arg("--bind").arg(socket).arg(&inside);
        relay.push(format!("{port}={inside}"));
    }
    for port in listen {
        let Some(room) = inbound(*port) else { return refused("A listening port's bridge is not running.") };
        let inside = format!("/run/miniclaw/in-{port}");
        command.arg("--bind").arg(room).arg(&inside);
        relay.push(format!("listen:{port}={inside}/s.sock"));
    }
    command.arg("--").arg(RELAY).args(relay).arg("--").arg(program);
    command
}

/// The backend binary, which doubles as the relay. A test binary in `target/<profile>/deps` is
/// not the backend, so it uses the `miniclaw-server` Cargo builds beside it.
fn relay_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    if exe.file_name().is_some_and(|name| name == "miniclaw-server") { return Some(exe); }
    let folder = exe.parent()?;
    [Some(folder), folder.parent()].into_iter().flatten()
        .map(|folder| folder.join("miniclaw-server")).find(|path| path.is_file())
}

/// A command that runs nothing and explains why; callers append their own arguments to it.
fn refused(reason: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg("echo \"$0\" >&2; exit 126").arg(reason).arg("--");
    command
}

/// Copies both ways between two connected streams until each side has finished.
fn pipe<A, B>(one: A, other: B)
where A: std::io::Read + std::io::Write + Send + 'static + Clone2, B: std::io::Read + std::io::Write + Send + 'static + Clone2 {
    let (Ok(mut one_read), Ok(mut other_write)) = (one.duplicate(), other.duplicate()) else { return };
    let forward = std::thread::spawn(move || {
        let _ = std::io::copy(&mut one_read, &mut other_write);
        other_write.close_write();
    });
    let (mut one, mut other) = (one, other);
    let _ = std::io::copy(&mut other, &mut one);
    one.close_write();
    let _ = forward.join();
}

/// What `pipe` needs of a stream: a second handle, and a way to say nothing more is sent.
trait Clone2: Sized {
    fn duplicate(&self) -> std::io::Result<Self>;
    fn close_write(&self);
}

impl Clone2 for std::net::TcpStream {
    fn duplicate(&self) -> std::io::Result<Self> { self.try_clone() }
    fn close_write(&self) { let _ = self.shutdown(std::net::Shutdown::Write); }
}

impl Clone2 for std::os::unix::net::UnixStream {
    fn duplicate(&self) -> std::io::Result<Self> { self.try_clone() }
    fn close_write(&self) { let _ = self.shutdown(std::net::Shutdown::Write); }
}

/// The relay inside the sandbox. Each `port=socket` pair listens on that loopback port and
/// forwards to the socket; each `listen:port=socket` pair listens on the socket, which the host's
/// bridge dials, and forwards to the program's own loopback port. Then it runs the program after
/// `--` and returns its exit code.
pub fn relay(args: &[String]) -> i32 {
    use std::net::{TcpListener, TcpStream};
    use std::os::unix::net::{UnixListener, UnixStream};
    let Some(split) = args.iter().position(|arg| arg == "--") else { return 126 };
    let (pairs, program) = (&args[..split], &args[split + 1..]);
    if program.is_empty() { return 126; }
    for pair in pairs {
        let (served, pair) = match pair.strip_prefix("listen:") { Some(rest) => (true, rest), None => (false, pair.as_str()) };
        let Some((port, socket)) = pair.split_once('=') else { return 126 };
        let (port, socket) = (port.to_owned(), socket.to_owned());
        if served {
            let Ok(listener) = UnixListener::bind(&socket) else {
                eprintln!("MiniClaw could not open a listening port inside the sandbox.");
                return 126;
            };
            std::thread::spawn(move || for host in listener.incoming().flatten() {
                let port = port.clone();
                std::thread::spawn(move || {
                    if let Ok(server) = TcpStream::connect(format!("127.0.0.1:{port}")) { pipe(host, server); }
                });
            });
        } else {
            let Ok(listener) = TcpListener::bind(format!("127.0.0.1:{port}")) else {
                eprintln!("MiniClaw could not open a local port inside the sandbox.");
                return 126;
            };
            std::thread::spawn(move || for client in listener.incoming().flatten() {
                let socket = socket.clone();
                std::thread::spawn(move || {
                    if let Ok(upstream) = UnixStream::connect(&socket) { pipe(client, upstream); }
                });
            });
        }
    }
    match std::process::Command::new(&program[0]).args(&program[1..]).status() {
        Ok(status) => status.code().unwrap_or(128 + 9),
        Err(_) => 127,
    }
}

/// The roots no grant opens on Linux: the providers' sign-ins, the desktop keyrings and
/// MiniClaw's data, plus any provider home moved by its environment variable.
pub fn protected_roots(home: &str, get: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let data_home = get("XDG_DATA_HOME").filter(|path| path.starts_with('/'))
        .unwrap_or_else(|| format!("{home}/.local/share"));
    let mut roots: Vec<String> = [".codex", ".claude", ".claude.json"].iter()
        .map(|tail| format!("{home}/{tail}")).collect();
    roots.extend([format!("{data_home}/keyrings"), format!("{data_home}/miniclaw"),
        format!("{home}/.local/share/keyrings"), format!("{home}/.local/share/miniclaw")]);
    for variable in ["CODEX_HOME", "CLAUDE_CONFIG_DIR"] {
        if let Some(path) = get(variable).filter(|path| path.starts_with('/')) { roots.push(path); }
    }
    roots.sort();
    roots.dedup();
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without bubblewrap the sandbox reports why; nothing falls back to running unsandboxed.
    #[test]
    fn a_missing_bubblewrap_is_reported() {
        let missing = Path::new("/nonexistent/bwrap");
        assert!(probe(missing).unwrap_err().contains("could not be started"));
    }

    /// Grants are mounted shallowest first and protected roots are masked after every grant.
    #[test]
    fn protected_roots_are_masked_after_the_grants() {
        let home = std::env::temp_dir().join(format!("mc-linux-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(home.join(".claude.json"), "{}").unwrap();
        let home = home.to_string_lossy().into_owned();
        let grants = [Grant { path: format!("{home}/project"), write: true, folder: true, blocked: false },
            Grant { path: home.clone(), write: false, folder: true, blocked: false }];
        let args = arguments(&grants, &protected_roots(&home, |_| None), true);
        let at = |needle: &str| args.iter().position(|arg| arg == needle).unwrap();
        assert!(at(&home) < at(&format!("{home}/project")), "{args:?}");
        let codex = at(&format!("{home}/.codex"));
        assert_eq!(args[codex - 1], "--tmpfs");
        assert!(codex > at(&format!("{home}/project")));
        assert_eq!(args[at(&format!("{home}/.claude.json")) - 1], "/dev/null");
        std::fs::remove_dir_all(&home).unwrap();
    }

    /// Provider homes moved by their variables are protected too; relative values are ignored.
    #[test]
    fn moved_provider_homes_are_protected() {
        let roots = protected_roots("/home/a", |key| match key {
            "CODEX_HOME" => Some("/srv/codex".into()),
            "CLAUDE_CONFIG_DIR" => Some("relative".into()),
            _ => None,
        });
        assert!(roots.contains(&"/srv/codex".to_owned()) && !roots.contains(&"relative".to_owned()));
        assert!(roots.contains(&"/home/a/.local/share/keyrings".to_owned()));
    }
}
