//! Per-run AppContainer profiles with explicit, handle-bound filesystem access.
// Reuses checked paths and explicit grants only.
use super::contains;
use crate::{platform_fs, seatbelt::Grant, windows_process::SecurityContext};
// Owns profiles, ACL handles and scratch directories.
use std::{
    ffi::OsString,
    fs::{self, File},
    io,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Path, PathBuf},
    ptr,
};
// Creates package identities and edits exact ACLs.
use windows_sys::Win32::{
    Foundation::{INVALID_HANDLE_VALUE, LocalFree},
    Security::{Authorization::*, Isolation::*, *},
    Storage::FileSystem::*,
};

#[path = "windows_sandbox_journal.rs"]
mod journal;

/// The capabilities of a run without approved hosts: the installation's Winsock-read capability,
/// or none when no network service is configured or its name cannot be read. Missing it only
/// takes access away (Winsock then fails to start), so the launch goes on without it.
fn offline_capabilities() -> Vec<String> {
    let installed = crate::windows_network_broker::winsock_capability().map_err(|error| {
        tracing::debug!(%error, "No Winsock capability for an offline run");
    }).ok();
    crate::windows_runtimes::token_capabilities(installed.as_deref(), false).unwrap_or_else(|error| {
        tracing::warn!(%error, "The installed Winsock capability was refused; the run gets no capability");
        Vec::new()
    })
}

fn cleanup_queue() -> &'static super::cleanup::CleanupQueue {
    static QUEUE: std::sync::OnceLock<super::cleanup::CleanupQueue> = std::sync::OnceLock::new();
    QUEUE.get_or_init(Default::default)
}

/// Waits for registered resource revocations during orderly backend shutdown.
pub fn drain_cleanup(timeout: std::time::Duration) -> io::Result<()> {
    cleanup_queue().drain(timeout)
}

/// Recovers abandoned setup and verified stopped jobs from private durable records.
pub fn recover() -> io::Result<()> {
    journal::recover_default()
}

/// One isolated launch's identity, resources and revocable ACL additions.
pub struct Sandbox {
    name: Vec<u16>,
    capabilities: SECURITY_CAPABILITIES,
    scratch: PathBuf,
    access: Vec<File>,
    pending: Vec<Pending>,
    pins: Vec<platform_fs::GuardedPath>,
    tree: Option<crate::windows_process::TreeGuard>,
    journal: Option<journal::Journal>,
    started: std::time::Instant,
    git_env: Vec<(OsString, OsString)>,
    /// Normalized roots this run may reach; a hard-linked file needs all its names inside them.
    roots: Vec<Grant>,
    /// Normalized blocked and protected paths.
    blocked: Vec<String>,
    /// Only the paths this run's own rules block; MiniClaw's read-only runtimes are refused by these.
    rules: Vec<String>,
    /// The token's capability SIDs; `capabilities` points into this storage.
    granted: Option<Box<Granted>>,
    /// Whether the token was given the network capabilities.
    networked: bool,
    /// MiniClaw's private Node, when this run was given it.
    node: Option<std::sync::Arc<crate::windows_runtimes::PrivateNode>>,
    /// MiniClaw's npm, when this run was given it.
    npm: Option<std::sync::Arc<crate::windows_runtimes::PrivateNpm>>,
    /// MiniClaw's sitecustomize for sandboxed Python, when this run was given it.
    python: Option<std::sync::Arc<crate::windows_runtimes::PythonHook>>,
}

/// A token's capability SIDs, in aligned storage that never moves while the sandbox lives.
struct Granted {
    _sids: Vec<Vec<u32>>,
    entries: Vec<SID_AND_ATTRIBUTES>,
}
// The SID points to a stable native allocation, exclusively owned until drop.
unsafe impl Send for Sandbox {}

/// One checked object whose grant waits for its journal record to be flushed.
struct Pending {
    file: File,
    path: PathBuf,
    directory: bool,
    rights: u32,
    inheritance: u32,
}
/// Grants recorded and applied together: one journal flush and one ACL-lock acquisition.
const GRANT_BATCH: usize = 512;

impl Sandbox {
    /// Creates a fresh identity and grants only explicitly allowed existing objects.
    pub fn new(grants: &[Grant], cwd: &Path) -> io::Result<Self> {
        Self::build(grants, Some(cwd))
    }

