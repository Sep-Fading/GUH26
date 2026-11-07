//! What a model is told about where it is. Without this it knows nothing of MiniClaw, this Mac,
//! its working folder or what is switched on, and it asks the user things it could have known.
//! The briefing has two parts, because a provider treats them differently:
//!
//! - The standing part goes into the base instructions when a conversation starts or is loaded
//!   again: what MiniClaw is, the machine, the tools this conversation really has, its working
//!   folder with what is in it, the toolchain on this Mac, and the rule to look before asking.
//! - The current state goes with each message under its own context key: the date, the agent
//!   and project, and what is on, off or not allowed. It holds no clock time, so it only changes
//!   when the state does; the real Codex keeps an unchanged key as one copy.
//!
//! Everything here is read from MiniClaw's own state. Nothing a model says can change it.

// Runs the version checks and reads the system's own description.
use std::{sync::OnceLock, time::Duration};
#[cfg(unix)]
use std::process::Stdio;
// Runs each check with a time limit.
#[cfg(unix)]
use tokio::process::Command;

/// A PATH for the version checks that does not depend on how MiniClaw was started.
#[cfg(unix)]
const PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
/// The tools whose presence saves a question. Each prints its version with this flag.
#[cfg(unix)]
const TOOLCHAIN: [(&str, &str); 8] = [("git", "--version"), ("node", "--version"), ("npm", "--version"), ("python3", "--version"),
    ("uv", "--version"), ("cargo", "--version"), ("go", "version"), ("swift", "--version")];

static MACHINE: OnceLock<String> = OnceLock::new();
static TOOLS: OnceLock<String> = OnceLock::new();

/// Runs one fixed program with one fixed flag and returns its first line, bounded.
#[cfg(unix)]
async fn first_line(program: &str, flag: &str, path: &str) -> Option<String> {
    let mut command = Command::new(program);
    command.arg(flag).env_clear().env("PATH", path).env("LANG", "en_US.UTF-8").current_dir("/")
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    if let Ok(home) = std::env::var("HOME") { command.env("HOME", home); }
    let output = tokio::time::timeout(Duration::from_secs(4), command.output()).await.ok()?.ok()?;
    if !output.status.success() { return None; }
    let text = String::from_utf8_lossy(&output.stdout);
    let line: String = text.lines().next()?.chars().filter(|c| !c.is_control()).take(80).collect();
    (!line.trim().is_empty()).then(|| line.trim().to_owned())
}

/// Probes only a native executable in PATH through the owned Windows process launcher.
#[cfg(windows)]
async fn first_line(program: &str, flag: &str, path: &str) -> Option<String> {
    let binary = std::env::split_paths(path).filter(|folder| folder.is_absolute())
        .map(|folder| folder.join(format!("{program}.exe"))).find(|file| file.is_file())?;
    let (output, _, code) = tokio::time::timeout(Duration::from_secs(4),
        crate::windows_process::capture(&binary, &[flag.to_owned()], &std::env::temp_dir()))
        .await.ok()?.ok()?;
    if code != 0 { return None; }
    let text = String::from_utf8_lossy(&output);
    let line: String = text.lines().next()?.chars().filter(|c| !c.is_control()).take(80).collect();
    (!line.trim().is_empty()).then(|| line.trim().to_owned())
}

