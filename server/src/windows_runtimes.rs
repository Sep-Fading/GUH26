//! What a sandboxed process needs so language runtimes start inside MiniClaw's Windows sandbox.
//!
//! Three things, each measured natively (docs/windows/sandbox-review.md, "Language runtimes"):
//! the token's capabilities (Winsock must start even in a run without network, because Node
//! aborts when it cannot), Node's module resolution (`--preserve-symlinks`, so Node never asks
//! for the real path of the drive root), and Python's side-by-side manifest (the launcher asks
//! Windows for its activation context before the sandboxed process does).
//! This file holds the decisions that need no Windows API; the native part applies them.

// Reports refused capability choices.
use std::io;
// Names images and installation folders.
use std::path::{Path, PathBuf};

/// How long a sandboxed launch waits for the one-time check of MiniClaw's own runtime files (the private
/// Node, npm's tree, Python's hook) that the backend starts at once (`warm_up`): hashing them reads about
/// 100 MB and 2,000 files, seconds on a cold disk. Nothing unchecked is used; a launch that waits longer
/// is refused with a reason and can be tried again. A waiter on an async worker hands the worker's other
/// tasks to another thread first (`off_worker`); an MCP launch waits off the workers, within its server's
/// start wait (`settled`).
pub const CHECK_WAIT: std::time::Duration = std::time::Duration::from_secs(180);

/// A check that runs once for the life of the backend. The first caller runs it; callers that arrive
/// meanwhile wait for that answer (up to their limit) instead of starting another; a failure is kept like a
/// success. Unlike `OnceLock` a waiter can give up, and it then gets an error, never an unchecked value.
pub struct Checked<T> { state: std::sync::Mutex<CheckState<T>>, done: std::sync::Condvar }
enum CheckState<T> { Idle, Running, Done(Result<std::sync::Arc<T>, String>) }

impl<T> Default for Checked<T> { fn default() -> Self { Self::new() } }

impl<T> Checked<T> {
    pub const fn new() -> Self { Self { state: std::sync::Mutex::new(CheckState::Idle), done: std::sync::Condvar::new() } }

    /// Returns the check's answer, running `check` if nobody has started it, waiting at most `wait` for a
    /// check another caller is running.
    pub fn get(&self, check: impl FnOnce() -> Result<T, String>, wait: std::time::Duration) -> Result<std::sync::Arc<T>, String> {
        let deadline = std::time::Instant::now() + wait;
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            match &*state {
                CheckState::Done(result) => return result.clone(),
                CheckState::Idle => {
                    *state = CheckState::Running;
                    drop(state);
                    // A check that panics still ends: its waiters get an error instead of waiting out their limit.
                    struct Finish<'a, T>(&'a Checked<T>, Option<Result<std::sync::Arc<T>, String>>);
                    impl<T> Drop for Finish<'_, T> {
                        fn drop(&mut self) {
                            let result = self.1.take().unwrap_or_else(|| Err("The check stopped unexpectedly".to_owned()));
                            *self.0.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = CheckState::Done(result);
                            self.0.done.notify_all();
                        }
                    }
                    let mut finish = Finish(self, None);
                    let result = check().map(std::sync::Arc::new);
                    finish.1 = Some(result.clone());
                    return result;
                }
                CheckState::Running => {
                    let now = std::time::Instant::now();
                    if now >= deadline {
                        return Err(format!("MiniClaw's runtime files are still being checked after {} s; try again in a moment", wait.as_secs()));
                    }
                    state = off_worker(|| self.done.wait_timeout(state, deadline - now).unwrap_or_else(|poisoned| poisoned.into_inner()).0);
                }
            }
        }
    }
}

/// Runs a blocking wait without holding up the async runtime: on a worker of the backend's multi-threaded
/// runtime, the worker's queued tasks (its next one included, which no other worker may take) and its
/// timers move to another thread first (`tokio::task::block_in_place`). Elsewhere it simply waits.
fn off_worker<R>(wait: impl FnOnce() -> R) -> R {
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) if runtime.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => tokio::task::block_in_place(wait),
        _ => wait(),
    }
}

/// Waits, on a blocking thread and at most `wait`, for runtime checks that a launch is about to use, so
/// the launch's own calls find them answered. A check still running at the limit is an error that says to
/// try again; the checks go on and their answer is kept. Nothing unchecked is ever used.
#[cfg(any(windows, test))]
pub async fn settled(wait: std::time::Duration, checks: impl FnOnce() + Send + 'static) -> Result<(), String> {
    match tokio::time::timeout(wait, tokio::task::spawn_blocking(checks)).await {
        Ok(_) => Ok(()),
        Err(_) => Err(format!("MiniClaw's runtime files are still being checked after {} s; try again in a moment.", wait.as_secs())),
    }
}

/// The well-known capability that lets a token create sockets. Only networked runs carry it.
pub const INTERNET_CLIENT: &str = "internetClient";

/// The flags every sandboxed Node process receives. Without them Node resolves the real path of
/// the script and of each module, which starts with the drive root the sandbox cannot open.
pub const NODE_FLAGS: [&str; 2] = ["--preserve-symlinks", "--preserve-symlinks-main"];

/// The OpenSSL configuration MiniClaw's private Node reads inside the sandbox (`OPENSSL_CONF`).
/// It names MiniClaw's seed source; the packaged file must equal it byte for byte.
pub const OPENSSL_CONFIG: &str = include_str!("../../openssl-seed/openssl.cnf");

/// The seed provider library beside the private `node.exe`, found through `OPENSSL_MODULES`.
pub const SEED_LIBRARY: &str = "miniclaw_openssl_seed.dll";

/// The private Node release: Node.js 24.21.0, the version MiniClaw itself runs on.
pub const NODE_VERSION: &str = "24.21.0";

/// The npm release inside that Node archive, which MiniClaw ships beside the private Node.
pub const NPM_VERSION: &str = "11.19.0";

/// MiniClaw's preload for every sandboxed Node process (`bin\node\child-pipes.cjs`), added to
/// `NODE_OPTIONS` by the process launcher. Node 24.21.0's libuv 1.52.1 names the pipes of a
/// child's piped standard handles (and `fork`'s channel) outside the AppContainer's namespace and
/// retries forever when that is refused, so the program stops for good; the preload refuses such a
/// child at once instead, as Node reports a child that cannot start, and says why on standard
/// error. It does nothing with libuv 1.53.0 or later (docs/windows/sandbox-review.md, "Child
/// processes of sandboxed Node"). The packaged file must equal it byte for byte.
pub const NODE_PRELOAD: &str = include_str!("../../scripts/setup/node-sandbox/child-pipes.cjs");

/// The preload's file name beside the private `node.exe`.
pub const NODE_PRELOAD_FILE: &str = "child-pipes.cjs";

/// The code of the preload's refusal, also at the start of the line it writes to standard error.
pub const CHILD_PIPES_CODE: &str = "ERR_MINICLAW_SANDBOX_CHILD_PIPES";