    /// Adds the installer's embedded Git tree read-only without expanding model-selected grants.
    pub fn new_shell(grants: &[Grant], cwd: &Path) -> io::Result<Self> {
        let runtime = crate::windows_git::load()?;
        let mut sandbox = Self::new(grants, cwd)?;
        if let Some(runtime) = runtime {
            let root = platform_fs::guarded_acl_path(&runtime.root, true)?;
            for grant in grants.iter().filter(|grant| grant.blocked) {
                let blocked = platform_fs::permission_path(Path::new(&grant.path))?;
                if contains(&blocked, &root.key) || contains(&root.key, &blocked) {
                    return Err(io::ErrorKind::PermissionDenied.into());
                }
            }
            let _program = platform_fs::guarded_path(&runtime.root.join(&runtime.program), false, false)?;
            let _helpers = platform_fs::guarded_acl_path(&runtime.root.join(&runtime.exec_path), true)?;
            let identity = platform_fs::file_identity(&root.file)?;
            sandbox.roots.push(Grant { path: root.key.clone(), write: false, folder: true, blocked: false });
            sandbox.walk(&root.path, false, true, 0, identity)?;
            sandbox.pins.push(root);
            let empty_config = sandbox.scratch.join("git-empty-config");
            platform_fs::private_file(&empty_config, true)?.sync_all()?;
            sandbox.allow(&empty_config, false, false, false, None)?;
            sandbox.git_env = runtime.environment(&empty_config, grants)?;
        }
        let shell = crate::windows_shell::runtime()?;
        let shell_key = platform_fs::permission_path(&shell.program)?;
        for grant in grants.iter().filter(|grant| grant.blocked) {
            let blocked = platform_fs::permission_path(Path::new(&grant.path))?;
            if contains(&blocked, &shell_key) { return Err(io::ErrorKind::PermissionDenied.into()); }
        }
        sandbox.roots.push(Grant { path: shell_key, write: false, folder: false, blocked: false });
        sandbox.allow(&shell.program, false, false, false, None)?;
        sandbox.flush()?;
        // `node` typed in the shell or terminal is MiniClaw's private Node, never the user's.
        sandbox.add_private_node()?;
        sandbox.add_python_hook()?;
        Ok(sandbox)
    }

