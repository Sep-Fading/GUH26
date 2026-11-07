//! How an MCP server written as `npx …` or `npm exec …` starts on Windows without npm's batch
//! launchers. npm ships `npx.cmd` and `npm.cmd` there, and MiniClaw never starts a batch file
//! (cmd.exe would parse the arguments again). Instead MiniClaw's own npm installs the package
//! into the server's private folder, and MiniClaw's own Node runs the package's command script
//! with the arguments as a real argument list. This file holds the decisions that need no
//! Windows API: recognising the launchers, reading their arguments, choosing the package's
//! command, and the npm settings. `mcp_client` applies them. macOS runs the user's `npx`.

// Names the folders and files of a server's install.
use std::path::{Path, PathBuf};
// Reads a package's manifest.
use serde_json::Value;

/// npm's two launchers that MiniClaw recognises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shim { Npx, Npm }

/// What a recognised command asks for: packages from the registry and the command to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Registry package specifications to install, in the order given.
    pub packages: Vec<String>,
    /// The command to run when `--package` named the packages; otherwise the first package's own.
    pub command: Option<String>,
    /// The command's arguments, exactly as saved.
    pub args: Vec<String>,
    /// A registry other than the default, as an `https` address.
    pub registry: Option<String>,
    /// False for `--no-install` or `--yes=false`: run only what is already installed.
    pub install: bool,
    /// The user asked npm not to run install scripts.
    pub ignore_scripts: bool,
}

/// The registry npm uses when nothing else is configured.
pub const DEFAULT_REGISTRY: &str = "https://registry.npmjs.org/";

impl Shim {
    /// The bare name an import saves for this launcher.
    pub fn name(self) -> &'static str {
        match self { Self::Npx => "npx", Self::Npm => "npm" }
    }
}

/// Recognises npm's launchers by the last path component, in any case: `npx`, `npx.cmd`, `npm`
/// and `npm.cmd`, bare or as a path. Nothing at that path is ever run; only the name is used.
pub fn shim(command: &str) -> Option<Shim> {
    let leaf = command.rsplit(['\\', '/']).next()?;
    match leaf.to_ascii_lowercase().as_str() {
        "npx" | "npx.cmd" => Some(Shim::Npx),
        "npm" | "npm.cmd" => Some(Shim::Npm),
        _ => None,
    }
}

/// Recognises the bare launcher name a saved server may carry instead of an absolute path.
pub fn bare(command: &str) -> Option<Shim> {
    match command { "npx" => Some(Shim::Npx), "npm" => Some(Shim::Npm), _ => None }
}

