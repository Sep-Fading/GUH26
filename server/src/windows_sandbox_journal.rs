//! Durable, private records for interrupted AppContainer ACL cleanup.
use super::*;
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};

#[path = "windows_sandbox_retirement.rs"]
mod retirement;
use retirement::{Proof, Retirement, ensure_unused_profile};
#[path = "windows_sandbox_revoke.rs"]
mod revoke;
pub(super) use revoke::{Batch, remove_scratch, strip};
use revoke::{Ledger, reopen};
#[path = "windows_sandbox_liveness.rs"]
mod liveness;

/// Bytes a record file may reach: its records plus a terminal certificate that repeats them.
const FILE_BYTES: u64 = 3 * revoke::RECORD_BYTES;
/// Lines a record file may hold; writers stop well below this.
const FILE_LINES: usize = 400_000;

#[derive(Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    path: PathBuf,
    identity: (u64, u128),
    directory: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    inheritance_root: bool,
    /// Found by a cleanup walk under a writable root, never granted by MiniClaw: the run's entry
    /// can be on it only by inheritance or by whoever created it, not by a grant.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    discovered: bool,
}

pub(super) struct Journal {
    file: File,
    path: PathBuf,
    inheritance: Vec<Entry>,
    ledger: Ledger,
    job: Option<String>,
}