    /// Gives this run MiniClaw's private Node: read access to its folder, the folder first on
    /// `PATH`, and the OpenSSL variables that let it seed without the registry. Returns its
    /// `node.exe`, or `None` when this installation has no verified private Node; the run then
    /// goes on without it.
    pub fn add_private_node(&mut self) -> io::Result<Option<PathBuf>> {
        if let Some(node) = &self.node {
            return Ok(Some(node.program.clone()));
        }
        let node = match crate::windows_runtimes::private_node() {
            Ok(node) => node,
            Err(error) => {
                tracing::debug!(%error, "A sandboxed run has no private Node");
                return Ok(None);
            }
        };
        let root = platform_fs::guarded_acl_path(&node.folder, true)?;
        // An installation keeps these runtimes in its protected `runtime\bin`; only the run's own rules refuse them.
        if super::runtime_refused(&self.rules, &root.key) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("A rule of this run blocks MiniClaw's runtime folder {}", root.key)));
        }
        let identity = platform_fs::file_identity(&root.file)?;
        self.roots.push(Grant { path: root.key.clone(), write: false, folder: true, blocked: false });
        // The folder and its own files only: npm's tree below it goes to runs that use npm.
        self.allow(&root.path, false, true, false, Some(identity))?;
        for entry in fs::read_dir(&root.path)? {
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if platform_fs::is_link(&metadata) {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            if metadata.is_file() {
                self.allow(&entry.path(), false, false, false, Some(identity))?;
            }
        }
        self.pins.push(root);
        self.flush()?;
        let program = node.program.clone();
        self.node = Some(node);
        Ok(Some(program))
    }

    /// Gives this run MiniClaw's sitecustomize for Python, read-only: the launcher puts its folder
    /// first on `PYTHONPATH`, so asyncio programs can start (`windows_runtimes::PYTHON_HOOK`). An
    /// installation without a verified hook goes on without it.
    pub fn add_python_hook(&mut self) -> io::Result<()> {
        if self.python.is_some() {
            return Ok(());
        }
        let hook = match crate::windows_runtimes::python_hook() {
            Ok(hook) => hook,
            Err(error) => {
                tracing::debug!(%error, "A sandboxed run has no Python hook");
                return Ok(());
            }
        };
        let root = platform_fs::guarded_acl_path(&hook.folder, true)?;
        // An installation keeps these runtimes in its protected `runtime\bin`; only the run's own rules refuse them.
        if super::runtime_refused(&self.rules, &root.key) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("A rule of this run blocks MiniClaw's runtime folder {}", root.key)));
        }
        let identity = platform_fs::file_identity(&root.file)?;
        self.roots.push(Grant { path: root.key.clone(), write: false, folder: true, blocked: false });
        // The folder, so Python can list it, and the hook file; nothing that appears there later.
        self.allow(&root.path, false, true, false, Some(identity))?;
        self.allow(&hook.file, false, false, false, Some(identity))?;
        self.pins.push(root);
        self.flush()?;
        self.python = Some(hook);
        Ok(())
    }

    /// Gives this run MiniClaw's npm as well as its Node: read access to npm's tree, and npm
    /// settings that keep its configuration, cache and global folder in this run's scratch and
    /// run package scripts with BusyBox `sh`. Returns `None`, and grants nothing more, when this
    /// installation has no verified npm.
    pub fn add_private_npm(&mut self) -> io::Result<Option<std::sync::Arc<crate::windows_runtimes::PrivateNpm>>> {
        if let Some(npm) = &self.npm {
            return Ok(Some(npm.clone()));
        }
        if self.add_private_node()?.is_none() {
            return Ok(None);
        }
        let npm = match crate::windows_runtimes::private_npm() {
            Ok(npm) => npm,
            Err(error) => {
                tracing::debug!(%error, "A sandboxed run has no private npm");
                return Ok(None);
            }
        };
        // npm starts `sh.exe` from inside the sandbox. BusyBox's manifest sets the UTF-8 code page,
        // and Windows cannot build that activation context with the sandbox's token (14001, as for
        // Python); built here first, outside, Windows reuses it (P1, undocumented; best effort).
        let primed = crate::windows_runtimes::activation_context(&npm.shell, 1);
        tracing::debug!(primed, "Primed the script shell's activation context");
        let root = platform_fs::guarded_acl_path(&npm.root, true)?;
        // An installation keeps these runtimes in its protected `runtime\bin`; only the run's own rules refuse them.
        if super::runtime_refused(&self.rules, &root.key) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("A rule of this run blocks MiniClaw's runtime folder {}", root.key)));
        }
        let identity = platform_fs::file_identity(&root.file)?;
        self.allow(npm.root.parent().ok_or(io::ErrorKind::InvalidData)?, false, true, false, None)?;
        self.roots.push(Grant { path: root.key.clone(), write: false, folder: true, blocked: false });
        self.walk(&root.path, false, true, 0, identity)?;
        self.pins.push(root);
        self.flush()?;
        self.npm = Some(npm.clone());
        Ok(Some(npm))
    }

    /// Grants a folder MiniClaw itself owns inside its protected data folder, such as one MCP
    /// server's npm install, which no grant from a request may name. Read-only, or read-write
    /// with access passed to new children and recorded for cleanup like any writable grant.
    pub fn grant_owned(&mut self, path: &Path, write: bool) -> io::Result<()> {
        let root = platform_fs::guarded_acl_path(path, true)?;
        let identity = platform_fs::file_identity(&root.file)?;
        if write {
            self.journal.as_mut().ok_or(io::ErrorKind::InvalidData)?.inheritance_root(&root.path, &root.file)?;
        }
        self.roots.push(Grant { path: root.key.clone(), write, folder: true, blocked: false });
        self.walk(&root.path, write, true, 0, identity)?;
        self.pins.push(root);
        self.flush()
    }

    /// The program a sandboxed launch runs: MiniClaw's private Node in place of any `node.exe`,
    /// and the given program otherwise. Every launch through here also gets the private Node on
    /// `PATH`, for servers that start `node` themselves.
    pub fn program_for(&mut self, program: &Path) -> io::Result<PathBuf> {
        let private = self.add_private_node()?;
        self.add_python_hook()?;
        Ok(match private {
            Some(node) if crate::windows_runtimes::node_image(program) => node,
            _ => program.to_owned(),
        })
    }

    /// Gives a tool private working storage without granting a shared temporary tree.
    pub fn with_private_cwd(grants: &[Grant]) -> io::Result<Self> {
        Self::build(grants, None)
    }

    fn build(grants: &[Grant], cwd: Option<&Path>) -> io::Result<Self> {
        let protected = crate::seatbelt::protected();
        let mut normalized = Vec::new();
        for grant in grants {
            let path = platform_fs::permission_path(Path::new(&grant.path))?;
            normalized.push(Grant {
                path,
                ..grant.clone()
            });
        }
        let rules = super::rule_blocks(&normalized);
        let mut blocked = rules.clone();
        for path in protected {
            blocked.push(platform_fs::permission_path(Path::new(&path))?);
        }
        // ACL inheritance cannot represent future absent nested exclusions safely.
        for grant in normalized.iter().filter(|g| !g.blocked) {
            if blocked
                .iter()
                .any(|block| contains(&grant.path, block) || contains(block, &grant.path))
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Shell grants overlap a blocked or protected path.",
                ));
            }
        }
        if let Some(cwd) = cwd {
            let cwd_key = platform_fs::permission_path(cwd)?;
            if !super::Policy::from_normalized(normalized.clone()).allows(&cwd_key, false) {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
        }
        let mut sandbox = Self {
            name: Vec::new(),
            capabilities: SECURITY_CAPABILITIES {
                AppContainerSid: ptr::null_mut(),
                Capabilities: ptr::null_mut(),
                CapabilityCount: 0,
                Reserved: 0,
            },
            scratch: PathBuf::new(),
            access: Vec::new(),
            pending: Vec::new(),
            pins: Vec::new(),
            tree: None,
            journal: None,
            started: std::time::Instant::now(),
            git_env: Vec::new(),
            roots: normalized.iter().filter(|g| !g.blocked).cloned().collect(),
            blocked,
            rules,
            granted: None,
            networked: false,
            node: None,
            npm: None,
            python: None,
        };
        // Every granted object is opened for the access its entry needs before any record,
        // profile or entry exists. An item this user cannot manage refuses the launch here, so a
        // refused folder leaves nothing behind (no journal for recovery to resolve later).
        let mut inheritance = Vec::new();
        for grant in normalized.iter().filter(|g| !g.blocked) {
            let root = platform_fs::guarded_acl_path(Path::new(&grant.path), grant.folder)
                .map_err(|error| managed(error, Path::new(&grant.path)))?;
            let id = platform_fs::file_identity(&root.file)?;
            sandbox.walk(Path::new(&grant.path), grant.write, grant.folder, 0, id)?;
            if grant.write && grant.folder {
                inheritance.push(sandbox.pins.len());
            }
            sandbox.pins.push(root);
        }
        let name: Vec<u16> = format!("MiniClaw.Run.{}", uuid::Uuid::new_v4().simple())
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let profile = String::from_utf16_lossy(&name[..name.len() - 1]);
        let mut journal = journal::Journal::create(&profile)?;
        let mut sid = ptr::null_mut();
        let status = unsafe {
            CreateAppContainerProfile(
                name.as_ptr(),
                name.as_ptr(),
                name.as_ptr(),
                ptr::null(),
                0,
                &mut sid,
            )
        };
        if status < 0 {
            journal.complete()?;
            return Err(io::Error::other(format!(
                "CreateAppContainerProfile failed: {status:#x}"
            )));
        }
        // Writable roots are recorded before the first batch, which carries their inheritable entry.
        for index in inheritance {
            let root = &sandbox.pins[index];
            if let Err(error) = journal.inheritance_root(&root.path, &root.file) {
                sandbox.adopt(name, sid, journal);
                return Err(error);
            }
        }
        sandbox.adopt(name, sid, journal);
        // Offline runs read the Winsock catalog, so Winsock starts, but cannot create a socket.
        sandbox.set_capabilities(&offline_capabilities())?;
        let scratch = sandbox
            .journal
            .as_ref()
            .ok_or(io::ErrorKind::InvalidData)?
            .scratch()?;
        platform_fs::private_dir(&scratch, false)?;
        sandbox.scratch = scratch;
        sandbox.allow(&sandbox.scratch.clone(), true, true, true, None)?;
        sandbox.flush()?;
        Ok(sandbox)
    }

    /// Takes the run's profile and record: from here on, dropping the sandbox cleans them up.
    fn adopt(&mut self, name: Vec<u16>, sid: PSID, journal: journal::Journal) {
        self.name = name;
        self.capabilities.AppContainerSid = sid;
        self.journal = Some(journal);
    }

    /// Retains job identity so profile cleanup waits for every descendant, including cancellation.
    pub fn bind_process(&mut self, process: &crate::windows_process::Process) -> io::Result<()> {
        self.tree = Some(process.tree_guard());
        if let Some(journal) = self.journal.as_mut() {
            journal.bind(&process.job_descriptor().0)?;
        }
        Ok(())
    }

    /// Returns the unique profile name used to derive this run's AppContainer SID.
    pub fn profile_name(&self) -> String {
        String::from_utf16_lossy(&self.name[..self.name.len() - 1])
    }

    /// Returns the package SID for the authenticated network broker's token comparison.
    pub fn package_sid(&self) -> io::Result<String> {
        unsafe {
            let mut text = ptr::null_mut();
            if ConvertSidToStringSidW(self.capabilities.AppContainerSid, &mut text) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut length = 0;
            while *text.add(length) != 0 {
                length += 1;
            }
            let value = String::from_utf16(std::slice::from_raw_parts(text, length))
                .map_err(io::Error::other);
            LocalFree(text.cast());
            value
        }
    }

    /// Gives a run that will receive a network lease the two capabilities Winsock needs:
    /// `internetClient`, and this installation's capability for reading the Winsock catalog keys.
    ///
    /// Call it before the process is created, and resume the process only after the network
    /// service has granted its lease: the service's filters, not these capabilities, decide
    /// which endpoints are reachable. Runs without approved hosts never call it and keep only
    /// the Winsock-read capability, which allows no socket. `registryRead` is never added.
    pub fn enable_network(&mut self) -> io::Result<()> {
        if self.networked {
            return Ok(());
        }
        let custom = crate::windows_network_broker::winsock_capability()?;
        self.set_capabilities(&crate::windows_runtimes::token_capabilities(Some(&custom), true)?)?;
        self.networked = true;
        Ok(())
    }

    /// Replaces the token's capabilities with exactly these names.
    fn set_capabilities(&mut self, names: &[String]) -> io::Result<()> {
        let mut sids = Vec::with_capacity(names.len());
        for name in names {
            let bytes = crate::windows_winsock_acl::derive(name)?;
            let mut words = vec![0u32; bytes.len().div_ceil(4)];
            unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), words.as_mut_ptr().cast::<u8>(), bytes.len()) };
            sids.push(words);
        }
        // SE_GROUP_ENABLED from winnt.h.
        let entries = sids.iter_mut().map(|sid| SID_AND_ATTRIBUTES { Sid: sid.as_mut_ptr().cast(), Attributes: 0x4 }).collect::<Vec<_>>();
        let mut granted = Box::new(Granted { _sids: sids, entries });
        self.capabilities.Capabilities = if granted.entries.is_empty() { ptr::null_mut() } else { granted.entries.as_mut_ptr() };
        self.capabilities.CapabilityCount = granted.entries.len() as u32;
        self.granted = Some(granted);
        Ok(())
    }

    /// Borrows the token description: the Winsock-read capability (or none without an installed
    /// service), or the two a networked run was given.
    pub fn security(&self) -> SecurityContext<'_> {
        SecurityContext {
            capabilities: &self.capabilities,
            low_privilege: true,
        }
    }

    /// Returns the private directory available for command scripts and temporary files.
    pub fn scratch(&self) -> &Path {
        &self.scratch
    }

    /// Redirects mutable process state into this sandbox's disposable directory. `TMPDIR` is the
    /// name POSIX shell commands use for it; the bundled shell runs such commands.
    pub fn environment(&self) -> Vec<(OsString, OsString)> {
        let mut values: Vec<_> = [
            "TEMP",
            "TMP",
            "TMPDIR",
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
        ]
        .into_iter()
        .map(|name| (name.into(), self.scratch.as_os_str().to_owned()))
        .collect();
        values.extend(self.git_env.iter().cloned());
        if let Some(node) = &self.node {
            // The private Node goes first on the search path, before the embedded Git's folders.
            let path = values.iter().position(|(name, _)| name.eq_ignore_ascii_case("PATH"))
                .map(|index| values.remove(index).1).or_else(|| std::env::var_os("PATH")).unwrap_or_default();
            let mut folders = vec![node.folder.clone()];
            folders.extend(std::env::split_paths(&path));
            if let Ok(joined) = std::env::join_paths(folders) { values.push(("PATH".into(), joined)); }
            values.extend(node.environment());
        }
        if let Some(hook) = &self.python {
            // Read by the process launcher, which puts it first on PYTHONPATH (windows_process).
            values.push((crate::windows_runtimes::PYTHON_HOOK_FOLDER.into(), hook.folder.as_os_str().to_owned()));
        }
        if let Some(npm) = &self.npm {
            // npm's user configuration, cache (under LOCALAPPDATA) and global folder stay in this
            // run's scratch; package scripts run in BusyBox `sh`, never cmd.exe.
            values.extend([
                ("npm_config_script_shell".into(), npm.shell.as_os_str().to_owned()),
                ("npm_config_userconfig".into(), self.scratch.join(".npmrc").into_os_string()),
                ("npm_config_prefix".into(), self.scratch.join("npm-global").into_os_string()),
                ("npm_config_update_notifier".into(), "false".into()),
                // Scripts share npm's own standard handles: Node 24.21's libuv cannot create the
                // named pipes a piped child needs inside an AppContainer and retries forever.
                ("npm_config_foreground_scripts".into(), "true".into()),
            ]);
        }
        values
    }

    /// Grants existing descendants individually, never propagating an ACE through host paths.
    fn walk(
        &mut self,
        path: &Path,
        write: bool,
        directory: bool,
        depth: usize,
        root: (u64, u128),
    ) -> io::Result<()> {
        if depth > 64
            || self.access.len() + self.pending.len() >= 100_000
            || self.started.elapsed() > std::time::Duration::from_secs(120)
        {
            return Err(super::refused(
                "Sandbox folder preparation exceeds 100000 objects, 64 levels, or 120 seconds; select narrower folders.".into(),
            ));
        }
        self.allow(path, write, directory, write && directory, Some(root))?;
        if directory {
            for entry in fs::read_dir(path).map_err(|error| managed(error, path))? {
                let entry = entry.map_err(|error| managed(error, path))?;
                let metadata = fs::symlink_metadata(entry.path()).map_err(|error| managed(error, &entry.path()))?;
                if platform_fs::is_link(&metadata) {
                    return Err(io::ErrorKind::PermissionDenied.into());
                }
                self.walk(&entry.path(), write, metadata.is_dir(), depth + 1, root)?;
            }
        }
        Ok(())
    }

    /// Appends a unique package ACE; writable directories pass access to new children only.
    fn allow(
        &mut self,
        path: &Path,
        write: bool,
        directory: bool,
        inherit: bool,
        root: Option<(u64, u128)>,
    ) -> io::Result<()> {
        let pin = if directory {
            platform_fs::guarded_acl_path(path, true).map_err(|error| managed(error, path))?
        } else {
            let (pin, names) = platform_fs::guarded_linked_file(path).map_err(|error| managed(error, path))?;
            if names.len() > 1 {
                self.linked(path, &pin, &names, write)?;
            }
            pin
        };
        if root.is_some_and(|root| !pin.identities().is_ok_and(|ids| ids.contains(&root))) {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let file = acl_handle(&pin, directory).map_err(|error| managed(error, path))?;
        let rights = FILE_GENERIC_READ
            | FILE_GENERIC_EXECUTE
            | if write {
                FILE_GENERIC_WRITE | DELETE
            } else {
                0
            };
        self.pending.push(Pending {
            file,
            path: pin.path.clone(),
            directory,
            rights,
            inheritance: if inherit {
                SUB_CONTAINERS_AND_OBJECTS_INHERIT
            } else {
                NO_INHERITANCE
            },
        });
        if root.is_none() {
            self.pins.push(pin);
        }
        // Before the record exists (the walk of the requested grants) everything waits for it.
        if self.pending.len() >= GRANT_BATCH && self.journal.is_some() {
            self.flush()?;
        }
        Ok(())
    }

    /// Admits a hard-linked file only when the grant stays inside what the user approved.
    ///
    /// The ACL entry lands on the file and so applies under every name. All names must lie in
    /// this run's roots with at least this access, none under a blocked path, and each must
    /// open the pinned file. The error names the approved path only, never another name.
    fn linked(
        &self,
        path: &Path,
        pin: &platform_fs::GuardedPath,
        names: &[String],
        write: bool,
    ) -> io::Result<()> {
        if super::links_within(names, write, &self.roots, &self.blocked)
            && names.iter().all(|name| platform_fs::names_file(&pin.file, name))
        {
            return Ok(());
        }
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} has another hard link outside the folders granted to this run with the same access, or under a protected path. Remove the extra link, or grant the folder that holds it.",
                path.display()
            ),
        ))
    }

    /// Flushes the queued identities once, then applies their grants under one ACL lock.
    ///
    /// No queued object is changed before the flush returns, so every grant still has a durable
    /// record first. Constructors call this before returning a usable sandbox.
    fn flush(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        // Nothing is ever granted without a durable record of it.
        if self.journal.is_none() || self.capabilities.AppContainerSid.is_null() {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let mut pending = std::mem::take(&mut self.pending);
        // The walk of the requested grants queues everything at once; it is applied in batches.
        while !pending.is_empty() {
            let rest = pending.split_off(pending.len().min(GRANT_BATCH));
            let batch = std::mem::replace(&mut pending, rest);
            if let Some(journal) = self.journal.as_mut() {
                journal.record(batch.iter().map(|item| (item.path.as_path(), &item.file, item.directory)))?;
            }
            let _lock = crate::windows_security_lock::acquire(false)?;
            for item in batch {
                // The handle is kept even if this change fails, so cleanup still visits the object.
                let changed = change_acl_locked(
                    &item.file,
                    self.capabilities.AppContainerSid,
                    item.rights,
                    GRANT_ACCESS,
                    item.inheritance,
                );
                self.access.push(item.file);
                changed?;
            }
        }
        Ok(())
    }
}

