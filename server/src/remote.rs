// Dials authenticated VM workers through private sockets.
use crate::{
    rpc::{Client, Failure},
    worker,
};
// Decodes private installation settings without extensions.
use serde::Deserialize;
// Reads service-owned files without following links.
use std::{
    fs::OpenOptions,
    io::{self, Read},
    path::{Component, Path},
};

// Validates Unix VM control file ownership.
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

/// Where one VM worker is reached: its private Unix socket on the Mac.
#[derive(Clone)]
pub enum Route {
    Socket(std::path::PathBuf),
}

/// Paths of the private VM link files a VM session leaves for the Mac backend.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Links {
    pub control_socket: std::path::PathBuf,
    pub control_key: std::path::PathBuf,
    pub worker: std::path::PathBuf,
    pub master: std::path::PathBuf,
    pub dedicated: std::path::PathBuf,
}

impl Links {
    /// Reads the private link file; every path in it must be absolute.
    pub fn read(path: &Path) -> io::Result<Self> {
        let links: Self = serde_json::from_slice(&private_bytes(path, 4096)?)?;
        let paths = [&links.control_socket, &links.control_key, &links.worker,
            &links.master, &links.dedicated];
        if paths.iter().any(|path| !path.is_absolute()) {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(links)
    }

    /// Reads the supervisor's private 32-byte control key.
    pub fn control(&self) -> io::Result<worker::Control> {
        let text = private_bytes(&self.control_key, 64)?;
        let mut key = [0; 32];
        if text.len() != 64 { return Err(io::ErrorKind::InvalidData.into()); }
        for (index, byte) in key.iter_mut().enumerate() {
            let pair = std::str::from_utf8(&text[index * 2..index * 2 + 2])
                .map_err(|_| io::ErrorKind::InvalidData)?;
            *byte = u8::from_str_radix(pair, 16).map_err(|_| io::ErrorKind::InvalidData)?;
        }
        Ok(worker::Control { path: self.control_socket.clone(), key })
    }
}

/// Reads a bounded, private, unlinked file owned by this user without following links.
#[cfg(unix)]
fn private_bytes(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let file = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path)?;
    let info = file.metadata()?;
    if !info.is_file() || info.mode() & 0o077 != 0 || info.nlink() != 1
        || info.uid() != rustix::process::getuid().as_raw() || info.len() > limit {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[derive(Clone)]
pub struct Settings {
    pub key: [u8; 32],
    pub cwd: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    key: String,
    cwd: String,
}

impl Settings {
    /// Reads a bounded private settings file, rejecting links and invalid paths or keys.
    #[cfg(unix)]
    pub fn read(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        let info = file.metadata()?;
        if !info.is_file()
            || info.mode() & 0o077 != 0
            || info.nlink() != 1
            || info.uid() != rustix::process::getuid().as_raw()
            || info.len() > 4096
        {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let mut bytes = Vec::new();
        file.take(4097).read_to_end(&mut bytes)?;
        let stored: Stored = serde_json::from_slice(&bytes)?;
        let path = Path::new(&stored.cwd);
        if stored.key.len() != 64
            || !stored.key.bytes().all(|b| b.is_ascii_hexdigit())
            || !path.is_absolute()
            || path.components().any(|c| matches!(c, Component::ParentDir))
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let mut key = [0; 32];
        for (index, byte) in key.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&stored.key[index * 2..index * 2 + 2], 16)
                .map_err(|_| io::ErrorKind::InvalidData)?;
        }
        Ok(Self {
            key,
            cwd: stored.cwd,
        })
    }

    /// Connects to one VM worker through its private Unix socket on the Mac. A missing
    /// socket means the VM is off; the caller decides when to try again.
    #[cfg(unix)]
    pub async fn connect_unix(&self, path: &Path) -> Result<Client, Failure> {
        let stream = tokio::net::UnixStream::connect(path).await
            .map_err(|_| Failure::Unavailable)?;
        worker::connect(stream, self.key).await
    }

    /// Windows hosts deliberately exclude virtual machine workers.
    #[cfg(windows)]
    pub fn read(_path: &Path) -> io::Result<Self> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "VMs are not supported on Windows"))
    }

    /// Refuses unsupported VM routes before opening any transport.
    #[cfg(windows)]
    pub async fn connect_unix(&self, _path: &Path) -> Result<Client, Failure> {
        Err(Failure::Rejected)
    }

    /// Connects to one worker route without trying a different machine.
    pub async fn connect_route(&self, route: &Route) -> Result<Client, Failure> {
        match route { Route::Socket(path) => self.connect_unix(path).await }
    }
}

/// Rejects VM link configuration on Windows without reading credential files.
#[cfg(windows)]
fn private_bytes(_path: &Path, _limit: u64) -> io::Result<Vec<u8>> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "VMs are not supported on Windows"))
}