/// Finds out, once per backend start, what this Mac is and which common tools it has. A
/// conversation that starts before the checks are done waits for them, since a briefing that
/// says "not checked" invites exactly the questions it is meant to save.
pub async fn detect() {
    static DONE: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
    DONE.get_or_init(|| async {
        #[cfg(unix)]
        let (path, toolchain) = {
            let path = match std::env::var("HOME") {
                Ok(home) => format!("{PATH}:{home}/.cargo/bin:{home}/.local/bin"),
                Err(_) => PATH.to_owned(),
            };
            if cfg!(target_os = "linux") {
                // The distribution's own name, as /etc/os-release gives it.
                let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
                let name = release.lines().find_map(|line| line.strip_prefix("PRETTY_NAME="))
                    .map(|value| value.trim_matches('"').chars().filter(|c| !c.is_control()).take(60).collect::<String>())
                    .filter(|value| !value.is_empty()).unwrap_or_else(|| "Linux".into());
                let _ = MACHINE.set(format!("{name}, {}", std::env::consts::ARCH));
            } else {
            let (system, arch) = tokio::join!(first_line("/usr/bin/sw_vers", "-productVersion", PATH), first_line("/usr/bin/uname", "-m", PATH));
            let _ = MACHINE.set(format!("macOS {}, {}", system.unwrap_or_else(|| "unknown version".into()), arch.unwrap_or_else(|| "unknown".into())));
            }
            (path, TOOLCHAIN.as_slice())
        };
        #[cfg(windows)]
        let (path, toolchain) = {
            const NATIVE_TOOLS: &[(&str, &str)] = &[("git", "--version"), ("node", "--version"),
                ("python", "--version"), ("uv", "--version"), ("cargo", "--version"),
                ("go", "version"), ("pwsh", "--version")];
            let _ = MACHINE.set(format!("Windows, {}", std::env::consts::ARCH));
            (std::env::var("PATH").unwrap_or_default(), NATIVE_TOOLS)
        };
        // The checks run side by side; each is bounded, so the whole takes as long as the slowest.
        let lines = futures_util::future::join_all(toolchain.iter().map(|(program, flag)| first_line(program, flag, &path))).await;
        // Each program words its version differently ("git version 2.55.0", "v25.8.1", "go1.22"), so
        // only the first token that is a version number is kept.
        let found: Vec<String> = toolchain.iter().zip(lines).filter_map(|((program, _), line)| {
            let line = line?;
            let version = line.split_whitespace().map(|token| token.trim_start_matches("go").trim_start_matches('v'))
                .find(|token| token.starts_with(|c: char| c.is_ascii_digit()))?
                .trim_end_matches(|c: char| !c.is_ascii_alphanumeric()).to_owned();
            Some(format!("{program} {version}"))
        }).collect();
        let _ = TOOLS.set(if found.is_empty() { if cfg!(windows) { "no supported native .exe tool was found in PATH".into() }
            else { "none of git, node, python3, uv, cargo, go or swift was found on the usual paths".into() } }
            else { found.join("; ") });
    }).await;
}

/// What this conversation's working folder looked like when the conversation was started or loaded.
#[derive(Default)]
pub struct Folder {
    pub path: String,
    /// Names at the top level, folders with a trailing slash; empty when it could not be read.
    pub names: Vec<String>,
    pub more: usize,
    /// The branch and whether anything is uncommitted; `None` when it is no git repository.
    pub git: Option<String>,
    /// Git could not be asked (on Windows: no private Git, or the sandbox refused the folder), so
    /// `git` being `None` says nothing about a repository.
    pub unchecked: bool,
}

/// What the standing part is built from.
pub struct Standing<'a> {
    /// Where the conversation runs, in words: "this Mac" or the VM's name.
    pub machine: &'a str,
    pub in_vm: bool,
    /// The names of the tools this conversation was really given.
    pub tools: Vec<String>,
    pub web_search: bool,
    /// Set when the conversation uses Computer Use instead of MiniClaw's own tools.
    pub computer_use: bool,
    pub folder: Option<Folder>,
    pub rules: Rules,
}

/// A project's standing instructions: the instruction files of its folder, as Claude Code and
/// Codex would read them, and what the user wrote in the project's settings.
#[derive(Default)]
pub struct Rules {
    /// File name and text, in reading order.
    pub files: Vec<(String, String)>,
    /// Set when the files were longer than the limit and were cut.
    pub cut: bool,
    pub instructions: String,
}

/// The most instruction file text a conversation is told, the limit Codex documents for AGENTS.md.
pub const RULES_BYTES: usize = 32 * 1024;

/// Writes the project's instructions as their own section, or nothing when there are none.
fn rules(input: &Rules) -> String {
    if input.files.is_empty() && input.instructions.trim().is_empty() { return String::new(); }
    let mut text = String::from("\n## Project instructions\nThe user wrote these for every conversation in this project. Follow them; where they conflict with a skill, they win.\n");
    for (name, body) in &input.files {
        text.push_str(&format!("\n### {name} (from the working folder)\n{}\n", body.trim_end()));
    }
    if input.cut { text.push_str("\n(The instruction files were cut at 32 KiB. Read the file itself if you need the rest.)\n"); }
    if !input.instructions.trim().is_empty() {
        text.push_str(&format!("\n### From the project's settings\n{}\n", input.instructions.trim()));
    }
    text
}

/// Writes the standing part. A tool the conversation was not given is not described.
pub fn standing(input: &Standing) -> String {
    let closed = (!crate::seatbelt::available()).then(|| crate::seatbelt::unavailable_reason().unwrap_or_default());
    standing_for(input, cfg!(windows), closed.as_deref())
}