impl Drop for Sandbox {
    /// Moves cleanup off the calling thread and waits for the owned process tree first.
    fn drop(&mut self) {
        // Refused while walking the requested grants: no profile, record or entry exists yet.
        if self.journal.is_none() && self.capabilities.AppContainerSid.is_null() {
            return;
        }
        let mut cleanup = Cleanup {
            name: std::mem::take(&mut self.name),
            sid: self.capabilities.AppContainerSid,
            scratch: std::mem::take(&mut self.scratch),
            access: std::mem::take(&mut self.access),
            pins: std::mem::take(&mut self.pins),
            journal: self.journal.take(),
        };
        let tree = self.tree.take();
        if let Err(error) = cleanup_queue().spawn(move || {
            if let Some(tree) = tree {
                if let Err(error) = tree.terminate_and_wait(std::time::Duration::from_secs(30)) {
                    tracing::error!(%error, "Sandbox tree remains uncertain; retaining its ACL and profile records");
                    return Err(error);
                }
                // The job dies with this handle; the record lets a failed cleanup be retried.
                if let Some(Err(error)) = cleanup.journal.as_mut().map(journal::Journal::job_empty) {
                    tracing::warn!(%error, "Could not record the empty sandbox job");
                }
            }
            cleanup.run()
        }) { tracing::error!(%error, "Could not schedule sandbox cleanup; retaining its ACL and profile records"); }
    }
}

