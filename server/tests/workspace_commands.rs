use miniclaw_server::workspace_commands::{git_arguments, git_script, relative_path};

#[test]
fn windows_git_preserves_literal_argv() {
    let name = "O'Brien $x; &(雪) ‘quoted’.txt";
    let arguments = git_arguments(&["diff", "--", name], "C:/private run/empty-config");
    assert_eq!(&arguments[arguments.len() - 3..], &["diff", "--", name]);
    assert!(arguments.contains(&"core.hooksPath=C:/private run/empty-config".to_owned()));
    assert!(arguments.contains(&"diff.external=".to_owned()));
    assert!(arguments.contains(&"core.fsmonitor=false".to_owned()));
    assert!(!arguments.iter().any(|value| value.contains("/dev/null") || value.contains("FromBase64String")));
}

#[test]
fn vm_git_keeps_posix_quoting_and_disables_pathspec_magic() {
    let script = git_script(&["diff", "--", "O'Brien.txt"]);
    assert!(script.contains("GIT_CONFIG_GLOBAL=/dev/null"));
    assert!(script.contains("GIT_LITERAL_PATHSPECS=1"));
    assert!(script.contains("'O'\\''Brien.txt'"));
}

#[test]
fn windows_workspace_paths_reject_root_aliases_and_reserved_names() {
    for path in [
        "../a",
        "a\\..\\b",
        "C:secret",
        "\\server\\share",
        "a:stream",
        "a/.GIT/config",
        "a/NUL.txt",
        "a./b",
        "a /b",
        "a\rb",
    ] {
        assert!(!relative_path(path, true), "{path}");
    }
    for path in ["", "folder/O'Brien.txt", "folder/雪.txt"] {
        assert!(relative_path(path, true));
    }
    assert!(relative_path("a\\b", false));
}

#[test]
fn activity_paths_use_windows_separators_and_component_boundaries() {
    use miniclaw_server::workspace_commands::activity_path;
    assert_eq!(
        activity_path(r"C:\work", r"C:\work\folder\file.txt", true).as_deref(),
        Some("folder/file.txt")
    );
    assert!(activity_path(r"C:\work", r"C:\workspace\file.txt", true).is_none());
}

/// The Mac and VM rule is the baseline one: only NUL and a newline are refused, so a file whose
/// name holds a tab or another control character can still be viewed and diffed. Windows names
/// cannot hold control characters, so every one stays refused there.
#[test]
fn mac_paths_refuse_only_nul_and_newline() {
    for path in ["notes\tdraft.txt", "folder/bell\u{7}.txt", "a\rb", "esc\u{1b}.md"] {
        assert!(relative_path(path, false), "{path:?}");
        assert!(!relative_path(path, true), "{path:?}");
    }
    for path in ["a\0b", "a\nb", "folder/\n.txt"] {
        assert!(!relative_path(path, false), "{path:?}");
    }
}
