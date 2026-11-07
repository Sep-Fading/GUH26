//! Reads what `claude auth login` prints: its sign-in page, the code Claude's page shows, and
//! its errors. On Windows MiniClaw's own executable is the CLI's browser opener (see `opener`).

/// Hosts the CLI's Claude subscription and Console sign-in pages are served from.
const HOSTS: [&str; 4] = ["claude.com", "claude.ai", "platform.claude.com", "console.anthropic.com"];

/// Whether the login CLI's opener may open a URL: Claude's HTTPS sign-in page (with the CLI's
/// localhost callback inside it) or the CLI's own callback page, `http://localhost:<port>/callback`
/// or the same on 127.0.0.1.
pub fn opener_url(url: &str) -> bool {
    if sign_in_page(url) { return true; }
    if url.len() > 4096 || url.chars().any(|c| c.is_whitespace() || c.is_control()
        || matches!(c, '"' | '\'' | '<' | '>' | '\\' | '^' | '`' | '{' | '|' | '}')) {
        return false;
    }
    let Ok(uri) = url.parse::<axum::http::Uri>() else { return false };
    let Some(authority) = uri.authority() else { return false };
    uri.scheme_str() == Some("http") && !authority.as_str().contains('@')
        && matches!(authority.host(), "localhost" | "127.0.0.1")
        && authority.port_u16().is_some_and(|port| port > 0) && uri.path() == "/callback"
}

/// The variable MiniClaw sets for `claude auth login` on Windows; the CLI passes it on to the
/// opener it runs, which is how `miniclaw-server.exe <url>` knows it is that opener.
pub const OPENER: &str = "MINICLAW_SIGN_IN_OPENER";

/// The second opener step, run outside the CLI's job: `miniclaw-server.exe --open-sign-in-url <url>`.
pub const OPEN_FLAG: &str = "--open-sign-in-url";

/// Handles the two opener invocations of the backend executable and returns its exit code, or
/// None for any other command line. The CLI runs `<exe> <url>` (its BROWSER convention) inside
/// its kill-on-close job; that step starts `<exe> --open-sign-in-url <url>` with
/// CREATE_BREAKAWAY_FROM_JOB, which the sign-in job allows, so the browser the shell starts is
/// not killed with the CLI. If breakaway is refused the page is opened from inside the job.
#[cfg(windows)]
pub fn opener(args: &[std::ffi::OsString]) -> Option<i32> {
    let text = |value: &std::ffi::OsString| value.to_str().map(str::to_owned);
    let open_here = |url: &str| if open(url).is_ok() { 0 } else { 1 };
    match args {
        [flag, url] if flag == OPEN_FLAG => {
            let url = text(url).filter(|url| opener_url(url))?;
            Some(open_here(&url))
        }
        [url] if std::env::var_os(OPENER).is_some_and(|mark| mark == "1") => {
            let url = text(url).filter(|url| opener_url(url))?;
            use std::os::windows::process::CommandExt;
            use windows_sys::Win32::System::Threading::{CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW};
            let started = std::env::current_exe().and_then(|exe| std::process::Command::new(exe)
                .args([OPEN_FLAG, &url]).env_remove(OPENER)
                .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
                .creation_flags(CREATE_BREAKAWAY_FROM_JOB | CREATE_NO_WINDOW).spawn());
            Some(match started {
                Ok(mut child) => child.wait().ok().and_then(|status| status.code()).unwrap_or(1),
                Err(_) => open_here(&url),
            })
        }
        _ => None,
    }
}

/// Removes terminal escape sequences and control characters from one output line.
pub fn plain(line: &str) -> String {
    let mut text = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.next() {
                // A control sequence ends at its final byte, `@` through `~`.
                Some('[') => { for c in chars.by_ref() { if ('@'..='~').contains(&c) { break; } } }
                // An operating-system command (a hyperlink is one) ends at BEL or ESC \.
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' { break; }
                        if c == '\x1b' && chars.peek() == Some(&'\\') { chars.next(); break; }
                    }
                }
                _ => {}
            }
        } else if !c.is_control() || c == '\t' {
            text.push(c);
        }
    }
    text
}

/// Returns the sign-in page one CLI output line names, or None when the line has none.
pub fn sign_in_url(line: &str) -> Option<String> {
    let text = plain(line);
    let url = text[text.find("https://")?..].split_whitespace().next()?;
    sign_in_page(url).then(|| url.to_owned())
}

