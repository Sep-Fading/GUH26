//! Finds the Windows Computer Use helper inside the newest installed Codex app each time it is needed.
//! The app's package folder carries its version, so a saved path breaks after every Codex update;
//! setup saves the package family and the helper's file name instead.

// Joins package folders and checks helper files.
use std::path::{Path, PathBuf};

/// The helper's folders inside the Codex app package, in the order setup's detection tries them.
const FOLDERS: [&str; 2] = [r"app\resources\cua_node\bin\node_modules\@oai\cua\bin\windows",
    r"app\resources\cua_node\bin\node_modules\@oai\sky\bin\windows"];
/// The helper's file names: x64 and ARM64.
const NAMES: [&str; 2] = ["codex-computer-use.exe", "codex-computer-use-arm64.exe"];

/// Why no helper can be used now, in words the Tools menu shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unresolved {
    NotInstalled,
    Missing,
    Unsigned,
    Unchecked,
}

impl Unresolved {
    /// Returns the reason shown in the Tools menu and in refused requests.
    pub fn message(self) -> &'static str {
        match self {
            Self::NotInstalled => "The Codex app is not installed for this Windows account, so its Computer Use helper is missing. Install the Codex app; miniclaw guide computer-use shows how.",
            Self::Missing => "The installed Codex app has no Computer Use helper. In the Codex app open Plugins > Computer Use and install it; miniclaw guide computer-use shows how.",
            Self::Unsigned => "The Computer Use helper in the installed Codex app has no valid OpenAI signature, so MiniClaw does not start it.",
            Self::Unchecked => "MiniClaw could not check the Computer Use helper's signature. Try again.",
        }
    }
}

/// The helper the user agreed to at setup: a file name inside the Codex app's package family.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    family: String,
    file: String,
}

impl Installed {
    /// Accepts only the Codex app's family (`OpenAI.Codex_` plus its 13-character publisher ID)
    /// and one of the helper's two names; returns None for anything else.
    pub fn new(family: &str, file: &str) -> Option<Self> {
        let publisher = family.strip_prefix("OpenAI.Codex_")?;
        (publisher.len() == 13 && publisher.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            && NAMES.contains(&file))
            .then(|| Self { family: family.to_owned(), file: file.to_owned() })
    }

    /// Returns the package family the backend asks Windows about.
    pub fn family(&self) -> &str { &self.family }

    /// Picks the helper as setup's detection does: the newest package version first, and in it the
    /// first folder that holds the file. `packages` pairs each package full name with its folder.
    pub fn choose(&self, packages: &[(String, PathBuf)], exists: impl Fn(&Path) -> bool)
        -> Result<PathBuf, Unresolved>
    {
        if packages.is_empty() { return Err(Unresolved::NotInstalled); }
        let mut sorted: Vec<_> = packages.iter().map(|(name, folder)| (version(name), folder)).collect();
        sorted.sort_by(|left, right| right.0.cmp(&left.0));
        sorted.into_iter()
            .flat_map(|(_, folder)| FOLDERS.iter().map(move |inner| folder.join(inner).join(&self.file)))
            .find(|path| exists(path))
            .ok_or(Unresolved::Missing)
    }
}

/// Reads the version from a package full name (`Name_Version_Architecture_ResourceId_PublisherId`);
/// an unreadable one sorts last.
fn version(full_name: &str) -> [u32; 4] {
    let mut parts = [0; 4];
    if let Some(text) = full_name.split('_').nth(1) {
        for (slot, part) in parts.iter_mut().zip(text.split('.')) { *slot = part.parse().unwrap_or(0); }
    }
    parts
}

/// Reports whether a certificate subject names OpenAI as its organization, with the same
/// spellings setup accepts (scripts/setup/windows-computer.mjs).
#[cfg(any(windows, test))]
fn openai_signer(subject: &str) -> bool {
    const ACCEPTED: [&str; 7] = ["openai", "openai llc", "openai opco llc", "\"openai, inc.\"", "\"openai, llc\"",
        "\"openai, l.l.c.\"", "\"openai opco, llc\""];
    // Split at commas outside quotes, as a distinguished name is written.
    let mut parts = Vec::new();
    let (mut quoted, mut start) = (false, 0);
    for (index, letter) in subject.char_indices() {
        match letter {
            '"' => quoted = !quoted,
            ',' if !quoted => { parts.push(&subject[start..index]); start = index + 1; }
            _ => {}
        }
    }
    parts.push(&subject[start..]);
    parts.iter().any(|part| {
        let part = part.trim_start();
        part.get(..2).is_some_and(|key| key.eq_ignore_ascii_case("O="))
            && ACCEPTED.contains(&part[2..].to_lowercase().as_str())
    })
}