/// Names the verified preload to the process launcher, which adds it to `NODE_OPTIONS` of a
/// sandboxed process. It never reaches the process.
pub const NODE_PRELOAD_PATH: &str = "MINICLAW_NODE_PRELOAD";

/// Gives a sandboxed process's environment block (upper-case names) Node's flags and, when its
/// sandbox has the private Node, the preload, after any `NODE_OPTIONS` the caller set. Inside
/// quotes `NODE_OPTIONS` reads a backslash as an escape (`ParseNodeOptionsEnvVar`), so the path is
/// written escaped. The preload is never listed twice.
pub fn node_environment(entries: &mut std::collections::BTreeMap<String, String>) {
    let preload = entries.remove(NODE_PRELOAD_PATH).filter(|path| !path.is_empty());
    let mut options = node_options(entries.get("NODE_OPTIONS").map(String::as_str));
    if let Some(path) = preload {
        let require = format!("--require \"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""));
        if !options.contains(&require) {
            options.push(' ');
            options.push_str(&require);
        }
    }
    entries.insert("NODE_OPTIONS".into(), options);
}

/// The `sh` launchers beside the private `node.exe`, so `npm` and `npx` typed in the sandboxed
/// shell run npm's own scripts with the private Node. The packaged files must equal them.
pub const NPM_LAUNCHER: &str = include_str!("../../scripts/setup/npm-launchers/npm");
pub const NPX_LAUNCHER: &str = include_str!("../../scripts/setup/npm-launchers/npx");
/// What the launchers load into npm first: npm's tree code checks every folder of a path from
/// the drive root, which the sandbox may not read above its granted folders. Same exact-bytes rule.
pub const NPM_PRELOAD: &str = include_str!("../../scripts/setup/npm-launchers/npm-sandbox.cjs");

/// BusyBox under a name npm can start as its script shell (`npm_config_script_shell`): npm
/// runs `<shell> -c <script>`, and BusyBox picks its `sh` applet from the program name.
pub const SCRIPT_SHELL: &str = "sh.exe";

/// npm's tree from the official Node archive (`node_modules/npm`, the same on ARM64 and x64):
/// its number of files and the [`tree_digest`] of their listing.
pub const NPM_FILES: usize = 1926;
pub const NPM_TREE_DIGEST: &str = "609999b72b258436fecbad1b66990c83097f8eae5a15f72e4475a34074e43a80";

/// Digests a file tree as SHA-256 over one `<sha256 hex>  <relative path>` line per file, paths
/// with `/` and sorted bytewise. `scripts/setup/windows-node-sandbox.mjs` computes the same.
pub fn tree_digest(files: &mut [(String, String)]) -> String {
    use sha2::{Digest, Sha256};
    files.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    let mut hash = Sha256::new();
    for (path, digest) in files.iter() { hash.update(format!("{digest}  {path}\n").as_bytes()); }
    hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
}

/// SHA-256 of the official `node.exe` of [`NODE_VERSION`] per architecture, from the release
/// archives whose checksums nodejs.org publishes (`scripts/package-release.mjs`).
pub fn node_digest(arch: &str) -> Option<&'static str> {
    match arch {
        "aarch64" => Some("dff59da18b6ffe1bf1ca99e1d2af4906080c481740619f5b5098c0fca28bd9b7"),
        "x86_64" => Some("ba4e6d110e8c1592a1ecd390f6b05f3da124b13871a5be62b341a07a853c6c32"),
        _ => None,
    }
}

/// Whether a command names Node, by file name only (`node.exe`, any case). Such a command runs
/// with MiniClaw's private Node inside the sandbox.
pub fn node_image(program: &Path) -> bool {
    image_name(program) == "node.exe"
}

/// Chooses the capability names of a sandboxed run's token.
///
/// A run without approved hosts carries only the installation's Winsock-read capability, so
/// Winsock starts but no socket can be created; without an installed capability it carries none.
/// A networked run adds `internetClient` and needs the installed capability. A name that is not
/// an installation's Winsock capability is refused, so no file can name `registryRead`.
pub fn token_capabilities(winsock: Option<&str>, networked: bool) -> io::Result<Vec<String>> {
    if let Some(name) = winsock && !crate::windows_winsock_acl::is_capability_name(name) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Not an installation's Winsock capability"));
    }
    match (winsock, networked) {
        (Some(name), false) => Ok(vec![name.to_owned()]),
        (None, false) => Ok(Vec::new()),
        (Some(name), true) => Ok(vec![INTERNET_CLIENT.to_owned(), name.to_owned()]),
        (None, true) => Err(io::Error::new(io::ErrorKind::Unsupported, "Windows network service is not configured")),
    }
}

/// Adds the sandbox's Node flags to a `NODE_OPTIONS` value without dropping what is there.
/// A flag already present, or turned off with its `--no-` form, is left as the user wrote it.
pub fn node_options(existing: Option<&str>) -> String {
    let existing = existing.unwrap_or_default().trim();
    let words: Vec<&str> = existing.split_whitespace().collect();
    let mut value = existing.to_owned();
    for flag in NODE_FLAGS {
        let negated = format!("--no-{}", &flag[2..]);
        if words.iter().any(|word| *word == flag || *word == negated) { continue; }
        if !value.is_empty() { value.push(' '); }
        value.push_str(flag);
    }
    value
}

/// What the launcher primes before starting a Python image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonImages {
    /// Files whose embedded manifest resource 1 (an executable's) is primed.
    pub programs: Vec<PathBuf>,
    /// The folders whose `python3*.dll` files' manifest resource 2 (a library's) is primed.
    pub folders: Vec<PathBuf>,
    /// Folders whose extension modules (`*.pyd`, and libraries beside them) are primed: every
    /// standard-library extension module carries its own manifest, and Windows builds its context
    /// when the module loads, inside the sandbox (`_overlapped` failed so on a fresh boot).
    pub extensions: Vec<PathBuf>,
    /// `site-packages` folders whose extension modules are primed, searched with a bound.
    pub packages: Vec<PathBuf>,
}

/// Whether an image name is a CPython interpreter or its windowless twin, by file name only.
pub fn python_image(program: &Path) -> bool {
    let name = image_name(program);
    let Some(stem) = name.strip_suffix(".exe") else { return false };
    let Some(version) = stem.strip_prefix("pythonw").or_else(|| stem.strip_prefix("python")) else { return false };
    version.is_empty() || version.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.') && version.starts_with('3')
}

/// The last component of a Windows path in lower case, whichever separator it uses.
fn image_name(program: &Path) -> String {
    let text = program.to_string_lossy().to_ascii_lowercase();
    text.rsplit(['\\', '/']).next().unwrap_or_default().to_owned()
}

