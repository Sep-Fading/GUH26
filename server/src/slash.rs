//! Slash commands. A message that starts with `/name` attaches one skill's instructions to
//! that turn only; the typed text is sent unchanged. The names are MiniClaw's built-in skills
//! and the user's own active skills marked callable. A command adds instructions and nothing
//! else: no tool, no permission and no server comes with it.

// The user's own skills, some of which are callable.
use crate::skills::Skill;
// Shapes the list for the composer's menu.
use serde::Serialize;

/// One entry of the composer's menu.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Command {
    pub name: String,
    pub description: String,
    pub builtin: bool,
}

/// A built-in skill: its command name, what the menu says, its title and its instructions.
struct Builtin { name: &'static str, description: &'static str, title: &'static str, instructions: &'static str }

const BUILTINS: [Builtin; 3] = [Builtin {
    name: "mcp-create",
    description: "Guides you through building an MCP server, tests it, and hands it over for you to add.",
    title: "Create an MCP server",
    instructions: MCP_CREATE,
}, Builtin {
    name: "skill-create",
    description: "Writes a new skill, or a better version of one you have, and hands it over for you to review.",
    title: "Create or change a skill",
    instructions: SKILL_CREATE,
}, Builtin {
    name: "agent-create",
    description: "Writes a new agent, or a better version of one you have, and hands it over for you to review.",
    title: "Create or change an agent",
    instructions: AGENT_CREATE,
}];

/// The `/skill-create` skill. The model drafts; only the user can save.
const SKILL_CREATE: &str = "\
The user wants a MiniClaw skill: reusable instructions that are added to a conversation, an agent or a project. Work through these steps. Keep each message short.

1. Understand it. What should the skill make the agent do, and when? If that is not clear, ask. If you have the miniclaw_ask tool, ask with it, all open points in one call; otherwise ask in your message. Do not ask what you can decide sensibly yourself.

2. Check what exists. The user's skills are listed below by name and description. If one already covers this, say so and offer to improve it instead of adding a near-duplicate. You see a skill's current instructions only when the user named it in their message; if you need one you cannot see, ask the user to call /skill-create again and name it.

3. Write it. Instructions are Markdown addressed to the agent, in the imperative: what to do, in which order, what to avoid, what a good result looks like. Be specific to this task and leave out general advice any capable model follows anyway. At most 16000 characters; shorter is better. The name is at most 80 characters and the description one plain sentence.

4. Hand it over. You cannot save a skill or change one; only the user can. End your final message with one fenced block exactly like this, which MiniClaw shows the user as a card that opens the skill form filled in:

```miniclaw-skill
{\"name\": \"Pull request review\", \"description\": \"Reviews a pull request for bugs and unclear code.\", \"instructions\": \"When asked to review...\", \"slash\": true, \"replaces\": null}
```

   `slash` is true when the user should be able to call it as /its-name for one message. `replaces` is the exact name of the existing skill this is a new version of, or null for a new skill. It must be valid JSON on the lines between the fences: escape quotes and write line breaks as \\n. Then tell the user in one or two sentences what the skill does and that nothing is saved until they review it and press Save.";

/// The `/agent-create` skill. The model drafts; the user saves, then approves each permission.
const AGENT_CREATE: &str = "\
The user wants a MiniClaw agent: a named role with its own instructions, model, skills and permissions. Work through these steps. Keep each message short.

1. Understand it. What is the agent for, and what must it be able to touch? If that is not clear, ask. If you have the miniclaw_ask tool, ask with it, all open points in one call; otherwise ask in your message.

2. Check what exists. The user's agents, skills and models are listed below. If an agent already covers this, offer to improve it. You see an agent's current instructions only when the user named it in their message; if you need one you cannot see, ask the user to call /agent-create again and name it.

3. Write it. Instructions are Markdown addressed to the agent: its role, how it works, what it must not do, what a finished task looks like. At most 16000 characters. Leave model and effort null, which means the user's default, unless the user asked for a particular one; then pick only from the list below. Pick skills only from the list, by exact name, at most 8.