#[cfg(windows)]
mod native {
    use super::{Installed, Unresolved, openai_signer};
    // Reads installed package names and folders from Windows.
    use windows_sys::Win32::Storage::Packaging::Appx::{GetPackagePathByFullName, GetPackagesByPackageFamily};
    // Remembers the last verified file between uses.
    use std::{path::{Path, PathBuf}, sync::Mutex, time::SystemTime};
    // Runs the signature check without a console window.
    use std::os::windows::process::CommandExt;

    const ERROR_SUCCESS: u32 = 0;
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// The last helper whose signature passed, with its size and change time.
    static VERIFIED: Mutex<Option<(PathBuf, u64, SystemTime)>> = Mutex::new(None);

    /// Converts text to a NUL-terminated UTF-16 string for Windows.
    fn wide(text: &str) -> Vec<u16> { text.encode_utf16().chain(Some(0)).collect() }

    /// Returns each package of this family installed for the current user with its folder.
    pub fn packages(family: &str) -> Result<Vec<(String, PathBuf)>, Unresolved> {
        let family = wide(family);
        let (mut count, mut length) = (0u32, 0u32);
        // SAFETY: a first call with null buffers only reports the sizes needed.
        let status = unsafe { GetPackagesByPackageFamily(family.as_ptr(), &mut count, std::ptr::null_mut(),
            &mut length, std::ptr::null_mut()) };
        if status == ERROR_SUCCESS && count == 0 { return Ok(Vec::new()); }
        if status != ERROR_INSUFFICIENT_BUFFER { return Err(Unresolved::NotInstalled); }
        let mut names = vec![std::ptr::null_mut(); count as usize];
        let mut buffer = vec![0u16; length as usize];
        // SAFETY: both buffers have the sizes Windows asked for; the name pointers point into `buffer`.
        let status = unsafe { GetPackagesByPackageFamily(family.as_ptr(), &mut count, names.as_mut_ptr(),
            &mut length, buffer.as_mut_ptr()) };
        if status != ERROR_SUCCESS { return Err(Unresolved::NotInstalled); }
        let mut found = Vec::new();
        for name in names.into_iter().take(count as usize) {
            // SAFETY: each pointer is a NUL-terminated string inside `buffer`, which is still alive.
            let full = unsafe { let mut end = 0; while *name.add(end) != 0 { end += 1; } std::slice::from_raw_parts(name, end + 1).to_vec() };
            let mut size = 0u32;
            // SAFETY: a first call with a null buffer only reports the size needed.
            if unsafe { GetPackagePathByFullName(full.as_ptr(), &mut size, std::ptr::null_mut()) } != ERROR_INSUFFICIENT_BUFFER { continue; }
            let mut path = vec![0u16; size as usize];
            // SAFETY: `path` has the size Windows asked for.
            if unsafe { GetPackagePathByFullName(full.as_ptr(), &mut size, path.as_mut_ptr()) } != ERROR_SUCCESS { continue; }
            let path = String::from_utf16_lossy(&path[..path.iter().position(|unit| *unit == 0).unwrap_or(path.len())]);
            found.push((String::from_utf16_lossy(&full[..full.len() - 1]), PathBuf::from(path)));
        }
        Ok(found)
    }

    /// Checks the file's Authenticode signature with Windows PowerShell, as setup does; the path reaches
    /// the fixed command only through an environment variable, never as code. Returns the signer subject.
    fn signature(path: &Path) -> Result<String, Unresolved> {
        let shell = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into()))
            .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        let script = "$s = Get-AuthenticodeSignature -LiteralPath $env:MINICLAW_HELPER_CHECK; \
            if ($s.Status -eq 'Valid') { 'valid:' + $s.SignerCertificate.Subject } else { 'invalid' }";
        let output = std::process::Command::new(shell).creation_flags(CREATE_NO_WINDOW)
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
            .env("MINICLAW_HELPER_CHECK", path).stdin(std::process::Stdio::null()).output()
            .map_err(|_| Unresolved::Unchecked)?;
        if !output.status.success() { return Err(Unresolved::Unchecked); }
        let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        text.strip_prefix("valid:").map(str::to_owned).ok_or(Unresolved::Unsigned)
    }

    impl Installed {
        /// Finds the helper in the newest installed Codex app and checks its OpenAI signature, reusing the
        /// last check while the same file is unchanged. Returns the helper's path or why there is none.
        pub fn resolve(&self) -> Result<String, Unresolved> {
            let path = self.choose(&packages(self.family())?, Path::is_file)?;
            let metadata = std::fs::metadata(&path).map_err(|_| Unresolved::Missing)?;
            let stamp = (path.clone(), metadata.len(), metadata.modified().map_err(|_| Unresolved::Unchecked)?);
            if VERIFIED.lock().map(|last| last.as_ref() != Some(&stamp)).unwrap_or(true) {
                if !openai_signer(&signature(&path)?) { return Err(Unresolved::Unsigned); }
                if let Ok(mut last) = VERIFIED.lock() { *last = Some(stamp); }
            }
            path.into_os_string().into_string().map_err(|_| Unresolved::Missing)
        }
    }
}