/// Keeps platform capabilities explicit while preserving the Mac briefing contract.
/// `closed` is `None` while the host sandbox gate (`seatbelt::available`) is open; otherwise it
/// holds the reason the gate gives, which a Windows conversation without the tool is told.
fn standing_for(input: &Standing, windows: bool, closed: Option<&str>) -> String {
    let has = |name: &str| input.tools.iter().any(|tool| tool == name);
    let linux = cfg!(target_os = "linux") && !windows;
    let mut text = if linux { String::from("You are an assistant running inside MiniClaw, a local harness on the user's Linux computer that gives AI models one chat and sandboxed tools. MiniClaw, not you, decides what you may use; nothing you write changes a setting, a permission or a tool.\n\n## Where you are\n") } else if windows { String::from("You are an assistant running inside MiniClaw, a local harness on the user's Windows computer. MiniClaw decides permissions and available tools; nothing you write changes them. Virtual machines are unavailable in this Windows build.\n\n## Where you are\n") } else { String::from("You are an assistant running inside MiniClaw, a local harness on the user's own Mac that gives AI models one chat, sandboxed tools and optional macOS virtual machines. MiniClaw, not you, decides what you may use; nothing you write changes a setting, a permission or a tool.\n\n## Where you are\n") };
    let machine = MACHINE.get().map(String::as_str).unwrap_or(if windows { "Windows" } else if linux { "Linux" } else { "macOS" });
    if windows {
        text.push_str(&format!("- You run on Windows, {}. MiniClaw {}.\n", std::env::consts::ARCH, env!("CARGO_PKG_VERSION")));
        text.push_str(&format!("- Native executable versions detected in PATH: {}. Batch wrappers are not probed.\n", TOOLS.get().map(String::as_str).unwrap_or("not checked yet")));
    } else if input.in_vm {
        text.push_str(&format!("- You are running inside a MiniClaw virtual machine, {}, a separate macOS computer. Your tools act on this VM, not on the user's Mac.\n", input.machine));
    } else if linux {
        text.push_str(&format!("- You run on the user's computer: {machine}. MiniClaw {}.\n", env!("CARGO_PKG_VERSION")));
        text.push_str(&format!("- Tools found on this computer: {}. Others may exist; check with a command before saying one is missing.\n",
            TOOLS.get().map(String::as_str).unwrap_or("not checked yet")));
    } else {
        text.push_str(&format!("- You run on the user's Mac: {machine}. MiniClaw {}.\n", env!("CARGO_PKG_VERSION")));
        text.push_str(&format!("- Tools found on this Mac: {}. Others may exist; check with a command before saying one is missing.\n",
            TOOLS.get().map(String::as_str).unwrap_or("not checked yet")));
    }
    match &input.folder {
        Some(folder) => {
            if windows {
                text.push_str(&format!("- Your working folder is {}. Use the permission-checked file tools there and use its absolute Windows path as the shell cwd. MiniClaw enforces the granted paths and any blocked subfolders.\n", folder.path));
                text.push_str(PROVIDER_FOLDER);
            } else {
                text.push_str(&format!("- Your working folder is {}. Read and write there without asking, and use it as the working directory for commands.\n", folder.path));
                text.push_str(PROVIDER_FOLDER);
            }
            if !folder.names.is_empty() {
                text.push_str(&format!("- At its top level when this conversation was loaded: {}{}.\n", folder.names.join(", "),
                    if folder.more > 0 { format!(", and {} more", folder.more) } else { String::new() }));
            }
            match &folder.git {
                Some(git) => text.push_str(&format!("- Git: {git}.\n")),
                None if folder.unchecked => text.push_str("- Git could not be checked for this folder; do not assume it is or is not a repository.\n"),
                None if !folder.names.is_empty() => text.push_str("- It is not a git repository.\n"),
                None => {}
            }
        }
        None if has(crate::scoped::TOOL) => text.push_str("- This conversation has no working folder of its own. Paths the user names need their approval.\n"),
        None => {}
    }
    text.push_str("\n## What you can use\n");
    if windows && input.computer_use {
        text.push_str("- Windows Computer Use is on through the configured native helper. It operates on the active foreground desktop with the user's normal app access, outside the shell sandbox. Each action needs explicit user approval. Use list_apps and list_windows to discover IDs, inspect before acting, and verify results. Stop after a denied approval, native helper failure, or physical Escape. Do not use other tools to bypass protected apps.\n");
    } else if input.computer_use {
        text.push_str("- Computer Use is on: your computer tools see and control apps on this machine. Each app needs the user's approval, which MiniClaw asks for by itself. MiniClaw's file and command tool is not part of a Computer Use conversation; the tools listed below are.\n");
    } else if has(crate::scoped::TOOL) {
        text.push_str(if windows { WINDOWS_FILE_TOOL } else if linux { LINUX_FILE_TOOL } else { FILE_TOOL });
    } else if let (true, Some(reason)) = (windows, closed) {
        text.push_str(&windows_no_sandbox(reason));
    } else {
        text.push_str("- You have no file or command tool in this conversation, so you cannot read files or run anything. The user gets one by starting the conversation with an agent or in a project.\n");
    }
    if has(crate::mcp_gateway::TOOL) {
        if windows { text.push_str("- Windows sandboxed stdio MCP servers use AppContainer and their configured folder and installation grants. Approved network hosts and local ports require the Windows network service. Remote MCP servers and explicitly user-configured unrestricted servers keep their own settings; never change access to bypass a failed sandbox setup.\n"); }
        text.push_str("- miniclaw_mcp reaches the MCP servers switched on for this conversation. Call it with action list_tools to see what exists before saying a service is unavailable, with query to find a tool by its words, and describe_tool for a tool's inputSchema before calling it; never guess a tool name. Each tool asks the user before it runs.\n");
        if input.in_vm {
            text.push_str("  MCP servers run on the user's Mac, not in this VM: their tools see and change things on the Mac, and they cannot see this VM's files.\n");
        }
    }
    if has("miniclaw_delegate") {
        text.push_str("- miniclaw_delegate hands a task to another agent; its description names who. Use action=status to check existing delegated tasks when the user asks for progress.\n");
        text.push_str(&format!("  The user chooses whether your turn waits for a run or lets it run in the background. A waited run still going after {} minutes continues in the background when its time limit is longer, and its answer arrives later in the conversation. Each message's environment note gives your current limits.\n",
            crate::agents::Delegation::MOST_WAIT_MINUTES));
    }
    if has(crate::ask::TOOL) {
        text.push_str("- miniclaw_ask puts questions to the user in a popup with options and room for their own answer. Whenever you need a decision or a missing requirement from the user, call it instead of writing the question in your message: the popup is how the user expects to be asked, and your turn continues with their answer. Write a question in your message only if the tool is switched off.\n");
    }
    if has(crate::project_tool::TOOL) {
        text.push_str("- miniclaw_project holds what this project's conversations share. Its memory is a set of short notes: you are told their names once, read one when it matters, and save what a later conversation will need. Its description says what belongs there.\n");
        text.push_str("  It also holds the project's issues, its known bugs and features. File each bug you notice and will not fix now with issue_create, after issue_list shows it is not there yet. Claim an issue before working on it (it moves from Backlog to In Dev), move it through issue_stage (dev, qa, agent_review) as you work, which the user watches live on a board; only its owner finishes it with issue_done or lets go with issue_release and a handover. The environment note says which severities you may close yourself; the rest wait for the user's review.\n");
    }
    text.push_str(if input.web_search { "- Web search is available through your provider.\n" }
        else { "- Web search is switched off for this conversation.\n" });
    text.push_str("- Each message also carries a short \"MiniClaw environment\" note with today's date and what is on, off or not allowed right now. Trust it over your memory of earlier messages.\n");
    text.push_str("\n## How to work here\n\
- Look before you ask. If a file, a folder listing, a command, a search, list_tools or the environment note can answer a question, find the answer yourself. Ask the user only for what only they can know: a preference, a choice between real alternatives, a missing requirement, or consent to something risky.\n\
- Never ask the user for permission in your message. When a step needs approval, MiniClaw shows the user a popup by itself; just make the call. If it is refused, do not retry the same call or look for a way around it; say what was refused and carry on with what is allowed.\n\
- Something marked \"not allowed\" cannot be switched on from the conversation. If the task needs it, tell the user once that they can allow it for this conversation's agent in Settings, Agents, and then switch it from Tools beside the message box.\n\
- Never tell the user to run a command for you when you have a tool that can run it.\n\
- Secrets: never ask for a password, token or key in the chat, and never write one into a file or a command.\n");
    text.push_str(&rules(&input.rules));
    text
}

