//! Secrets of MCP servers: a remote server's token and a local server's environment values.
//! They live in macOS Keychain or Windows Credential Manager, outside MiniClaw data. On Linux
//! they live in a private file in MiniClaw's data folder, which the sandbox never opens; Codex
//! keeps its own sign-in on Linux the same way, in `~/.codex/auth.json`.
//! A secret is written, replaced or removed; it is read only at the moment a server is
//! started or called, and it is never returned to the page or given to a provider.

// Stores and reads generic password items.
#[cfg(target_os = "macos")]
use security_framework::passwords::{delete_generic_password, get_generic_password, set_generic_password};
// Holds the test stand-in's names and values.
use std::collections::BTreeMap;
// Locates the test stand-in's private file.
use std::path::PathBuf;

/// The Keychain service every MiniClaw secret is saved under.
#[cfg(target_os = "macos")]
const SERVICE: &str = "MiniClaw MCP server";
/// Apple's status for an item that does not exist.
#[cfg(target_os = "macos")]
const NOT_FOUND: i32 = -25300;

#[derive(Clone, Debug)]
pub enum Secrets {
    /// The user's login Keychain.
    Keychain,
    /// A private file, used by tests so they never touch the real Keychain.
    File(PathBuf),
}

#[derive(Debug)]
pub struct Unavailable;

impl Secrets {
    /// Uses the Keychain unless a test names a stand-in file; on Linux, the private file.
    pub fn from_env() -> Self {
        match std::env::var_os("MINICLAW_TEST_SECRETS") {
            Some(path) if !path.is_empty() => Self::File(path.into()),
            _ => Self::native(&crate::platform_fs::data_directory()),
        }
    }

    /// The store of this platform for a backend whose data folder is `data`.
    pub fn native(data: &std::path::Path) -> Self {
        if cfg!(target_os = "linux") { Self::File(data.join("mcp-secrets.json")) } else { let _ = data; Self::Keychain }
    }

    /// Saves or replaces one secret.
    pub fn set(&self, key: &str, value: &str) -> Result<(), Unavailable> {
        match self {
            Self::Keychain => native_set(key, value),
            Self::File(path) => {
                let mut all = read(path)?;
                all.insert(key.to_owned(), value.to_owned());
                write(path, &all)
            }
        }
    }

    /// Reads one secret; `None` when it was never saved.
    pub fn get(&self, key: &str) -> Result<Option<String>, Unavailable> {
        match self {
            Self::Keychain => native_get(key),
            Self::File(path) => Ok(read(path)?.remove(key)),
        }
    }

    /// Removes one secret; removing one that does not exist succeeds.
    pub fn remove(&self, key: &str) -> Result<(), Unavailable> {
        match self {
            Self::Keychain => native_remove(key),
            Self::File(path) => {
                let mut all = read(path)?;
                all.remove(key);
                write(path, &all)
            }
        }
    }
}

/// Reads the stand-in file; a missing file is an empty store.
fn read(path: &PathBuf) -> Result<BTreeMap<String, String>, Unavailable> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| Unavailable),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(_) => Err(Unavailable),
    }
}

/// Writes the stand-in file for its owner only.
fn write(path: &PathBuf, all: &BTreeMap<String, String>) -> Result<(), Unavailable> {
    use std::io::Write;
    let mut file = crate::platform_fs::private_file(path, false).map_err(|_| Unavailable)?;
    file.write_all(&serde_json::to_vec(all).map_err(|_| Unavailable)?).map_err(|_| Unavailable)
}

/// The Keychain account name of a server's token.
pub fn token_key(server: &str) -> String { format!("{server}:token") }

/// The Keychain account name of one extra HTTP header of a remote server.
pub fn header_key(server: &str, name: &str) -> String { format!("{server}:header:{}", name.to_ascii_lowercase()) }

/// The Keychain account name of one environment value of a server.
pub fn env_key(server: &str, name: &str) -> String { format!("{server}:env:{name}") }

/// Writes one secret through the platform credential service.
#[cfg(target_os = "macos")]
fn native_set(key: &str, value: &str) -> Result<(), Unavailable> {
    set_generic_password(SERVICE, key, value.as_bytes()).map_err(|_| Unavailable)
}

/// Reads one secret, treating a missing item as absent.
#[cfg(target_os = "macos")]
fn native_get(key: &str) -> Result<Option<String>, Unavailable> {
    match get_generic_password(SERVICE, key) {
        Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|_| Unavailable),
        Err(error) if error.code() == NOT_FOUND => Ok(None),
        Err(_) => Err(Unavailable),
    }
}

/// Removes a credential without revealing native failure details.
#[cfg(target_os = "macos")]
fn native_remove(key: &str) -> Result<(), Unavailable> {
    match delete_generic_password(SERVICE, key) {
        Err(error) if error.code() != NOT_FOUND => Err(Unavailable),
        _ => Ok(()),
    }
}

#[cfg(windows)]
#[path = "secrets_windows.rs"]
mod windows;
#[cfg(windows)]
use windows::{native_get, native_remove, native_set};

/// Refuses production credential storage on unsupported platforms.
#[cfg(not(any(windows, target_os = "macos")))]
fn native_set(_: &str, _: &str) -> Result<(), Unavailable> { Err(Unavailable) }
/// Refuses production credential reads on unsupported platforms.
#[cfg(not(any(windows, target_os = "macos")))]
fn native_get(_: &str) -> Result<Option<String>, Unavailable> { Err(Unavailable) }
/// Refuses production credential deletion on unsupported platforms.
#[cfg(not(any(windows, target_os = "macos")))]
fn native_remove(_: &str) -> Result<(), Unavailable> { Err(Unavailable) }

#[cfg(test)]
mod linux_tests {
    use super::*;

    /// Linux keeps secrets in the data folder, never in a place the sandbox can open.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_secrets_live_in_the_protected_data_folder() {
        let data = std::path::Path::new("/home/a/.local/share/miniclaw");
        assert!(matches!(Secrets::native(data), Secrets::File(path) if path == data.join("mcp-secrets.json")));
        let roots = crate::sandbox_linux::protected_roots("/home/a", |_| None);
        assert!(crate::seatbelt::is_protected("/home/a/.local/share/miniclaw/mcp-secrets.json", &roots));
    }
}