/// Lists what to prime for a Python image: the image and its folder, and when the image is a
/// virtual environment's launcher (its folder's parent holds `pyvenv.cfg`), the base interpreter
/// it starts and that interpreter's folder as well.
pub fn python_images(program: &Path, venv_config: Option<&str>) -> PythonImages {
    let folder = program.parent().map(Path::to_owned);
    let home = venv_config.and_then(venv_home);
    // A virtual environment's packages are beside its `Scripts` folder; an installation's beside python.exe.
    let root = if home.is_some() { folder.as_deref().and_then(Path::parent).map(Path::to_owned) } else { folder.clone() };
    let packages = |root: &Path| root.join("Lib").join("site-packages");
    let mut images = PythonImages { programs: vec![program.to_owned()], folders: folder.clone().into_iter().collect(),
        extensions: folder.iter().map(|folder| folder.join("DLLs")).collect(), packages: root.iter().map(|root| packages(root)).collect() };
    if let Some(home) = home {
        let windowless = image_name(program).starts_with("pythonw");
        images.programs.push(home.join(if windowless { "pythonw.exe" } else { "python.exe" }));
        images.extensions.push(home.join("DLLs"));
        images.packages.push(packages(&home));
        images.folders.push(home);
    }
    images
}

/// Reads the base installation a virtual environment's `pyvenv.cfg` names in its `home` line.
pub fn venv_home(config: &str) -> Option<PathBuf> {
    config.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        let value = value.trim();
        (key.trim().eq_ignore_ascii_case("home") && windows_absolute(value)).then(|| PathBuf::from(value))
    })
}

/// Accepts a drive-absolute Windows path such as `C:\Python313`, on any host.
fn windows_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() > 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/') && !path.contains('\0')
}

/// MiniClaw's sitecustomize for sandboxed Python (`bin\python\sitecustomize.py` of the Windows
/// bundle). asyncio cannot start in the sandbox on its own: its event loop wakes itself through a
/// socket pair that CPython builds from a loopback listener, and its subprocess pipes have global
/// names; the sandbox refuses both. The hook replaces those two with named pipes in the run's own
/// namespace (docs/windows/sandbox-review.md, "asyncio in the sandbox"). The packaged file must
/// equal it byte for byte.
pub const PYTHON_HOOK: &str = include_str!("../../scripts/setup/python-sandbox/sitecustomize.py");

/// The hook's file name: Python imports it at startup from the first `PYTHONPATH` folder.
pub const PYTHON_HOOK_FILE: &str = "sitecustomize.py";

/// Turns the hook on inside a sandboxed process. Without it the hook changes nothing.
pub const PYTHON_MARKER: &str = "MINICLAW_PYTHON_SANDBOX";

/// Names the verified hook folder to the process launcher, which puts it first on `PYTHONPATH`
/// after the caller's own values are known. It never reaches the process.
pub const PYTHON_HOOK_FOLDER: &str = "MINICLAW_PYTHON_HOOK";

/// Puts the hook first on `PYTHONPATH` and turns it on, in an environment block with upper-case
/// names. Entries a caller set (its own `PYTHONPATH`, or the marker set to something else) stay.
pub fn python_environment(entries: &mut std::collections::BTreeMap<String, String>) {
    let Some(folder) = entries.remove(PYTHON_HOOK_FOLDER) else { return };
    let same = |entry: &str| entry.trim_end_matches(['\\', '/']).eq_ignore_ascii_case(folder.trim_end_matches(['\\', '/']));
    let mut path = folder.clone();
    for entry in entries.get("PYTHONPATH").map(String::as_str).unwrap_or_default().split(';') {
        if entry.is_empty() || same(entry) { continue; }
        path.push(';');
        path.push_str(entry);
    }
    entries.insert("PYTHONPATH".into(), path);
    entries.entry(PYTHON_MARKER.into()).or_insert_with(|| "1".into());
}

/// Whether a sandboxed MCP server needs the networked token even without approved hosts. CPython
/// imports `_overlapped` with `asyncio`, and that module creates an IPv4 socket at import
/// (Modules/overlapped.c, `initialize_function_pointers`). Windows lets a sandboxed process create
/// one only with a network capability: the TCP/IP Winsock helper key is readable to
/// `internetClient` and not to the offline token. Such a run's lease and proxy allow no host.
pub fn python_needs_lease(program: &Path) -> bool {
    python_image(program)
}

#[cfg(windows)]
mod native {
    // Reads a virtual environment's configuration, lists library files and hashes pinned files.
    use std::{fs, io::Read, path::Path};

    /// `ACTCTXW` from winbase.h, with only the fields this file sets.
    #[repr(C)]
    struct ActivationContext {
        size: u32,
        flags: u32,
        source: *const u16,
        architecture: u16,
        language: u16,
        directory: *const u16,
        resource: *const u16,
        application: *const u16,
        module: *mut std::ffi::c_void,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateActCtxW(context: *const ActivationContext) -> *mut std::ffi::c_void;
        fn ReleaseActCtx(context: *mut std::ffi::c_void);
    }

    /// The most libraries primed per folder; an installation has two (`python3.dll`, `python3XY.dll`).
    const MOST_LIBRARIES: usize = 8;

    /// Asks Windows, outside the sandbox, for the activation context of one embedded manifest and
    /// releases it at once. Returns the Windows error, or 0. Windows keeps what it built for the
    /// file, and the sandboxed process then finds it instead of building it in a token that may
    /// not read the side-by-side settings. Microsoft does not document that cache.
    pub fn activation_context(file: &Path, resource: usize) -> u32 {
        let source: Vec<u16> = file.as_os_str().encode_wide_with_nul();
        // ACTCTX_FLAG_RESOURCE_NAME_VALID; the resource is a MAKEINTRESOURCE number.
        let context = ActivationContext { size: size_of::<ActivationContext>() as u32, flags: 0x8, source: source.as_ptr(), architecture: 0,
            language: 0, directory: std::ptr::null(), resource: resource as *const u16, application: std::ptr::null(), module: std::ptr::null_mut() };
        let handle = unsafe { CreateActCtxW(&context) };
        if handle as isize == -1 { return std::io::Error::last_os_error().raw_os_error().unwrap_or(-1) as u32; }
        unsafe { ReleaseActCtx(handle) };
        0
    }

    /// Converts a path for a Win32 call.
    trait Wide { fn encode_wide_with_nul(&self) -> Vec<u16>; }
    impl Wide for std::ffi::OsStr {
        fn encode_wide_with_nul(&self) -> Vec<u16> {
            use std::os::windows::ffi::OsStrExt;
            self.encode_wide().chain(Some(0)).collect()
        }
    }

    /// MiniClaw's private Node for sandboxed runs, verified and pinned beside the backend in `node\`:
    /// the official `node.exe`, the seed provider and the OpenSSL configuration that loads it.
    pub struct PrivateNode {
        pub folder: std::path::PathBuf,
        pub program: std::path::PathBuf,
        /// MiniClaw's preload for sandboxed Node ([`super::NODE_PRELOAD`]).
        pub preload: std::path::PathBuf,
        _pins: Vec<crate::platform_fs::GuardedPath>,
    }