struct Cleanup {
    name: Vec<u16>,
    sid: PSID,
    scratch: PathBuf,
    access: Vec<File>,
    pins: Vec<platform_fs::GuardedPath>,
    journal: Option<journal::Journal>,
}
// This allocation and its handles have one owner on the cleanup thread.
unsafe impl Send for Cleanup {}

impl Cleanup {
    /// Removes only this package's ACEs after the associated job has no active processes.
    fn run(mut self) -> io::Result<()> {
        let mut complete = true;
        if let Some(journal) = self.journal.as_mut() {
            if let Err(error) = journal.revoke_descendants(self.sid) {
                complete = false;
                tracing::warn!(%error, "Retaining journal after inherited descendant cleanup failure");
            }
        }
        let mut batch = journal::Batch::default();
        for file in self.access.iter().rev() {
            if let Err(error) = journal::strip(file, self.sid, &mut batch) {
                complete = false;
                tracing::warn!(%error, "Could not remove expired sandbox ACL entry");
            }
        }
        drop(batch);
        self.access.clear();
        self.pins.clear();
        if !self.scratch.as_os_str().is_empty() {
            if let Err(error) = journal::remove_scratch(&self.scratch) {
                complete = false;
                tracing::warn!(%error, "Retaining sandbox journal after scratch cleanup failure");
            }
        }
        unsafe {
            let status = DeleteAppContainerProfile(self.name.as_ptr());
            if status < 0 {
                complete = false;
                tracing::warn!(status, "Could not remove expired AppContainer profile");
            }
            FreeSid(self.sid);
        }
        if complete {
            if let Some(journal) = self.journal.take() {
                journal.complete()?;
            }
        }
        if complete { Ok(()) } else { Err(io::Error::other("Sandbox resource cleanup remains incomplete")) }
    }
}