/// Reads `npx [options] <package> [args]` or `npm exec|x [options] [--] <package> [args]` into a
/// plan. Options are npx's own and end at the first positional or `--`; whatever follows is the
/// command's, byte for byte. Unknown options, shell strings (`--call`) and anything that is not
/// a registry package are refused with a sentence the user can act on.
pub fn plan(shim: Shim, args: &[String]) -> Result<Plan, String> {
    let exec = shim == Shim::Npm;
    let mut rest = args;
    if exec {
        match rest.first().map(String::as_str) {
            Some("exec" | "x") => rest = &rest[1..],
            _ => return Err("On Windows an npm command can start an MCP server only as `npm exec` or `npx`.".into()),
        }
    }
    let mut plan = Plan { packages: Vec::new(), command: None, args: Vec::new(), registry: None, install: true, ignore_scripts: false };
    let mut index = 0;
    let mut separated = false;
    while let Some(arg) = rest.get(index).map(String::as_str) {
        if arg == "--" { separated = true; index += 1; break; }
        if !arg.starts_with('-') || arg == "-" { break; }
        let (key, inline) = match arg.split_once('=') { Some((key, value)) => (key, Some(value)), None => (arg, None) };
        let flag = |value: Option<&str>| match value { None | Some("true") => Ok(true), Some("false") => Ok(false),
            Some(_) => Err(format!("{key} takes true or false.")) };
        let mut value = || -> Result<String, String> {
            if let Some(value) = inline { return Ok(value.to_owned()); }
            index += 1;
            rest.get(index).cloned().ok_or_else(|| format!("{key} needs a value."))
        };
        match key {
            "-y" | "--yes" => plan.install = flag(inline)?,
            "--no-install" | "--offline" => plan.install = false,
            "-q" | "--quiet" | "--silent" | "--prefer-offline" | "--prefer-online" => {}
            "--loglevel" => { value()?; }
            "--ignore-scripts" => plan.ignore_scripts = flag(inline)?,
            "-p" | "--package" => {
                let spec = value()?;
                if package_name(&spec).is_none() { return Err(not_registry(&spec)); }
                plan.packages.push(spec);
            }
            "--registry" => {
                let registry = value()?;
                registry_host(Some(&registry))?;
                plan.registry = Some(registry);
            }
            "-c" | "--call" | "--shell" | "--script-shell" =>
                return Err(format!("{key} runs a command line through a shell, which MiniClaw does not do. Name the package and its arguments instead.")),
            _ => return Err(format!("MiniClaw does not know the npx option {key}. Remove it, or put the command's own options after the package.")),
        }
        index += 1;
    }
    let first = rest.get(index).ok_or("Name the package to run.")?;
    let mut remaining: Vec<String> = rest[index + 1..].to_vec();
    if exec && !separated {
        // npm reads every option itself until the first `--`, so one meant for the command must follow it.
        let end = remaining.iter().position(|arg| arg == "--").unwrap_or(remaining.len());
        if remaining[..end].iter().any(|arg| arg.starts_with('-')) {
            return Err("Put `--` before the command's own options (npm exec -- <package> <options>); without it npm reads them itself.".into());
        }
        if end < remaining.len() { remaining.remove(end); }
    }
    if plan.packages.is_empty() {
        if package_name(first).is_none() { return Err(not_registry(first)); }
        plan.packages.push(first.clone());
    } else {
        let valid = !first.is_empty() && first.len() <= 214 && !first.starts_with(['.', '-'])
            && first.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._@+-".contains(&byte));
        if !valid { return Err(format!("{first} is not the name of a package's command.")); }
        plan.command = Some(first.clone());
    }
    plan.args = remaining;
    Ok(plan)
}

/// Says why a package specification was refused.
fn not_registry(spec: &str) -> String {
    format!("{spec} is not a package from the npm registry. On Windows MiniClaw installs MCP servers from the registry only \
        (a name with an optional version, such as @scope/server@1.2.0).")
}

/// Returns the package name of a registry specification (`name`, `@scope/name`, either with
/// `@version`, a range or a tag), or `None` for paths, URLs, Git shorthands, aliases and
/// anything with characters a version never has.
pub fn package_name(spec: &str) -> Option<&str> {
    let split = if let Some(scoped) = spec.strip_prefix('@') { scoped.find('@').map(|at| at + 1) } else { spec.find('@') };
    let (name, version) = match split { Some(at) => (&spec[..at], Some(&spec[at + 1..])), None => (spec, None) };
    let part = |text: &str| !text.is_empty() && !text.starts_with(['.', '_'])
        && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._~-".contains(&byte));
    let named = match name.strip_prefix('@') {
        Some(scoped) => scoped.split_once('/').is_some_and(|(scope, package)| part(scope) && part(package)),
        None => part(name),
    };
    let versioned = version.is_none_or(|version| !version.is_empty() && version.len() <= 256
        && version.bytes().all(|byte| byte.is_ascii_alphanumeric() || b".^~<>=*+_-".contains(&byte)));
    (named && versioned && name.len() <= 214).then_some(name)
}

/// The host a registry is reached at, which must be one of the server's approved hosts for an
/// install. Only `https` on the default port: the proxy opens nothing else.
pub fn registry_host(registry: Option<&str>) -> Result<String, String> {
    let Some(registry) = registry else { return Ok("registry.npmjs.org".into()) };
    let refused = || format!("{registry} is not an https registry address on port 443.");
    let rest = registry.get(..8).filter(|scheme| scheme.eq_ignore_ascii_case("https://")).map(|_| &registry[8..]).ok_or_else(refused)?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = match authority.rsplit_once(':') {
        Some((host, "443")) => host,
        Some(_) => return Err(refused()),
        None => authority,
    }.to_ascii_lowercase();
    (!authority.contains('@') && crate::permissions::dns_name(&host)).then_some(host).ok_or_else(refused)
}