    impl PrivateNode {
        /// The variables that make the private Node seed OpenSSL without the registry, and the one
        /// that names its preload to the process launcher. Other OpenSSL users in the sandbox
        /// (Git, Python) find no `openssl_conf` section and ignore it.
        pub fn environment(&self) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
            vec![("OPENSSL_CONF".into(), self.folder.join("openssl.cnf").into_os_string()),
                ("OPENSSL_MODULES".into(), self.folder.as_os_str().to_owned()),
                (super::NODE_PRELOAD_PATH.into(), self.preload.as_os_str().to_owned())]
        }
    }

    /// Hashes one pinned file as it is read, without holding it in memory.
    fn digest(file: &mut fs::File) -> std::io::Result<String> {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        let mut buffer = vec![0u8; 1 << 20];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 { break; }
            hash.update(&buffer[..count]);
        }
        Ok(hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
    }

    /// Opens, pins and checks the four files of a private Node folder. The seed library is
    /// checked against the hash this backend was built with; a build without one has no private Node.
    /// `openssl.cnf` and the preload must equal the bytes compiled into the backend.
    pub fn load_private_node(folder: &Path, seed_digest: Option<&str>) -> std::io::Result<PrivateNode> {
        use std::io::{Error, ErrorKind};
        let seed_digest = seed_digest.ok_or_else(|| Error::new(ErrorKind::Unsupported, "This backend was built without MiniClaw's OpenSSL seed library"))?;
        let expected = super::node_digest(std::env::consts::ARCH).ok_or(ErrorKind::Unsupported)?;
        let mut pins = Vec::new();
        for (name, wanted) in [("node.exe", expected), (super::SEED_LIBRARY, seed_digest)] {
            let mut pin = crate::platform_fs::guarded_path(&folder.join(name), false, false)?;
            if digest(&mut pin.file)? != wanted { return Err(Error::new(ErrorKind::InvalidData, format!("{name} of the private Node was changed"))); }
            pins.push(pin);
        }
        for (name, wanted) in [("openssl.cnf", super::OPENSSL_CONFIG), (super::NODE_PRELOAD_FILE, super::NODE_PRELOAD)] {
            let mut pin = crate::platform_fs::guarded_path(&folder.join(name), false, false)?;
            let mut text = Vec::new();
            (&mut pin.file).take(64 * 1024).read_to_end(&mut text)?;
            if text != wanted.as_bytes() { return Err(Error::new(ErrorKind::InvalidData, format!("{name} of the private Node was changed"))); }
            pins.push(pin);
        }
        let program = pins[0].path.clone();
        let preload = pins[3].path.clone();
        Ok(PrivateNode { folder: program.parent().ok_or(ErrorKind::InvalidData)?.to_owned(), program, preload, _pins: pins })
    }

    /// The seed library hash this backend was built with (`scripts/package-backend.mjs` sets it).
    pub const SEED_DIGEST: Option<&str> = option_env!("MINICLAW_OPENSSL_SEED_SHA256");

    /// The private Node beside this backend, checked once and held for the backend's lifetime.
    /// An installation without one (or with a changed one) runs sandboxed Node commands with the
    /// user's own Node, which cannot seed OpenSSL in the sandbox.
    pub fn private_node() -> std::io::Result<std::sync::Arc<PrivateNode>> {
        static NODE: super::Checked<PrivateNode> = super::Checked::new();
        NODE.get(|| {
            let started = std::time::Instant::now();
            let exe = std::env::current_exe().map_err(|error| error.to_string())?;
            let folder = exe.parent().ok_or("Backend directory missing")?.join("node");
            let node = load_private_node(&folder, SEED_DIGEST).map_err(|error| error.to_string());
            tracing::debug!(elapsed = ?started.elapsed(), ok = node.is_ok(), "Checked MiniClaw's private Node");
            node
        }, super::CHECK_WAIT).map_err(|error| std::io::Error::other(format!("MiniClaw's private Node is unavailable: {error}")))
    }

    /// Starts the one-time checks of MiniClaw's own runtime files (the private Node, npm's tree, Python's
    /// hook) when the backend starts, on a thread of their own, so the first sandboxed launch does not pay
    /// for them. A launch that comes earlier waits for the same check (`CHECK_WAIT`); nothing is used before
    /// it passed. An installation without these files only logs it, as a launch would.
    pub fn warm_up() {
        let spawned = std::thread::Builder::new().name("miniclaw-runtime-check".into()).spawn(|| {
            let started = std::time::Instant::now();
            let node = private_node().is_ok();
            let npm = node && private_npm().is_ok();
            let hook = super::python_hook().is_ok();
            tracing::debug!(elapsed = ?started.elapsed(), node, npm, hook, "Checked MiniClaw's runtime files at start");
        });
        if let Err(error) = spawned { tracing::warn!(%error, "The runtime files are checked at their first use instead"); }
    }

    /// Waits, without holding an async worker, at most `wait` for the checks a launch uses: the private Node,
    /// npm's tree when the launch installs with npm, and Python's hook (`super::settled`).
    pub async fn ready(npm: bool, wait: std::time::Duration) -> Result<(), String> {
        super::settled(wait, move || {
            if private_node().is_ok() && npm { let _ = private_npm(); }
            let _ = super::python_hook();
        }).await
    }

    /// MiniClaw's npm beside the private Node: npm's tree from the pinned Node archive in
    /// `node\node_modules\npm`, the `npm` and `npx` launchers for the shell, and BusyBox as
    /// `sh.exe` for package scripts. Verified once; the sandbox grants it read-only.
    pub struct PrivateNpm {
        /// `node_modules\npm`, granted with every file in it.
        pub root: std::path::PathBuf,
        /// npm's own command script, which the private Node runs.
        pub cli: std::path::PathBuf,
        /// The script shell npm starts instead of cmd.exe.
        pub shell: std::path::PathBuf,
        _pins: Vec<crate::platform_fs::GuardedPath>,
    }

    /// Lists every file under a folder with its SHA-256, paths relative with `/`. A link, or more
    /// than `most` files, is refused.
    fn listing(root: &Path, most: usize) -> std::io::Result<Vec<(String, String)>> {
        use std::io::{Error, ErrorKind};
        let mut files = Vec::new();
        let mut folders = vec![(root.to_path_buf(), String::new())];
        while let Some((folder, prefix)) = folders.pop() {
            for entry in fs::read_dir(&folder)? {
                let entry = entry?;
                let metadata = fs::symlink_metadata(entry.path())?;
                let name = entry.file_name().into_string().map_err(|_| Error::new(ErrorKind::InvalidData, "A file name of npm is not Unicode"))?;
                let relative = format!("{prefix}{name}");
                if crate::platform_fs::is_link(&metadata) { return Err(Error::new(ErrorKind::InvalidData, format!("npm's {relative} is a link"))); }
                if metadata.is_dir() { folders.push((entry.path(), format!("{relative}/"))); continue; }
                if files.len() >= most { return Err(Error::new(ErrorKind::InvalidData, "npm has more files than the pinned release")); }
                files.push((relative, digest(&mut fs::File::open(entry.path())?)?));
            }
        }
        Ok(files)
    }

    /// Checks MiniClaw's npm in a private Node folder: the launchers byte for byte, `sh.exe` against
    /// the pinned BusyBox, and npm's tree against the digest of the pinned archive's tree.
    pub fn load_private_npm(folder: &Path) -> std::io::Result<PrivateNpm> {
        use std::io::{Error, ErrorKind};
        let mut pins = Vec::new();
        for (name, wanted) in [("npm", super::NPM_LAUNCHER), ("npx", super::NPX_LAUNCHER), ("npm-sandbox.cjs", super::NPM_PRELOAD)] {
            let mut pin = crate::platform_fs::guarded_path(&folder.join(name), false, false)?;
            let mut text = Vec::new();
            (&mut pin.file).take(64 * 1024).read_to_end(&mut text)?;
            if text != wanted.as_bytes() { return Err(Error::new(ErrorKind::InvalidData, format!("The {name} launcher of the private Node was changed"))); }
            pins.push(pin);
        }
        let (size, wanted) = crate::windows_shell::busybox_pin(std::env::consts::ARCH).ok_or(ErrorKind::Unsupported)?;
        let mut shell = crate::platform_fs::guarded_path(&folder.join(super::SCRIPT_SHELL), false, false)?;
        if shell.file.metadata()?.len() != size || digest(&mut shell.file)? != wanted {
            return Err(Error::new(ErrorKind::InvalidData, "sh.exe of the private Node is not the pinned shell"));
        }
        let shell_path = shell.path.clone();
        pins.push(shell);
        let root = crate::platform_fs::guarded_acl_path(&folder.join("node_modules").join("npm"), true)?;
        let mut files = listing(&root.path, super::NPM_FILES + 1)?;
        if files.len() != super::NPM_FILES || super::tree_digest(&mut files) != super::NPM_TREE_DIGEST {
            return Err(Error::new(ErrorKind::InvalidData, "npm beside the private Node is not the npm of the pinned Node archive"));
        }
        let root_path = root.path.clone();
        pins.push(root);
        Ok(PrivateNpm { cli: root_path.join("bin").join("npm-cli.js"), root: root_path, shell: shell_path, _pins: pins })
    }

    /// MiniClaw's npm beside this backend, checked at its first use and held for the backend's
    /// lifetime. Without it, `npm`, `npx` and npm-launched MCP servers are unavailable.
    pub fn private_npm() -> std::io::Result<std::sync::Arc<PrivateNpm>> {
        static NPM: super::Checked<PrivateNpm> = super::Checked::new();
        NPM.get(|| {
            let node = private_node().map_err(|error| error.to_string())?;
            let started = std::time::Instant::now();
            let npm = load_private_npm(&node.folder).map_err(|error| error.to_string());
            tracing::debug!(elapsed = ?started.elapsed(), ok = npm.is_ok(), "Checked MiniClaw's npm");
            npm
        }, super::CHECK_WAIT).map_err(|error| std::io::Error::other(format!("MiniClaw's npm is unavailable: {error}")))
    }

    /// The most extension modules primed per launch, and the most folder entries looked at for them.
    const MOST_MODULES: usize = 512;
    const MOST_ENTRIES: usize = 20_000;

    /// Lists `*.pyd` files (and, at the top of a `DLLs` folder, `*.dll` files) under a folder,
    /// recursively when asked, within the bounds above. Links are not followed.
    fn extension_modules(root: &Path, recursive: bool, found: &mut Vec<std::path::PathBuf>) {
        let mut folders = vec![root.to_owned()];
        let mut seen = 0;
        while let Some(folder) = folders.pop() {
            let Ok(entries) = fs::read_dir(&folder) else { continue };
            for entry in entries.flatten() {
                seen += 1;
                if seen > MOST_ENTRIES || found.len() >= MOST_MODULES { return; }
                let Ok(kind) = entry.file_type() else { continue };
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                if kind.is_dir() && !kind.is_symlink() {
                    if recursive && !name.starts_with('.') && name != "__pycache__" { folders.push(entry.path()); }
                } else if kind.is_file() && (name.ends_with(".pyd") || !recursive && name.ends_with(".dll")) {
                    found.push(entry.path());
                }
            }
        }
    }

    /// Primes a Python image before it is started in the sandbox. Best effort: nothing here can
    /// fail the launch, and an image that is not Python costs one name comparison.
    pub fn prime(program: &Path) {
        if !super::python_image(program) { return; }
        let started = std::time::Instant::now();
        let venv = program.parent().and_then(Path::parent).map(|root| root.join("pyvenv.cfg"))
            .and_then(|config| fs::File::open(config).ok())
            .and_then(|file| { let mut text = String::new(); file.take(64 * 1024).read_to_string(&mut text).ok().map(|_| text) });
        let images = super::python_images(program, venv.as_deref());
        let mut results = Vec::new();
        for image in &images.programs {
            results.push((image.clone(), activation_context(image, 1)));
        }
        for folder in &images.folders {
            let Ok(entries) = fs::read_dir(folder) else { continue };
            let libraries = entries.flatten().map(|entry| entry.path()).filter(|path| path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy().to_ascii_lowercase();
                name.starts_with("python3") && name.ends_with(".dll")
            })).take(MOST_LIBRARIES);
            for library in libraries { results.push((library.clone(), activation_context(&library, 2))); }
        }
        let mut modules = Vec::new();
        for folder in &images.extensions { extension_modules(folder, false, &mut modules); }
        for folder in &images.packages { extension_modules(folder, true, &mut modules); }
        let primed = modules.iter().filter(|module| activation_context(module, 2) == 0).count();
        tracing::debug!(?results, modules = modules.len(), primed, elapsed = ?started.elapsed(), "Primed Python activation contexts for a sandboxed launch");
    }
}
#[cfg(windows)]
pub use native::{PrivateNode, PrivateNpm, SEED_DIGEST, activation_context, load_private_node, load_private_npm, prime, private_node, private_npm, ready, warm_up};