/// Names an item this user cannot open for the access a grant needs (access denied), as a refusal
/// for the user; every other error is kept as it is.
fn managed(error: io::Error, path: &Path) -> io::Error {
    if error.raw_os_error() == Some(5) { super::unmanageable(path) } else { error }
}

/// Opens ACL access while proving the handle is the already pinned object.
fn acl_handle(pin: &platform_fs::GuardedPath, directory: bool) -> io::Result<File> {
    let wide: Vec<u16> = pin.path.as_os_str().encode_wide().chain(Some(0)).collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            READ_CONTROL | WRITE_DAC | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    0
                },
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(handle) };
    if platform_fs::file_identity(&pin.file)? != platform_fs::file_identity(&file)? {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(file)
}

/// Adds or removes one package trustee while preserving all other current ACL entries.
#[cfg(test)]
fn change_acl(
    file: &File,
    sid: PSID,
    rights: u32,
    mode: ACCESS_MODE,
    inheritance: u32,
) -> io::Result<()> {
    let _lock = crate::windows_security_lock::acquire(false)?;
    change_acl_locked(file, sid, rights, mode, inheritance)
}

/// Performs one ACL change for a caller that already holds the cross-process ACL lock.
fn change_acl_locked(
    file: &File,
    sid: PSID,
    rights: u32,
    mode: ACCESS_MODE,
    inheritance: u32,
) -> io::Result<()> {
    write_acl(file, sid, rights, mode, inheritance, true)
}