/// Tells a Windows conversation without the tool why, with the gate's own reason and remedy, and the
/// one thing the user can grant meanwhile. While the sandbox is unavailable an agent or a project
/// does not bring the tool; only the timed shell does.
fn windows_no_sandbox(reason: &str) -> String {
    let why = if reason.is_empty() { String::new() } else { format!(" The reason MiniClaw gives: {reason} Tell the user that once if they ask for files or commands.") };
    format!("- You have no file or command tool in this conversation: MiniClaw's sandbox is not available on this Windows computer, so you cannot read files or run anything, and another agent or a project does not change that.{why} If the task needs a command meanwhile, tell the user once that they can grant timed Unsandboxed shell access from Tools beside the message box; it runs commands as the user, outside the sandbox, until it expires.\n")
}

/// Your provider reports MiniClaw's private chat folder as its working directory; a model that took
/// it for the conversation's folder sent commands there, where the sandbox may not go.
const PROVIDER_FOLDER: &str = "- Your provider may report a different folder as its own working directory. That folder is MiniClaw's, not this conversation's: never use that folder as cwd or for files; use the working folder above.\n";

/// Describes Windows file brokerage, native shell isolation and the explicit timed exception.
const WINDOWS_FILE_TOOL: &str = "- miniclaw_scoped_native offers list_folder, search, read_file, write_file, replace_in_file and run_shell with absolute Windows paths. File operations use MiniClaw's native permission broker. Your provider's own sandbox is read-only on purpose; this tool is how you write. write_file creates a file that does not exist yet (its folder must exist; make folders with run_shell) and replaces one that does. Never tell the user a folder is read-only because of your own sandbox; try the tool and report what it answers. Commands use the bundled POSIX shell inside AppContainer; use POSIX syntax and forward slashes in shell paths. A conversation with a working folder uses its shared terminal and preserves shell state. File grants and blocked paths remain enforced. Approved network access requires the installed Windows network service and the exact hosts requested by the command. If sandbox or network setup fails, report the failure without switching execution modes.\n\
- A live timed Unsandboxed shell grant is an explicit exception for run_shell only. It runs in a fresh PowerShell as the signed-in user with unrestricted file and network access; expiry or revocation stops its process tree. It does not share the terminal. Each operation still follows the conversation's permissions and approvals.\n";

