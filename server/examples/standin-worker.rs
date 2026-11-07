// Test stand-in for a VM worker: the same authenticated worker protocol, dialled over a
// Unix socket, so acceptance tests need no virtual machine. Not part of any package.
// Reuses strict worker authentication and Codex policy.
#[cfg(unix)]
use miniclaw_server::{guest_computer, remote::Settings, runtime, worker};
// Checks private settings, socket ownership, and host identity.
#[cfg(unix)]
use std::{error::Error, fs::{self, OpenOptions}, io::Read,
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf}, time::Duration};
// Dials one private relay without opening listeners.
#[cfg(unix)]
use tokio::net::UnixStream;

/// Runs a restricted Codex App Server under the Mac user's existing session.
#[cfg(unix)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let model = std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", "hw.model"]).output()?;
    if rustix::process::getuid().is_root()
        || !cfg!(all(target_os = "macos", target_arch = "aarch64"))
        || !model.status.success()
        || String::from_utf8_lossy(&model.stdout).trim().starts_with("VirtualMac")
    {
        return Err("Mac worker must run as a non-root user on the physical Mac".into());
    }
    let mut args = std::env::args_os().skip(1);
    let settings = PathBuf::from(args.next().ok_or("Missing Mac worker settings")?);
    let socket = PathBuf::from(args.next().ok_or("Missing private relay socket")?);
    let control_socket = PathBuf::from(args.next().ok_or("Missing VM control socket")?);
    let control_key_path = PathBuf::from(args.next().ok_or("Missing VM control key")?);
    if args.next().is_some() { return Err("Unexpected Mac worker argument".into()); }
    let settings = Settings::read(&settings)?;
    let cwd = Path::new(&settings.cwd);
    if cwd.canonicalize()? != cwd || !cwd.is_dir() {
        return Err("Mac workspace must be a real directory".into());
    }
    private_socket(&socket)?;
    private_socket(&control_socket)?;
    let control_key = private_key(&control_key_path)?;
    let binary = std::env::var("CODEX_BIN")
        .unwrap_or_else(|_| "/Applications/Codex.app/Contents/Resources/codex".into());
    let work = async {
        loop {
            let command = runtime::worker_command(&binary, &settings.cwd).await
                .map_err(|_| "Mac Codex configuration failed validation")?;
            if let Ok(stream) = UnixStream::connect(&socket).await {
                let policy = guest_computer::Policy::new(None);
                let control = worker::Control { path: control_socket.clone(), key: control_key };
                let _ = worker::serve_with_control(stream, settings.key, || command,
                    Some(policy), Some(control)).await;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        #[allow(unreachable_code)]
        Ok::<(), Box<dyn Error>>(())
    };
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        result = work => result,
        _ = term.recv() => Ok(()),
        _ = tokio::signal::ctrl_c() => Ok(()),
    }
}

/// Reads one owned private control key without following a symlink.
#[cfg(unix)]
fn private_key(path: &Path) -> Result<[u8; 32], Box<dyn Error>> {
    if !path.is_absolute() { return Err("VM control key path must be absolute".into()); }
    let mut file = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != rustix::process::getuid().as_raw()
        || meta.permissions().mode() & 0o077 != 0 || meta.nlink() != 1 || meta.len() != 64 {
        return Err("VM control key must be private and owned by this Mac user".into());
    }
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    if text.len() != 64 { return Err("Invalid VM control key".into()); }
    let mut key = [0; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)?;
    }
    Ok(key)
}

/// Rejects public, linked, or foreign relay endpoints before dialing them.
#[cfg(unix)]
fn private_socket(path: &Path) -> Result<(), Box<dyn Error>> {
    if !path.is_absolute() { return Err("Relay socket must be absolute".into()); }
    let parent = fs::symlink_metadata(path.parent().ok_or("Missing relay parent")?)?;
    let socket = fs::symlink_metadata(path)?;
    let uid = rustix::process::getuid().as_raw();
    if !parent.is_dir() || parent.uid() != uid || parent.permissions().mode() & 0o077 != 0
        || !socket.file_type().is_socket() || socket.uid() != uid
        || socket.permissions().mode() & 0o077 != 0
    {
        return Err("Relay socket must be private and owned by this Mac user".into());
    }
    Ok(())
}

/// The VM-only test fixture is intentionally unavailable on Windows.
#[cfg(windows)]
fn main() {
    eprintln!("The VM worker fixture is not supported on Windows.");
    std::process::exit(1);
}
