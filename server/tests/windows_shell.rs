// Checks sandbox shell syntax without executing host commands.
use miniclaw_server::windows_shell::{command_source, quote_path};

#[test]
fn shell_preserves_payload_and_quotes_windows_paths() {
    assert_eq!(quote_path(r"\\?\C:\Users\雪"), "'C:/Users/雪'");
    let cwd = r"C:\Users\a'b\雪 $x`\work";
    assert_eq!(quote_path(cwd), "'C:/Users/a'\"'\"'b/雪 $x`/work'");
    let payload = "value='quoted 雪'\nprintf '%s\\n' \"$value\"\nreturn 7";
    let source = command_source(payload, cwd).unwrap();
    assert!(source.starts_with("cd -- 'C:/Users/a'\"'\"'b/雪 $x`/work' || return $?\n"));
    assert!(source.ends_with(&format!("{payload}\n")));
}

#[test]
fn shell_rejects_unrepresentable_commands() {
    for text in ["", "echo\0secret"] { assert!(command_source(text, r"C:\work").is_err()); }
    assert!(command_source("echo fine", "C:\0bad").is_err());
    assert!(command_source(&"x".repeat(512 * 1024 + 1), r"C:\work").is_err());
}