/// Replays durable state after startup establishes the private application data directory.
///
/// The outcome is kept for the gate: while records stay unresolved, sandboxed work is refused
/// with this reason and everything else keeps working.
pub(super) fn recover_default() -> io::Result<()> {
    let data = std::env::var_os("MINICLAW_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("MiniClaw")))
        .ok_or(io::ErrorKind::NotFound)?;
    let root = data.join("sandbox-journals");
    let result = platform_fs::private_dir(&root, true).and_then(|()| recover(&root));
    super::super::set_recovery(result.as_ref().err().map(ToString::to_string));
    result
}

/// Appends one control line and flushes it before the step it describes.
fn control(file: &mut File, value: &serde_json::Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    file.seek(SeekFrom::End(0))?;
    file.write_all(&bytes)?;
    file.sync_all()
}

impl Journal {
    /// Locks a new record before any temporary host permission is added.
    pub(super) fn create(profile: &str) -> io::Result<Self> {
        let data = std::env::var_os("MINICLAW_DATA_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("MiniClaw")))
            .ok_or(io::ErrorKind::NotFound)?;
        let root = data.join("sandbox-journals");
        platform_fs::private_dir(&root, true)?;
        let admitted = recover_active(&root);
        super::super::set_recovery(admitted.as_ref().err().map(ToString::to_string));
        admitted?;
        ensure_unused_profile(&root, profile)?;
        let path = root.join(format!("{profile}.jsonl"));
        let boot = crate::windows_network_broker::verified_boot_epoch()?;
        let mut file = platform_fs::private_file(&path, true)?;
        let started = file
            .try_lock()
            .map_err(io::Error::other)
            .and_then(|()| control(&mut file, &serde_json::json!({"boot": boot})));
        if let Err(error) = started {
            // Nothing was granted yet, so the unusable record is removed rather than left behind.
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        let mut ledger = Ledger::default();
        ledger.line();
        Ok(Self {
            file,
            path,
            inheritance: Vec::new(),
            ledger,
            job: None,
        })
    }

    /// Keeps disposable state inside the protected application namespace.
    pub(super) fn scratch(&self) -> io::Result<PathBuf> {
        let data = self
            .path
            .parent()
            .and_then(Path::parent)
            .ok_or(io::ErrorKind::InvalidInput)?;
        let parent = data.join("sandbox-scratch");
        platform_fs::private_dir(&parent, true)?;
        Ok(parent.join(self.path.file_stem().ok_or(io::ErrorKind::InvalidInput)?))
    }

    /// Persists the job binding before an isolated process can resume.
    pub(super) fn bind(&mut self, name: &str) -> io::Result<()> {
        control(&mut self.file, &serde_json::json!({"job": name}))?;
        self.ledger.line();
        self.job = Some(name.to_owned());
        Ok(())
    }

    /// Persists that the bound job was seen with no process, so a retry needs no new proof.
    pub(super) fn job_empty(&mut self) -> io::Result<()> {
        let Some(job) = self.job.as_ref() else {
            return Ok(());
        };
        control(&mut self.file, &serde_json::json!({"job_empty": job}))?;
        self.ledger.line();
        Ok(())
    }

    /// Flushes a batch of object identities before any of their ACL changes can occur.
    pub(super) fn record<'a>(
        &mut self,
        objects: impl Iterator<Item = (&'a Path, &'a File, bool)>,
    ) -> io::Result<()> {
        let entries = objects
            .map(|(path, file, directory)| {
                Ok(Entry {
                    path: path.to_owned(),
                    identity: platform_fs::file_identity(file)?,
                    directory,
                    inheritance_root: false,
                    discovered: false,
                })
            })
            .collect::<io::Result<Vec<_>>>()?;
        self.ledger.append(&mut self.file, &entries, true)
    }

    /// Persists a writable root before it can pass the run SID to future children.
    pub(super) fn inheritance_root(&mut self, path: &Path, file: &File) -> io::Result<()> {
        let entry = Entry {
            path: path.to_owned(),
            identity: platform_fs::file_identity(file)?,
            directory: true,
            inheritance_root: true,
            discovered: false,
        };
        self.ledger
            .append(&mut self.file, std::slice::from_ref(&entry), true)?;
        self.inheritance.push(entry);
        Ok(())
    }

    /// Revokes reachable inherited entries only after the associated job has stopped.
    ///
    /// A descendant this user cannot open is not proven clean here: the record is kept, and
    /// recovery decides about it with the job's proof and the profile's deletion in hand.
    pub(super) fn revoke_descendants(&mut self, sid: PSID) -> io::Result<()> {
        let mut failure = None;
        for entry in self.inheritance.clone() {
            let result = reopen(&entry).and_then(|root| match root {
                Some(root) => revoke_root(root, &entry, &mut self.file, &mut self.ledger, sid).and_then(|unreadable| {
                    match unreadable.first() {
                        Some(first) => Err(io::Error::other(format!(
                            "{} cannot be opened by this Windows user to remove the run's access entry",
                            first.path.display()
                        ))),
                        None => Ok(()),
                    }
                }),
                None => Ok(()),
            });
            if let Err(error) = result {
                failure = Some(error);
            }
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Removes the durable record only after successful object and profile cleanup.
    ///
    /// The name is deleted while this handle still holds the record and its lock (the record was
    /// created with delete sharing), so another launch's admission scan never finds a finished
    /// record it could open and lock: the record is held (and skipped), its name is marked for
    /// deletion, or it is gone. Releasing first and deleting after let a scan open the finished
    /// record in between, and the scan or the deletion then failed
    /// (`records_completed_during_admission_are_not_unresolved`).
    pub(super) fn complete(self) -> io::Result<()> {
        let removed = fs::remove_file(&self.path);
        drop(self);
        removed
    }
}

/// Opens a listed active record. A name whose file is marked for deletion stays listed until its
/// last handle closes, and opening it meanwhile fails with access denied (`STATUS_DELETE_PENDING`);
/// the owner that finished the record closes that handle at once. So an access-denied open is
/// tried again for a moment: the name is then gone (finished), opens (an ordinary record), or is
/// still refused (unresolved, as before). Path metadata cannot tell this apart: the standard
/// library answers it from the folder listing, which still shows the name.
fn open_active(path: &Path) -> io::Result<File> {
    for _ in 0..50 {
        match platform_fs::open_existing(path, true, false) {
            Err(error) if error.raw_os_error() == Some(5) => std::thread::sleep(std::time::Duration::from_millis(10)),
            other => return other,
        }
    }
    platform_fs::open_existing(path, true, false)
}

/// Flushes an identity record before the change it describes.
#[cfg(test)]
fn append_entry(file: &mut File, entry: &Entry) -> io::Result<()> {
    Ledger::default().append(file, std::slice::from_ref(entry), true)
}

/// Resolves the current name of an identity-opened object without trusting its old path.
fn current_path(file: &File) -> io::Result<PathBuf> {
    let mut text = vec![0u16; 32768];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            text.as_mut_ptr(),
            text.len() as u32,
            FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
        )
    } as usize;
    if length == 0 {
        return Err(io::Error::last_os_error());
    }
    if length >= text.len() {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let value = String::from_utf16(&text[..length]).map_err(io::Error::other)?;
    let value = value
        .strip_prefix(r"\\?\")
        .ok_or(io::ErrorKind::InvalidData)?;
    if !platform_fs::plain_windows_path(value) {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(value.into())
}

/// Walks a retained root from its verified handle; its current name only labels new records.
fn revoke_root(
    root: File,
    saved: &Entry,
    journal: &mut File,
    ledger: &mut Ledger,
    sid: PSID,
) -> io::Result<Vec<Entry>> {
    let path = current_path(&root).unwrap_or_else(|_| saved.path.clone());
    revoke::tree(root, path, journal, ledger, sid)
}

/// What recovering one record left uncertain.
#[derive(Default)]
struct Left {
    /// The recorded identity could not be reopened (error 87); see "Retained unknown identities".
    unknown: bool,
    /// Objects this user cannot open for ACL access that MiniClaw never granted.
    unreadable: Vec<Entry>,
}

/// Revokes verified objects; an unsupported stale-ID result remains an explicit uncertainty.
///
/// An object MiniClaw granted that this user can no longer open is an error: its entry was
/// written and cannot be checked. One it never granted (`discovered`) is reported instead.
fn recover_object(
    saved: &Entry,
    journal: &mut File,
    ledger: &mut Ledger,
    sid: PSID,
) -> io::Result<Left> {
    let failed = |error: io::Error| io::Error::new(error.kind(), format!("{}: {error}", saved.path.display()));
    let object = match reopen(saved) {
        Ok(value) => value,
        Err(error) if error.raw_os_error() == Some(87) => match revoke::by_path(saved).map_err(failed)? {
            Some(file) => Some(file),
            None => return Ok(Left { unknown: true, ..Left::default() }),
        },
        Err(error) if error.raw_os_error() == Some(5) && saved.discovered => {
            return Ok(Left { unreadable: vec![saved.clone()], ..Left::default() });
        }
        Err(error) => return Err(failed(error)),
    };
    let Some(file) = object else {
        return Ok(Left::default());
    };
    if saved.inheritance_root {
        let unreadable = revoke_root(file, saved, journal, ledger, sid)?;
        return Ok(Left { unreadable, ..Left::default() });
    }
    strip(&file, sid, &mut Batch::default()).map_err(failed)?;
    Ok(Left::default())
}

/// Replays only orphan records and never applies a stale whole security descriptor.
fn recover(root: &Path) -> io::Result<()> {
    recover_with_profile_delete(root, |wide| unsafe {
        DeleteAppContainerProfile(wide.as_ptr())
    })
}

/// Launch admission checks active cleanup and reserves old profile names separately.
fn recover_active(root: &Path) -> io::Result<()> {
    recover_directory(
        root,
        &|wide| unsafe { DeleteAppContainerProfile(wide.as_ptr()) },
        false,
    )
}

/// Keeps the profile deletion result inside the recovery transaction's admission boundary.
fn recover_with_profile_delete(
    root: &Path,
    delete_profile: impl Fn(&[u16]) -> i32,
) -> io::Result<()> {
    let retired = root.join("retired");
    match fs::symlink_metadata(&retired) {
        Ok(_) => {
            platform_fs::private_dir(&retired, true)?;
            recover_directory(&retired, &delete_profile, true)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    recover_directory(root, &delete_profile, false)
}

#[derive(Default)]
struct Parsed {
    boot: Option<crate::windows_network_broker::BootEpoch>,
    job: Option<String>,
    job_empty: bool,
    records: Vec<Entry>,
    retirement: Option<Retirement>,
    lines: usize,
    /// Length of the complete lines when an unterminated tail follows them.
    torn: Option<u64>,
}

/// Reads complete lines strictly; `None` means the content cannot be trusted.
///
/// Each line is flushed before the change it describes, so an unterminated final line was never
/// acted on and is only reported, not parsed. Damage followed by further data is corruption.
fn parse(file: &File, deadline: std::time::Instant) -> io::Result<Option<Parsed>> {
    let mut parsed = Parsed::default();
    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(0))?;
    let mut offset = 0u64;
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            return Ok(Some(parsed));
        }
        if line.last() != Some(&b'\n') {
            parsed.torn = Some(offset);
            return Ok(Some(parsed));
        }
        offset += read as u64;
        line.pop();
        let index = parsed.lines;
        parsed.lines += 1;
        if parsed.lines > FILE_LINES
            || std::time::Instant::now() > deadline
            || parsed.retirement.is_some()
        {
            return Ok(None);
        }
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&line) else {
            return Ok(None);
        };
        let single = |name: &str| {
            value
                .get(name)
                .filter(|_| value.as_object().is_some_and(|object| object.len() == 1))
        };
        let control = ["boot", "job", "job_empty", "retirement"]
            .iter()
            .any(|name| value.get(name).is_some());
        if let Some(value) = single("boot") {
            let Ok(boot) = serde_json::from_value(value.clone()) else {
                return Ok(None);
            };
            if index != 0 {
                return Ok(None);
            }
            parsed.boot = Some(boot);
        } else if let Some(value) = single("job") {
            if parsed.job.is_some() || value.as_str().is_none() {
                return Ok(None);
            }
            parsed.job = value.as_str().map(str::to_owned);
        } else if let Some(value) = single("job_empty") {
            // Only the job this record bound can be declared empty.
            if parsed.job.is_none() || value.as_str() != parsed.job.as_deref() {
                return Ok(None);
            }
            parsed.job_empty = true;
        } else if single("retirement").is_some() {
            let Ok(certificate) = retirement::parse(&line) else {
                return Ok(None);
            };
            parsed.retirement = Some(certificate);
        } else {
            if control || line.len() > 32768 {
                return Ok(None);
            }
            let Ok(entry) = serde_json::from_slice::<Entry>(&line) else {
                return Ok(None);
            };
            parsed.records.push(entry);
        }
    }
}

/// Establishes that no process of a bound job can still run, or explains why that is unproven.
fn termination(
    job: &str,
    profile: &str,
    boot: &crate::windows_network_broker::BootEpoch,
    sid: PSID,
) -> io::Result<(Proof, bool)> {
    match crate::windows_process::TreeGuard::open_verified(job, profile) {
        Ok(tree) => {
            tree.terminate_and_wait(std::time::Duration::from_secs(30))?;
            Ok((Proof::JobEmpty { job: job.to_owned() }, true))
        }
        Err(error) if error.raw_os_error() == Some(2) => {
            // The name is gone, so the last handle closed and Windows began killing every member.
            if let Ok(current) = crate::windows_network_broker::verified_boot_epoch()
                && current.supersedes(boot)
            {
                return Ok((Proof::LaterBoot { job: job.to_owned(), current }, false));
            }
            // The SYSTEM service can inspect every process; this account often cannot. Without
            // its attestation this account's own view applies, which a live member also defeats.
            match crate::windows_network_broker::attest_job_dead(profile, job) {
                Ok(()) => return Ok((Proof::JobClosed { job: job.to_owned() }, false)),
                Err(error) => {
                    tracing::debug!(%error, "No service attestation; checking processes as this account");
                }
            }
            liveness::no_survivor(sid, std::time::Duration::from_secs(15)).map_err(|error| {
                io::Error::other(format!(
                    "Sandbox job termination cannot be proved in this boot ({error}); restart Windows to complete recovery."
                ))
            })?;
            Ok((Proof::JobClosed { job: job.to_owned() }, false))
        }
        Err(error) => Err(error),
    }
}

/// Counts records that stay unresolved and keeps the first reason, in words for the user.
#[derive(Default)]
struct Unresolved {
    count: usize,
    first: Option<String>,
}

impl Unresolved {
    /// Retains one record, remembering why if it is the first.
    fn retain(&mut self, profile: &str, why: impl FnOnce() -> String) {
        self.count += 1;
        if self.first.is_none() {
            self.first = Some(format!("{profile}: {}", why()));
        }
    }
}

/// Bounds active recovery separately from retained historical identity evidence.
fn recover_directory(
    root: &Path,
    delete_profile: &dyn Fn(&[u16]) -> i32,
    archives_only: bool,
) -> io::Result<()> {
    let mut unresolved = Unresolved::default();
    let mut active = 0;
    let started_scan = std::time::Instant::now();
    let mut deadline = started_scan + std::time::Duration::from_secs(120);
    for entry in fs::read_dir(root)? {
        if std::time::Instant::now() > deadline {
            return Err(io::Error::other(
                "Sandbox journal validation exceeded its time bound",
            ));
        }
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !archives_only && name == "retired" {
            continue;
        }
        let archived = name.ends_with(".retired.jsonl");
        if archives_only && !archived {
            unresolved.retain(name, || "an unexpected file among the retired records".into());
            continue;
        }
        let Some(profile) = name.strip_suffix(if archived { ".retired.jsonl" } else { ".jsonl" })
        else {
            continue;
        };
        let Some(id) = profile.strip_prefix("MiniClaw.Run.") else {
            if archives_only {
                unresolved.retain(name, || "an unexpected file among the retired records".into());
            }
            continue;
        };
        if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            if archives_only {
                unresolved.retain(name, || "an unexpected file among the retired records".into());
            }
            continue;
        }
        if !archives_only {
            active += 1;
            if active > 1000 {
                unresolved.retain(profile, || "more than 1000 records are waiting".into());
                break;
            }
        }
        let opened = if archived { platform_fs::open_existing(&entry.path(), false, false) } else { open_active(&entry.path()) };
        let mut file = match opened {
            Ok(file) => file,
            Err(error) if error.raw_os_error() == Some(32) => {
                if archives_only {
                    unresolved.retain(profile, || "the retired record is in use by another program".into());
                }
                continue;
            }
            // Listed, then gone: its owner, or another launch's scan, finished it meanwhile.
            Err(error) if error.kind() == io::ErrorKind::NotFound && !archived => continue,
            Err(error) => {
                tracing::warn!(%error, profile, "Retaining a sandbox record that could not be opened");
                unresolved.retain(profile, || format!("the record could not be opened ({error})"));
                continue;
            }
        };
        if !platform_fs::private_regular_file(&file)? {
            unresolved.retain(profile, || "the record is not a private file".into());
            continue;
        }
        if !archived && file.try_lock().is_err() {
            continue;
        }
        if file.metadata()?.len() > FILE_BYTES {
            unresolved.retain(profile, || "the record is larger than its limit".into());
            continue;
        }
        let Some(parsed) = parse(&file, deadline)? else {
            unresolved.retain(profile, || "the record is damaged".into());
            continue;
        };
        if let Some(length) = parsed.torn {
            // An archive is complete by construction; a torn one has been altered.
            if archived {
                unresolved.retain(profile, || "the retired record was altered".into());
                continue;
            }
            // The tail was never acted on. Drop it so later appends start on a line boundary.
            tracing::warn!(profile, "Discarding an unterminated final sandbox journal line");
            file.set_len(length)?;
            file.sync_all()?;
        }
        let Parsed { boot, job, job_empty, records, retirement, lines, .. } = parsed;
        // A released record lock alone does not prove all job descendants stopped.
        let Some(boot) = boot else {
            // No complete line means the launch stopped before its profile, scratch or any ACL
            // change existed. Any other headerless content is not a record this code wrote.
            if lines != 0 || archived {
                unresolved.retain(profile, || "the record has no header".into());
                continue;
            }
            drop(file);
            if !remove_resolved(&entry.path()) {
                unresolved.retain(profile, || "the finished record could not be removed".into());
            }
            continue;
        };
        if let Some(certificate) = retirement {
            if certificate
                .validate(
                    profile,
                    &boot,
                    job.as_deref(),
                    &records,
                    // History has one shared budget; an active record gets its own.
                    if archives_only { started_scan } else { std::time::Instant::now() },
                )
                .is_err()
            {
                unresolved.retain(profile, || "its retirement certificate is not valid".into());
                continue;
            }
            if !archived {
                drop(file);
                if retirement::archive(&entry.path(), profile).is_err() {
                    unresolved.retain(profile, || "the retired record could not be archived".into());
                }
            }
            continue;
        }
        if archived {
            unresolved.retain(profile, || "the retired record has no certificate".into());
            continue;
        }
        let wide: Vec<u16> = profile.encode_utf16().chain(Some(0)).collect();
        let mut sid = ptr::null_mut();
        if unsafe { DeriveAppContainerSidFromAppContainerName(wide.as_ptr(), &mut sid) } < 0 {
            unresolved.retain(profile, || "its sandbox identity could not be derived".into());
            continue;
        }
        let sid = Sid(sid);
        let mut ledger = Ledger::resume(&records, lines);
        let proof = match job.as_ref() {
            None => Proof::Prelaunch,
            Some(job) if job_empty => Proof::JobEmpty { job: job.clone() },
            Some(job) => match termination(job, profile, &boot, sid.0) {
                Ok((proof, observed)) => {
                    // The job handle closes below; keep the observation for any later retry.
                    if observed
                        && control(&mut file, &serde_json::json!({"job_empty": job})).is_ok()
                    {
                        ledger.line();
                    }
                    proof
                }
                Err(error) => {
                    tracing::warn!(%error, "Retaining sandbox record without verified job termination");
                    unresolved.retain(profile, || error.to_string());
                    continue;
                }
            },
        };
        let mut complete = true;
        let mut first_failure = None;
        let mut unknown = Vec::new();
        let mut unreadable: Vec<Entry> = Vec::new();
        for saved in &records {
            match recover_object(saved, &mut file, &mut ledger, sid.0) {
                Ok(left) => {
                    if left.unknown {
                        unknown.push(saved.clone());
                    }
                    for item in left.unreadable {
                        if !unreadable.iter().any(|known| known.identity == item.identity) {
                            unreadable.push(item);
                        }
                    }
                }
                Err(error) => {
                    complete = false;
                    tracing::warn!(%error, "Retaining unresolved sandbox journal object");
                    first_failure.get_or_insert_with(|| format!("{error}. It cannot be checked for the run's access entry"));
                }
            }
        }
        // The certificate names each object exactly as the record already holds it.
        for item in &mut unreadable {
            if let Some(recorded) = records.iter().find(|record| record.discovered && record.identity == item.identity) {
                *item = recorded.clone();
            }
        }
        // An object MiniClaw granted is never retired as unreadable: its entry was written.
        if let Some(granted) = unreadable.iter().find(|item| {
            records.iter().any(|record| !record.discovered && record.identity == item.identity)
        }) {
            complete = false;
            first_failure.get_or_insert_with(|| format!(
                "{} was given the run's access entry and this Windows user can no longer open it to remove it",
                granted.path.display()
            ));
        }
        // Each unreadable object is in the record before the certificate that names it.
        if complete && !unreadable.is_empty() && let Err(error) = ledger.append(&mut file, &unreadable, true) {
            complete = false;
            first_failure.get_or_insert_with(|| error.to_string());
        }
        // Revocation is unbounded by design; it must not consume the validation budget.
        deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        if complete {
            let scratch = root
                .parent()
                .ok_or(io::ErrorKind::InvalidInput)?
                .join("sandbox-scratch")
                .join(profile);
            if let Err(error) = remove_scratch(&scratch) {
                complete = false;
                tracing::warn!(%error, "Retaining journal after interrupted scratch cleanup");
                first_failure.get_or_insert_with(|| format!("its private folder could not be removed ({error})"));
            }
        }
        let deleted = complete && delete_profile(&wide) >= 0;
        if complete && !deleted {
            first_failure.get_or_insert_with(|| "its sandbox profile could not be deleted".into());
        }
        if deleted {
            if unknown.is_empty() && unreadable.is_empty() {
                drop(file);
                if !remove_resolved(&entry.path()) {
                    unresolved.retain(profile, || "the finished record could not be removed".into());
                }
            } else {
                let certificate = Retirement::new(profile, proof, unknown, unreadable.clone());
                let validation = std::time::Instant::now();
                let mut known = records.clone();
                known.extend(unreadable.iter().cloned());
                if certificate
                    .validate(profile, &boot, job.as_deref(), &known, validation)
                    .and_then(|()| certificate.persist(&mut file))
                    .is_err()
                {
                    unresolved.retain(profile, || "its retirement certificate could not be written".into());
                } else {
                    drop(file);
                    if retirement::archive(&entry.path(), profile).is_err() {
                        unresolved.retain(profile, || "the retired record could not be archived".into());
                    } else {
                        tracing::warn!(
                            profile,
                            unreadable = unreadable.len(),
                            "Retired sandbox retains unknown or unreadable object identities; ACL removal is not claimed"
                        );
                    }
                }
            }
        } else {
            unresolved.retain(profile, || first_failure.unwrap_or_else(|| "its cleanup did not complete".into()));
        }
    }
    if unresolved.count != 0 {
        let count = unresolved.count;
        let first = unresolved.first.unwrap_or_default();
        return Err(super::super::refused(format!(
            "Windows sandbox recovery retained {count} unresolved records; native job/object cleanup must complete before serving. First: {first}"
        )));
    }
    Ok(())
}

/// Removes a record this scan resolved. Its handle was opened without delete sharing, so it is
/// removed after the handle closes; another launch's scan may open it in between. That scan then
/// holds it and finishes it, as a record held at the open is left to its holder; a name already
/// gone was finished by that scan. Any other failure keeps it unresolved.
fn remove_resolved(path: &Path) -> bool {
    match fs::remove_file(path) {
        Ok(()) => true,
        Err(error) => error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(32),
    }
}

struct Sid(PSID);
impl Drop for Sid {
    /// Releases the derived package SID on every path out of one record's recovery.
    fn drop(&mut self) {
        unsafe {
            FreeSid(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(file: &mut File) {
        let boot = crate::windows_network_broker::BootEpoch {
            installation: format!("MiniClawNetwork-{}", "a".repeat(32)),
            epoch: "b".repeat(32),
        };
        serde_json::to_writer(&mut *file, &serde_json::json!({"boot": boot})).unwrap();
        file.write_all(b"\n").unwrap();
    }

    /// Unknown IDs remain explicit diagnostics and cannot satisfy a replacement's identity.
    #[test]
    fn retirement_preserves_unknown_identity() {
        let data = std::env::temp_dir().join(format!("mc-retirement-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&data, false).unwrap();
        let root = data.join("sandbox-journals");
        platform_fs::private_dir(&root, false).unwrap();
        let profile = format!("MiniClaw.Run.{}", uuid::Uuid::new_v4().simple());
        let wide: Vec<u16> = profile.encode_utf16().chain(Some(0)).collect();
        let mut created_sid = ptr::null_mut();
        assert!(
            unsafe {
                CreateAppContainerProfile(
                    wide.as_ptr(),
                    wide.as_ptr(),
                    wide.as_ptr(),
                    ptr::null(),
                    0,
                    &mut created_sid,
                )
            } >= 0
        );
        unsafe {
            FreeSid(created_sid);
        }
        let path = root.join(format!("{profile}.jsonl"));
        let target = data.join("deleted");
        let original = platform_fs::private_file(&target, true).unwrap();
        let saved = Entry {
            path: target.clone(),
            identity: platform_fs::file_identity(&original).unwrap(),
            directory: false,
            inheritance_root: false,
            discovered: false,
        };
        drop(original);
        fs::remove_file(&target).unwrap();
        let replacement = platform_fs::private_file(&target, true).unwrap();
        let replacement_id = platform_fs::file_identity(&replacement).unwrap();
        assert_ne!(replacement_id, saved.identity);
        let mut file = platform_fs::private_file(&path, true).unwrap();
        header(&mut file);
        append_entry(&mut file, &saved).unwrap();
        drop(file);
        assert!(
            recover_with_profile_delete(&root, |_| -1).is_err(),
            "Profile deletion failure must block retirement"
        );
        assert!(path.exists());
        assert!(
            !root
                .join("retired")
                .join(format!("{profile}.retired.jsonl"))
                .exists()
        );
        // Represents a crash after profile deletion but before a retirement certificate exists.
        assert!(unsafe { DeleteAppContainerProfile(wide.as_ptr()) } >= 0);
        recover(&root)
            .expect("Proven prelaunch orphan may retire unknown IDs after profile deletion");
        let retired = root
            .join("retired")
            .join(format!("{profile}.retired.jsonl"));
        assert!(!path.exists());
        assert!(retired.exists());
        let value = fs::read_to_string(&retired).unwrap();
        assert!(value.contains("UnknownIdentity"));
        assert!(value.contains("profile_deleted"));
        assert!(ensure_unused_profile(&root, &profile).is_err());
        // Represents a crash after the durable certificate but before its atomic rename.
        fs::rename(&retired, &path).unwrap();
        recover(&root).unwrap();
        recover(&root).unwrap();
        assert_eq!(fs::read_dir(root.join("retired")).unwrap().count(), 1);
        assert!(retired.exists());
        let valid = fs::read_to_string(&retired).unwrap();
        let (prefix, terminal) = valid.trim_end().rsplit_once('\n').unwrap();
        for mutation in ["profile", "proof", "unknown", "profile_deleted"] {
            let mut certificate: serde_json::Value = serde_json::from_str(terminal).unwrap();
            let value = &mut certificate["retirement"];
            match mutation {
                "profile" => value["profile"] = "MiniClaw.Run.wrong".into(),
                "proof" => {
                    value["proof"] = serde_json::json!({"kind":"JobEmpty", "job":"unproved"})
                }
                "unknown" => value["unknown"][0]["object"]["identity"] = serde_json::json!([0, 0]),
                _ => value["profile_deleted"] = false.into(),
            }
            fs::write(&retired, format!("{prefix}\n{certificate}\n")).unwrap();
            assert!(
                recover(&root).is_err(),
                "Certificate tamper accepted: {mutation}"
            );
        }
        fs::write(&retired, valid).unwrap();
        let reader = platform_fs::open_existing(&retired, false, false).unwrap();
        recover(&root).expect("Immutable archive readers must not block other sandbox starts");
        drop(reader);
        assert_eq!(
            platform_fs::file_identity(&replacement).unwrap(),
            replacement_id
        );
        drop(replacement);
        fs::remove_dir_all(data).unwrap();
    }

    /// A malformed retirement certificate must never bypass active journal recovery.
    #[test]
    fn malformed_retirement_fails_closed() {
        let root =
            std::env::temp_dir().join(format!("mc-retired-invalid-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&root, false).unwrap();
        let profile = format!("MiniClaw.Run.{}", uuid::Uuid::new_v4().simple());
        platform_fs::private_dir(&root.join("retired"), false).unwrap();
        let path = root
            .join("retired")
            .join(format!("{profile}.retired.jsonl"));
        let mut file = platform_fs::private_file(&path, true).unwrap();
        header(&mut file);
        file.write_all(b"{\"retired\":true}\n").unwrap();
        file.sync_all().unwrap();
        drop(file);
        assert!(recover(&root).is_err());
        assert!(path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    /// Immutable audit history is validated at startup, not for each fresh command.
    #[test]
    fn admission_checks_active_records_without_rescanning_history() {
        let root =
            std::env::temp_dir().join(format!("mc-retired-admission-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&root, false).unwrap();
        platform_fs::private_dir(&root.join("retired"), false).unwrap();
        let old = format!("MiniClaw.Run.{}", uuid::Uuid::new_v4().simple());
        let path = root.join("retired").join(format!("{old}.retired.jsonl"));
        let mut archive = platform_fs::private_file(&path, true).unwrap();
        archive.write_all(b"invalid certificate\n").unwrap();
        archive.sync_all().unwrap();
        drop(archive);
        recover_active(&root).expect("Launches must not rescan inert audit history");
        assert!(ensure_unused_profile(&root, &old).is_err());
        let fresh = format!("MiniClaw.Run.{}", uuid::Uuid::new_v4().simple());
        ensure_unused_profile(&root, &fresh).unwrap();
        assert!(
            recover(&root).is_err(),
            "Explicit recovery must validate historical evidence"
        );
        let active = root.join(format!("{fresh}.jsonl"));
        let mut file = platform_fs::private_file(&active, true).unwrap();
        header(&mut file);
        file.write_all(b"invalid active record\n").unwrap();
        file.sync_all().unwrap();
        drop(file);
        assert!(
            recover_active(&root).is_err(),
            "Active uncertainty must still block launches"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// Historical certificates do not consume the bounded active recovery queue.
    #[test]
    fn retired_history_does_not_block_launches() {
        let root =
            std::env::temp_dir().join(format!("mc-retired-history-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&root, false).unwrap();
        let archives = root.join("retired");
        platform_fs::private_dir(&archives, false).unwrap();
        for _ in 0..1001 {
            let profile = format!("MiniClaw.Run.{}", uuid::Uuid::new_v4().simple());
            let mut file =
                platform_fs::private_file(&archives.join(format!("{profile}.retired.jsonl")), true)
                    .unwrap();
            header(&mut file);
            let saved = Entry {
                path: root.join("historical-object"),
                identity: (1, 2),
                directory: false,
                inheritance_root: false,
                discovered: false,
            };
            append_entry(&mut file, &saved).unwrap();
            Retirement::new(&profile, Proof::Prelaunch, vec![saved], Vec::new())
                .persist(&mut file)
                .unwrap();
        }
        recover(&root).unwrap();
        assert_eq!(fs::read_dir(&archives).unwrap().count(), 1001);
        fs::remove_dir_all(root).unwrap();
    }

    /// An old run's retained ACE cannot authorize a fresh sandbox after profile deletion.
    #[tokio::test]
    #[ignore = "Requires authenticated native broker and private test data"]
    async fn retired_sid_cannot_authorize_new_sandbox() {
        let root = std::env::temp_dir().join(format!("mc-retired-sid-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&root, false).unwrap();
        let inside = root.join("inside");
        fs::create_dir(&inside).unwrap();
        fs::write(inside.join("control.txt"), "control").unwrap();
        let outside = root.join("old-only.txt");
        fs::write(&outside, "old identity only").unwrap();
        let program = std::env::current_exe().unwrap();
        let grants = vec![
            Grant {
                path: inside.to_string_lossy().into_owned(),
                write: true,
                folder: true,
                blocked: false,
            },
            Grant {
                path: program.to_string_lossy().into_owned(),
                write: false,
                folder: false,
                blocked: false,
            },
        ];
        let old = Sandbox::new(&grants, &inside).unwrap();
        let old_profile = old.profile_name();
        let outside_pin = platform_fs::guarded_acl_path(&outside, false).unwrap();
        let outside_acl = acl_handle(&outside_pin, false).unwrap();
        // Simulates a trusted host moving a new old-SID object outside the retained roots.
        change_acl(
            &outside_acl,
            old.security().capabilities.AppContainerSid,
            FILE_GENERIC_READ,
            GRANT_ACCESS,
            NO_INHERITANCE,
        )
        .unwrap();
        for (sandbox, allowed) in [(&old, true)] {
            let mut env: Vec<_> = sandbox
                .environment()
                .into_iter()
                .map(|(k, v)| (k.into_string().unwrap(), v.into_string().unwrap()))
                .collect();
            env.push((
                "MINICLAW_RETIRED_SID_PROBE".into(),
                outside.to_string_lossy().into_owned(),
            ));
            env.push(("MINICLAW_RETIRED_SID_ALLOWED".into(), allowed.to_string()));
            let args = [
                "--exact",
                "windows_sandbox::native::journal::tests::retired_sid_child",
                "--nocapture",
            ]
            .map(str::to_owned);
            let (out, err, code) = crate::windows_process::capture_restricted(
                &program,
                &args,
                &inside,
                &env,
                &sandbox.security(),
            )
            .await
            .unwrap();
            assert_eq!(
                code,
                0,
                "{} {}",
                String::from_utf8_lossy(&out),
                String::from_utf8_lossy(&err)
            );
        }
        drop(old);
        drain_cleanup(std::time::Duration::from_secs(35)).unwrap();
        let new = Sandbox::new(&grants, &inside).unwrap();
        assert_ne!(new.profile_name(), old_profile);
        let mut env: Vec<_> = new
            .environment()
            .into_iter()
            .map(|(k, v)| (k.into_string().unwrap(), v.into_string().unwrap()))
            .collect();
        env.push((
            "MINICLAW_RETIRED_SID_PROBE".into(),
            outside.to_string_lossy().into_owned(),
        ));
        env.push(("MINICLAW_RETIRED_SID_ALLOWED".into(), "false".into()));
        let args = [
            "--exact",
            "windows_sandbox::native::journal::tests::retired_sid_child",
            "--nocapture",
        ]
        .map(str::to_owned);
        let (out, err, code) = crate::windows_process::capture_restricted(
            &program,
            &args,
            &inside,
            &env,
            &new.security(),
        )
        .await
        .unwrap();
        assert_eq!(
            code,
            0,
            "{} {}",
            String::from_utf8_lossy(&out),
            String::from_utf8_lossy(&err)
        );
        drop(new);
        drain_cleanup(std::time::Duration::from_secs(35)).unwrap();
        let wide: Vec<u16> = old_profile.encode_utf16().chain(Some(0)).collect();
        let mut sid = ptr::null_mut();
        assert!(unsafe { DeriveAppContainerSidFromAppContainerName(wide.as_ptr(), &mut sid) } >= 0);
        change_acl(&outside_acl, sid, 0, REVOKE_ACCESS, NO_INHERITANCE).unwrap();
        unsafe {
            FreeSid(sid);
        }
        drop(outside_acl);
        drop(outside_pin);
        fs::remove_dir_all(root).unwrap();
    }

    /// Runs both positive and denied reads under the actual AppContainer token.
    #[test]
    fn retired_sid_child() {
        let Some(path) = std::env::var_os("MINICLAW_RETIRED_SID_PROBE") else {
            return;
        };
        assert_eq!(fs::read_to_string("control.txt").unwrap(), "control");
        let result = fs::read_to_string(path);
        if std::env::var("MINICLAW_RETIRED_SID_ALLOWED").unwrap() == "true" {
            assert_eq!(result.unwrap(), "old identity only");
        } else {
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        }
    }

    #[test]
    fn active_journal_is_not_recovered() {
        let root = std::env::temp_dir().join(format!("mc-journal-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&root, false).unwrap();
        let path = root.join(format!(
            "MiniClaw.Run.{}.jsonl",
            uuid::Uuid::new_v4().simple()
        ));
        let file = platform_fs::private_file(&path, true).unwrap();
        file.try_lock().unwrap();
        recover(&root).unwrap();
        assert!(path.exists(), "Live owner record must survive recovery");
        drop(file);
        fs::remove_dir_all(root).unwrap();
    }

    /// A run that ends while another launch is admitted: the ending run's cleanup completes its
    /// record while the new launch scans the records folder. A record its owner completed is gone,
    /// never unresolved, and the owner's completion never fails because a scan looked at it.
    /// Natively this race failed `default_mcp_workspaces_are_private_between_runs` once in the
    /// standard-user sequence (`gate-acceptance-vm2-2.log`): "retained 1 unresolved records".
    #[test]
    fn records_completed_during_admission_are_not_unresolved() {
        // A failed scan names its reason on stderr when the runner asks for MiniClaw's events.
        if std::env::var_os("MINICLAW_LOG").is_some() { let _ = crate::logging::install(); }
        let data = std::env::temp_dir().join(format!("mc-admission-race-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&data, false).unwrap();
        let root = data.join("sandbox-journals");
        platform_fs::private_dir(&root, false).unwrap();
        let folder = root.clone();
        let owner = std::thread::spawn(move || {
            for round in 0..600 {
                let profile = format!("MiniClaw.Run.{}", uuid::Uuid::new_v4().simple());
                let path = folder.join(format!("{profile}.jsonl"));
                let mut file = platform_fs::private_file(&path, true).unwrap();
                file.try_lock().unwrap();
                header(&mut file);
                file.sync_all().unwrap();
                let journal = Journal { file, path, inheritance: Vec::new(), ledger: Ledger::default(), job: None };
                journal.complete().unwrap_or_else(|error| panic!("round {round}: the owner could not complete its record: {error}"));
            }
        });
        let mut scans = 0;
        while !owner.is_finished() {
            recover_active(&root).unwrap_or_else(|error| panic!("scan {scans}: {error}"));
            scans += 1;
        }
        owner.join().unwrap();
        println!("ADMISSION RACE {scans} scans beside 600 completed records");
        assert!(scans > 10, "the scans ran beside the completions: {scans}");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0, "every completed record is gone");
        fs::remove_dir_all(data).unwrap();
    }

    // Replaces `incomplete_object_record_is_retained`, which codified review finding 4: a
    // header followed by an unterminated record was a permanent startup failure. Every record
    // is flushed before the change it describes, so an unterminated final line was never
    // applied. Corruption that is followed by further data still fails closed below.
    #[test]
    fn mid_file_corruption_is_retained() {
        let root = std::env::temp_dir().join(format!("mc-journal-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&root, false).unwrap();
        for damaged in [&b"{\"path\":\n"[..], b"\0\0\0\0\n", b"not json\n"] {
            let path = root.join(format!(
                "MiniClaw.Run.{}.jsonl",
                uuid::Uuid::new_v4().simple()
            ));
            let mut file = platform_fs::private_file(&path, true).unwrap();
            header(&mut file);
            file.write_all(damaged).unwrap();
            file.write_all(b"{\"job\":\"later\"}\n").unwrap();
            file.sync_all().unwrap();
            drop(file);
            assert!(recover(&root).is_err());
            assert!(
                path.exists(),
                "Uncertain cleanup evidence must survive recovery"
            );
            fs::remove_file(path).unwrap();
        }
        fs::remove_dir_all(root).unwrap();
    }

    /// Nothing was changed before the first complete record, so these need no restart.
    #[test]
    fn empty_and_torn_journals_are_prelaunch() {
        let data = std::env::temp_dir().join(format!("mc-torn-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&data, false).unwrap();
        let root = data.join("sandbox-journals");
        platform_fs::private_dir(&root, false).unwrap();
        for (name, with_header, tail) in [
            ("empty", false, &b""[..]),
            ("torn header", false, b"{\"boot\":{\"installation\":\"MiniClawNet"),
            ("zero-filled header", false, b"\0\0\0\0\0\0\0\0"),
            ("header only", true, b""),
            ("torn record", true, b"{\"path\":"),
            ("torn job binding", true, b"{\"job\":\"Global\\\\MiniClaw.Job."),
        ] {
            let path = root.join(format!(
                "MiniClaw.Run.{}.jsonl",
                uuid::Uuid::new_v4().simple()
            ));
            let mut file = platform_fs::private_file(&path, true).unwrap();
            if with_header {
                header(&mut file);
            }
            file.write_all(tail).unwrap();
            file.sync_all().unwrap();
            drop(file);
            recover(&root).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert!(!path.exists(), "{name}: resolved record must be removed");
        }
        fs::remove_dir_all(data).unwrap();
    }

    /// A torn tail is dropped, yet every complete record before it is still revoked.
    #[test]
    fn torn_tail_keeps_earlier_records() {
        let data = std::env::temp_dir().join(format!("mc-torn-tail-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&data, false).unwrap();
        let root = data.join("sandbox-journals");
        platform_fs::private_dir(&root, false).unwrap();
        let profile = format!("MiniClaw.Run.{}", uuid::Uuid::new_v4().simple());
        let sid = Trustee::derive(&profile);
        let target = data.join("granted.txt");
        fs::write(&target, b"host data").unwrap();
        let handle = raw(&target);
        change_acl(&handle, sid.0, FILE_GENERIC_READ, GRANT_ACCESS, NO_INHERITANCE).unwrap();
        assert!(sddl(&handle).contains(&sid.text()));
        let path = root.join(format!("{profile}.jsonl"));
        let mut file = platform_fs::private_file(&path, true).unwrap();
        header(&mut file);
        append_entry(
            &mut file,
            &Entry {
                path: target.clone(),
                identity: platform_fs::file_identity(&handle).unwrap(),
                directory: false,
                inheritance_root: false,
                discovered: false,
            },
        )
        .unwrap();
        file.write_all(b"{\"path\":\"C:\\\\never-applied").unwrap();
        file.sync_all().unwrap();
        drop(file);
        recover(&root).unwrap();
        assert!(!path.exists());
        assert!(!sddl(&handle).contains(&sid.text()), "Recorded grant must be revoked");
        assert_eq!(fs::read(&target).unwrap(), b"host data");
        drop(handle);
        fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn mismatched_identity_is_never_replayed() {
        let root = std::env::temp_dir().join(format!("mc-journal-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&root, false).unwrap();
        let target = root.join("target");
        drop(platform_fs::private_file(&target, true).unwrap());
        let path = root.join(format!(
            "MiniClaw.Run.{}.jsonl",
            uuid::Uuid::new_v4().simple()
        ));
        let mut file = platform_fs::private_file(&path, true).unwrap();
        header(&mut file);
        serde_json::to_writer(
            &mut file,
            &Entry {
                path: target,
                identity: (0, 0),
                directory: false,
                inheritance_root: false,
                discovered: false,
            },
        )
        .unwrap();
        file.write_all(b"\n").unwrap();
        file.sync_all().unwrap();
        drop(file);
        assert!(recover(&root).is_err());
        assert!(
            path.exists(),
            "A different filesystem object cannot satisfy recovery"
        );
        fs::remove_dir_all(root).unwrap();
    }
    // Replaces `bound_job_record_requires_termination_proof`, which codified review finding 1:
    // a bound record whose job no longer exists was unrecoverable until Windows restarted. The
    // job is created kill-on-close without breakaway, so its absence means Windows terminated
    // every member; recovery additionally requires that no inspectable process still carries
    // the run identity. `live_member_blocks_absent_job_proof` keeps the fail-closed half.
    #[test]
    fn absent_job_without_survivor_is_recovered() {
        let root = std::env::temp_dir().join(format!("mc-journal-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&root, false).unwrap();
        let path = root.join(format!(
            "MiniClaw.Run.{}.jsonl",
            uuid::Uuid::new_v4().simple()
        ));
        let mut file = platform_fs::private_file(&path, true).unwrap();
        header(&mut file);
        let job = format!("Global\\MiniClaw.Job.{}", uuid::Uuid::new_v4().simple());
        serde_json::to_writer(&mut file, &serde_json::json!({"job": job})).unwrap();
        file.write_all(b"\n").unwrap();
        // A claim about a different job is not this record's proof.
        file.write_all(b"{\"job_empty\":\"Global\\\\MiniClaw.Job.other\"}\n").unwrap();
        file.sync_all().unwrap();
        drop(file);
        assert!(recover(&root).is_err(), "A mismatched empty-job claim must fail closed");
        let file = platform_fs::open_existing(&path, true, false).unwrap();
        let length = file.metadata().unwrap().len();
        file.set_len(length - "{\"job_empty\":\"Global\\\\MiniClaw.Job.other\"}\n".len() as u64).unwrap();
        drop(file);
        let profile = path.file_stem().unwrap().to_str().unwrap().to_owned();
        let sid = Trustee::derive(&profile);
        // With the service running, its attestation covers what this account cannot inspect.
        let attested = crate::windows_network_broker::verified_boot_epoch().is_ok();
        match liveness::no_survivor(sid.0, std::time::Duration::from_secs(2)) {
            Ok(()) => {
                recover(&root)
                    .expect("An absent kill-on-close job with no surviving process is stopped");
                assert!(!path.exists());
            }
            Err(error) if attested => {
                println!("Liveness proof unavailable to this account, attested by the service: {error}");
                recover(&root).expect("The service attests an absent job with no surviving process");
                assert!(!path.exists());
            }
            // Without the service, an account that cannot inspect every process in its session
            // has no proof, and must keep the record rather than guess.
            Err(error) => {
                println!("Liveness proof unavailable to this account: {error}");
                assert!(!error.to_string().contains("still running"), "{error}");
                assert!(recover(&root).is_err());
                assert!(path.exists(), "An unproven record must be retained");
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    /// A running process with the run identity defeats the proof even though no job is found.
    #[tokio::test]
    #[ignore = "Requires authenticated native broker and private test data"]
    async fn live_member_blocks_absent_job_proof() {
        let root = std::env::temp_dir().join(format!("mc-live-member-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&root, false).unwrap();
        let inside = root.join("inside");
        fs::create_dir(&inside).unwrap();
        let program = std::env::current_exe().unwrap();
        let grants = vec![
            Grant { path: inside.to_string_lossy().into_owned(), write: true, folder: true, blocked: false },
            Grant { path: program.to_string_lossy().into_owned(), write: false, folder: false, blocked: false },
        ];
        let sandbox = Sandbox::new(&grants, &inside).unwrap();
        let sid = sandbox.security().capabilities.AppContainerSid;
        // An account that cannot inspect every process has no proof of its own; it must still
        // never report a member that is not there.
        let own_view = |limit| match liveness::no_survivor(sid, limit) {
            Ok(()) => {}
            Err(error) => assert!(!error.to_string().contains("still running"), "{error}"),
        };
        own_view(std::time::Duration::from_secs(2));
        let mut env: Vec<_> = sandbox
            .environment()
            .into_iter()
            .map(|(k, v)| (k.into_string().unwrap(), v.into_string().unwrap()))
            .collect();
        env.push(("MINICLAW_LIVE_MEMBER".into(), inside.to_string_lossy().into_owned()));
        let args = [
            "--exact",
            "windows_sandbox::native::journal::tests::live_member_child",
            "--nocapture",
        ]
        .map(str::to_owned);
        let server = crate::windows_process::stdio_restricted(
            &program,
            &args,
            &inside,
            &env,
            &sandbox.security(),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !inside.join("started.txt").exists() {
            assert!(std::time::Instant::now() < deadline, "Sandboxed child did not start");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let error = liveness::no_survivor(sid, std::time::Duration::from_millis(500))
            .expect_err("A live member must keep termination unproven");
        assert!(error.to_string().contains("still running"), "{error}");
        // The service refuses too: while the job exists, and for the live member alone when
        // asked under a job name that does not exist.
        use crate::windows_network_broker::{attest_job_dead, attest_within};
        let profile = sandbox.profile_name();
        let job = server.process.job_descriptor().0;
        let absent = format!("Global\\MiniClaw.Job.{}", uuid::Uuid::new_v4().simple());
        let brief = std::time::Duration::from_secs(1);
        for name in [&job, &absent] {
            let error = attest_within(&profile, name, brief)
                .expect_err("The service must not attest a run with a live member");
            assert_eq!(error.kind(), io::ErrorKind::ResourceBusy, "{name}: {error}");
        }
        // The same absent job with an unused run identity is attested, so the refusal above
        // came from the live member and not from a process the service could not inspect.
        let unused = format!("MiniClaw.Run.{}", uuid::Uuid::new_v4().simple());
        attest_within(&unused, &absent, brief).expect("No process carries an unused run identity");
        // Destroying the job is what makes the proof succeed.
        drop(server);
        attest_job_dead(&profile, &job).expect("The service attests the destroyed job's run");
        own_view(std::time::Duration::from_secs(2));
        drop(sandbox);
        drain_cleanup(std::time::Duration::from_secs(35)).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    /// Stays alive under the real AppContainer token until its job is destroyed.
    #[test]
    fn live_member_child() {
        let Some(path) = std::env::var_os("MINICLAW_LIVE_MEMBER") else {
            return;
        };
        fs::write(PathBuf::from(path).join("started.txt"), b"started").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(300));
    }

    #[test]
    fn identity_reopen_follows_rename_not_replacement() {
        let root = std::env::temp_dir().join(format!("mc-reopen-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&root, false).unwrap();
        let original = root.join("original");
        let file = platform_fs::private_file(&original, true).unwrap();
        let entry = Entry {
            path: original.clone(),
            identity: platform_fs::file_identity(&file).unwrap(),
            directory: false,
            inheritance_root: false,
            discovered: false,
        };
        drop(file);
        fs::rename(&original, root.join("moved")).unwrap();
        drop(platform_fs::private_file(&original, true).unwrap());
        let reopened = reopen(&entry).unwrap().unwrap();
        assert_eq!(
            platform_fs::file_identity(&reopened).unwrap(),
            entry.identity
        );
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }
    /// Deleted file and directory IDs cannot resolve to newly created namesakes.
    #[test]
    fn deleted_identity_never_reopens_replacement() {
        let root = std::env::temp_dir().join(format!("mc-deleted-id-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&root, false).unwrap();
        for directory in [false, true] {
            let path = root.join(if directory { "directory" } else { "file" });
            if directory {
                fs::create_dir(&path).unwrap();
            } else {
                drop(platform_fs::private_file(&path, true).unwrap());
            }
            let pin = platform_fs::guarded_acl_path(&path, directory).unwrap();
            let entry = Entry {
                path: path.clone(),
                identity: platform_fs::file_identity(&pin.file).unwrap(),
                directory,
                inheritance_root: false,
                discovered: false,
            };
            drop(pin);
            if directory {
                fs::remove_dir(&path).unwrap();
                fs::create_dir(&path).unwrap();
            } else {
                fs::remove_file(&path).unwrap();
                drop(platform_fs::private_file(&path, true).unwrap());
            }
            let replacement = platform_fs::guarded_acl_path(&path, directory).unwrap();
            assert_ne!(
                platform_fs::file_identity(&replacement.file).unwrap(),
                entry.identity
            );
            match reopen(&entry) {
                Ok(None) => {}
                Err(error) => assert_eq!(
                    error.raw_os_error(),
                    Some(87),
                    "Unknown ID must remain an error, not a replacement"
                ),
                Ok(Some(_)) => panic!("A replacement must not satisfy the deleted identity"),
            }
            drop(replacement);
        }
        fs::remove_dir_all(root).unwrap();
    }

    /// Gives an object a protected access list that leaves this user nothing to manage it with:
    /// SYSTEM only, and an OWNER RIGHTS entry that leaves the owner only `DELETE` (no `READ_CONTROL`,
    /// `WRITE_DAC` or `SYNCHRONIZE`). Like the WebView2 folders an elevated Studio left on the second
    /// guest, it opens for none of them, whoever created it (an elevated token too, without
    /// privileges). Its parent still lists it, and `remove_locked` deletes it, so tests clean up.
    pub(crate) fn lock_out(path: &Path) {
        let sddl: Vec<u16> = "D:P(A;OICI;FA;;;SY)(A;;SD;;;OW)\0".encode_utf16().collect();
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe {
            let mut descriptor = ptr::null_mut();
            assert_ne!(ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), 1, &mut descriptor, ptr::null_mut()), 0);
            let set = SetFileSecurityW(wide.as_ptr(), DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, descriptor);
            LocalFree(descriptor);
            assert_ne!(set, 0, "{}: {}", path.display(), io::Error::last_os_error());
        }
        assert_eq!(platform_fs::guarded_acl_path(path, path.is_dir()).err().and_then(|error| error.raw_os_error()), Some(5),
            "the fixture must be unmanageable for this user: {}", path.display());
    }

    /// Deletes an object through `DELETE` alone, the right its folder's `FILE_DELETE_CHILD` (and the
    /// fixtures' `OWNER RIGHTS` entry) gives. `std::fs` asks for `SYNCHRONIZE` too, which these
    /// fixtures withhold as the real Administrators-only folders do.
    pub(crate) fn remove_locked(path: &Path) {
        use windows_sys::{Wdk::{Foundation::OBJECT_ATTRIBUTES, Storage::FileSystem as nt}, Win32::{Foundation::{CloseHandle, UNICODE_STRING}, System::IO::IO_STATUS_BLOCK}};
        let mut text: Vec<u16> = format!(r"\??\{}", path.display()).encode_utf16().collect();
        let length = (text.len() * 2) as u16;
        let mut name = UNICODE_STRING { Length: length, MaximumLength: length, Buffer: text.as_mut_ptr() };
        let attributes = OBJECT_ATTRIBUTES { Length: size_of::<OBJECT_ATTRIBUTES>() as u32, RootDirectory: std::ptr::null_mut(),
            ObjectName: &mut name, Attributes: 0, SecurityDescriptor: std::ptr::null(), SecurityQualityOfService: std::ptr::null_mut() };
        let mut status: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
        let mut handle = std::ptr::null_mut();
        let result = unsafe { nt::NtCreateFile(&mut handle, 0x0001_0000, &attributes, &mut status, std::ptr::null(), 0,
            7, nt::FILE_OPEN, nt::FILE_DELETE_ON_CLOSE | nt::FILE_OPEN_REPARSE_POINT, std::ptr::null(), 0) };
        assert!(result >= 0, "delete {}: {result:#x}", path.display());
        unsafe { CloseHandle(handle) };
        assert!(fs::symlink_metadata(path).is_err(), "{} is gone", path.display());
    }

    /// Writes a pre-launch record (no job) with these entries, as a launch that stopped after its grants.
    fn prelaunch_record(root: &Path, entries: &[Entry]) -> (String, PathBuf) {
        let profile = format!("MiniClaw.Run.{}", uuid::Uuid::new_v4().simple());
        let path = root.join(format!("{profile}.jsonl"));
        let mut file = platform_fs::private_file(&path, true).unwrap();
        header(&mut file);
        for entry in entries {
            append_entry(&mut file, entry).unwrap();
        }
        (profile, path)
    }

    /// A writable root's descendant that this user cannot open, and that MiniClaw never granted,
    /// does not keep the record forever. With the run proven stopped (here: it never ran) and its
    /// profile deleted, the record retires naming it as `Unreadable`, without claiming its entry
    /// was removed; the archive then validates at every later start. Everything readable is still
    /// cleaned, and a recorded grant that can no longer be opened still keeps the record.
    #[test]
    fn unreadable_descendants_retire_and_unreadable_grants_stay() {
        let data = std::env::temp_dir().join(format!("mc-unreadable-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&data, false).unwrap();
        let root = data.join("sandbox-journals");
        platform_fs::private_dir(&root, false).unwrap();
        let folder = data.join("granted");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("plain.txt"), b"readable").unwrap();
        let locked = folder.join("EBWebView-component");
        fs::create_dir(&locked).unwrap();
        lock_out(&locked);
        let pin = platform_fs::guarded_acl_path(&folder, true).unwrap();
        let entry = |path: &Path, file: &File, directory: bool, inheritance_root: bool| Entry {
            path: path.to_owned(), identity: platform_fs::file_identity(file).unwrap(), directory, inheritance_root, discovered: false };
        let records = [entry(&folder, &pin.file, true, true), entry(&folder, &pin.file, true, false)];
        let (profile, path) = prelaunch_record(&root, &records);
        let deletes = std::cell::Cell::new(0);
        recover_with_profile_delete(&root, |_| { deletes.set(deletes.get() + 1); 0 })
            .expect("an unreadable descendant never granted retires the record");
        assert_eq!(deletes.get(), 1, "the profile is deleted before retirement");
        assert!(!path.exists());
        let archive = root.join("retired").join(format!("{profile}.retired.jsonl"));
        let text = fs::read_to_string(&archive).unwrap();
        let certificate: serde_json::Value = serde_json::from_str(text.trim_end().rsplit_once('\n').unwrap().1).unwrap();
        let certificate = &certificate["retirement"];
        assert_eq!(certificate["version"], 2);
        let named = certificate["unknown"].as_array().unwrap();
        assert_eq!(named.len(), 1, "{certificate}");
        assert_eq!((named[0]["kind"].as_str(), named[0]["error"].as_i64()), (Some("Unreadable"), Some(5)));
        assert_eq!(named[0]["object"]["path"].as_str(), Some(locked.to_str().unwrap()));
        assert_eq!(named[0]["object"]["discovered"], true);
        assert!(ensure_unused_profile(&root, &profile).is_err(), "the retired name is reserved");
        recover_with_profile_delete(&root, |_| -1).expect("the archive validates at a later start");
        // Tampering: an unreadable entry for an object MiniClaw granted, or under version 1, is refused.
        let (prefix, terminal) = text.trim_end().rsplit_once('\n').unwrap();
        for mutation in ["granted", "version"] {
            let mut changed: serde_json::Value = serde_json::from_str(terminal).unwrap();
            match mutation {
                "granted" => changed["retirement"]["unknown"][0]["object"] = serde_json::to_value(&records[1]).unwrap(),
                _ => changed["retirement"]["version"] = 1.into(),
            }
            fs::write(&archive, format!("{prefix}\n{changed}\n")).unwrap();
            assert!(recover_with_profile_delete(&root, |_| 0).is_err(), "accepted: {mutation}");
        }
        fs::write(&archive, &text).unwrap();
        // The same object recorded as a grant: its entry was written and cannot be checked.
        let locked_identity = {
            let names: Vec<u16> = "EBWebView-component".encode_utf16().collect();
            revoke::identify(&pin.file, &names).unwrap().unwrap()
        };
        let granted = Entry { path: locked.clone(), identity: locked_identity, directory: true, inheritance_root: false, discovered: false };
        let (_, kept) = prelaunch_record(&root, &[records[0].clone(), granted]);
        let error = recover_with_profile_delete(&root, |_| 0).expect_err("an unreadable grant keeps its record");
        let words = error.to_string();
        assert!(words.contains("retained 1 unresolved") && words.contains("EBWebView-component"), "{words}");
        assert!(crate::windows_sandbox::refusal(&error).is_some(), "the reason is for the user: {words}");
        assert!(kept.exists());
        // Once the object is gone, the record resolves.
        drop(pin);
        remove_locked(&locked);
        recover_with_profile_delete(&root, |_| 0).expect("a deleted grant resolves");
        assert!(!kept.exists());
        fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn private_scratch_cleanup_preserves_neighbor() {
        let root = std::env::temp_dir().join(format!("mc-scratch-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&root, false).unwrap();
        let scratch = root.join("scratch");
        platform_fs::private_dir(&scratch, false).unwrap();
        fs::create_dir(scratch.join("nested")).unwrap();
        fs::write(scratch.join("nested/generated"), b"fixture").unwrap();
        fs::write(root.join("neighbor"), b"keep").unwrap();
        remove_scratch(&scratch).unwrap();
        assert!(!scratch.exists());
        assert_eq!(fs::read(root.join("neighbor")).unwrap(), b"keep");
        fs::remove_dir_all(root).unwrap();
    }

    struct Trustee(PSID);
    impl Trustee {
        /// Derives the run identity exactly as recovery does, without creating a profile.
        fn derive(profile: &str) -> Self {
            let wide: Vec<u16> = profile.encode_utf16().chain(Some(0)).collect();
            let mut sid = ptr::null_mut();
            assert!(unsafe { DeriveAppContainerSidFromAppContainerName(wide.as_ptr(), &mut sid) } >= 0);
            Self(sid)
        }

        /// Returns the SDDL spelling used to find this trustee in a descriptor.
        fn text(&self) -> String {
            unsafe {
                let mut text = ptr::null_mut();
                assert_ne!(ConvertSidToStringSidW(self.0, &mut text), 0);
                let mut length = 0;
                while *text.add(length) != 0 {
                    length += 1;
                }
                let value = String::from_utf16(std::slice::from_raw_parts(text, length)).unwrap();
                LocalFree(text.cast());
                value
            }
        }
    }
    impl Drop for Trustee {
        /// Releases the derived SID allocation.
        fn drop(&mut self) {
            unsafe {
                FreeSid(self.0);
            }
        }
    }

    /// Opens the named object itself, including a link or a multiply linked file.
    fn raw(path: &Path) -> File {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                READ_CONTROL | WRITE_DAC | FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
                ptr::null_mut(),
            )
        };
        assert_ne!(handle, INVALID_HANDLE_VALUE, "{}", path.display());
        unsafe { File::from_raw_handle(handle) }
    }

    /// Renders the object's DACL so a test can look for one trustee.
    fn sddl(file: &File) -> String {
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
            let mut text = ptr::null_mut();
            assert_ne!(
                ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    descriptor,
                    1,
                    DACL_SECURITY_INFORMATION,
                    &mut text,
                    ptr::null_mut()
                ),
                0
            );
            let mut length = 0;
            while *text.add(length) != 0 {
                length += 1;
            }
            let value = String::from_utf16(std::slice::from_raw_parts(text, length)).unwrap();
            LocalFree(text.cast());
            LocalFree(descriptor);
            value
        }
    }

    /// Turns an empty directory into a junction, as an unprivileged program can.
    fn junction(link: &Path, target: &Path) {
        use windows_sys::Win32::System::IO::DeviceIoControl;
        fs::create_dir(link).unwrap();
        let wide: Vec<u16> = link.as_os_str().encode_wide().chain(Some(0)).collect();
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
                ptr::null_mut(),
            )
        };
        assert_ne!(handle, INVALID_HANDLE_VALUE);
        let file = unsafe { File::from_raw_handle(handle) };
        let plain = target.to_str().unwrap().trim_start_matches(r"\\?\");
        let substitute: Vec<u16> = format!(r"\??\{plain}").encode_utf16().collect();
        let print: Vec<u16> = plain.encode_utf16().collect();
        let mut names: Vec<u16> = substitute.clone();
        names.push(0);
        names.extend(&print);
        names.push(0);
        let mut buffer = Vec::new();
        buffer.extend(0xA000_0003u32.to_le_bytes());
        buffer.extend(((8 + names.len() * 2) as u16).to_le_bytes());
        buffer.extend(0u16.to_le_bytes());
        buffer.extend(0u16.to_le_bytes());
        buffer.extend(((substitute.len() * 2) as u16).to_le_bytes());
        buffer.extend(((substitute.len() * 2 + 2) as u16).to_le_bytes());
        buffer.extend(((print.len() * 2) as u16).to_le_bytes());
        buffer.extend(names.iter().flat_map(|unit| unit.to_le_bytes()));
        let mut returned = 0;
        // FSCTL_SET_REPARSE_POINT
        assert_ne!(
            unsafe {
                DeviceIoControl(
                    file.as_raw_handle(),
                    0x0009_00A4,
                    buffer.as_ptr().cast(),
                    buffer.len() as u32,
                    ptr::null_mut(),
                    0,
                    &mut returned,
                    ptr::null_mut(),
                )
            },
            0,
            "{}",
            io::Error::last_os_error()
        );
    }

    /// Builds an orphan record whose writable root passed the run SID to later children.
    fn inherited_orphan(data: &Path, populate: impl FnOnce(&Path)) -> (PathBuf, PathBuf, Trustee) {
        let root = data.join("sandbox-journals");
        platform_fs::private_dir(&root, false).unwrap();
        let profile = format!("MiniClaw.Run.{}", uuid::Uuid::new_v4().simple());
        let sid = Trustee::derive(&profile);
        let tree = data.join("tree");
        fs::create_dir(&tree).unwrap();
        let pin = platform_fs::guarded_acl_path(&tree, true).unwrap();
        let handle = acl_handle(&pin, true).unwrap();
        let path = root.join(format!("{profile}.jsonl"));
        let mut file = platform_fs::private_file(&path, true).unwrap();
        header(&mut file);
        append_entry(
            &mut file,
            &Entry {
                path: pin.path.clone(),
                identity: platform_fs::file_identity(&handle).unwrap(),
                directory: true,
                inheritance_root: true,
                discovered: false,
            },
        )
        .unwrap();
        drop(file);
        change_acl(
            &handle,
            sid.0,
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE,
            GRANT_ACCESS,
            SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        )
        .unwrap();
        drop((handle, pin));
        populate(&tree);
        (path, tree, sid)
    }

    /// Hard links and junctions made during a run must not make its record permanent.
    #[test]
    fn links_created_during_run_are_revoked() {
        let data = std::env::temp_dir().join(format!("mc-links-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&data, false).unwrap();
        let outside = data.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep.txt"), b"keep").unwrap();
        let before = sddl(&raw(&outside.join("keep.txt")));
        let (path, tree, sid) = inherited_orphan(&data, |tree| {
            fs::write(tree.join("first.txt"), b"linked").unwrap();
            fs::hard_link(tree.join("first.txt"), tree.join("second.txt")).unwrap();
            fs::create_dir(tree.join("nested")).unwrap();
            fs::hard_link(tree.join("first.txt"), tree.join("nested/third.txt")).unwrap();
            junction(&tree.join("escape"), &outside);
        });
        let trustee = sid.text();
        for name in ["first.txt", "second.txt", "nested", "escape"] {
            assert!(sddl(&raw(&tree.join(name))).contains(&trustee), "{name} lacks the run SID");
        }
        recover(data.join("sandbox-journals").as_path())
            .expect("Links created during a run must not block recovery");
        assert!(!path.exists());
        for name in ["", "first.txt", "second.txt", "nested", "nested/third.txt", "escape"] {
            assert!(
                !sddl(&raw(&tree.join(name))).contains(&trustee),
                "Run SID remains at {name}"
            );
        }
        assert!(platform_fs::is_link(&fs::symlink_metadata(tree.join("escape")).unwrap()));
        assert_eq!(fs::read(outside.join("keep.txt")).unwrap(), b"keep");
        assert_eq!(sddl(&raw(&outside.join("keep.txt"))), before, "Link target must be untouched");
        assert_eq!(fs::read(tree.join("second.txt")).unwrap(), b"linked");
        fs::remove_dir(tree.join("escape")).unwrap();
        fs::remove_dir_all(data).unwrap();
    }

    /// Retried recoveries stay parseable: a visited child is recorded at most once.
    #[test]
    fn repeated_recovery_does_not_grow_journal() {
        let data = std::env::temp_dir().join(format!("mc-growth-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&data, false).unwrap();
        let (path, tree, sid) = inherited_orphan(&data, |tree| {
            fs::create_dir(tree.join("nested")).unwrap();
            for index in 0..20 {
                fs::write(tree.join(format!("nested/{index}.txt")), b"x").unwrap();
            }
        });
        let root = data.join("sandbox-journals");
        assert!(recover_with_profile_delete(&root, |_| -1).is_err());
        let first = fs::read(&path).unwrap();
        for _ in 0..5 {
            assert!(recover_with_profile_delete(&root, |_| -1).is_err());
        }
        let last = fs::read(&path).unwrap();
        assert_eq!(
            first.len(),
            last.len(),
            "Failed recovery attempts must not append duplicate records"
        );
        assert!(last.split(|byte| *byte == b'\n').count() <= 30);
        assert!(!sddl(&raw(&tree.join("nested/7.txt"))).contains(&sid.text()));
        recover(&root).expect("The same record must still complete once deletion succeeds");
        assert!(!path.exists());
        fs::remove_dir_all(data).unwrap();
    }

    /// Scratch may hold a top-level junction; removing it must never touch the target.
    #[test]
    fn scratch_cleanup_removes_links_without_following() {
        let root = std::env::temp_dir().join(format!("mc-scratch-link-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&root, false).unwrap();
        let outside = root.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep.txt"), b"keep").unwrap();
        let scratch = root.join("scratch");
        platform_fs::private_dir(&scratch, false).unwrap();
        junction(&scratch.join("top"), &outside);
        fs::create_dir(scratch.join("nested")).unwrap();
        junction(&scratch.join("nested/inner"), &outside);
        fs::write(scratch.join("first.txt"), b"x").unwrap();
        fs::hard_link(scratch.join("first.txt"), scratch.join("second.txt")).unwrap();
        remove_scratch(&scratch).unwrap();
        assert!(!scratch.exists());
        assert_eq!(fs::read(outside.join("keep.txt")).unwrap(), b"keep");
        fs::remove_dir_all(root).unwrap();
    }

    /// A full journal stops recording discoveries but stays readable and keeps revoking.
    #[test]
    fn full_journal_stays_valid_and_revocation_continues() {
        let data = std::env::temp_dir().join(format!("mc-full-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&data, false).unwrap();
        let (path, tree, sid) = inherited_orphan(&data, |tree| {
            for index in 0..12 {
                fs::write(tree.join(format!("{index}.txt")), b"x").unwrap();
            }
        });
        let mut file = platform_fs::open_existing(&path, true, false).unwrap();
        let parsed = parse(&file, std::time::Instant::now() + std::time::Duration::from_secs(60))
            .unwrap()
            .unwrap();
        let root = reopen(&parsed.records[0]).unwrap().unwrap();
        let mut ledger = Ledger::resume(&parsed.records, parsed.lines).with_limit(5);
        revoke::tree(root, tree.clone(), &mut file, &mut ledger, sid.0).unwrap();
        for index in 0..12 {
            assert!(!sddl(&raw(&tree.join(format!("{index}.txt")))).contains(&sid.text()));
        }
        let parsed = parse(&file, std::time::Instant::now() + std::time::Duration::from_secs(60))
            .unwrap()
            .expect("A journal at its record budget must remain parseable");
        assert_eq!(parsed.lines, 5, "Header, root and three discovered children");
        assert!(parsed.torn.is_none());
        let outside = raw(&data);
        let required = Entry {
            path: data.clone(),
            identity: platform_fs::file_identity(&outside).unwrap(),
            directory: true,
            inheritance_root: false,
            discovered: false,
        };
        ledger.append(&mut file, &[required], true).expect("Launch records are not line-capped");
        drop(file);
        recover(&data.join("sandbox-journals")).unwrap();
        assert!(!path.exists());
        fs::remove_dir_all(data).unwrap();
    }
}