/// Whether a URL is an HTTPS page on one of Claude's sign-in hosts, with nothing that could
/// make the Windows shell treat it as anything but a web address.
pub fn sign_in_page(url: &str) -> bool {
    if url.len() > 4096 || url.chars().any(|c| c.is_whitespace() || c.is_control()
        || matches!(c, '"' | '\'' | '<' | '>' | '\\' | '^' | '`' | '{' | '|' | '}')) {
        return false;
    }
    let Ok(uri) = url.parse::<axum::http::Uri>() else { return false };
    let Some(authority) = uri.authority() else { return false };
    uri.scheme_str() == Some("https") && !authority.as_str().contains('@') && authority.port().is_none()
        && HOSTS.iter().any(|host| authority.host().eq_ignore_ascii_case(host))
        && uri.path().len() > 1
}

/// Returns one CLI message line in a form safe to show: plain text, URLs without their query,
/// at most 300 characters. None for a blank line.
pub fn message(line: &str) -> Option<String> {
    let mut rest = plain(line);
    let mut text = String::new();
    // Each web address keeps its scheme, host and path; its query and fragment are dropped.
    while let Some(at) = rest.find("http://").into_iter().chain(rest.find("https://")).min() {
        text.push_str(&rest[..at]);
        let end = rest[at..].find(char::is_whitespace).map_or(rest.len(), |end| at + end);
        text.push_str(rest[at..end].split(['?', '#']).next().unwrap_or_default());
        rest = rest[end..].to_owned();
    }
    text.push_str(&rest);
    let text = text.trim();
    (!text.is_empty()).then(|| text.chars().take(300).collect())
}

/// Whether text is the `code#state` value Claude's code page shows after a sign-in.
pub fn code(text: &str) -> bool {
    let part = |value: &str| !value.is_empty()
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~'));
    text.len() <= 2048 && text.split_once('#').is_some_and(|(code, state)| part(code) && part(state))
}