/// The file and command tool, described for a conversation that has it.
const FILE_TOOL: &str = "- miniclaw_scoped_native is how you touch files and run commands: list_folder, search (literal text, not a pattern), read_file, write_file, replace_in_file (one exact passage that occurs once) and run_shell. Paths are absolute. Prefer list_folder and search to explore, and replace_in_file to change part of a file. AppleScript and GUI control are unavailable.\n\
- A live timed Unsandboxed shell grant is an explicit exception for run_shell only: it runs as the macOS user with unrestricted files and network, in a fresh shell that stops when access expires or is revoked. It does not share the terminal. Otherwise commands run in a macOS sandbox that reaches only the files and folders this conversation was granted, and has no network unless the environment note says network is on and the command lists the exact hosts it needs. Your provider's own sandbox is read-only on purpose; this tool is how you write. write_file creates a file that does not exist yet (its folder must exist; make folders with run_shell) and replaces one that does. Never tell the user a folder is read-only because of your own sandbox; try the tool and report what it answers.\n";

/// The file and command tool on Linux, where commands run under bubblewrap.
const LINUX_FILE_TOOL: &str = "- miniclaw_scoped_native is how you touch files and run commands: list_folder, search (literal text, not a pattern), read_file, write_file, replace_in_file (one exact passage that occurs once) and run_shell. Paths are absolute. Prefer list_folder and search to explore, and replace_in_file to change part of a file. GUI control is unavailable.\n\
- A live timed Unsandboxed shell grant is an explicit exception for run_shell only: it runs as the user's own account with unrestricted files and network, in a fresh shell that stops when access expires or is revoked. It does not share the terminal. Otherwise commands run in a bubblewrap sandbox that contains only the system folders, read-only, and the files and folders this conversation was granted; anything else does not exist inside it. It has no network unless the environment note says network is on and the command lists the exact hosts it needs. Your provider's own sandbox is read-only on purpose; this tool is how you write. write_file creates a file that does not exist yet (its folder must exist; make folders with run_shell) and replaces one that does. Never tell the user a folder is read-only because of your own sandbox; try the tool and report what it answers.\n";

/// One capability's state, for the note that travels with each message.
pub struct Capability { pub name: String, pub allowed: bool, pub on: bool }

/// What the note is built from.
pub struct State {
    pub machine: String,
    pub agent: Option<String>,
    pub project: Option<String>,
    pub capabilities: Vec<Capability>,
    /// Names of the MCP servers that are switched on; `None` where the conversation has no gateway.
    pub servers: Option<Vec<String>>,
    pub delegates: Vec<String>,
    /// The limits that apply to this conversation's delegations, said beside the recipients.
    pub limits: Option<crate::agents::Delegation>,
    /// A conversation with no project and no agent: its own folder once made, else where it will be.
    pub own_folder: Option<OwnFolder>,
}

/// Where a conversation with no project keeps its files.
pub enum OwnFolder { Made(String), Later(String) }

