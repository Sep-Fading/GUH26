//! Trusted embedded Git runtime paths, separate from model filesystem grants.
// Decodes installer-owned configuration and validates paths.
use std::{io, path::{Path, PathBuf}};

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Runtime {
    pub root: PathBuf,
    pub program: String,
    pub exec_path: String,
}

/// What every sandboxed Git run gets so that it never waits for a person it cannot reach.
/// - `GIT_PAGER=cat`: the bundled Git has no pager program, and paging commands on the terminal failed with
///   "cannot spawn less" (exit 128). Git looks for a pager in `GIT_PAGER`, then `core.pager`, then `PAGER`,
///   then its built-in `less`, and runs none when the value is empty or `cat` (git(1) GIT_PAGER, git-config(1)
///   core.pager), so `cat` needs no program of that name. A command can still set its own `GIT_PAGER`, and
///   `git -P` / `--no-pager` keep working; no allowlisted inherited variable carries a pager to override.
/// - `GIT_TERMINAL_PROMPT=0`: a credential request fails at once with "terminal prompts disabled" instead of
///   trying the terminal (git(1) GIT_TERMINAL_PROMPT); `GIT_ASKPASS` and `SSH_ASKPASS` are not inherited.
/// The editor needs nothing: with no editor in the sandbox, `git commit` without `-m` already stops at once
/// with Git's own "Please supply the message using either -m or -F option" (native test
/// `git_never_waits_for_a_pager_an_editor_or_a_password`).
pub const POLICY: [(&str, &str); 2] = [("GIT_PAGER", "cat"), ("GIT_TERMINAL_PROMPT", "0")];

impl Runtime {
    /// Limits automatic read access to one generated installation beneath private application data.
    pub fn validate(&self, data: &Path) -> io::Result<()> {
        let parent = data.join("windows-git-runtime");
        let generated = self.root.file_name().and_then(|value| value.to_str())
            .is_some_and(|value| uuid::Uuid::parse_str(value).is_ok());
        if !data.is_absolute() || self.root.parent() != Some(parent.as_path()) || !generated
            || self.program.is_empty() || self.exec_path.is_empty()
            || !crate::workspace_commands::relative_path(&self.program, true)
            || !crate::workspace_commands::relative_path(&self.exec_path, true)
            || !self.program.ends_with("/bin/git.exe")
            || !self.exec_path.ends_with("/libexec/git-core") {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "Invalid embedded Git runtime paths"));
        }
        Ok(())
    }

    /// Uses the embedded helpers and configuration while preserving the ordinary OS command path, and
    /// keeps repository discovery inside the run's granted folders (`ceiling_directories`).
    #[cfg(windows)]
    pub fn environment(&self, empty_config: &Path, grants: &[crate::seatbelt::Grant]) -> io::Result<Vec<(std::ffi::OsString, std::ffi::OsString)>> {
        let program = self.root.join(&self.program);
        let bin = program.parent().ok_or(io::ErrorKind::InvalidInput)?;
        let mut paths = vec![bin.to_owned(), self.root.join("usr/bin")];
        if let Some(existing) = std::env::var_os("PATH") { paths.extend(std::env::split_paths(&existing)); }
        let mut values: Vec<(std::ffi::OsString, std::ffi::OsString)> = vec![
            ("PATH".into(), std::env::join_paths(paths).map_err(io::Error::other)?),
            ("GIT_EXEC_PATH".into(), self.root.join(&self.exec_path).into_os_string()),
            ("GIT_SSL_CAINFO".into(), bin.parent().ok_or(io::ErrorKind::InvalidInput)?.join("etc/ssl/certs/ca-bundle.crt").into_os_string()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("GIT_CONFIG_GLOBAL".into(), empty_config.as_os_str().to_owned()),
        ];
        values.extend(POLICY.iter().map(|(name, value)| ((*name).into(), (*value).into())));
        let roots = grants.iter().filter(|grant| grant.folder && !grant.blocked)
            .map(|grant| crate::platform_fs::permission_path(Path::new(&grant.path)))
            .collect::<io::Result<Vec<_>>>()?;
        if let Some(ceiling) = ceiling_directories(&roots) { values.push(("GIT_CEILING_DIRECTORIES".into(), ceiling.into())); }
        Ok(values)
    }
}