/// Opens Claude's sign-in page (or the CLI's callback page) in the default browser through the
/// Windows shell (SW_SHOWNORMAL). Tests set
/// MINICLAW_TEST_BROWSER_LOG to record the page instead of opening a browser.
#[cfg(windows)]
pub fn open(url: &str) -> std::io::Result<()> {
    use windows_sys::Win32::{System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE},
        UI::Shell::{SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW}};
    if !opener_url(url) { return Err(std::io::ErrorKind::InvalidInput.into()); }
    if let Some(log) = std::env::var_os("MINICLAW_TEST_BROWSER_LOG") {
        use std::io::Write;
        return std::fs::OpenOptions::new().create(true).append(true).open(log)?.write_all(format!("{url}\n").as_bytes());
    }
    let wide = |value: &str| value.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (verb, file) = (wide("open"), wide(url));
    // ShellExecute may hand the address to COM shell extensions; Microsoft asks callers to
    // initialize COM as a single-threaded apartment first.
    let com = unsafe { CoInitializeEx(std::ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32) };
    // NOASYNC: this short-lived thread waits until the shell has handed the page over;
    // FLAG_NO_UI: a failure is reported to the page, not in a dialog on the desktop.
    let mut info = SHELLEXECUTEINFOW { cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI, lpVerb: verb.as_ptr(), lpFile: file.as_ptr(),
        nShow: 1, ..Default::default() };
    let opened = unsafe { ShellExecuteExW(&mut info) } != 0;
    let error = std::io::Error::last_os_error();
    if com >= 0 { unsafe { CoUninitialize(); } }
    if opened { Ok(()) } else { Err(error) }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shaped like `claude auth login --claudeai` 2.1.289 on Windows and 2.1.274 on macOS
    // (work/windows-vm/claude-login-exp-vm1.log); every value here is made up.
    const PAGE: &str = "https://claude.com/cai/oauth/authorize?code=true&client_id=00000000-0000-4000-8000-000000000000\
        &response_type=code&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback\
        &scope=user%3Aprofile+user%3Ainference&code_challenge=fake-challenge&code_challenge_method=S256&state=fake-state";

    #[test]
    fn the_sign_in_page_is_read_from_cli_output() {
        assert_eq!(sign_in_url("Opening browser to sign in\u{2026}"), None);
        assert_eq!(sign_in_url(&format!("If the browser didn't open, visit: {PAGE}")).as_deref(), Some(PAGE));
        assert_eq!(sign_in_url(&format!("If the browser didn't open, visit: {PAGE}\r")).as_deref(), Some(PAGE));
        // A terminal hyperlink (OSC 8) and colour around the address.
        let linked = format!("If the browser didn't open, visit: \x1b]8;;{PAGE}\x07{PAGE}\x1b]8;;\x07");
        assert_eq!(sign_in_url(&linked).as_deref(), Some(PAGE));
        let st = format!("visit: \x1b]8;;{PAGE}\x1b\\\x1b[36m{PAGE}\x1b[39m\x1b]8;;\x1b\\");
        assert_eq!(sign_in_url(&st).as_deref(), Some(PAGE));
        assert_eq!(sign_in_url("Paste code here if prompted > "), None);
        let console = "visit: https://platform.claude.com/oauth/authorize?code=true&state=s";
        assert_eq!(sign_in_url(console).as_deref(), Some("https://platform.claude.com/oauth/authorize?code=true&state=s"));
    }

    #[test]
    fn only_https_pages_on_claude_hosts_are_accepted() {
        assert!(sign_in_page(PAGE));
        assert!(sign_in_page("https://claude.ai/oauth/authorize?x=1"));
        assert!(sign_in_page("https://CLAUDE.COM/cai/oauth/authorize"));
        for bad in ["http://claude.com/cai/oauth/authorize", "https://claude.com.example.test/x",
            "https://example.test/?u=https://claude.com/", "https://evil.claude.com/x",
            "https://claude.com@example.test/x", "https://user@claude.com/x", "https://claude.com:8443/x",
            "https://claude.com", "file:///C:/Windows/System32/calc.exe", "C:\\Windows\\System32\\calc.exe",
            "https://claude.com/x\"y", "https://claude.com/x y", "https://claude.com/x\\y", "https://claude.com/x^y",
            "", "https://"] {
            assert!(!sign_in_page(bad), "{bad}");
        }
        assert!(!sign_in_page(&format!("https://claude.com/{}", "a".repeat(8192))));
    }

    #[test]
    fn messages_are_plain_and_drop_url_queries() {
        assert_eq!(message("   \r"), None);
        assert_eq!(message("Login failed: Token exchange failed (400): Bad Request\r").as_deref(),
            Some("Login failed: Token exchange failed (400): Bad Request"));
        assert_eq!(message(&format!("\x1b[31mvisit {PAGE}\x1b[0m")).as_deref(),
            Some("visit https://claude.com/cai/oauth/authorize"));
        assert_eq!(message(&format!("visit:{PAGE} and http://localhost:1/callback?code=x#y now")).as_deref(),
            Some("visit:https://claude.com/cai/oauth/authorize and http://localhost:1/callback now"));
        let long = message(&"é".repeat(400)).unwrap();
        assert_eq!(long.chars().count(), 300);
    }

    #[test]
    fn a_pasted_code_must_be_code_hash_state() {
        assert!(code("AbC-12_x.y~z#St4te-_x"));
        for bad in ["", "#", "abc", "abc#", "#abc", "a#b#c", "a b#c", "a#b\n", "a#b\"", "a;rm#b"] {
            assert!(!code(bad), "{bad:?}");
        }
        assert!(!code(&format!("{}#s", "a".repeat(4096))));
    }

    #[test]
    fn the_cli_opener_accepts_only_sign_in_pages_and_its_own_callback() {
        let automatic = PAGE.replace("https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback", "http%3A%2F%2Flocalhost%3A54545%2Fcallback");
        assert!(opener_url(&automatic));
        assert!(opener_url(PAGE));
        assert!(opener_url("http://localhost:54545/callback?code=abc&state=def"));
        assert!(opener_url("http://127.0.0.1:1/callback"));
        for bad in ["http://localhost/callback", "http://localhost:0/callback", "http://localhost:65536/callback",
            "http://localhost:54545/other", "http://localhost.example.test:54545/callback", "http://example.test:54545/callback",
            "http://user@localhost:54545/callback", "https://localhost:54545/callback", "http://[::1]:54545/callback",
            "http://localhost:54545/callback?x=\"y", "http://localhost:54545/callback?x=a b", "http://localhost:54545/callback?x=a^b",
            "file:///C:/Windows/System32/calc.exe", "C:\\Windows\\System32\\calc.exe", "--open-sign-in-url", "",
            "http://claude.com/cai/oauth/authorize"] {
            assert!(!opener_url(bad), "{bad}");
        }
        assert!(!opener_url(&format!("http://localhost:1/callback?{}", "a".repeat(8192))));
    }
}