#[cfg(windows)]
mod python_native {
    // Pins and checks the packaged hook.
    use std::{io::Read, path::Path};

    /// MiniClaw's sitecustomize, verified and pinned in `python\` beside the backend.
    pub struct PythonHook {
        pub folder: std::path::PathBuf,
        pub file: std::path::PathBuf,
        _pin: crate::platform_fs::GuardedPath,
    }

    /// Opens, pins and checks the hook in a folder: it must equal [`super::PYTHON_HOOK`].
    pub fn load_python_hook(folder: &Path) -> std::io::Result<PythonHook> {
        use std::io::{Error, ErrorKind};
        let mut pin = crate::platform_fs::guarded_path(&folder.join(super::PYTHON_HOOK_FILE), false, false)?;
        let mut text = Vec::new();
        (&mut pin.file).take(256 * 1024).read_to_end(&mut text)?;
        if text != super::PYTHON_HOOK.as_bytes() { return Err(Error::new(ErrorKind::InvalidData, "MiniClaw's sitecustomize was changed")); }
        let file = pin.path.clone();
        Ok(PythonHook { folder: file.parent().ok_or(ErrorKind::InvalidData)?.to_owned(), file, _pin: pin })
    }

    /// The hook beside this backend, checked once and held for the backend's lifetime. Without
    /// it, sandboxed asyncio programs fail as they did before.
    pub fn python_hook() -> std::io::Result<std::sync::Arc<PythonHook>> {
        static HOOK: super::Checked<PythonHook> = super::Checked::new();
        HOOK.get(|| {
            let exe = std::env::current_exe().map_err(|error| error.to_string())?;
            let folder = exe.parent().ok_or("Backend directory missing")?.join("python");
            load_python_hook(&folder).map_err(|error| error.to_string())
        }, super::CHECK_WAIT).map_err(|error| std::io::Error::other(format!("MiniClaw's sitecustomize is unavailable: {error}")))
    }
}
#[cfg(windows)]
pub use python_native::{PythonHook, load_python_hook, python_hook};

