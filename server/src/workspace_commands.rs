//! Literal workspace paths and platform-specific Git invocation without user configuration.

/// Rejects filesystem aliases before a workspace path reaches either shell or filesystem.
/// A Windows name cannot hold a control character, so all are refused there; on the Mac and
/// in a VM only NUL and a newline are, as before, so a name with a tab stays reachable.
pub fn relative_path(path: &str, windows: bool) -> bool {
    if path.is_empty() {
        return true;
    }
    path.len() <= 1024
        && !path.starts_with('/')
        && (if windows { !path.chars().any(char::is_control) } else { !path.contains(['\0', '\n']) })
        && path.split('/').all(|name| {
            !name.is_empty()
                && name != "."
                && name != ".."
                && name != ".git"
                && (!windows
                    || (!name.eq_ignore_ascii_case(".git")
                        && !name.ends_with(['.', ' '])
                        && !name
                            .chars()
                            .any(|c| matches!(c, '\\' | ':' | '"' | '<' | '>' | '|' | '?' | '*'))
                        && !device(name)))
        })
}

/// Rejects reserved Windows device stems even when they have an extension.
fn device(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|digit| {
            matches!(
                digit,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    })
}

/// Quotes literal POSIX arguments for the Mac or VM worker.
pub fn posix_argument(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Adds fixed safety options while retaining each caller argument as one literal value.
pub fn git_arguments(arguments: &[&str], hooks_path: &str) -> Vec<String> {
    let mut argv = ["--no-pager", "-c", "core.fsmonitor=false", "-c"]
        .map(str::to_owned).to_vec();
    argv.push(format!("core.hooksPath={hooks_path}"));
    argv.extend(["-c", "diff.external=", "-c", "core.pager=cat", "-c", "core.quotepath=off", "-c", "core.untrackedCache=false"].map(str::to_owned));
    argv.extend(arguments.iter().map(|argument| (*argument).to_owned()));
    argv
}

/// Uses literal POSIX quoting only for the Mac and existing VM worker protocol.
pub fn git_script(arguments: &[&str]) -> String {
    format!(
        "GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0 GIT_OPTIONAL_LOCKS=0 GIT_LITERAL_PATHSPECS=1 git {}",
        git_arguments(arguments, "/dev/null").iter().map(|argument| posix_argument(argument)).collect::<Vec<_>>().join(" ")
    )
}

/// Converts recorded absolute paths into component-bounded workspace display paths.
pub fn activity_path(root: &str, path: &str, windows: bool) -> Option<String> {
    let root = if windows {
        root.replace('\\', "/")
    } else {
        root.to_owned()
    };
    let path = if windows {
        path.replace('\\', "/")
    } else {
        path.to_owned()
    };
    let root = root.trim_end_matches('/');
    let relative = path.strip_prefix(root)?.strip_prefix('/')?;
    relative_path(relative, windows).then(|| relative.to_owned())
}
