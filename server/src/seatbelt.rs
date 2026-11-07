// Runs commands under a generated sandbox profile.
use tokio::process::Command;

/// One path the sandboxed command may touch, or an exact path it must not.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub path: String,
    pub write: bool,
    pub folder: bool,
    pub blocked: bool,
}

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

// Everything is denied unless listed. Reads cover the system toolchain only; stat is
// allowed everywhere so path resolution works, but directory listing and file content
// are not. There is no network rule, so every socket is refused.
const BASE: &str = r#"(version 1)
(deny default)
(allow process-exec)
(allow process-fork)
(allow signal (target same-sandbox))
(allow process-info* (target same-sandbox))
(allow sysctl-read)
(allow mach-lookup (global-name "com.apple.system.opendirectoryd.libinfo"))
(allow file-read-metadata)
(allow file-read* (literal "/")
  (subpath "/usr") (subpath "/bin") (subpath "/sbin") (subpath "/System")
  (subpath "/opt/homebrew") (subpath "/Library/Developer/CommandLineTools")
  (subpath "/private/etc") (subpath "/private/var/db/timezone")
  (subpath "/private/var/db/dyld") (literal "/private/var/db/xcode_select_link")
  (literal "/dev/null") (literal "/dev/zero") (literal "/dev/random")
  (literal "/dev/urandom") (subpath "/dev/fd"))
(allow file-write-data (literal "/dev/null"))
(allow file-ioctl (literal "/dev/null"))
"#;

/// Paths no conversation can be granted: the providers' sign-in files and MiniClaw's own
/// state. A user may grant a folder that contains them, their home folder for example, and
/// a command that can read a sign-in token or rewrite approval state defeats every other
/// rule. `data` is the backend's data folder and `links` the VM link file, when set.
pub fn protected_roots(home: &str, data: Option<&str>, links: Option<&str>) -> Vec<String> {
    let mut roots: Vec<String> = [".codex", ".claude", ".claude.json", "Library/Keychains",
        "Library/Application Support/MiniClaw"].iter().map(|tail| format!("{home}/{tail}")).collect();
    if let Some(data) = data {
        let data = std::path::Path::new(data);
        // An installation keeps its settings and the VM pairing key beside the data folder.
        let root = data.parent().filter(|parent| parent.join("settings.json").is_file());
        roots.push(root.unwrap_or(data).to_string_lossy().into_owned());
    }
    if let Some(parent) = links.and_then(|path| std::path::Path::new(path).parent()) {
        roots.push(parent.to_string_lossy().into_owned());
    }
    // Seatbelt matches resolved paths, so a root reached through a link is resolved too.
    roots.iter().map(|root| std::fs::canonicalize(root)
        .map(|real| real.to_string_lossy().into_owned()).unwrap_or_else(|_| root.clone())).collect()
}

