//! Defines the pinned POSIX shell used inside Windows AppContainers.
// Reports validation and native loader errors.
use std::io;

/// Converts the checked native namespace prefix and separators for BusyBox path handling.
pub fn native_path(path: &str) -> String {
    path.strip_prefix(r"\\?\").unwrap_or(path).replace('\\', "/")
}

/// Quotes a native Windows path for the shell's literal POSIX argument syntax.
pub fn quote_path(path: &str) -> String {
    format!("'{}'", native_path(path).replace('\'', "'\"'\"'"))
}

/// Keeps a sourced command's exact text after changing to its approved directory.
pub fn command_source(text: &str, cwd: &str) -> io::Result<String> {
    validate(text)?;
    if cwd.is_empty() || cwd.contains('\0') { return Err(io::ErrorKind::InvalidInput.into()); }
    Ok(format!("cd -- {} || return $?\n{text}\n", quote_path(cwd)))
}

/// Rejects empty, NUL-containing or unbounded scripts before creating command files.
pub fn validate(text: &str) -> io::Result<()> {
    if text.trim().is_empty() || text.contains('\0') || text.len() > 512 * 1024 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(())
}

/// The size and SHA-256 of the pinned BusyBox FRP-6075 build per Rust architecture name. The
/// private Node's `sh.exe`, which npm starts for package scripts, is the same file.
pub fn busybox_pin(arch: &str) -> Option<(u64, &'static str)> {
    match arch {
        "aarch64" => Some((646656, "e67f873d19d58c535cc9f0c4965ffd622e19b7bab87e3da89cb2185fb54464d7")),
        "x86_64" => Some((675840, "6e263d154d8548d1eb936f65d1d8312c80df31c45974e48d6335e4dcc0f4f34c")),
        _ => None,
    }
}

/// Defines prompt and command markers while sourcing commands in the persistent shell. An agent's
/// command is typed as `__mc <call>`; that line is replaced by the command itself, dimmed, so the
/// user watching the terminal sees what ran, as in the Mac shell. The first line of the command
/// file is the change to the approved folder and is not shown.
pub const STARTUP: &str = "PS1='\x1b]7331;prompt\x07sh> '\n__mc() {\n  local __mc_call=\"$1\"\n  [ -r \"$MC_RUN/$__mc_call.sh\" ] || { printf 'MiniClaw: nothing to run\\n'; return 1; }\n  printf '\\033[1A\\033[2K\\r\\033[2m$ %s\\033[0m\\n' \"$(tail -n +2 \"$MC_RUN/$__mc_call.sh\")\"\n  printf '\\033]7331;start;%s\\007' \"$__mc_call\"\n  . \"$MC_RUN/$__mc_call.sh\"\n  local __mc_code=$?\n  printf '\\033]7331;end;%s;%s\\007' \"$__mc_call\" \"$__mc_code\"\n  return \"$__mc_code\"\n}\n";

#[cfg(test)]
mod startup_tests {
    /// The command is shown, dimmed, before the start marker, so it is never part of the capture.
    #[test]
    fn the_command_is_shown_before_its_output_is_captured() {
        let shown = super::STARTUP.find("\\033[2m$ %s\\033[0m").expect("the command line is printed dimmed");
        let start = super::STARTUP.find("7331;start").unwrap();
        assert!(shown < start);
        assert!(super::STARTUP.contains("tail -n +2"), "the folder change on the first line is not shown");
    }
}

#[cfg(windows)]
mod native {
    // Retains checked executable and ancestor handles.
    use crate::platform_fs::{self, GuardedPath};
    // Verifies the exact official release bytes.
    use sha2::{Digest, Sha256};
    // Shares immutable pinned runtime across commands.
    use std::{io::{self, Read}, path::{Path, PathBuf}, sync::{Arc, OnceLock}};

    pub struct Runtime {
        pub program: PathBuf,
        _file: GuardedPath,
    }

    /// Opens and pins only the expected architecture's BusyBox FRP6075 executable.
    fn load(folder: &Path) -> io::Result<Runtime> {
        let (size, expected) = super::busybox_pin(std::env::consts::ARCH).ok_or(io::ErrorKind::Unsupported)?;
        let mut file = platform_fs::guarded_path(&folder.join("busybox.exe"), false, false)?;
        if file.file.metadata()?.len() != size { return Err(io::ErrorKind::InvalidData.into()); }
        let mut bytes = Vec::with_capacity(size as usize);
        file.file.read_to_end(&mut bytes)?;
        let actual: String = Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect();
        if actual != expected { return Err(io::Error::new(io::ErrorKind::InvalidData, "Shell runtime hash mismatch")); }
        Ok(Runtime { program: file.path.clone(), _file: file })
    }

    /// Holds verified runtime files until backend exit; setup changes require a restart.
    pub fn runtime() -> io::Result<Arc<Runtime>> {
        static RUNTIME: OnceLock<Result<Arc<Runtime>, String>> = OnceLock::new();
        RUNTIME.get_or_init(|| {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let parent = exe.parent().ok_or("Backend directory missing")?;
            load(&parent.join("shell")).map(Arc::new).map_err(|e| e.to_string())
        }).as_ref().cloned().map_err(|error| io::Error::other(format!("Sandbox shell unavailable; run MiniClaw setup and restart: {error}")))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        /// Rejects missing/tampered runtime and retains write/rename pins for valid bytes.
        #[test]
        fn runtime_integrity() {
            let root = std::env::temp_dir().join(format!("mc-shell-integrity-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            assert!(load(&root).is_err());
            std::fs::write(root.join("busybox.exe"), b"tampered").unwrap();
            assert!(load(&root).is_err());
            let actual = std::env::current_exe().unwrap().parent().unwrap().join("shell/busybox.exe");
            std::fs::copy(actual, root.join("busybox.exe")).unwrap();
            let runtime = load(&root).unwrap();
            assert!(std::fs::OpenOptions::new().write(true).open(&runtime.program).is_err());
            assert!(std::fs::rename(&runtime.program, root.join("renamed.exe")).is_err());
            assert!(std::fs::rename(&root, root.with_extension("moved")).is_err());
            drop(runtime);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
#[cfg(windows)]
pub use native::{Runtime, runtime};