#[cfg(test)]
mod tests {
    use super::*;

    const WINSOCK: &str = "miniclawWinsockRead.0123456789abcdef0123456789abcdef";

    /// One check runs once: callers that arrive while it runs wait for its answer instead of starting
    /// another, a failure is kept like a success, and a caller that waits too long gets an error, never a
    /// value that was not checked.
    #[test]
    fn a_runtime_check_runs_once_and_late_callers_wait_for_it() {
        use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
        use std::time::Duration;
        let check = Arc::new(Checked::<String>::new());
        let runs = Arc::new(AtomicUsize::new(0));
        let slow = { let runs = runs.clone(); move || { runs.fetch_add(1, Ordering::SeqCst); std::thread::sleep(Duration::from_millis(300)); Ok("verified".to_owned()) } };
        let first = { let check = check.clone(); let slow = slow.clone(); std::thread::spawn(move || check.get(slow, Duration::from_secs(10))) };
        std::thread::sleep(Duration::from_millis(50));
        let waiters: Vec<_> = (0..4).map(|_| { let check = check.clone(); let slow = slow.clone(); std::thread::spawn(move || check.get(slow, Duration::from_secs(10))) }).collect();
        assert_eq!(*first.join().unwrap().unwrap(), "verified");
        for waiter in waiters { assert_eq!(*waiter.join().unwrap().unwrap(), "verified"); }
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        // A caller whose wait ends before the check does gets an error; the check still finishes once.
        let pending = Arc::new(Checked::<String>::new());
        let started = { let pending = pending.clone(); std::thread::spawn(move || pending.get(|| { std::thread::sleep(Duration::from_millis(400)); Ok("late".to_owned()) }, Duration::from_secs(10))) };
        std::thread::sleep(Duration::from_millis(50));
        let impatient = pending.get(|| panic!("a second check started"), Duration::from_millis(50));
        assert!(impatient.unwrap_err().contains("still being checked"));
        assert_eq!(*started.join().unwrap().unwrap(), "late");
        assert_eq!(*pending.get(|| panic!("checked again"), Duration::from_millis(1)).unwrap(), "late");
        // A failed check is the answer from then on.
        let failed = Checked::<String>::new();
        assert_eq!(failed.get(|| Err("changed".to_owned()), Duration::from_secs(1)).unwrap_err(), "changed");
        assert_eq!(failed.get(|| Ok("never".to_owned()), Duration::from_secs(1)).unwrap_err(), "changed");
    }

    /// A launch that waits from async code for a check another thread is stuck in must not hold up the
    /// backend's other work: an MCP server's 15 s limit elsewhere still fires on time, and the waiter
    /// gets an error at its own limit, never an unchecked value. (On the installed Windows build a check
    /// retried right after a timed-out one neither answered nor reached its limit.)
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn a_stuck_check_holds_up_neither_other_work_nor_its_waiter() {
        use std::sync::{Arc, mpsc};
        use std::time::{Duration, Instant};
        let check = Arc::new(Checked::<String>::new());
        let (release, stuck) = mpsc::channel::<()>();
        let holder = { let check = check.clone(); std::thread::spawn(move || check.get(move || { let _ = stuck.recv(); Ok("late".to_owned()) }, Duration::from_secs(30))) };
        while !matches!(*check.state.lock().unwrap(), CheckState::Running) { std::thread::sleep(Duration::from_millis(5)); }
        // The runtime's only worker: one task waits synchronously, as `Sandbox::program_for` does in a launch.
        let started = Instant::now();
        let waiter = { let check = check.clone(); tokio::spawn(async move { check.get(|| panic!("a second check started"), Duration::from_secs(3)) }) };
        let limit = tokio::spawn(async move {
            assert!(tokio::time::timeout(Duration::from_millis(200), std::future::pending::<()>()).await.is_err());
            started.elapsed()
        });
        let fired = limit.await.unwrap();
        assert!(fired < Duration::from_millis(1500), "another task's 200 ms limit fired only after {fired:?}");
        assert!(waiter.await.unwrap().unwrap_err().contains("still being checked"));
        // A launch that waits for the checks it uses gives up at its own limit, on a blocking thread.
        let started = Instant::now();
        let refused = { let check = check.clone(); settled(Duration::from_millis(200), move || { let _ = check.get(|| panic!("a second check started"), Duration::from_secs(30)); }).await };
        assert!(refused.unwrap_err().contains("still being checked") && started.elapsed() < Duration::from_millis(1500), "{:?}", started.elapsed());
        release.send(()).unwrap();
        assert_eq!(*holder.join().unwrap().unwrap(), "late");
        // Once the check has answered, a waiter is not held up.
        let check = check.clone();
        assert!(settled(Duration::from_secs(5), move || assert_eq!(*check.get(|| panic!("checked again"), Duration::from_millis(1)).unwrap(), "late")).await.is_ok());
    }