/// Words an agent's delegation limits for the model, as the delegation tool and the note say them.
/// The hour a waited run gets is named only where a run may outlast it.
pub fn delegation_limits(limits: &crate::agents::Delegation) -> String {
    let wait = crate::agents::Delegation::MOST_WAIT_MINUTES;
    let mut text = format!("at most {} delegation{} per user message; each run may take up to {} minutes",
        limits.max_per_turn, if limits.max_per_turn == 1 { "" } else { "s" }, limits.minutes);
    if limits.hands_over() {
        text.push_str(&format!("; a run the user chose to wait for that is still going after {wait} minutes continues in the background"));
    }
    text
}

/// Writes the note. It names the date and never the time, so it changes only when the state does.
pub fn state(input: &State) -> String {
    let mut text = format!("MiniClaw environment (written by MiniClaw from its own state, current as of this message)\n- Date: {}\n- Machine: {}\n- Agent: {}\n",
        chrono::Local::now().format("%A %-d %B %Y"),
        if cfg!(windows) && input.machine == "this Mac" { "this Windows computer" }
        else if cfg!(target_os = "linux") && input.machine == "this Mac" { "this computer" } else { &input.machine }, input.agent.as_deref().unwrap_or("none (this conversation belongs to the main agent)"));
    if let Some(project) = &input.project { text.push_str(&format!("- Project: {project}\n")); }
    for capability in &input.capabilities {
        text.push_str(&format!("- {}: {}\n", capability.name, match (capability.allowed, capability.on) {
            (false, _) => "not allowed for this conversation's agent",
            (true, true) => "on",
            (true, false) => "off (the user can switch it on from Tools)",
        }));
    }
    // Named with the way to reach them: a model that is asked for "the Blender MCP" looks for a tool of
    // that name, finds none, and says it has no access, unless it is told the gateway is the way in.
    text.push_str(&format!("- MCP servers: {}\n", match &input.servers {
        None => "not available in a VM conversation".to_owned(),
        Some(servers) if servers.is_empty() => "none switched on".to_owned(),
        Some(servers) => format!("{} (switched on; reach them only through the miniclaw_mcp tool: list_tools, then call_tool)", servers.join(", ")),
    }));
    if !input.delegates.is_empty() {
        let limits = input.limits.as_ref().map(|limits| format!(" ({})", delegation_limits(limits))).unwrap_or_default();
        text.push_str(&format!("- You may delegate to: {}{limits}\n", input.delegates.join(", ")));
    }
    match &input.own_folder {
        Some(OwnFolder::Made(folder)) => text.push_str(&format!("- Working folder: {folder}. This conversation has no project; this is its own folder. Commands without cwd run there; write files there.\n")),
        Some(OwnFolder::Later(root)) => text.push_str(&format!("- Working folder: none yet. This conversation has no project; MiniClaw makes it its own folder in {root} the first time it runs a command or writes a file. Leave cwd out of run_shell to use it.\n")),
        None => {}
    }
    text
}

