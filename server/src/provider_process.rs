//! Owns provider subprocesses without changing their authenticated user environment.
// Shares Tokio command construction across platforms.
use tokio::process::Command;
// Reports process startup and termination failures.
use std::io;

#[cfg(unix)]
pub type Child = tokio::process::Child;

/// Starts the existing Unix provider command unchanged.
#[cfg(unix)]
pub fn spawn(command: &mut Command) -> io::Result<Child> { command.spawn() }

/// Starts the Unix provider sign-in command unchanged; its own stdio settings decide what is captured.
#[cfg(unix)]
pub fn spawn_sign_in(command: &mut Command) -> io::Result<Child> { command.spawn() }

/// Captures a Unix provider status command unchanged.
#[cfg(unix)]
pub async fn output(command: &mut Command) -> io::Result<std::process::Output> { command.output().await }

/// Owns the Windows provider's contained process and asynchronous stdio handles.
#[cfg(windows)]
pub struct Child {
    pub stdin: Option<tokio::fs::File>,
    pub stdout: Option<tokio::fs::File>,
    // Only `spawn_sign_in` hands the error stream to the caller; `spawn` discards it.
    pub stderr: Option<tokio::fs::File>,
    process: crate::windows_process::Process,
}

#[cfg(windows)]
impl Child {
    /// Polls a background provider sign-in without blocking its status endpoint.
    pub fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        use std::os::windows::process::ExitStatusExt;
        self.process.try_wait().map(|status| status.map(std::process::ExitStatus::from_raw))
    }

    /// Terminates the provider and every child it started before acknowledging shutdown.
    pub async fn kill(&mut self) -> io::Result<()> {
        self.process.kill()?;
        self.process.wait().await.map(|_| ())
    }

    /// Waits for the native provider and returns its full Windows exit status.
    pub async fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        use std::os::windows::process::ExitStatusExt;
        self.process.wait().await.map(std::process::ExitStatus::from_raw)
    }
}

/// Starts a Windows provider suspended, assigns its job, and restores deliberate CLI overrides.
/// Its error output is read and discarded.
#[cfg(windows)]
pub fn spawn(command: &mut Command) -> io::Result<Child> { start(command, false) }

/// Starts a provider's sign-in like `spawn`, hands its error output to the caller, who must read
/// it, and lets the browser opener it runs start the browser outside its job (explicit breakaway).
#[cfg(windows)]
pub fn spawn_sign_in(command: &mut Command) -> io::Result<Child> { start(command, true) }

/// Starts one Windows provider in its own kill-on-close job with piped standard streams.
#[cfg(windows)]
fn start(command: &mut Command, sign_in: bool) -> io::Result<Child> {
    let keep_errors = sign_in;
    let command = command.as_std();
    let program = std::path::Path::new(command.get_program());
    let cwd = command.get_current_dir().map(std::path::PathBuf::from)
        .map(Ok).unwrap_or_else(std::env::current_dir)?;
    let args = command.get_args().map(|value| value.to_str().map(str::to_owned)
        .ok_or(io::ErrorKind::InvalidInput.into())).collect::<io::Result<Vec<_>>>()?;
    let mut values = std::collections::BTreeMap::new();
    for name in ["CODEX_HOME", "CLAUDE_CONFIG_DIR", "HTTPS_PROXY", "HTTP_PROXY", "NO_PROXY"] {
        if let Ok(value) = std::env::var(name) { values.insert(name.to_owned(), value); }
    }
    for (name, value) in command.get_envs() {
        let name = name.to_str().ok_or(io::ErrorKind::InvalidInput)?.to_uppercase();
        match value {
            Some(value) => { values.insert(name, value.to_str().ok_or(io::ErrorKind::InvalidInput)?.to_owned()); }
            None => { values.remove(&name); }
        }
    }
    let values = values.into_iter().collect::<Vec<_>>();
    let process = if sign_in { crate::windows_process::stdio_allowing_breakaway(program, &args, &cwd, &values) }
        else { crate::windows_process::stdio(program, &args, &cwd, &values) }?;
    let mut errors = process.errors;
    let stderr = if keep_errors { Some(tokio::fs::File::from_std(errors)) } else {
        tokio::task::spawn_blocking(move || { let _ = std::io::copy(&mut errors, &mut std::io::sink()); });
        None
    };
    Ok(Child { stdin: Some(tokio::fs::File::from_std(process.input)),
        stdout: Some(tokio::fs::File::from_std(process.output)), stderr, process: process.process })
}

/// Captures a bounded Windows provider status response and suppresses diagnostic output.
#[cfg(windows)]
pub async fn output(command: &mut Command) -> io::Result<std::process::Output> {
    use tokio::io::AsyncReadExt;
    let mut child = spawn(command)?;
    drop(child.stdin.take());
    let mut output = Vec::new();
    child.stdout.take().unwrap().take(1_048_577).read_to_end(&mut output).await?;
    if output.len() > 1_048_576 { return Err(io::ErrorKind::InvalidData.into()); }
    let status = child.wait().await?;
    Ok(std::process::Output { status, stdout: output, stderr: Vec::new() })
}