    #[test]
    fn offline_runs_carry_only_the_winsock_capability() {
        assert_eq!(token_capabilities(Some(WINSOCK), false).unwrap(), vec![WINSOCK.to_owned()]);
        // Without an installed service nothing is added, and nothing fails.
        assert_eq!(token_capabilities(None, false).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn networked_runs_add_internet_client_and_need_the_installation() {
        assert_eq!(token_capabilities(Some(WINSOCK), true).unwrap(), vec![INTERNET_CLIENT.to_owned(), WINSOCK.to_owned()]);
        assert_eq!(token_capabilities(None, true).unwrap_err().kind(), io::ErrorKind::Unsupported);
    }

    #[test]
    fn no_other_capability_can_be_named() {
        for name in ["registryRead", INTERNET_CLIENT, "lpacCom", "miniclawWinsockRead.", "miniclawWinsockRead.0123456789ABCDEF0123456789abcdef",
            "miniclawWinsockRead.0123456789abcdef0123456789abcdef0", ""] {
            assert!(token_capabilities(Some(name), false).is_err(), "{name:?}");
            assert!(token_capabilities(Some(name), true).is_err(), "{name:?}");
        }
    }

    #[test]
    fn node_options_are_appended_never_replaced() {
        assert_eq!(node_options(None), "--preserve-symlinks --preserve-symlinks-main");
        assert_eq!(node_options(Some("")), "--preserve-symlinks --preserve-symlinks-main");
        assert_eq!(node_options(Some("--max-old-space-size=4096")), "--max-old-space-size=4096 --preserve-symlinks --preserve-symlinks-main");
        assert_eq!(node_options(Some("--preserve-symlinks")), "--preserve-symlinks --preserve-symlinks-main");
        assert_eq!(node_options(Some("--preserve-symlinks-main --preserve-symlinks")), "--preserve-symlinks-main --preserve-symlinks");
        // The user's explicit choice stands.
        assert_eq!(node_options(Some("--no-preserve-symlinks")), "--no-preserve-symlinks --preserve-symlinks-main");
        // A quoted value keeps its spelling; only whole words are compared.
        assert_eq!(node_options(Some(r#"--require "C:\a b\x.js""#)), r#"--require "C:\a b\x.js" --preserve-symlinks --preserve-symlinks-main"#);
        assert_eq!(node_options(Some("--preserve-symlinks-mainx")), "--preserve-symlinks-mainx --preserve-symlinks --preserve-symlinks-main");
    }

    #[test]
    fn node_commands_are_recognised_by_name() {
        for name in [r"C:\Program Files\nodejs\node.exe", r"C:\x\NODE.EXE", "C:/nvm/v20/node.exe"] { assert!(node_image(Path::new(name)), "{name}"); }
        for name in [r"C:\x\nodejs.exe", r"C:\x\node.cmd", r"C:\x\npx.cmd", r"C:\node.exe\python.exe", r"C:\x\node"] { assert!(!node_image(Path::new(name)), "{name}"); }
    }

    #[test]
    fn the_private_node_configuration_names_only_the_seed_source() {
        // Node reads its own section; every other OpenSSL user of the sandbox finds no default section.
        assert!(OPENSSL_CONFIG.lines().any(|line| line == "nodejs_conf = miniclaw_init"));
        assert!(!OPENSSL_CONFIG.lines().any(|line| line.trim_start().starts_with("openssl_conf")));
        assert!(OPENSSL_CONFIG.contains("module = miniclaw_openssl_seed.dll") && OPENSSL_CONFIG.contains("seed = MINICLAW-SYSTEM-SEED"));
        // OpenSSL reads a backslash in a value as an escape, which once broke an absolute path.
        assert!(!OPENSSL_CONFIG.lines().filter(|line| !line.starts_with('#')).any(|line| line.contains('\\')));
        assert_eq!(node_digest("aarch64").map(str::len), Some(64));
        assert_eq!(node_digest("x86_64").map(str::len), Some(64));
        assert_eq!(node_digest("x86"), None);
    }

    #[test]
    fn the_npm_tree_digest_is_a_sorted_listing() {
        use sha2::{Digest, Sha256};
        let mut files = vec![("lib/b.js".to_owned(), "22".to_owned()), ("bin/npm-cli.js".to_owned(), "11".to_owned()), ("Z.md".to_owned(), "33".to_owned())];
        let expected: String = Sha256::digest(b"33  Z.md\n11  bin/npm-cli.js\n22  lib/b.js\n").iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(tree_digest(&mut files), expected);
        assert_eq!(NPM_TREE_DIGEST.len(), 64);
        // The preload answers only refused checks of folders above the run's own folders.
        assert!(NPM_PRELOAD.contains("['EPERM', 'EACCES'].includes(error.code)") && NPM_PRELOAD.contains("!above(target)"));
        // The launchers run npm's scripts with the private Node and nothing else; they are sh, not batch.
        for (launcher, script) in [(NPM_LAUNCHER, "npm-cli.js"), (NPX_LAUNCHER, "npx-cli.js")] {
            assert!(launcher.starts_with("#!/bin/sh\n") && !launcher.contains('\r'), "{launcher}");
            assert!(launcher.contains(&format!("node_modules/npm/bin/{script}")), "{launcher}");
            assert!(launcher.ends_with("exec \"$here/node.exe\" --require \"$here/npm-sandbox.cjs\" \"$cli\" \"$@\"\n"), "{launcher}");
            assert!(!launcher.lines().filter(|line| !line.starts_with('#')).any(|line| line.to_ascii_lowercase().contains("cmd")), "{launcher}");
        }
    }

    #[test]
    fn the_node_preload_is_the_reviewed_file() {
        assert!(NODE_PRELOAD.starts_with("// MiniClaw: loaded into every Node process inside MiniClaw's Windows sandbox"));
        assert_eq!(NODE_PRELOAD_FILE, "child-pipes.cjs");
        // The code the backend looks for on a server's standard error is the one the preload uses.
        assert!(NODE_PRELOAD.contains(&format!("const CODE = '{CHILD_PIPES_CODE}';")));
        // It acts only on Windows with a libuv older than 1.53.0, and never opens a pipe or a socket itself.
        assert!(NODE_PRELOAD.contains("if (process.platform === 'win32' && !fixedLibuv(process.versions.uv)) install();"));
        assert!(NODE_PRELOAD.contains("return major > 1 || (major === 1 && minor >= 53);"));
        for word in ["require('node:net')", "createServer", "openSync", "connect("] { assert!(!NODE_PRELOAD.contains(word), "{word}"); }
    }

    #[test]
    fn sandboxed_node_gets_its_flags_and_the_preload() {
        let entries = |pairs: &[(&str, &str)]| pairs.iter().map(|(name, value)| ((*name).to_owned(), (*value).to_owned())).collect::<std::collections::BTreeMap<_, _>>();
        let mut plain = entries(&[("PATH", r"C:\Windows")]);
        node_environment(&mut plain);
        assert_eq!(plain, entries(&[("PATH", r"C:\Windows"), ("NODE_OPTIONS", "--preserve-symlinks --preserve-symlinks-main")]));
        // NODE_OPTIONS reads a backslash inside quotes as an escape, so the path is written escaped.
        let mut run = entries(&[(NODE_PRELOAD_PATH, r"C:\Mini Claw\bin\node\child-pipes.cjs")]);
        node_environment(&mut run);
        assert_eq!(run, entries(&[("NODE_OPTIONS", r#"--preserve-symlinks --preserve-symlinks-main --require "C:\\Mini Claw\\bin\\node\\child-pipes.cjs""#)]));
        // A server's own options stay first; the preload is never listed twice.
        let mut own = entries(&[(NODE_PRELOAD_PATH, r"C:\m\child-pipes.cjs"), ("NODE_OPTIONS", r#"--max-old-space-size=512 --require "C:\\m\\child-pipes.cjs""#)]);
        node_environment(&mut own);
        assert_eq!(own["NODE_OPTIONS"], r#"--max-old-space-size=512 --require "C:\\m\\child-pipes.cjs" --preserve-symlinks --preserve-symlinks-main"#);
        assert!(!own.contains_key(NODE_PRELOAD_PATH));
    }

    #[test]
    fn python_images_are_recognised_by_name() {
        for name in [r"C:\Python313\python.exe", r"C:\Python313\PYTHONW.EXE", r"C:\x\python3.exe", r"C:\x\python3.13.exe", r"C:\v\Scripts\python.exe",
            r"C:\x\pythonw3.13.exe"] {
            assert!(python_image(Path::new(name)), "{name}");
        }
        for name in [r"C:\x\node.exe", r"C:\x\python.cmd", r"C:\x\pythonista.exe", r"C:\x\py.exe", r"C:\python.exe\node.exe", r"C:\x\python3.13.dll"] {
            assert!(!python_image(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn a_venv_launcher_primes_its_base_interpreter_too() {
        let plain = python_images(Path::new(r"C:\Py\python.exe"), None);
        let folder = Path::new(r"C:\Py\python.exe").parent().unwrap().to_owned();
        assert_eq!(plain, PythonImages { programs: vec![PathBuf::from(r"C:\Py\python.exe")], folders: vec![folder.clone()],
            extensions: vec![folder.join("DLLs")], packages: vec![folder.join("Lib").join("site-packages")] });
        let venv = python_images(Path::new(r"C:\work\.venv\Scripts\pythonw.exe"), Some("home = C:\\Py313\n"));
        assert_eq!(venv.programs[1], PathBuf::from(r"C:\Py313").join("pythonw.exe"));
        assert_eq!(venv.folders[1], PathBuf::from(r"C:\Py313"));
        // Extension modules carry their own manifest: the base installation's, and the packages
        // of both the virtual environment and the base installation.
        // (Forward slashes, so the parent folders are the same on every host.)
        let venv = python_images(Path::new("C:/work/.venv/Scripts/python.exe"), Some("home = C:\\Py313\n"));
        assert_eq!(venv.extensions, vec![PathBuf::from("C:/work/.venv/Scripts/DLLs"), PathBuf::from(r"C:\Py313").join("DLLs")]);
        assert_eq!(venv.packages, vec![PathBuf::from("C:/work/.venv/Lib/site-packages"), PathBuf::from(r"C:\Py313").join("Lib").join("site-packages")]);
        assert_eq!(python_images(Path::new(r"C:\v\Scripts\python.exe"), Some("version = 3.13\n")).programs.len(), 1);
    }

    #[test]
    fn venv_home_is_read_from_its_home_line() {
        let config = "home = C:\\Users\\S\\AppData\\Local\\Programs\\Python\\Python313\ninclude-system-site-packages = false\nversion = 3.13.15\n";
        assert_eq!(venv_home(config), Some(PathBuf::from(r"C:\Users\S\AppData\Local\Programs\Python\Python313")));
        assert_eq!(venv_home("Home=C:\\Py\r\n"), Some(PathBuf::from(r"C:\Py")));
        assert_eq!(venv_home("version = 3.13\n"), None);
        assert_eq!(venv_home("home = relative\\path\n"), None);
        assert_eq!(venv_home("home = \n"), None);
        assert_eq!(venv_home("homes = C:\\Py\n"), None);
    }

    #[test]
    fn sandboxed_python_gets_the_hook_first_on_its_path() {
        let entries = |pairs: &[(&str, &str)]| pairs.iter().map(|(name, value)| ((*name).to_owned(), (*value).to_owned())).collect::<std::collections::BTreeMap<_, _>>();
        let mut plain = entries(&[("PATH", r"C:\Windows")]);
        python_environment(&mut plain);
        assert_eq!(plain, entries(&[("PATH", r"C:\Windows")]), "nothing changes for a run without the hook");
        let mut run = entries(&[(PYTHON_HOOK_FOLDER, r"C:\MiniClaw\bin\python")]);
        python_environment(&mut run);
        assert_eq!(run, entries(&[("PYTHONPATH", r"C:\MiniClaw\bin\python"), (PYTHON_MARKER, "1")]));
        // A server's own PYTHONPATH is kept after the hook; the hook is never listed twice.
        let mut own = entries(&[(PYTHON_HOOK_FOLDER, r"C:\MiniClaw\bin\python"), ("PYTHONPATH", r"C:\proj\src;c:\miniclaw\BIN\python\;;C:\lib")]);
        python_environment(&mut own);
        assert_eq!(own["PYTHONPATH"], r"C:\MiniClaw\bin\python;C:\proj\src;C:\lib");
        // A server that turns the hook off keeps its choice.
        let mut off = entries(&[(PYTHON_HOOK_FOLDER, r"C:\MiniClaw\bin\python"), (PYTHON_MARKER, "0")]);
        python_environment(&mut off);
        assert_eq!(off[PYTHON_MARKER], "0");
    }

    #[test]
    fn the_python_hook_is_the_reviewed_file() {
        assert!(PYTHON_HOOK.starts_with("# MiniClaw: lets asyncio programs run inside MiniClaw's Windows sandbox."));
        assert!(PYTHON_HOOK.contains(&format!("MARKER = \"{PYTHON_MARKER}\"")));
        assert!(PYTHON_HOOK.contains("def run_hidden_sitecustomize():") && PYTHON_HOOK.contains("if __name__ == \"sitecustomize\":"));
        // Pipes only: the hook never opens a socket or touches the network configuration.
        for word in ["socket.socket(", "bind(", "listen(", "connect(", "socketpair = "] { assert!(!PYTHON_HOOK.contains(word), "{word}"); }
        assert_eq!(PYTHON_HOOK_FILE, "sitecustomize.py");
    }

    #[test]
    fn python_servers_run_with_a_lease_even_without_hosts() {
        for name in [r"C:\Py\python.exe", r"C:\v\Scripts\pythonw.exe", r"C:\x\python3.13.exe"] { assert!(python_needs_lease(Path::new(name)), "{name}"); }
        for name in [r"C:\x\node.exe", r"C:\x\uv.exe", r"C:\x\py.exe", r"C:\x\python.cmd"] { assert!(!python_needs_lease(Path::new(name)), "{name}"); }
    }
}