#[cfg(test)]
mod tests {
    /// On Linux the model hears it is on the user's Linux computer, in the bubblewrap sandbox, with
    /// no virtual machines and nothing of macOS.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_linux_briefing_names_its_computer_and_sandbox() {
        use super::*;
        let text = standing(&Standing { machine: "this Mac", in_vm: false, tools: vec![crate::scoped::TOOL.into()],
            web_search: false, computer_use: false, folder: None, rules: Rules::default() });
        assert!(text.contains("the user's Linux computer") && text.contains("bubblewrap"), "{text}");
        assert!(!text.contains("Mac") && !text.contains("macOS") && !text.contains("virtual machine"), "{text}");
        let note = state(&State { machine: "this Mac".into(), agent: None, project: None, capabilities: Vec::new(),
            servers: Some(Vec::new()), delegates: Vec::new(), limits: None, own_folder: None });
        assert!(note.contains("- Machine: this computer"), "{note}");
    }

    use super::*;

    // Lead decision 2026-10-11: the model is told its delegation limits and the hour a waited run gets.
    #[test]
    fn the_delegation_limits_are_worded_for_the_model() {
        use crate::agents::Delegation;
        assert_eq!(delegation_limits(&Delegation { max_per_turn: 1, minutes: 30 }),
            "at most 1 delegation per user message; each run may take up to 30 minutes");
        assert_eq!(delegation_limits(&Delegation { max_per_turn: 4, minutes: 150 }),
            "at most 4 delegations per user message; each run may take up to 150 minutes; \
             a run the user chose to wait for that is still going after 60 minutes continues in the background");
        let note = state(&State { machine: "this Mac".into(), agent: None, project: None, capabilities: Vec::new(), servers: Some(Vec::new()),
            delegates: vec!["Blender".into()], limits: Some(Delegation { max_per_turn: 2, minutes: 5 }), own_folder: None });
        assert!(note.contains("- You may delegate to: Blender (at most 2 delegations per user message; each run may take up to 5 minutes)\n"), "{note}");
        let standing = standing_for(&Standing { machine: "this Mac", in_vm: false, tools: vec!["miniclaw_delegate".into()], web_search: true,
            computer_use: false, folder: None, rules: Rules::default() }, false, None);
        assert!(standing.contains("The user chooses whether your turn waits for a run or lets it run in the background"), "{standing}");
        assert!(standing.contains("still going after 60 minutes continues in the background"), "{standing}");
    }

    #[test]
    fn a_tool_that_was_not_given_is_not_described() {
        let plain = standing(&Standing { machine: "this Mac", in_vm: false, tools: vec![crate::ask::TOOL.into()], web_search: true, computer_use: false, folder: None, rules: Rules::default() });
        assert!(!plain.contains("miniclaw_scoped_native"));
        assert!(!plain.contains("miniclaw_mcp"));
        assert!(plain.contains("no file or command tool"));
        let full = standing(&Standing { machine: "this Mac", in_vm: false, tools: vec![crate::scoped::TOOL.into(), crate::mcp_gateway::TOOL.into()],
            web_search: false, computer_use: false, folder: Some(Folder { path: "/work".into(), names: vec!["src/".into()], more: 3, git: Some("branch main, clean".into()), unchecked: false }),
            rules: Rules { files: vec![("AGENTS.md".into(), "Run the tests.".into())], cut: true, instructions: "British English.".into() } });
        assert!(full.contains("## Project instructions") && full.contains("### AGENTS.md") && full.contains("Run the tests.")
            && full.contains("cut at 32 KiB") && full.contains("British English."));
        assert!(!plain.contains("## Project instructions"));
        assert!(full.contains("/work") && full.contains("src/, and 3 more") && full.contains("branch main, clean"));
        assert!(full.contains("Web search is switched off"));
    }

    #[test]
    fn windows_briefing_describes_only_available_execution() {
        let input = Standing { machine: "this Windows PC", in_vm: false,
            tools: vec![crate::scoped::TOOL.into(), crate::mcp_gateway::TOOL.into()],
            web_search: false, computer_use: false,
            folder: Some(Folder { path: r"C:\Work\project".into(), ..Folder::default() }), rules: Rules::default() };
        // The gate is open: the conversation was given the tools, and they are described.
        let text = standing_for(&input, true, None);
        assert!(text.contains("Windows") && text.contains("POSIX shell"));
        assert!(text.contains("fresh PowerShell"));
        assert!(text.contains("timed Unsandboxed shell grant"));
        assert!(text.contains("AppContainer") && text.contains("shared terminal"));
        assert!(text.contains("MCP servers use AppContainer"));
        assert!(text.contains("permission-checked file tools"));
        // The provider names its own folder as its working directory; the model is told it is not this one.
        assert!(text.contains(r"Your working folder is C:\Work\project.") && text.contains("never use that folder as cwd"), "{text}");
        assert!(!text.contains("macOS") && !text.contains("Seatbelt") && !text.contains("user's Mac"));
        assert!(!text.contains("sandbox is not available"), "an open gate is not described as closed");
    }

    /// The provider's own sandbox is read-only on Windows too, so the model is told the same
    /// thing as on the Mac: the tool is how it writes, and a new file needs no other step.
    #[test]
    fn windows_file_tool_says_how_to_write() {
        let input = Standing { machine: "this Windows PC", in_vm: false, tools: vec![crate::scoped::TOOL.into()],
            web_search: false, computer_use: false, folder: None, rules: Rules::default() };
        let text = standing_for(&input, true, None);
        assert!(text.contains("read-only on purpose") && text.contains("this tool is how you write"), "{text}");
        assert!(text.contains("write_file creates a file that does not exist yet"), "{text}");
        assert!(text.contains("Never tell the user a folder is read-only because of your own sandbox"), "{text}");
        assert!(!text.contains("macOS sandbox") && !text.contains("AppleScript"));
    }

    /// A folder whose Git state could not be asked is not called "not a git repository".
    #[test]
    fn an_unchecked_folder_is_not_called_a_folder_without_git() {
        let folder = |unchecked| Folder { path: r"C:\Work\project".into(), names: vec!["src/".into()], more: 0, git: None, unchecked };
        let input = |unchecked| Standing { machine: "this Windows PC", in_vm: false, tools: vec![crate::scoped::TOOL.into()],
            web_search: false, computer_use: false, folder: Some(folder(unchecked)), rules: Rules::default() };
        assert!(standing_for(&input(false), true, None).contains("It is not a git repository."));
        let unchecked = standing_for(&input(true), true, None);
        assert!(!unchecked.contains("not a git repository"), "{unchecked}");
        assert!(unchecked.contains("Git could not be checked for this folder"), "{unchecked}");
        // The Mac sentence is as it was.
        assert!(standing_for(&input(false), false, None).contains("It is not a git repository."));
    }

    #[test]
    fn windows_with_the_sandbox_and_no_tool_gets_the_general_wording() {
        let input = Standing { machine: "this Windows PC", in_vm: false, tools: vec![crate::ask::TOOL.into()],
            web_search: true, computer_use: false, folder: None, rules: Rules::default() };
        let text = standing_for(&input, true, None);
        assert!(text.contains("You have no file or command tool in this conversation, so you cannot read files or run anything."));
        assert!(text.contains("starting the conversation with an agent or in a project"));
        assert!(!text.contains("sandbox is not available") && !text.contains("Unsandboxed shell access"));
    }

    #[test]
    fn windows_without_the_sandbox_says_what_the_user_can_do() {
        let input = Standing { machine: "this Windows PC", in_vm: false, tools: vec![crate::ask::TOOL.into()],
            web_search: true, computer_use: false, folder: None, rules: Rules::default() };
        let reason = "The MiniClaw network service is not running. Start \"MiniClaw network isolation\" in Windows Services, or restart Windows.";
        let text = standing_for(&input, true, Some(reason));
        assert!(text.contains("You have no file or command tool in this conversation"));
        assert!(text.contains("sandbox is not available on this Windows computer"));
        // The model is given the reason and its remedy, so it can pass both on once.
        assert!(text.contains(reason), "{text}");
        assert!(!text.contains("computer yet"), "the port's interim wording is gone");
        assert!(text.contains("timed Unsandboxed shell access") && text.contains("Tools beside the message box"));
        // Another agent or a project does not bring the tool while the sandbox is unavailable.
        assert!(!text.contains("starting the conversation with an agent or in a project"));
        // Once the sandbox is available, and on the Mac, the general wording stays.
        assert!(standing_for(&input, true, None).contains("starting the conversation with an agent or in a project"));
        assert!(standing_for(&input, false, Some("")).contains("starting the conversation with an agent or in a project"));
        assert!(standing_for(&input, false, None).contains("starting the conversation with an agent or in a project"));
    }

    /// Batch wrappers are not launched implicitly while collecting tool versions.
    #[cfg(windows)]
    #[tokio::test]
    async fn windows_probes_ignore_batch_wrappers() {
        let folder = std::env::temp_dir().join(format!("mc-probe-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("fixture.cmd"), "@echo off\r\necho should-not-run\r\n").unwrap();
        assert!(first_line("fixture", "--version", folder.to_str().unwrap()).await.is_none());
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn a_conversation_without_a_project_is_told_where_its_folder_is_or_will_be() {
        let note = |own_folder| state(&State { machine: "this Mac".into(), agent: None, project: None, capabilities: Vec::new(),
            servers: None, delegates: Vec::new(), limits: None, own_folder: Some(own_folder) });
        let later = note(OwnFolder::Later("/Users/u/Documents/MiniClaw".into()));
        assert!(later.contains("none yet") && later.contains("/Users/u/Documents/MiniClaw") && later.contains("first time"), "{later}");
        let made = note(OwnFolder::Made("/Users/u/Documents/MiniClaw/Chat 3f2a9c1d".into()));
        assert!(made.contains("Working folder: /Users/u/Documents/MiniClaw/Chat 3f2a9c1d.") && made.contains("no project"), "{made}");
    }

    #[test]
    fn the_note_holds_no_clock_time() {
        let note = state(&State { machine: "this Mac".into(), agent: None, project: None, capabilities: vec![
            Capability { name: "Web search".into(), allowed: true, on: true }, Capability { name: "Network for commands".into(), allowed: false, on: false }],
            servers: Some(Vec::new()), delegates: Vec::new(), limits: None, own_folder: None });
        assert!(note.contains("Web search: on") && note.contains("Network for commands: not allowed"));
        assert!(!note.contains("Working folder"));
        assert!(!note.chars().collect::<Vec<_>>().windows(3).any(|w| w[0].is_ascii_digit() && w[1] == ':' && w[2].is_ascii_digit()));
    }
}