/// Chooses the command to run the way npx does (libnpmexec `get-bin-from-manifest.js`): the one
/// asked for; else the only one, or the only script all names share; else the one named like
/// the unscoped package. Returns the command's name and its script path as the package wrote it.
pub fn bin(manifest: &Value, wanted: Option<&str>) -> Result<(String, String), String> {
    let name = manifest["name"].as_str().unwrap_or_default();
    let unscoped = name.strip_prefix('@').and_then(|scoped| scoped.split_once('/')).map_or(name, |(_, package)| package);
    let bins: Vec<(String, String)> = match &manifest["bin"] {
        Value::String(path) => vec![(unscoped.to_owned(), path.clone())],
        Value::Object(map) => map.iter().filter_map(|(key, path)| Some((key.clone(), path.as_str()?.to_owned()))).collect(),
        _ => Vec::new(),
    };
    if let Some(wanted) = wanted {
        return bins.into_iter().find(|(key, _)| key == wanted).ok_or_else(|| format!("{name} has no command named {wanted}."));
    }
    if bins.is_empty() { return Err(format!("{name} has no command to run.")); }
    if bins.iter().all(|(_, path)| *path == bins[0].1) { return Ok(bins[0].clone()); }
    let names: Vec<&str> = bins.iter().map(|(key, _)| key.as_str()).collect();
    bins.iter().find(|(key, _)| key == unscoped).cloned()
        .ok_or_else(|| format!("{name} has several commands ({}). Name one: npx --package {name} <command>.", names.join(", ")))
}

/// Turns a command's script path from a manifest into a relative path that stays inside the
/// package: no root, drive, stream or `..`.
pub fn entry_path(text: &str) -> Option<PathBuf> {
    if text.is_empty() || text.contains(['\0', ':']) || text.starts_with(['/', '\\']) { return None; }
    let mut path = PathBuf::new();
    for part in text.split(['/', '\\']) {
        match part { "" | "." => {}, ".." => return None, part => path.push(part) }
    }
    (path.components().next().is_some()).then_some(path)
}

/// Decides how a package's command script runs: with Node and the options its `#!` line gives
/// Node (`#!/usr/bin/env -S node --flag`), or, without that line, with Node when it is a `.js`,
/// `.cjs` or `.mjs` file. Anything else is refused.
pub fn runner(head: &[u8], name: &str) -> Result<Vec<String>, String> {
    let refused = || format!("{name} is not a Node script, so MiniClaw cannot run it without a shell.");
    if let Some(line) = head.strip_prefix(b"#!") {
        let line = String::from_utf8_lossy(line.split(|byte| *byte == b'\n').next().unwrap_or_default());
        let mut words = line.split_whitespace();
        let mut program = words.next().ok_or_else(refused)?;
        let leaf = |path: &str| path.rsplit(['/', '\\']).next().unwrap_or_default().to_owned();
        if leaf(program) == "env" {
            program = words.by_ref().find(|word| !word.starts_with('-')).ok_or_else(refused)?;
        }
        if !matches!(leaf(program).as_str(), "node" | "node.exe") { return Err(refused()); }
        return Ok(words.map(str::to_owned).collect());
    }
    let extension = name.rsplit_once('.').map(|(_, extension)| extension.to_ascii_lowercase()).unwrap_or_default();
    matches!(extension.as_str(), "js" | "cjs" | "mjs").then(Vec::new).ok_or_else(refused)
}

/// What decides whether an install can be reused: the packages, the registry, the scripts
/// choice and the runtime versions. The command's own arguments do not.
pub fn install_key(plan: &Plan) -> String {
    serde_json::json!({ "packages": plan.packages, "registry": plan.registry, "ignoreScripts": plan.ignore_scripts,
        "node": crate::windows_runtimes::NODE_VERSION, "npm": crate::windows_runtimes::NPM_VERSION }).to_string()
}

