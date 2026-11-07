//! Loads the pinned Microsoft console runtime beside the backend.
// Pins verified runtime files throughout execution.
use crate::platform_fs::{self, GuardedPath};
// Checks official runtime bytes before loading.
use sha2::{Digest, Sha256};
// Owns shared APIs and native paths.
use std::{
    io::{self, Read},
    path::Path,
    ptr::null_mut,
    sync::{Arc, OnceLock},
};
// Loads only explicitly selected native libraries.
use windows_sys::Win32::{
    Foundation::{FreeLibrary, HANDLE, HMODULE},
    System::{
        Console::{COORD, HPCON},
        LibraryLoader::*,
    },
};

type Create = unsafe extern "system" fn(COORD, HANDLE, HANDLE, u32, *mut HPCON) -> i32;
type Resize = unsafe extern "system" fn(HPCON, COORD) -> i32;
type Close = unsafe extern "system" fn(HPCON);

pub(super) struct Api {
    module: HMODULE,
    pub create: Create,
    pub resize: Resize,
    pub close: Close,
    _files: Vec<GuardedPath>,
}
// Immutable entry points remain pinned while any console uses them.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}
impl Drop for Api {
    /// Releases the module before its verified files and ancestors.
    fn drop(&mut self) {
        unsafe {
            FreeLibrary(self.module);
        }
    }
}

/// Shares one verified runtime, failing closed until setup and restart complete.
pub(super) fn runtime() -> io::Result<Arc<Api>> {
    static API: OnceLock<Result<Arc<Api>, String>> = OnceLock::new();
    API.get_or_init(|| load().map(Arc::new).map_err(|e| e.to_string()))
        .as_ref()
        .cloned()
        .map_err(|e| {
            io::Error::other(format!(
                "Microsoft ConPTY runtime unavailable; run MiniClaw setup and restart: {e}"
            ))
        })
}

/// Pins a regular runtime file and compares its full bytes with the release hash.
fn verified(path: &Path, expected: &str) -> io::Result<GuardedPath> {
    let mut guard = platform_fs::guarded_path(path, false, false)?;
    if guard.file.metadata()?.len() > 4 * 1024 * 1024 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut data = Vec::new();
    guard.file.read_to_end(&mut data)?;
    let actual: String = Sha256::digest(&data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if actual != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ConPTY runtime hash mismatch",
        ));
    }
    Ok(guard)
}

/// Loads the exact architecture's NuGet 1.24.260710001 pair without PATH lookup.
fn load() -> io::Result<Api> {
    #[cfg(target_arch = "aarch64")]
    let hashes = (
        "db3d173640b172bafd42d5b541b638a9aeec1c7d0e40dd636bf02822a32c912c",
        "ed7622fd0d3bedc9ab9f122f5e58edf0def9e7999224f52dd395ba9f54edbe09",
    );
    #[cfg(not(target_arch = "aarch64"))]
    let hashes = (
        "39fba2713e2495117b1591ae8c32a3b904bea7aa66069cf7815e2844c76d75d8",
        "b7fd936c2668b87b9ecf7b3366dc6568afc1c6f981874cba3e955a1c35cf8160",
    );
    let exe = std::env::current_exe()?;
    let folder = exe.parent().ok_or(io::ErrorKind::NotFound)?.join("conpty");
    let dll = verified(&folder.join("conpty.dll"), hashes.0)?;
    let host = verified(&folder.join("OpenConsole.exe"), hashes.1)?;
    let name: Vec<u16> = dll
        .path
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe {
        let module = LoadLibraryExW(
            name.as_ptr(),
            null_mut(),
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        );
        if module.is_null() {
            return Err(io::Error::last_os_error());
        }
        let create = GetProcAddress(module, c"ConptyCreatePseudoConsole".as_ptr().cast());
        let resize = GetProcAddress(module, c"ConptyResizePseudoConsole".as_ptr().cast());
        let close = GetProcAddress(module, c"ConptyClosePseudoConsole".as_ptr().cast());
        let (Some(create), Some(resize), Some(close)) = (create, resize, close) else {
            FreeLibrary(module);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ConPTY entry points missing",
            ));
        };
        Ok(Api {
            module,
            create: std::mem::transmute::<unsafe extern "system" fn() -> isize, Create>(create),
            resize: std::mem::transmute::<unsafe extern "system" fn() -> isize, Resize>(resize),
            close: std::mem::transmute::<unsafe extern "system" fn() -> isize, Close>(close),
            _files: vec![dll, host],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rejects absent or altered bytes and pins accepted files against replacement.
    #[test]
    fn verified_runtime_pins_exact_bytes() {
        let folder = std::env::temp_dir().join(format!("miniclaw-conpty-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("runtime.dll");
        let hash = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(
            verified(&path, hash).err().unwrap().kind(),
            io::ErrorKind::NotFound
        );
        std::fs::write(&path, b"altered").unwrap();
        assert_eq!(
            verified(&path, hash).err().unwrap().kind(),
            io::ErrorKind::InvalidData
        );
        std::fs::write(&path, b"abc").unwrap();
        let guard = verified(&path, hash).unwrap();
        assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
        assert!(std::fs::rename(&path, folder.join("moved.dll")).is_err());
        assert!(std::fs::rename(&folder, folder.with_extension("moved")).is_err());
        drop(guard);
        std::fs::remove_dir_all(folder).unwrap();
    }
}