/// Builds Windows denials even for credentials that a future sign-in has not created yet.
#[cfg(any(windows, test))]
pub(crate) fn windows_protected_roots(get: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let read = |key| get(key).filter(|value| !value.is_empty());
    let home = read("USERPROFILE").or_else(|| read("HOME")).unwrap_or_default();
    let join = |root: &str, tail: &str| format!("{}\\{tail}", root.trim_end_matches(['\\', '/']));
    let local = read("LOCALAPPDATA").unwrap_or_else(|| join(&home, r"AppData\Local"));
    let roaming = read("APPDATA").unwrap_or_else(|| join(&home, r"AppData\Roaming"));
    let mut roots = vec![join(&home, ".codex"), join(&home, ".claude"), join(&home, ".claude.json"),
        join(&local, "MiniClaw")];
    // Some provider launchers honor HOME even on Windows; protect both home identities.
    if let Some(other_home) = read("HOME").filter(|value| value != &home) {
        for tail in [".codex", ".claude", ".claude.json"] { roots.push(join(&other_home, tail)); }
    }
    for base in [&local, &roaming] {
        for tail in [r"Microsoft\Credentials", r"Microsoft\Vault", r"Microsoft\Protect"] {
            roots.push(join(base, tail));
        }
    }
    for variable in ["CODEX_HOME", "CLAUDE_CONFIG_DIR", "MINICLAW_NETWORK_CONFIG"] {
        if let Some(path) = read(variable) { roots.push(path); }
    }
    // Native host helpers and their adjacent DLLs must not be writable through tool grants.
    for variable in ["MINICLAW_HELPER_BIN", "CODEX_BIN", "CLAUDE_BIN"] {
        if let Some(path) = read(variable) {
            let path = path.replace('/', "\\");
            let bytes = path.as_bytes();
            if bytes.len() > 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
                && !path[2..].contains(':') && !path.split('\\').any(|part| part == ".." || part == ".")
                && path.rsplit('.').next().is_some_and(|extension| extension.eq_ignore_ascii_case("exe") || extension.eq_ignore_ascii_case("com"))
                && let Some((parent, _)) = path.rsplit_once('\\') {
                    roots.push(if parent.len() == 2 { format!("{parent}\\") } else { parent.to_owned() });
                }
        }
    }
    // A helper chosen by package is found again in its newest version folder, so close them all.
    if read("MINICLAW_HELPER_PACKAGE").is_some() {
        roots.push(join(&read("ProgramFiles").unwrap_or_else(|| r"C:\Program Files".into()), "WindowsApps"));
    }
    if let Some(data) = read("MINICLAW_DATA_DIR") {
        let path = data.replace('/', "\\").trim_end_matches('\\').to_owned();
        roots.push(path.clone());
        if let Some((parent, leaf)) = path.rsplit_once('\\') {
            // Settings must remain protected before the installer writes them as well.
            roots.push(join(parent, "settings.json"));
            if leaf.eq_ignore_ascii_case("data") {
                roots.push(join(parent, "runtime"));
                roots.push(join(parent, "settings.json.tmp"));
            }
        }
    }
    if let Some(path) = read("MINICLAW_VM_LINKS") {
        if let Some((parent, _)) = path.replace('/', "\\").rsplit_once('\\') {
            roots.push(parent.to_owned());
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

/// Applies a conservative Windows spelling check; native brokers additionally check object IDs.
#[cfg(any(windows, test))]
fn windows_is_protected(path: &str, roots: &[String]) -> bool {
    // `std::fs::canonicalize` answers `\\?\C:\...`; that is the same local disk path.
    let plain = |text: &str| {
        let text = text.replace('/', "\\");
        match text.strip_prefix(r"\\?\") {
            Some(rest) if rest.len() >= 3 && rest.as_bytes()[0].is_ascii_alphabetic()
                && rest.as_bytes()[1] == b':' && rest.as_bytes()[2] == b'\\' => rest.to_owned(),
            _ => text,
        }
    };
    let key = plain(path).to_uppercase();
    roots.iter().any(|root| {
        let root = plain(root).trim_end_matches('\\').to_uppercase();
        key == root || key.strip_prefix(&root).is_some_and(|suffix| suffix.starts_with('\\'))
    })
}

/// Reads the protected roots for this process from its environment.
pub fn protected() -> Vec<String> {
    #[cfg(windows)]
    { return windows_protected_roots(|key| std::env::var(key).ok()); }
    #[cfg(target_os = "linux")]
    {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut roots = crate::sandbox_linux::protected_roots(&home, |key| std::env::var(key).ok());
    roots.extend(protected_roots(&home, std::env::var("MINICLAW_DATA_DIR").ok().as_deref(),
        std::env::var("MINICLAW_VM_LINKS").ok().as_deref()));
    roots.sort();
    roots.dedup();
    return roots;
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
    let home = std::env::var("HOME").unwrap_or_default();
    protected_roots(&home, std::env::var("MINICLAW_DATA_DIR").ok().as_deref(),
        std::env::var("MINICLAW_VM_LINKS").ok().as_deref())
    }
}

/// Reports whether a path is a protected root or lies inside one.
pub fn is_protected(path: &str, roots: &[String]) -> bool {
    #[cfg(windows)]
    { return windows_is_protected(path, roots); }
    #[cfg(unix)]
    {
    let path = std::path::Path::new(path);
    roots.iter().any(|root| path.starts_with(root))
    }
}

/// Returns whether this machine can enforce the sandbox. On Windows that is decided afresh from
/// this installation's state (`windows_gate`); on the Mac it is the presence of `sandbox-exec`;
/// on Linux, a bubblewrap that can create the sandbox (`sandbox_linux::state`).
pub fn available() -> bool {
    #[cfg(windows)]
    { return crate::windows_gate::state().is_ok(); }
    #[cfg(target_os = "linux")]
    { return crate::sandbox_linux::state().is_ok(); }
    #[cfg(not(any(windows, target_os = "linux")))]
    { cfg!(target_os = "macos") && std::path::Path::new(SANDBOX_EXEC).is_file() }
}

/// Says, in words for the user, why the sandbox is unavailable. `None` when it is available, and
/// on the Mac, where the only cause is a system without `sandbox-exec`.
pub fn unavailable_reason() -> Option<String> {
    #[cfg(windows)]
    { return crate::windows_gate::state().err().map(|missing| missing.reason()); }
    #[cfg(target_os = "linux")]
    { return crate::sandbox_linux::state().as_ref().err().cloned(); }
    #[cfg(not(any(windows, target_os = "linux")))]
    { None }
}

/// Appends the reason the sandbox is unavailable to a refusal, when one is known.
pub fn with_reason(refusal: &str) -> String {
    match unavailable_reason() {
        Some(reason) => format!("{refusal} {reason}"),
        None => refusal.to_owned(),
    }
}

/// Gives a refusal followed by `reason` the lifetime an API error needs. The reasons are a small
/// fixed set on one machine, so each distinct sentence is kept once; past that bound the bare
/// refusal is answered rather than keeping more.
pub fn kept_refusal(refusal: &'static str, reason: Option<&str>) -> &'static str {
    static KEPT: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    let Some(reason) = reason.filter(|reason| !reason.is_empty()) else { return refusal; };
    let text = format!("{refusal} {reason}");
    let mut kept = KEPT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(known) = kept.iter().find(|known| **known == text) { return known; }
    if kept.len() >= 64 { return refusal; }
    let text: &'static str = Box::leak(text.into_boxed_str());
    kept.push(text);
    text
}

/// Builds the profile text and its path parameters. Paths travel as `-D` parameters,
/// never inside the profile, so no path can add a rule. Blocks come last and win.
pub fn profile(grants: &[Grant]) -> (String, Vec<(String, String)>) { profile_with(grants, None) }

/// As `profile`, and with `proxy` the command may open connections to that loopback port
/// only. The port is a number, so it cannot carry a rule into the profile text.
pub fn profile_with(grants: &[Grant], proxy: Option<u16>) -> (String, Vec<(String, String)>) {
    profile_ports(grants, proxy.as_slice())
}

/// As `profile`, and the command may open connections to these loopback ports only. A port is
/// a number, so it cannot carry a rule into the profile text.
pub fn profile_ports(grants: &[Grant], ports: &[u16]) -> (String, Vec<(String, String)>) { profile_net(grants, ports, &[]) }

/// As `profile_ports`, and the command may also accept connections on the `listen` ports, on the
/// loopback addresses only: an MCP server that an app on this Mac connects to, as Roblox
/// Studio's does, needs to bind its port. Nothing outside this Mac can reach it.
pub fn profile_net(grants: &[Grant], ports: &[u16], listen: &[u16]) -> (String, Vec<(String, String)>) {
    let mut text = String::from(BASE);
    for port in ports {
        text.push_str(&format!("(allow network-outbound (remote tcp \"localhost:{port}\"))\n"));
    }
    // A server that listens may also connect to its own port: Roblox's proxy is a host and a
    // client in one process, and its client half dials the host over loopback (seen with the real
    // binary: "WS host connection error: Operation not permitted" without this rule).
    for port in listen {
        text.push_str(&format!("(allow network-bind (local tcp \"localhost:{port}\"))\n"));
        text.push_str(&format!("(allow network-inbound (local tcp \"localhost:{port}\"))\n"));
        text.push_str(&format!("(allow network-outbound (remote tcp \"localhost:{port}\"))\n"));
    }
    let mut parameters = Vec::new();
    // Protected roots are appended as blocks, so no grant can open them.
    let closed: Vec<Grant> = protected().into_iter().map(|path| Grant { path, write: false,
        folder: true, blocked: true }).collect();
    let ordered = grants.iter().filter(|grant| !grant.blocked)
        .chain(grants.iter().filter(|grant| grant.blocked)).chain(closed.iter());
    for (index, grant) in ordered.enumerate() {
        let name = format!("P{index}");
        let filter = if grant.folder { "subpath" } else { "literal" };
        let rule = if grant.blocked { "deny file-read* file-write*" }
            else if grant.write { "allow file-read* file-write*" }
            else { "allow file-read*" };
        text.push_str(&format!("({rule} ({filter} (param \"{name}\")))\n"));
        parameters.push((name, grant.path.clone()));
    }
    (text, parameters)
}

/// Returns a command that runs `program` inside the profile for these grants.
pub fn command(grants: &[Grant], program: &str) -> Command { command_with(grants, program, None) }

/// As `command`, with the one loopback port of this command's proxy reachable.
pub fn command_with(grants: &[Grant], program: &str, proxy: Option<u16>) -> Command {
    command_ports(grants, program, proxy.as_slice())
}

/// As `command_with`, for a shell that lives on a pseudo-terminal: the shell may read, write and
/// set the terminal modes of its own tty device and of `/dev/tty`, which its line editor needs.
/// The device path travels as a parameter, like every other path.
pub fn command_terminal(grants: &[Grant], program: &str, proxy: u16, tty: &str) -> Command {
    #[cfg(target_os = "linux")]
    { let _ = tty; return crate::sandbox_linux::command(grants, &protected(), program, &[proxy], &[], false); }
    #[allow(unreachable_code)]
    let (mut text, mut parameters) = profile_ports(grants, &[proxy]);
    text.push_str("(allow file-read* file-write* file-ioctl (literal (param \"TTY\")))\n");
    text.push_str("(allow file-read* file-write* file-ioctl (literal \"/dev/tty\"))\n");
    parameters.push(("TTY".into(), tty.to_owned()));
    build(text, parameters, program)
}

/// As `command`, with these loopback ports reachable and nothing else on any network.
pub fn command_ports(grants: &[Grant], program: &str, ports: &[u16]) -> Command { command_net(grants, program, ports, &[]) }

/// As `command_ports`, and the program may listen on the `listen` loopback ports.
pub fn command_net(grants: &[Grant], program: &str, ports: &[u16], listen: &[u16]) -> Command {
    #[cfg(target_os = "linux")]
    { return crate::sandbox_linux::command(grants, &protected(), program, ports, listen, true); }
    #[allow(unreachable_code)]
    let (text, parameters) = profile_net(grants, ports, listen);
    build(text, parameters, program)
}

/// Assembles the `sandbox-exec` invocation from a profile and its parameters.
fn build(text: String, parameters: Vec<(String, String)>, program: &str) -> Command {
    let mut command = Command::new(SANDBOX_EXEC);
    command.arg("-p").arg(text);
    for (name, value) in parameters {
        command.arg("-D").arg(format!("{name}={value}"));
    }
    command.arg("--").arg(program);
    command
}

#[cfg(test)]
mod tests {
    // Checks generated profile text only.
    use super::*;

    /// A refusal with a reason is one kept sentence, however often it is asked for.
    #[test]
    fn a_refusal_with_its_reason_is_kept_once() {
        assert_eq!(kept_refusal("Refused.", None), "Refused.");
        assert_eq!(kept_refusal("Refused.", Some("")), "Refused.");
        let first = kept_refusal("Refused.", Some("The service is not running."));
        assert_eq!(first, "Refused. The service is not running.");
        assert!(std::ptr::eq(first, kept_refusal("Refused.", Some("The service is not running."))));
        // The Mac names no reason, so its sentences are exactly the ones written at the call.
        #[cfg(not(windows))]
        {
            assert_eq!(unavailable_reason(), None);
            assert_eq!(with_reason("The sandbox is not available."), "The sandbox is not available.");
        }
    }

    /// A hostile path stays a parameter value and blocks follow every allow.
    #[test]
    fn paths_never_enter_the_profile_text() {
        let hostile = "/tmp/x\")) (allow default) ((\"";
        let (text, parameters) = profile(&[
            Grant { path: "/blocked".into(), write: false, folder: false, blocked: true },
            Grant { path: hostile.into(), write: true, folder: true, blocked: false },
        ]);
        assert!(!text.contains("allow default") && !text.contains("/tmp/x"));
        assert_eq!(parameters[0].1, hostile);
        let allow = text.find("(allow file-read* file-write* (subpath (param \"P0\")))").unwrap();
        let deny = text.find("(deny file-read* file-write* (literal (param \"P1\")))").unwrap();
        assert!(allow < deny);
    }

    /// A listening port makes exactly the bind and inbound rules, loopback only, and nothing else.
    #[test]
    fn a_listening_port_opens_bind_and_inbound_on_loopback_only() {
        let (text, _) = profile_net(&[], &[9876], &[13469]);
        assert!(text.contains("(allow network-outbound (remote tcp \"localhost:9876\"))"));
        assert!(text.contains("(allow network-bind (local tcp \"localhost:13469\"))"));
        assert!(text.contains("(allow network-inbound (local tcp \"localhost:13469\"))"));
        assert!(!text.contains("network-inbound (local tcp \"localhost:9876\""), "an outbound port is not opened for listening");
        assert!(text.contains("(allow network-outbound (remote tcp \"localhost:13469\"))"), "a listening server may dial its own port");
        assert!(!text.contains("tcp \"*"), "no wildcard address");
        let (plain, _) = profile_ports(&[], &[]);
        assert!(!plain.contains("network-bind") && !plain.contains("network-inbound"));
    }

    /// Sign-in files and MiniClaw's state are closed; an installation is closed as a whole.
    #[test]
    fn protected_roots_cover_sign_ins_state_and_the_installation() {
        let install = std::env::temp_dir().canonicalize().unwrap()
            .join(format!("mc-protected-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(install.join("data")).unwrap();
        std::fs::create_dir_all(install.join("ActiveSession")).unwrap();
        let data = install.join("data").to_string_lossy().into_owned();
        let links = install.join("ActiveSession/links.json").to_string_lossy().into_owned();
        // Without settings.json beside it, only the data folder itself is closed.
        let roots = protected_roots("/Users/someone", Some(&data), None);
        assert!(is_protected(&format!("{data}/agents.sqlite3"), &roots));
        assert!(!is_protected(&install.join("notes.txt").to_string_lossy(), &roots));
        std::fs::write(install.join("settings.json"), "{}").unwrap();
        let roots = protected_roots("/Users/someone", Some(&data), Some(&links));
        for closed in ["/Users/someone/.codex/auth.json", "/Users/someone/.claude.json",
            "/Users/someone/.claude/settings.json", "/Users/someone/Library/Keychains/login.keychain-db",
            "/Users/someone/Library/Application Support/MiniClaw/Resources/reservations.sqlite"] {
            assert!(is_protected(closed, &roots), "{closed}");
        }
        assert!(is_protected(&install.join("worker.json").to_string_lossy(), &roots));
        assert!(is_protected(&install.join("ActiveSession/control.key").to_string_lossy(), &roots));
        // A name that only starts like a protected one is a different folder.
        assert!(!is_protected("/Users/someone/.codex-notes/plan.md", &roots));
        assert!(!is_protected("/Users/someone/Documents/project/main.rs", &roots));
        std::fs::remove_dir_all(install).unwrap();
    }
}

#[cfg(test)]
mod windows_root_tests {
    use super::*;

    #[test]
    fn absent_windows_credentials_and_provider_overrides_remain_protected() {
        let values = [
            ("USERPROFILE", r"C:\Users\Someone"),
            ("LOCALAPPDATA", r"D:\Local Data"),
            ("APPDATA", r"D:\Roaming Data"),
            ("CODEX_HOME", r"E:\Private\Codex"),
            ("CLAUDE_CONFIG_DIR", r"E:\Private\Claude"),
            ("MINICLAW_DATA_DIR", r"D:\MiniClaw Install\data"),
        ];
        let roots = windows_protected_roots(|key| values.iter().find(|(name, _)| *name == key).map(|(_, value)| (*value).into()));
        for path in [r"C:\Users\Someone\.codex", r"C:\Users\Someone\.claude.json",
            r"D:\Local Data\MiniClaw", r"D:\Local Data\Microsoft\Credentials",
            r"D:\Roaming Data\Microsoft\Protect", r"D:\Roaming Data\Microsoft\Vault",
            r"E:\Private\Codex", r"E:\Private\Claude", r"D:\MiniClaw Install\data",
            r"D:\MiniClaw Install\settings.json"] {
            assert!(roots.contains(&path.into()), "missing {path}");
        }
        assert!(windows_is_protected(r"e:/private/CODEX/auth.json", &roots));
        assert!(!windows_is_protected(r"E:\Private\Codex-notes\plan.md", &roots));
    }

    /// `std::fs::canonicalize` answers `\\?\C:\...` on Windows; that spelling of a protected
    /// file is still protected, and a protected root written that way still covers its files.
    #[test]
    fn verbatim_disk_paths_match_their_protected_roots() {
        let values = [("USERPROFILE", r"C:\Users\Someone"), ("CODEX_HOME", r"E:\Private\Codex"),
            ("CLAUDE_CONFIG_DIR", r"\\?\F:\Claude Home")];
        let roots = windows_protected_roots(|key| values.iter().find(|(name, _)| *name == key).map(|(_, value)| (*value).into()));
        for path in [r"\\?\E:\Private\Codex\auth.json", r"\\?\e:\private\codex", r"\\?\C:\Users\Someone\.claude\x",
            r"F:\Claude Home\settings.json", r"\\?\F:\Claude Home"] {
            assert!(windows_is_protected(path, &roots), "{path}");
        }
        for path in [r"\\?\E:\Private\Codex-notes\plan.md", r"\\?\UNC\E:\Private\Codex\auth.json",
            r"\\.\E:\Private\Codex-notes", r"\\?\D:\Private\Codex\auth.json"] {
            assert!(!windows_is_protected(path, &roots), "{path}");
        }
    }

    #[test]
    fn configured_native_launchers_protect_their_dependency_folders() {
        let values = [("MINICLAW_HELPER_BIN", r"D:\Desktop Helper\helper.exe"),
            ("CODEX_BIN", r"E:\Provider\codex.exe"), ("CLAUDE_BIN", "claude"),
            ("MINICLAW_NETWORK_CONFIG", r"F:\Private Broker\client.json")];
        let roots = windows_protected_roots(|key| values.iter().find(|(name, _)| *name == key).map(|(_, value)| (*value).into()));
        assert!(roots.contains(&r"D:\Desktop Helper".into()));
        assert!(roots.contains(&r"E:\Provider".into()));
        assert!(!roots.contains(&"claude".into()));
        assert!(roots.contains(&r"F:\Private Broker\client.json".into()));
    }

    /// The helper chosen by package moves to a new folder with each Codex update, so the whole
    /// packaged-apps folder is closed to tool grants while it is configured.
    #[test]
    fn a_packaged_helper_protects_the_packaged_apps_folder() {
        let values = [("MINICLAW_HELPER_PACKAGE", "OpenAI.Codex_2p2nqsd0c76g0"), ("ProgramFiles", r"D:\Programs")];
        let roots = windows_protected_roots(|key| values.iter().find(|(name, _)| *name == key).map(|(_, value)| (*value).into()));
        assert!(windows_is_protected(r"D:\Programs\WindowsApps\OpenAI.Codex_26.1006.1.0_arm64__2p2nqsd0c76g0\app", &roots));
        let without = windows_protected_roots(|key| (key == "ProgramFiles").then(|| r"D:\Programs".into()));
        assert!(!windows_is_protected(r"D:\Programs\WindowsApps\Other", &without));
        let default = windows_protected_roots(|key| (key == "MINICLAW_HELPER_PACKAGE").then(|| "OpenAI.Codex_2p2nqsd0c76g0".into()));
        assert!(default.contains(&r"C:\Program Files\WindowsApps".into()));
    }

    #[test]
    fn windows_roots_derive_appdata_when_environment_paths_are_absent() {
        let roots = windows_protected_roots(|key| (key == "USERPROFILE").then(|| r"C:\Users\New User".into()));
        assert!(roots.contains(&r"C:\Users\New User\AppData\Local\MiniClaw".into()));
        assert!(roots.contains(&r"C:\Users\New User\AppData\Roaming\Microsoft\Credentials".into()));
    }
}