/// `GIT_CEILING_DIRECTORIES` for a sandboxed run: the parent of every granted folder that is not inside
/// another granted folder. Git's repository discovery then never looks above a grant, where the
/// sandbox may not read a folder and Git would stop with "failed to stat" (git(1), "GIT_CEILING_DIRECTORIES").
/// The leading empty entry tells Git not to resolve the entries, which the sandbox could not do either;
/// they are compared as text with Git's working directory (forward slashes, canonical case from the
/// final path), so both spellings of the drive letter are listed. `roots` are canonical plain drive
/// paths (`platform_fs::permission_path`); anything else, and a path containing Git's list separator
/// `;`, is left out.
pub fn ceiling_directories(roots: &[String]) -> Option<String> {
    let trimmed: Vec<&str> = roots.iter().filter(|root| crate::platform_fs::plain_windows_path(root))
        .map(|root| root.trim_end_matches(['\\', '/'])).collect();
    let key = |path: &str| path.replace('/', "\\").to_lowercase();
    let mut entries: Vec<String> = Vec::new();
    for root in &trimmed {
        let own = key(root);
        if trimmed.iter().any(|other| own.strip_prefix(&key(other)).is_some_and(|rest| rest.starts_with('\\'))) {
            continue;
        }
        // A drive root ("C:") has no parent; "C:\\work" has the drive root "C:/".
        let Some((parent, _)) = root.rsplit_once(['\\', '/']) else { continue };
        if parent.contains(';') { continue; }
        let parent = if parent.len() == 2 { format!("{parent}/") } else { parent.replace('\\', "/") };
        let (drive, rest) = parent.split_at(1);
        for spelling in [drive.to_ascii_uppercase(), drive.to_ascii_lowercase()] {
            let entry = format!("{spelling}{rest}");
            if !entries.contains(&entry) { entries.push(entry); }
        }
    }
    (!entries.is_empty()).then(|| format!(";{}", entries.join(";")))
}

/// Reads only the private installer record; absent optional Git leaves ordinary shells available.
#[cfg(windows)]
pub fn load() -> io::Result<Option<Runtime>> {
    use std::io::Read;
    let data = std::env::var_os("MINICLAW_DATA_DIR").map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(|value| PathBuf::from(value).join("MiniClaw")))
        .ok_or(io::ErrorKind::NotFound)?;
    let file = match crate::platform_fs::open_read_nofollow(&data.join("windows-git.json")) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !crate::platform_fs::private_regular_file(&file)? || file.metadata()?.len() > 16384 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    // The enclosing data root stays private even while separate package ACEs read the runtime.
    crate::platform_fs::private_dir(&data, true)?;
    let mut bytes = Vec::new();
    file.take(16385).read_to_end(&mut bytes)?;
    let runtime: Runtime = serde_json::from_slice(&bytes)?;
    runtime.validate(&data)?;
    Ok(Some(runtime))
}

#[cfg(test)]
mod tests {
    use super::ceiling_directories;

    fn roots(paths: &[&str]) -> Vec<String> { paths.iter().map(|path| (*path).to_owned()).collect() }

    /// Discovery stops at the parent of each outermost granted folder, written the way Git for Windows
    /// spells its working directory, after an empty entry that turns off resolving the entries.
    #[test]
    fn ceiling_is_the_parent_of_each_outermost_grant() {
        assert_eq!(ceiling_directories(&roots(&[r"C:\Users\S\work"])).as_deref(), Some(";C:/Users/S;c:/Users/S"));
        // A grant inside another grant adds nothing: the repository may sit in the outer grant.
        assert_eq!(ceiling_directories(&roots(&[r"C:\Users\S\work\sub\extra", r"C:\Users\S\work"])).as_deref(),
            Some(";C:/Users/S;c:/Users/S"));
        assert_eq!(ceiling_directories(&roots(&[r"D:\a\b", r"C:\x"])).as_deref(), Some(";D:/a;d:/a;C:/;c:/"));
        // Siblings share a parent once; a name that only shares a prefix is not nested.
        assert_eq!(ceiling_directories(&roots(&[r"C:\w\one", r"C:\w\one-more", r"C:\w\one"])).as_deref(), Some(";C:/w;c:/w"));
        assert_eq!(ceiling_directories(&roots(&[r"c:\w\One", r"C:\W\one\x"])).as_deref(), Some(";C:/w;c:/w"));
    }

    /// Git in a sandboxed run never waits for a person: paging commands print directly and a credential
    /// request fails at once. Both are environment variables the run's own command can still override.
    #[test]
    fn sandboxed_git_pages_nothing_and_asks_for_no_credentials() {
        assert_eq!(super::POLICY, [("GIT_PAGER", "cat"), ("GIT_TERMINAL_PROMPT", "0")]);
    }

    /// No usable entry means no variable: a drive root has no parent, and a path with Git's list
    /// separator cannot be written into the list.
    #[test]
    fn ceiling_skips_entries_it_cannot_express() {
        assert_eq!(ceiling_directories(&roots(&[])), None);
        assert_eq!(ceiling_directories(&roots(&[r"C:\"])), None);
        assert_eq!(ceiling_directories(&roots(&[r"C:\a;b\work"])), None);
        assert_eq!(ceiling_directories(&roots(&[r"\\server\share\work", "relative"])), None);
        assert_eq!(ceiling_directories(&roots(&[r"C:\a;b\work", r"C:\ok\work"])).as_deref(), Some(";C:/ok;c:/ok"));
    }
}
