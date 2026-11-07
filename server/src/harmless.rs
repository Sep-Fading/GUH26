//! Decides whether a shell command is harmless enough to run without asking: a command line made
//! only of programs from a fixed read-only list, joined by `;`, `&&`, `||` or `|`, with plain
//! words as arguments. Anything the list does not name, and anything the shell would expand or
//! redirect, asks as before. The list is the whole rule; there is no guessing from the words.

/// Programs that read and print and change nothing. `git`, `npm` and `cargo` are read-only only
/// with the subcommands named below.
const PROGRAMS: [&str; 30] = ["ls", "cat", "head", "tail", "wc", "grep", "rg", "find", "pwd", "echo", "printf", "which", "file",
    "stat", "du", "df", "date", "whoami", "uname", "basename", "dirname", "realpath", "tr", "sort", "uniq", "cut", "less", "more",
    "true", "false"];
/// Read-only subcommands of tools that also write.
const SUBCOMMANDS: [(&str, &[&str]); 4] = [
    ("git", &["status", "log", "diff", "show", "branch", "rev-parse", "remote", "tag", "describe", "blame", "ls-files"]),
    ("npm", &["ls", "list", "view", "outdated", "--version", "-v"]),
    ("cargo", &["--version", "-V", "metadata", "tree"]),
    ("node", &["--version", "-v"]),
];
/// Flags that make a read-only program write or run something else.
const WRITE_FLAGS: [(&str, &[&str]); 4] = [
    ("find", &["-delete", "-exec", "-execdir", "-ok", "-okdir", "-fprint", "-fprintf", "-fls"]),
    ("git", &["-c", "--exec-path", "-C"]),
    ("sort", &["-o", "--output"]),
    ("tail", &["-f", "--follow"]),
];

/// Splits a plain command line into its programs, or nothing when the shell would do more than
/// run them in turn: anything that expands, redirects, backgrounds, quotes or substitutes. A
/// rule for a program means that program run plainly, so this is the shape both the read-only
/// list and the user's own allowed programs are judged by.
pub fn programs(text: &str) -> Option<Vec<String>> {
    if text.chars().any(|c| matches!(c, '>' | '<' | '`' | '$' | '(' | ')' | '{' | '}' | '\\' | '\n' | '\r' | '\'' | '"' | '*' | '?' | '[' | '~' | '!' | '#')) {
        return None;
    }
    if text.contains('&') && !text.contains("&&") { return None; }
    let commands = text.split("&&").flat_map(|part| part.split("||")).flat_map(|part| part.split(';')).flat_map(|part| part.split('|'));
    let mut found = Vec::new();
    for command in commands {
        let program = command.split_whitespace().next()?;
        // An assignment or a path to a program is not a program by name.
        if program.contains('=') || program.contains('/') { return None; }
        if !found.iter().any(|known| known == program) { found.push(program.to_owned()); }
    }
    (!found.is_empty()).then_some(found)
}

/// Reports whether the whole command line is harmless.
pub fn harmless(text: &str) -> bool {
    if programs(text).is_none() { return false; }
    let commands = text.split("&&").flat_map(|part| part.split("||")).flat_map(|part| part.split(';')).flat_map(|part| part.split('|'));
    let mut any = false;
    for command in commands {
        let words: Vec<&str> = command.split_whitespace().collect();
        let Some((program, arguments)) = words.split_first() else { return false; };
        any = true;
        if let Some((_, subcommands)) = SUBCOMMANDS.iter().find(|(name, _)| name == program) {
            let Some(first) = arguments.iter().find(|argument| !argument.starts_with('-') || subcommands.contains(argument)) else { return false; };
            if !subcommands.contains(first) { return false; }
        } else if !PROGRAMS.contains(program) {
            return false;
        }
        if let Some((_, flags)) = WRITE_FLAGS.iter().find(|(name, _)| name == program)
            && arguments.iter().any(|argument| flags.contains(argument)) {
            return false;
        }
    }
    any
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_are_named_only_for_plain_lines() {
        assert_eq!(programs("sed -n 1p a; cat a | wc -l && sed q"), Some(vec!["sed".into(), "cat".into(), "wc".into()]));
        for line in ["cat a > b", "X=1 ls", "/bin/ls", "ls &", "cat 'a'", "", "ls; "] { assert_eq!(programs(line), None, "{line}"); }
    }

    #[test]
    fn read_only_lines_pass_and_everything_else_asks() {
        for line in ["ls -la", "cat notes.txt | head -n 1", "git status --porcelain; pwd", "grep -rn TODO src", "rg -n fn . | wc -l",
            "npm ls --depth 0", "cargo --version && node --version", "find . -name x.txt", "git log --oneline -5", "echo hi"] {
            assert!(harmless(line), "{line}");
        }
        for line in ["rm notes.txt", "echo hi > out.txt", "python3 -c 1", "cat $(echo notes.txt)", "ls; touch made", "cat a || rm -rf .",
            "sudo ls", "ls `pwd`", "git push", "git -c core.pager=x log", "find . -delete", "find . -exec rm {} ;", "sort -o out in",
            "tail -f log", "X=1 ls", "/bin/ls", "ls &", "cat 'a b'", "ls *.txt", "cat ~/.ssh/id", "npm install", "cargo build", "", "  "] {
            assert!(!harmless(line), "{line}");
        }
    }
}