#[cfg(not(windows))]
impl Installed {
    /// Codex app packages exist only on Windows.
    pub fn resolve(&self) -> Result<String, Unresolved> { Err(Unresolved::NotInstalled) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAMILY: &str = "OpenAI.Codex_2p2nqsd0c76g0";
    const ARM: &str = "codex-computer-use-arm64.exe";

    /// Builds a package entry the way Windows reports it.
    fn package(version: &str) -> (String, PathBuf) {
        let name = format!("OpenAI.Codex_{version}_arm64__2p2nqsd0c76g0");
        (name.clone(), PathBuf::from(format!(r"C:\Program Files\WindowsApps\{name}")))
    }

    #[test]
    fn only_the_codex_family_and_helper_names_are_accepted() {
        assert!(Installed::new(FAMILY, ARM).is_some());
        assert!(Installed::new(FAMILY, "codex-computer-use.exe").is_some());
        for (family, file) in [("OpenAI.Codex_2p2nqsd0c76g0", "cmd.exe"), ("Other.App_2p2nqsd0c76g0", ARM),
            ("OpenAI.Codex_short", ARM), ("OpenAI.Codex_2P2NQSD0C76G0", ARM), ("OpenAI.Codex_2p2nqsd0c76g0", r"..\codex-computer-use.exe")] {
            assert!(Installed::new(family, file).is_none(), "{family} {file}");
        }
    }

    #[test]
    fn the_newest_package_that_has_the_helper_wins() {
        let helper = Installed::new(FAMILY, ARM).unwrap();
        let old = package("26.930.3930.0");
        let new = package("26.1006.100.0");
        let packages = [old.clone(), new.clone()];
        let in_folder = |package: &(String, PathBuf), folder: usize| package.1.join(FOLDERS[folder]).join(ARM);
        // Both versions have it: the newer one, cua folder first.
        let everywhere = |_: &Path| true;
        assert_eq!(helper.choose(&packages, everywhere), Ok(in_folder(&new, 0)));
        // The newest has it only in the sky folder.
        let sky = in_folder(&new, 1);
        assert_eq!(helper.choose(&packages, |path| path == sky), Ok(sky.clone()));
        // The newest lacks it: the older package is used.
        let older = in_folder(&old, 0);
        assert_eq!(helper.choose(&packages, |path| path == older), Ok(older.clone()));
        assert_eq!(helper.choose(&packages, |_| false), Err(Unresolved::Missing));
        assert_eq!(helper.choose(&[], everywhere), Err(Unresolved::NotInstalled));
        // Versions compare as numbers, not text.
        let ninth = package("26.930.9.0");
        let tenth = package("26.1000.1.0");
        assert_eq!(helper.choose(&[ninth, tenth.clone()], everywhere), Ok(in_folder(&tenth, 0)));
    }

    #[test]
    fn the_signer_must_be_openai_as_in_setup() {
        for subject in [r#"CN="OpenAI OpCo, LLC", O="OpenAI OpCo, LLC", L=San Francisco, S=California, C=US"#,
            "CN=OpenAI, O=OpenAI, L=San Francisco", r#"O="OpenAI, Inc.", C=US"#, "o=openai llc"] {
            assert!(openai_signer(subject), "{subject}");
        }
        for subject in ["O=Untrusted Corp", "O=OpenAI Unrelated Ltd", "CN=OpenAI", r#"CN="x, O=OpenAI""#, ""] {
            assert!(!openai_signer(subject), "{subject}");
        }
    }

    #[test]
    fn every_reason_names_what_to_do() {
        for reason in [Unresolved::NotInstalled, Unresolved::Missing, Unresolved::Unsigned, Unresolved::Unchecked] {
            assert!(!reason.message().is_empty());
        }
        assert!(Unresolved::Missing.message().contains("miniclaw guide computer-use"));
    }
}