/// Removes the run's entries with a plain write, for an object whose inherited entry survived a
/// write that carried the auto-inherit request. The object loses its auto-inherited marker.
pub(super) fn revoke_plainly(file: &File, sid: PSID) -> io::Result<()> {
    write_acl(file, sid, 0, REVOKE_ACCESS, NO_INHERITANCE, false)
}

/// Reads the access list the kernel stores for this object, with the descriptor's control bits.
/// `None` means the object has no list at all.
///
/// `GetSecurityInfo` is not used for this. For an object without the auto-inherited marker it
/// infers inheritance from the parent folder's list as it is now; once the parent carries a
/// run's inheritable entry, an existing file's entries no longer match it and are reported as
/// explicit entries on a protected list. Writing that inferred view back made it true: after
/// one grant and revoke `D:(A;ID;..)` had become `D:P(A;;..)` (observed natively), so a later
/// permission change on the folder would not have reached the file.
pub(super) fn stored_acl(file: &File) -> io::Result<Option<(Vec<u8>, u16)>> {
    unsafe {
        let mut needed = 0;
        GetKernelObjectSecurity(file.as_raw_handle(), DACL_SECURITY_INFORMATION, ptr::null_mut(), 0, &mut needed);
        if needed == 0 || needed > 1 << 20 {
            return Err(io::Error::last_os_error());
        }
        let mut storage = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
        let descriptor = storage.as_mut_ptr().cast();
        if GetKernelObjectSecurity(file.as_raw_handle(), DACL_SECURITY_INFORMATION, descriptor, needed, &mut needed) == 0 {
            return Err(io::Error::last_os_error());
        }
        let (mut control, mut revision) = (0, 0);
        let (mut present, mut defaulted) = (0, 0);
        let mut dacl: *mut ACL = ptr::null_mut();
        if GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) == 0
            || GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) == 0
        {
            return Err(io::Error::last_os_error());
        }
        if present == 0 || dacl.is_null() {
            return Ok(None);
        }
        let bytes = std::slice::from_raw_parts(dacl.cast::<u8>(), usize::from((*dacl).AclSize)).to_vec();
        Ok(Some((bytes, control)))
    }
}