/// The arguments of the install run: npm's own script, then `install` with the packages, into
/// the folder the run starts in.
pub fn install_arguments(cli: &Path, plan: &Plan) -> Vec<String> {
    let mut args = vec![cli.to_string_lossy().into_owned(), "install".into(), "--no-audit".into(), "--no-fund".into(), "--save-exact".into()];
    if plan.ignore_scripts { args.push("--ignore-scripts".into()); }
    args.extend(plan.packages.iter().cloned());
    args
}

/// npm's settings for one server's install: configuration, cache and global prefix inside the
/// server's own folder (never the user's `.npmrc`), BusyBox `sh` for package scripts instead of
/// cmd.exe with npm's own standard handles, no update check, and few short retries so a dead
/// registry fails within seconds.
pub fn install_environment(folder: &Path, shell: &Path, plan: &Plan) -> Vec<(String, String)> {
    let text = |path: PathBuf| path.to_string_lossy().into_owned();
    let mut values = vec![
        ("npm_config_userconfig".into(), text(folder.join("npmrc"))),
        // npm refuses one file as both user and global configuration.
        ("npm_config_globalconfig".into(), text(folder.join("global").join("npmrc"))),
        ("npm_config_cache".into(), text(folder.join("cache"))),
        ("npm_config_prefix".into(), text(folder.join("global"))),
        ("npm_config_script_shell".into(), shell.to_string_lossy().into_owned()),
        ("npm_config_update_notifier".into(), "false".into()),
        // Install scripts share npm's handles: a piped child would hang Node 24.21 in the sandbox.
        ("npm_config_foreground_scripts".into(), "true".into()),
        ("npm_config_fund".into(), "false".into()),
        ("npm_config_audit".into(), "false".into()),
        ("npm_config_fetch_retries".into(), "1".into()),
        ("npm_config_fetch_retry_mintimeout".into(), "2000".into()),
        ("npm_config_fetch_retry_maxtimeout".into(), "10000".into()),
        ("npm_config_fetch_timeout".into(), "60000".into()),
    ];
    if let Some(registry) = &plan.registry { values.push(("npm_config_registry".into(), registry.clone())); }
    values
}

/// The private folder of one server's npm install inside MiniClaw's data folder, for a saved
/// server's identifier only.
pub fn server_folder(data: &Path, id: &str) -> Option<PathBuf> {
    let valid = !id.is_empty() && id.len() <= 64 && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    valid.then(|| data.join("mcp-npm").join(id))
}