4. Suggest permissions sparingly. A new agent can use nothing until the user allows it. List only what this agent cannot work without, each with a one-sentence reason the user can judge. The user approves each one separately after saving, and may refuse any; write the instructions so the agent still behaves sensibly when something was refused. Kinds:
   - {\"kind\": \"feature\", \"name\": \"network\"} lets its commands reach approved hosts; \"web_fetch\" reads web pages from permitted sites; \"subagents\" delegates to restricted helper agents; \"task_tracking\" maintains a task checklist. Each is a separate permission.
   - {\"kind\": \"folder\", \"path\": \"/absolute/path\", \"action\": \"read\"} or \"write\"; {\"kind\": \"file\", ...} for one file. Ask the user for the path; never guess one, and never suggest a home folder, a whole disk, or a folder of keys or sign-ins.
   - {\"kind\": \"host\", \"host\": \"api.example.com\"} or {\"kind\": \"domain\", \"domain\": \"example.com\"} for commands that need the network.
   - {\"kind\": \"mcp_server\", \"name\": \"exact server name\"} for one of the user's MCP servers.
   - {\"kind\": \"application\", \"id\": \"com.example.App\", \"action\": \"observe\"} or \"control\".

5. Hand it over. You cannot save an agent, change one or grant it anything; only the user can. End your final message with one fenced block exactly like this, which MiniClaw shows the user as a card that opens the agent form filled in:

```miniclaw-agent
{\"name\": \"Reviewer\", \"instructions\": \"You review code...\", \"model\": null, \"effort\": null, \"skills\": [\"Pull request review\"], \"permissions\": [{\"kind\": \"folder\", \"path\": \"/Users/me/site\", \"action\": \"read\", \"reason\": \"It reads the code it reviews.\"}], \"replaces\": null}
```

   `replaces` is the exact name of the existing agent this is a new version of, or null. It must be valid JSON on the lines between the fences: escape quotes and write line breaks as \\n. Then tell the user what the agent does, and that they review and save it first and then approve each suggested permission one by one.";

/// The `/mcp-create` skill. The package names and API shapes were read from the official
/// quickstart (modelcontextprotocol.io/docs/develop/build-server) on 2026-09-21; they have
/// changed before, so the skill also tells the model to check that page.
const MCP_CREATE: &str = "\
The user wants to create an MCP server and add it to MiniClaw. Work through these steps in order. Keep each message short.

1. Understand the server. If the user has not said what it should do, ask: what should it do, and what tools should it offer? Propose a short lowercase name made of letters, digits, - or _.

2. Ask the user which language to use, every time, even if earlier servers used one: TypeScript or Python. Do not pick for them. If you have the miniclaw_ask tool, ask with it, and ask step 1's open points in the same call; otherwise ask in your message. Wait for the answer before writing any code.

3. Build it in a new folder named after the server, inside this conversation's working folder. Use the official SDK and the stdio transport:
   - TypeScript: packages `@modelcontextprotocol/server` and `zod` (dev: `typescript`, `@types/node`), `\"type\": \"module\"`. Create the server with `new McpServer({ name, version })`, add each tool with `server.registerTool(name, { description, inputSchema: z.object({...}) }, handler)`, and start it with `await server.connect(new StdioServerTransport())` from `@modelcontextprotocol/server/stdio`. Compile to JavaScript; the saved command runs the compiled file with node.
   - Python: package `mcp[cli]` in a virtual environment inside the folder (`uv add \"mcp[cli]\"`, or `python3 -m venv .venv` and pip). Create the server with `MCPServer(name)` from `mcp.server`, add each tool with the `@mcp.tool()` decorator on a typed function with a docstring, and start it with `mcp.run(transport=\"stdio\")`.
   These names have changed between SDK versions. If an import or install fails, read the official quickstart at https://modelcontextprotocol.io/docs/develop/build-server and follow what it says now.
   A stdio server must never write to stdout except protocol messages: no `console.log`, no `print`. Log to stderr.
   Installing packages needs network. If a command is refused for that reason, tell the user to allow Network for commands for this conversation's agent in Settings, Agents, to switch it on from Tools beside the message box, and to approve the registry hosts (registry.npmjs.org, or pypi.org and files.pythonhosted.org), then try again.

4. Test it before handing it over. Start the server with its real command and send it, on stdin, one line each: an `initialize` request (protocolVersion \"2025-06-18\", empty capabilities, a clientInfo), the `notifications/initialized` notification, and a `tools/list` request. Check that the answers are valid JSON-RPC and list every tool. Then call one tool with `tools/call` and check the result. Fix what fails and test again. Tell the user plainly what you tested and what you did not.

5. Hand it over. You cannot add a server to MiniClaw, switch one on, or change one; only the user can. End your final message with one fenced block exactly like this, which MiniClaw shows the user as a card they confirm:

```miniclaw-mcp-server
{\"name\": \"notes\", \"command\": \"/absolute/path/to/node\", \"args\": [\"/absolute/path/to/notes/build/index.js\"], \"folder\": \"/absolute/path/to/notes\", \"hosts\": [], \"ports\": [], \"listen\": [], \"env\": [\"NOTES_API_KEY\"]}
```

   `command` is an absolute path to the program (find it with `which`), `args` are absolute paths and flags, `folder` is the server's own folder, `hosts` lists the HTTPS hosts the server must reach (empty if none), `ports` lists local ports the server connects to when it drives an app through the app's own socket (Blender's add-on listens on 9876), `listen` lists local ports the server itself listens on when the app connects to it (Roblox Studio's plugin dials 13469), and `env` lists only the NAMES of the secrets it needs. Never ask the user for a secret's value and never write one into a file or the chat; the user enters values in MiniClaw, which keeps them in the macOS Keychain.
   Then tell the user: the server will run sandboxed, confined to its folder and hosts; after adding it they allow it for an agent in Settings, Agents (the main agent covers plain conversations) and can switch it per conversation from Tools, beside the message box; each tool asks before it runs.";

/// Turns a skill's name into its command name: lowercase, with runs of other characters as one `-`.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() { out.push(c.to_ascii_lowercase()); }
        else if !out.ends_with('-') && !out.is_empty() { out.push('-'); }
    }
    out.trim_end_matches('-').chars().take(40).collect()
}

/// Lists the commands: built-ins first, then callable active skills whose names are free.
pub fn commands(skills: &[Skill]) -> Vec<Command> {
    let mut all: Vec<Command> = BUILTINS.iter().map(|builtin| Command { name: builtin.name.into(),
        description: builtin.description.into(), builtin: true }).collect();
    for skill in skills.iter().filter(|skill| skill.definition.active && skill.definition.slash) {
        let name = slug(&skill.definition.name);
        if name.is_empty() || all.iter().any(|known| known.name == name) { continue; }
        all.push(Command { name, description: skill.definition.description.clone(), builtin: false });
    }
    all
}

/// Returns the command a message starts with: `/name` followed by a space or the end.
fn named(text: &str) -> Option<&str> {
    let rest = text.strip_prefix('/')?;
    let name = rest.split(char::is_whitespace).next()?;
    (!name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'))
        .then_some(name)
}

/// What `/skill-create` and `/agent-create` are told about the user's setup.
#[derive(Default)]
pub struct Inventory<'a> {
    pub agents: &'a [crate::agents::Agent],
    /// Model ids with the efforts each takes.
    pub models: Vec<(String, Vec<String>)>,
}

/// Reports whether a message names an item: its name appears in the message, in any letter case.
fn mentions(text: &str, name: &str) -> bool {
    let name = name.trim().to_lowercase();
    !name.is_empty() && text.to_lowercase().contains(&name)
}

/// Lists what exists for the two creating commands: names and descriptions always, and the
/// instructions of at most three items the user named. Nothing else is handed out.
fn inventory(command: &str, text: &str, skills: &[Skill], inventory: &Inventory) -> String {
    let mut out = String::from("\n### What the user has now\n\nSkills:\n");
    for skill in skills {
        out.push_str(&format!("- {}: {}{}\n", skill.definition.name, skill.definition.description,
            if skill.definition.active { "" } else { " (inactive)" }));
    }
    if skills.is_empty() { out.push_str("- none\n"); }
    if command == "agent-create" {
        out.push_str("\nAgents:\n");
        for agent in inventory.agents {
            out.push_str(&format!("- {}{}\n", agent.definition.name, if agent.definition.active { "" } else { " (inactive)" }));
        }
        if inventory.agents.is_empty() { out.push_str("- none\n"); }
        out.push_str("\nModels (id, then efforts):\n");
        for (model, efforts) in &inventory.models { out.push_str(&format!("- {model}: {}\n", efforts.join(", "))); }
        if inventory.models.is_empty() { out.push_str("- unknown right now; leave model and effort null\n"); }
    }
    let mut shown = 0;
    if command == "skill-create" {
        for skill in skills.iter().filter(|skill| mentions(text, &skill.definition.name)).take(3) {
            out.push_str(&format!("\nThe user named the skill \"{}\". Its current instructions:\n<current>\n{}\n</current>\n",
                skill.definition.name, skill.definition.instructions));
            shown += 1;
        }
    } else {
        for agent in inventory.agents.iter().filter(|agent| mentions(text, &agent.definition.name)).take(3) {
            let skills: Vec<&str> = skills.iter().filter(|skill| agent.definition.skill_ids.contains(&skill.id))
                .map(|skill| skill.definition.name.as_str()).collect();
            out.push_str(&format!("\nThe user named the agent \"{}\" (model {}, effort {}, skills: {}). Its current instructions:\n<current>\n{}\n</current>\n",
                agent.definition.name, agent.definition.model.as_deref().unwrap_or("default"), agent.definition.effort.as_deref().unwrap_or("default"),
                if skills.is_empty() { "none".to_owned() } else { skills.join(", ") }, agent.definition.instructions));
            shown += 1;
        }
    }
    if shown > 0 { out.push_str("\nTo change one of these, hand over the whole new version with `replaces` set to its exact name.\n"); }
    out
}

/// Returns the skill text a message calls for, as a section in the same shape as the
/// conversation's own skills, or nothing when the message names no known command.
pub fn section(text: &str, skills: &[Skill], known: &Inventory) -> Option<String> {
    let name = named(text)?;
    if let Some(builtin) = BUILTINS.iter().find(|builtin| builtin.name == name) {
        let extra = if name == "mcp-create" { String::new() } else { inventory(name, text, skills, known) };
        return Some(format!("\n## Skill called with /{name}: {}\n\n{}\n{extra}", builtin.title, builtin.instructions));
    }
    // The same rule as the menu, so what is listed is what runs.
    if !commands(skills).iter().any(|command| !command.builtin && command.name == name) { return None; }
    let skill = skills.iter().find(|skill| skill.definition.active && skill.definition.slash && slug(&skill.definition.name) == name)?;
    Some(format!("\n## Skill called with /{name}: {}\n\n{}\n", skill.definition.name, skill.definition.instructions))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_become_command_names() {
        assert_eq!(slug("Release Notes"), "release-notes");
        assert_eq!(slug("  MCP   create!! "), "mcp-create");
        assert_eq!(slug("日本"), "");
    }

    #[test]
    fn only_a_leading_command_counts() {
        assert_eq!(named("/mcp-create a server"), Some("mcp-create"));
        assert_eq!(named("/mcp-create"), Some("mcp-create"));
        assert_eq!(named("/release-notes/old is a folder"), None);
        assert_eq!(named("please /mcp-create"), None);
        assert_eq!(named("/Users/me/file"), None);
    }
}