/// Reads the object's stored access list, edits the run's entry in a byte-exact copy and writes
/// it back to this handle only. `request` carries the object's auto-inherit marker through the write.
fn write_acl(
    file: &File,
    sid: PSID,
    rights: u32,
    mode: ACCESS_MODE,
    inheritance: u32,
    request: bool,
) -> io::Result<()> {
    if mode != GRANT_ACCESS && mode != REVOKE_ACCESS {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let Some((current, control)) = stored_acl(file)? else {
        return Err(io::ErrorKind::PermissionDenied.into());
    };
    unsafe {
        // Every other entry is copied byte for byte; see `acl` for why SetEntriesInAclW is not used.
        let trustee = std::slice::from_raw_parts(sid.cast::<u8>(), GetLengthSid(sid) as usize);
        let replacement = if mode == REVOKE_ACCESS {
            super::acl::without_trustee(&current, trustee)?
        } else {
            let flags = u8::try_from(inheritance).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
            super::acl::with_trustee(&current, trustee, rights, flags)?
        };
        // NtSetSecurityObject changes this handle only. SetSecurityInfo recursively
        // propagates existing inheritable ACEs, including through unchecked descendants.
        let mut exact: SECURITY_DESCRIPTOR = std::mem::zeroed();
        let initialized =
            InitializeSecurityDescriptor((&mut exact as *mut SECURITY_DESCRIPTOR).cast(), 1);
        let mask = SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED | SE_DACL_AUTO_INHERIT_REQ;
        let mut bits = control & mask;
        // A stored descriptor never carries the request bit, and the kernel keeps
        // SE_DACL_AUTO_INHERITED only when the request accompanies it. Without this the flag
        // is silently cleared (observed natively). The kernel itself never propagates.
        if control & SE_DACL_AUTO_INHERITED != 0 {
            bits |= SE_DACL_AUTO_INHERIT_REQ;
        }
        if !request {
            bits &= SE_DACL_PROTECTED;
        }
        let preserved = SetSecurityDescriptorControl(
            (&mut exact as *mut SECURITY_DESCRIPTOR).cast(),
            mask,
            bits,
        );
        let attached = SetSecurityDescriptorDacl(
            (&mut exact as *mut SECURITY_DESCRIPTOR).cast(),
            1,
            replacement.as_ptr().cast::<ACL>().cast_mut(),
            i32::from(control & SE_DACL_DEFAULTED != 0),
        );
        if initialized == 0 || attached == 0 || preserved == 0 {
            return Err(io::Error::last_os_error());
        }
        let status = windows_sys::Wdk::Storage::FileSystem::NtSetSecurityObject(
            file.as_raw_handle(),
            DACL_SECURITY_INFORMATION,
            (&mut exact as *mut SECURITY_DESCRIPTOR).cast(),
        );
        if status < 0 {
            return Err(io::Error::other(format!("NtSetSecurityObject failed: {status:#x}")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn control(file: &File) -> u16 {
        unsafe {
            let mut descriptor = ptr::null_mut();
            assert_eq!(
                GetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &mut descriptor
                ),
                0
            );
            let mut control = 0;
            let mut revision = 0;
            assert_ne!(
                GetSecurityDescriptorControl(descriptor, &mut control, &mut revision),
                0
            );
            LocalFree(descriptor);
            control
        }
    }

    fn dacl(file: &File) -> Vec<u8> {
        unsafe {
            let mut acl = ptr::null_mut();
            let mut descriptor = ptr::null_mut();
            assert_eq!(
                GetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &mut acl,
                    ptr::null_mut(),
                    &mut descriptor
                ),
                0
            );
            let bytes =
                std::slice::from_raw_parts(acl.cast::<u8>(), (*acl).AclSize as usize).to_vec();
            LocalFree(descriptor);
            bytes
        }
    }

    #[test]
    fn acl_change_does_not_rewrite_existing_children() {
        let root = std::env::temp_dir().join(format!("mc-acl-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&root, false).unwrap();
        drop(platform_fs::private_file(&root.join("child"), true).unwrap());
        let child_path: Vec<u16> = root
            .join("child")
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let raw = unsafe {
            CreateFileW(
                child_path.as_ptr(),
                READ_CONTROL | WRITE_DAC,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };
        assert_ne!(raw, INVALID_HANDLE_VALUE);
        let child = unsafe { File::from_raw_handle(raw) };
        let mut original = dacl(&child);
        assert_eq!(
            unsafe {
                SetSecurityInfo(
                    child.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | UNPROTECTED_DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    original.as_mut_ptr().cast(),
                    ptr::null(),
                )
            },
            0
        );
        let before = dacl(&child);
        let wide: Vec<u16> = root.as_os_str().encode_wide().chain(Some(0)).collect();
        let raw = unsafe {
            CreateFileW(
                wide.as_ptr(),
                READ_CONTROL | WRITE_DAC,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                ptr::null_mut(),
            )
        };
        assert_ne!(raw, INVALID_HANDLE_VALUE);
        let parent = unsafe { File::from_raw_handle(raw) };
        let original_control = control(&parent);
        let sid_text: Vec<u16> = "S-1-15-2-1\0".encode_utf16().collect();
        let mut sid = ptr::null_mut();
        assert_ne!(
            unsafe { ConvertStringSidToSidW(sid_text.as_ptr(), &mut sid) },
            0
        );
        change_acl(
            &parent,
            sid,
            FILE_GENERIC_READ,
            GRANT_ACCESS,
            SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        )
        .unwrap();
        assert_eq!(original_control, control(&parent));
        assert_eq!(
            before,
            dacl(&child),
            "Existing child DACL must remain byte-for-byte unchanged"
        );
        change_acl(&parent, sid, 0, REVOKE_ACCESS, NO_INHERITANCE).unwrap();
        assert_eq!(original_control, control(&parent));
        unsafe {
            LocalFree(sid);
        }
        drop((child, parent));
        fs::remove_dir_all(root).unwrap();
    }

    /// Opens an ordinary object for the same ACL access the sandbox uses.
    fn acl_file(path: &Path) -> File {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let raw = unsafe {
            CreateFileW(
                wide.as_ptr(),
                READ_CONTROL | WRITE_DAC,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                ptr::null_mut(),
            )
        };
        assert_ne!(raw, INVALID_HANDLE_VALUE);
        unsafe { File::from_raw_handle(raw) }
    }

    /// Later inheritance updates by Windows tools depend on this flag staying set.
    #[test]
    fn inherited_objects_keep_auto_inherited_flag() {
        let root = std::env::temp_dir().join(format!("mc-acl-ai-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("file.txt"), b"ordinary").unwrap();
        let sid_text: Vec<u16> = "S-1-15-2-1\0".encode_utf16().collect();
        let mut sid = ptr::null_mut();
        assert_ne!(
            unsafe { ConvertStringSidToSidW(sid_text.as_ptr(), &mut sid) },
            0
        );
        for (path, inheritance) in [
            (root.clone(), SUB_CONTAINERS_AND_OBJECTS_INHERIT),
            (root.join("file.txt"), NO_INHERITANCE),
        ] {
            let object = acl_file(&path);
            // A freshly created object lacks the flag; Explorer, icacls and SetSecurityInfo set
            // it when they apply inheritance, which is the state of ordinary user folders.
            let mut current = dacl(&object);
            assert_eq!(
                unsafe {
                    SetSecurityInfo(
                        object.as_raw_handle(),
                        SE_FILE_OBJECT,
                        DACL_SECURITY_INFORMATION | UNPROTECTED_DACL_SECURITY_INFORMATION,
                        ptr::null_mut(),
                        ptr::null_mut(),
                        current.as_mut_ptr().cast(),
                        ptr::null(),
                    )
                },
                0
            );
            let before = control(&object);
            let original = dacl(&object);
            assert_ne!(
                before & SE_DACL_AUTO_INHERITED,
                0,
                "Fixture must be an ordinary inherited object: {}",
                path.display()
            );
            change_acl(&object, sid, FILE_GENERIC_READ, GRANT_ACCESS, inheritance).unwrap();
            assert_eq!(control(&object), before, "Grant changed control bits");
            assert_ne!(dacl(&object), original);
            change_acl(&object, sid, 0, REVOKE_ACCESS, NO_INHERITANCE).unwrap();
            assert_eq!(control(&object), before, "Revoke changed control bits");
            assert_eq!(dacl(&object), original, "Revoke must restore the DACL");
        }
        unsafe {
            LocalFree(sid);
        }
        fs::remove_dir_all(root).unwrap();
    }
}