/// Accepts only a native Windows image, `.exe` or `.com`, as a program the launcher may start.
/// CreateProcessW runs a `.bat` or `.cmd` file through cmd.exe, which parses the argument text
/// again by its own rules, so arguments such as a system prompt could become commands
/// (CVE-2024-24576). The name is matched exactly: Windows strips trailing dots and spaces, and
/// a colon would name an alternate stream, so neither can stand in for the extension.
pub fn native_image(program: &std::path::Path) -> io::Result<()> {
    let refused = || io::Error::new(io::ErrorKind::InvalidInput,
        "Only a native .exe or .com program can be started; a batch file is never run through cmd.exe.");
    let text = program.to_str().ok_or_else(refused)?;
    let name = text.rsplit(['\\', '/']).next().unwrap_or_default();
    let (stem, extension) = name.rsplit_once('.').ok_or_else(refused)?;
    let native = extension.eq_ignore_ascii_case("exe") || extension.eq_ignore_ascii_case("com");
    if stem.is_empty() || name.contains(':') || !native { return Err(refused()); }
    Ok(())
}

/// Reads a provider override (`CODEX_BIN`, `CLAUDE_BIN`) as the launcher will use it. An empty
/// value switches the provider off. On Windows the value must be the provider's own absolute
/// `.exe`; the error says what to point it at instead of a launcher script.
pub fn provider_override(variable: &str, value: &std::ffi::OsStr, windows: bool) -> Result<Option<String>, String> {
    if value.is_empty() { return Ok(None); }
    let text = value.to_string_lossy().into_owned();
    if !windows { return Ok(Some(text)); }
    let bytes = text.as_bytes();
    let rooted = bytes.len() > 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/')
        || text.starts_with(r"\\");
    if !rooted || native_image(std::path::Path::new(&text)).is_err() {
        return Err(format!("{variable} must be the full path of the provider's native .exe (or .com) file. \
            A .cmd or .bat launcher, such as the shim npm puts in its bin folder, is not started, because \
            cmd.exe would parse the provider's arguments again. Point {variable} at the real executable \
            inside the installed package, or unset it so MiniClaw finds the provider itself."));
    }
    Ok(Some(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsStr, path::Path};

    #[test]
    fn only_native_images_are_launchable() {
        for program in [r"C:\Program Files\Codex\codex.exe", r"C:\Tools\a.b\CLAUDE.EXE", r"C:\Tools\tool.com",
            "C:/Tools/claude.exe", r"C:\Users\Zo\u{eb}\.local\bin\claude.exe"] {
            assert!(native_image(Path::new(program)).is_ok(), "{program}");
        }
        // cmd.exe would parse the arguments of a batch file again; trailing dots and spaces
        // are stripped by Windows, so `x.cmd.` is still that batch file.
        for program in [r"C:\npm\codex.cmd", r"C:\npm\codex.CMD", r"C:\npm\run.bat", r"C:\npm\codex.cmd.",
            r"C:\npm\codex.cmd ", r"C:\npm\codex.bat. .", r"C:\npm\codex.exe.cmd", r"C:\npm\codex",
            r"C:\npm\codex.ps1", r"C:\npm\codex.exe.", r"C:\npm\codex.exe ", r"C:\npm\codex.cmd:stream.exe",
            r"C:\npm\.exe", r"C:\npm\", "", "codex.cmd"] {
            let error = native_image(Path::new(program)).expect_err(program);
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{program}");
        }
    }

    #[test]
    fn a_windows_provider_override_must_be_a_native_absolute_executable() {
        let good = r"C:\Users\S\AppData\Roaming\npm\node_modules\@openai\codex\vendor\codex.exe";
        assert_eq!(provider_override("CODEX_BIN", OsStr::new(good), true), Ok(Some(good.to_owned())));
        assert_eq!(provider_override("CLAUDE_BIN", OsStr::new(""), true), Ok(None));
        for bad in [r"C:\Users\S\AppData\Roaming\npm\codex.cmd", r"C:\Tools\claude.bat", "codex.exe", r"bin\codex.exe", r"C:\Tools\codex"] {
            let error = provider_override("CODEX_BIN", OsStr::new(bad), true).expect_err(bad);
            assert!(error.contains("CODEX_BIN") && error.contains(".exe"), "{error}");
        }
        let shim = provider_override("CLAUDE_BIN", OsStr::new(r"C:\npm\claude.cmd"), true).unwrap_err();
        assert!(shim.contains("CLAUDE_BIN") && shim.contains(".cmd"), "{shim}");
    }

    #[test]
    fn a_mac_provider_override_is_taken_as_given() {
        assert_eq!(provider_override("CODEX_BIN", OsStr::new("/Applications/Codex.app/Contents/Resources/codex"), false),
            Ok(Some("/Applications/Codex.app/Contents/Resources/codex".to_owned())));
        assert_eq!(provider_override("CODEX_BIN", OsStr::new("codex"), false), Ok(Some("codex".to_owned())));
        assert_eq!(provider_override("CODEX_BIN", OsStr::new("/tmp/fixture.mjs"), false), Ok(Some("/tmp/fixture.mjs".to_owned())));
        assert_eq!(provider_override("CLAUDE_BIN", OsStr::new(""), false), Ok(None));
    }
}