/// Says in one sentence what actually runs for a server saved as `npx`/`npm exec`, for the page.
/// `None` for any other command, or arguments MiniClaw would refuse to start.
pub fn describe(command: &str, args: &[String]) -> Option<String> {
    let plan = plan(shim(command)?, args).ok()?;
    let host = registry_host(plan.registry.as_deref()).ok()?;
    let what = match &plan.command {
        Some(command) => format!("the command {command} of {}", plan.packages.join(" and ")),
        None => format!("the command of {}", plan.packages[0]),
    };
    let with = if plan.args.is_empty() { String::new() } else { format!(" with {}", plan.args.join(" ")) };
    Some(format!("On Windows, MiniClaw's Node runs {what}{with}. MiniClaw's npm installs it into this server's private folder \
        from {host} at the first start, which needs {host} approved; never npx.cmd or cmd.exe."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> { items.iter().map(|item| (*item).to_owned()).collect() }

    #[test]
    fn npm_launchers_are_recognised_by_their_exact_names() {
        for (command, expected) in [("npx", Shim::Npx), ("npm", Shim::Npm), ("npx.cmd", Shim::Npx), ("NPM.CMD", Shim::Npm),
            (r"C:\Program Files\nodejs\npx.cmd", Shim::Npx), (r"C:\Program Files\nodejs\npx", Shim::Npx), ("C:/nvm/v20/npm.cmd", Shim::Npm),
            (r"D:\Søren's tools\nodejs\NPX.Cmd", Shim::Npx)] {
            assert_eq!(shim(command), Some(expected), "{command}");
        }
        // Anything else is not npm's launcher: a native program stays one, other scripts stay refused.
        for command in ["npx.cmd.exe", r"C:\x\npx.cmd.exe", "npx .cmd", "npx.cmd ", "npx.bat", r"C:\x\npx.ps1", "npx.exe", r"C:\x\server.cmd",
            r"C:\x\npxx.cmd", "xnpx", "", r"C:\npx.cmd\server.exe", "npx.cmd.", "corepack.cmd", "pnpm", "npm.cmd:stream"] {
            assert_eq!(shim(command), None, "{command:?}");
        }
        // What an import saves is the bare name only.
        assert_eq!(bare("npx"), Some(Shim::Npx));
        assert_eq!(bare("npm"), Some(Shim::Npm));
        for command in ["npx.cmd", "NPX", r"C:\x\npx", " npx"] { assert_eq!(bare(command), None, "{command}"); }
    }

    #[test]
    fn npx_arguments_become_a_plan_with_the_command_arguments_untouched() {
        let plan = plan(Shim::Npx, &strings(&["-y", "@upstash/context7-mcp"])).unwrap();
        assert_eq!(plan, Plan { packages: strings(&["@upstash/context7-mcp"]), command: None, args: vec![], registry: None, install: true, ignore_scripts: false });
        // Every byte of the server's own arguments reaches it: shell and batch metacharacters included.
        let hostile = ["--api-key", "a&b|c", "%PATH%", "^caret", "\"quoted\"", "two words", "", "-y", "--registry=https://evil.example/", "'single'", "$(x)", "back\\slash\\"];
        let mut args = strings(&["--yes", "server-pkg@1.2.3"]);
        args.extend(strings(&hostile));
        let plan = super::plan(Shim::Npx, &args).unwrap();
        assert_eq!((plan.packages, plan.args, plan.registry), (strings(&["server-pkg@1.2.3"]), strings(&hostile), None));
        // `--package` names what to install and the positional is the command; `--` ends npx's options.
        let plan = super::plan(Shim::Npx, &strings(&["--package=@scope/a@^2", "-p", "b", "--registry", "https://registry.example.com/npm/", "--ignore-scripts", "--", "a-cli", "--flag"])).unwrap();
        assert_eq!(plan, Plan { packages: strings(&["@scope/a@^2", "b"]), command: Some("a-cli".into()), args: strings(&["--flag"]),
            registry: Some("https://registry.example.com/npm/".into()), install: true, ignore_scripts: true });
        assert!(!super::plan(Shim::Npx, &strings(&["--no-install", "pkg"])).unwrap().install);
        assert!(!super::plan(Shim::Npx, &strings(&["--yes=false", "pkg"])).unwrap().install);
        assert!(super::plan(Shim::Npx, &strings(&["-q", "--prefer-offline", "pkg@latest"])).is_ok());
    }

    #[test]
    fn npm_exec_is_read_like_npx_and_other_npm_commands_are_refused() {
        let plan = plan(Shim::Npm, &strings(&["exec", "--yes", "--", "pkg", "--stdio"])).unwrap();
        assert_eq!((plan.packages, plan.args), (strings(&["pkg"]), strings(&["--stdio"])));
        let plan = super::plan(Shim::Npm, &strings(&["x", "-y", "pkg", "serve", "two words"])).unwrap();
        assert_eq!((plan.packages, plan.args), (strings(&["pkg"]), strings(&["serve", "two words"])));
        // Without `--`, npm itself would take an option meant for the command, so it is refused.
        assert!(super::plan(Shim::Npm, &strings(&["exec", "pkg", "--stdio"])).unwrap_err().contains("--"));
        for args in [vec!["run", "start"], vec!["start"], vec!["install", "pkg"], vec![], vec!["exec"], vec!["exec", "--"]] {
            assert!(super::plan(Shim::Npm, &strings(&args)).is_err(), "{args:?}");
        }
    }

    #[test]
    fn only_registry_packages_and_known_options_are_accepted() {
        for args in [vec![], vec!["-y"], vec!["-c", "echo hi"], vec!["--call=echo"], vec!["--shell", "cmd"], vec!["--unknown", "pkg"],
            vec!["-p"], vec!["--registry=http://registry.example.com/", "pkg"], vec!["--registry", "ftp://x/", "pkg"],
            vec!["./local"], vec!["../evil"], vec!["file:../x"], vec![r"C:\pkgs\x.tgz"], vec!["git+https://github.com/a/b.git"], vec!["github:a/b"],
            vec!["https://example.com/x.tgz"], vec!["alias@npm:other"], vec!["Has Space"], vec!["pkg@1 || 2"], vec!["@scope"], vec!["@scope/"],
            vec!["-p", "a/b/c", "cmd"], vec!["--package=", "cmd"], vec!["pkg@$(x)"], vec!["%PATH%"]] {
            assert!(plan(Shim::Npx, &strings(&args)).is_err(), "{args:?}");
        }
        for spec in ["pkg", "pkg@1.2.3", "pkg@^1.2.0", "pkg@~1.2", "pkg@latest", "pkg@next", "@scope/pkg", "@scope/pkg@>=1.0.0", "a.b_c-d", "pkg@1.0.0-beta.1+build"] {
            assert!(plan(Shim::Npx, &strings(&["-y", spec])).is_ok(), "{spec}");
        }
        assert_eq!(package_name("@scope/pkg@^1.2.0"), Some("@scope/pkg"));
        assert_eq!(package_name("pkg@latest"), Some("pkg"));
        assert_eq!(package_name("pkg"), Some("pkg"));
        assert_eq!(package_name("../pkg"), None);
    }

    #[test]
    fn the_registry_host_is_what_must_be_approved() {
        assert_eq!(registry_host(None).unwrap(), "registry.npmjs.org");
        assert_eq!(registry_host(Some("https://Registry.Example.com:443/npm/")).unwrap(), "registry.example.com");
        // The proxy opens HTTPS on port 443 only.
        for bad in ["http://registry.example.com/", "https://registry.example.com:8443/", "https://user:pw@registry.example.com/", "https://", "registry.example.com"] {
            assert!(registry_host(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_command_is_chosen_like_npx_does() {
        let manifest = |bin: Value| serde_json::json!({ "name": "@scope/tool", "bin": bin });
        assert_eq!(bin(&manifest(Value::String("./dist/cli.js".into())), None).unwrap(), ("tool".into(), "./dist/cli.js".into()));
        assert_eq!(bin(&manifest(serde_json::json!({ "only": "a.js" })), None).unwrap(), ("only".into(), "a.js".into()));
        assert_eq!(bin(&manifest(serde_json::json!({ "x": "a.js", "y": "a.js" })), None).unwrap().1, "a.js");
        assert_eq!(bin(&manifest(serde_json::json!({ "tool": "t.js", "other": "o.js" })), None).unwrap(), ("tool".into(), "t.js".into()));
        assert!(bin(&manifest(serde_json::json!({ "a": "a.js", "b": "b.js" })), None).is_err());
        assert!(bin(&serde_json::json!({ "name": "tool" }), None).is_err());
        assert_eq!(bin(&manifest(serde_json::json!({ "a": "a.js", "b": "b.js" })), Some("b")).unwrap(), ("b".into(), "b.js".into()));
        assert!(bin(&manifest(serde_json::json!({ "a": "a.js" })), Some("b")).is_err());
    }

    #[test]
    fn a_command_script_stays_inside_its_package() {
        assert_eq!(entry_path("./dist/index.js"), Some(PathBuf::from("dist").join("index.js")));
        assert_eq!(entry_path("bin\\cli.js"), Some(PathBuf::from("bin").join("cli.js")));
        for bad in ["../x.js", "dist/../../x.js", "/etc/x.js", r"C:\x.js", "C:x.js", r"\\server\share\x.js", "", ".", "dist/./x.js/..", "a\0b"] {
            assert_eq!(entry_path(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn only_node_scripts_are_run() {
        assert_eq!(runner(b"#!/usr/bin/env node\nconsole.log(1)", "cli.js").unwrap(), Vec::<String>::new());
        assert_eq!(runner(b"#!/usr/bin/env node\r\n", "cli").unwrap(), Vec::<String>::new());
        assert_eq!(runner(b"#!/usr/bin/env -S node --no-warnings --enable-source-maps\n", "cli").unwrap(), strings(&["--no-warnings", "--enable-source-maps"]));
        assert_eq!(runner(b"#!/usr/local/bin/node\n", "cli").unwrap(), Vec::<String>::new());
        assert_eq!(runner(b"'use strict'\n", "index.mjs").unwrap(), Vec::<String>::new());
        for (head, name) in [(&b"#!/bin/sh\n"[..], "cli"), (b"#!/usr/bin/env python3\n", "cli.js"), (b"MZ\x90\x00", "tool.exe"), (b"echo hi\n", "cli.sh"),
            (b"#!/usr/bin/env nodejs-evil\n", "cli")] {
            assert!(runner(head, name).is_err(), "{name}");
        }
    }

    #[test]
    fn the_page_is_told_what_runs() {
        let text = describe("npx", &strings(&["-y", "@upstash/context7-mcp", "--port", "3000"])).unwrap();
        assert!(text.contains("@upstash/context7-mcp") && text.contains("registry.npmjs.org") && text.contains("--port 3000"), "{text}");
        assert!(text.contains("MiniClaw's Node") && text.contains("never npx.cmd"), "{text}");
        assert!(describe(r"C:\x\server.exe", &[]).is_none());
        assert!(describe("npm", &strings(&["run", "start"])).is_none());
    }

    #[test]
    fn npm_reads_only_this_servers_files() {
        let folder = Path::new(r"C:\Data\mcp-npm\0b8e3a1c");
        let shell = Path::new(r"C:\MiniClaw\bin\node\sh.exe");
        let plan = plan(Shim::Npx, &strings(&["--registry=https://registry.example.com/", "pkg"])).unwrap();
        let env = install_environment(folder, shell, &plan);
        let value = |name: &str| env.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone());
        assert_eq!(value("npm_config_userconfig"), Some(folder.join("npmrc").to_string_lossy().into_owned()));
        assert_eq!(value("npm_config_globalconfig"), Some(folder.join("global").join("npmrc").to_string_lossy().into_owned()));
        assert_eq!(value("npm_config_cache"), Some(folder.join("cache").to_string_lossy().into_owned()));
        assert_eq!(value("npm_config_script_shell"), Some(shell.to_string_lossy().into_owned()));
        assert_eq!(value("npm_config_registry"), Some("https://registry.example.com/".into()));
        assert_eq!(value("npm_config_update_notifier"), Some("false".into()));
        assert_eq!(value("npm_config_foreground_scripts"), Some("true".into()));
        // A dead registry fails within seconds, not after npm's default minutes of retries.
        assert_eq!(value("npm_config_fetch_retries"), Some("1".into()));
        let key = install_key(&plan);
        assert_ne!(key, install_key(&super::plan(Shim::Npx, &strings(&["pkg"])).unwrap()), "another registry installs again");
        assert_ne!(key, install_key(&super::plan(Shim::Npx, &strings(&["--registry=https://registry.example.com/", "pkg@2"])).unwrap()));
        assert_eq!(key, install_key(&super::plan(Shim::Npx, &strings(&["--registry=https://registry.example.com/", "pkg", "--other-arg"])).unwrap()),
            "the command's own arguments do not reinstall");
        let args = install_arguments(Path::new(r"C:\npm\bin\npm-cli.js"), &plan);
        assert_eq!(args[..2], strings(&[r"C:\npm\bin\npm-cli.js", "install"]));
        assert_eq!(args.last().unwrap(), "pkg");
        assert!(!args.contains(&"--ignore-scripts".to_owned()));
        let quiet = super::plan(Shim::Npx, &strings(&["--ignore-scripts", "pkg"])).unwrap();
        assert!(install_arguments(Path::new("npm-cli.js"), &quiet).contains(&"--ignore-scripts".to_owned()));
    }
}
