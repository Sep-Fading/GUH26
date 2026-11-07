//! Preserves uncertain object identities after their unique sandbox identity is inert.
// Uses private journal identities and native cleanup.
use super::*;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Retirement {
    version: u32,
    profile: String,
    profile_deleted: bool,
    scratch_removed: bool,
    proof: Proof,
    unknown: Vec<UnknownIdentity>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub(super) enum Proof {
    Prelaunch,
    JobEmpty {
        job: String,
    },
    LaterBoot {
        job: String,
        current: crate::windows_network_broker::BootEpoch,
    },
    /// The kill-on-close job no longer existed and no process still carried the run identity.
    JobClosed {
        job: String,
    },
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct UnknownIdentity {
    kind: String,
    error: i32,
    object: Entry,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RetirementLine {
    retirement: Retirement,
}

/// Parses full-width object identifiers without an intermediate floating-point JSON value.
pub(super) fn parse(bytes: &[u8]) -> io::Result<Retirement> {
    Ok(serde_json::from_slice::<RetirementLine>(bytes)?.retirement)
}

impl Retirement {
    /// Records uncertainty without claiming that the original object was deleted.
    ///
    /// `unreadable` lists objects MiniClaw never granted that this user cannot open to check
    /// (access denied). A certificate that names any is version 2; one that names only unknown
    /// identities keeps version 1, byte for byte what earlier builds wrote.
    pub(super) fn new(profile: &str, proof: Proof, unknown: Vec<Entry>, unreadable: Vec<Entry>) -> Self {
        Self {
            version: if unreadable.is_empty() { 1 } else { 2 },
            profile: profile.into(),
            profile_deleted: true,
            scratch_removed: true,
            proof,
            unknown: unknown
                .into_iter()
                .map(|object| UnknownIdentity {
                    kind: "UnknownIdentity".into(),
                    error: 87,
                    object,
                })
                .chain(unreadable.into_iter().map(|object| UnknownIdentity {
                    kind: "Unreadable".into(),
                    error: 5,
                    object,
                }))
                .collect(),
        }
    }

    /// Accepts only a complete certificate bound to this journal's original records.
    pub(super) fn validate(
        &self,
        profile: &str,
        boot: &crate::windows_network_broker::BootEpoch,
        job: Option<&str>,
        records: &[Entry],
        started: std::time::Instant,
    ) -> io::Result<()> {
        let mut known = std::collections::HashSet::with_capacity(records.len());
        for record in records {
            if started.elapsed() > std::time::Duration::from_secs(120) {
                return Err(io::Error::other(
                    "Retirement validation exceeded its time bound",
                ));
            }
            known.insert(record);
        }
        let proof_matches = match &self.proof {
            Proof::Prelaunch => job.is_none(),
            Proof::JobEmpty { job: proven } | Proof::JobClosed { job: proven } => {
                job == Some(proven.as_str())
            }
            Proof::LaterBoot {
                job: proven,
                current,
            } => job == Some(proven.as_str()) && current.supersedes(boot),
        };
        // Version 2 adds objects never granted that this user cannot open; only they may say so.
        let kind_matches = |item: &UnknownIdentity| match (item.kind.as_str(), item.error) {
            ("UnknownIdentity", 87) => true,
            ("Unreadable", 5) => self.version == 2 && item.object.discovered && !item.object.inheritance_root,
            _ => false,
        };
        let unreadable = self.unknown.iter().any(|item| item.kind == "Unreadable");
        if !matches!(self.version, 1 | 2)
            || (self.version == 2) != unreadable
            || self.profile != profile
            || !self.profile_deleted
            || !self.scratch_removed
            || !proof_matches
            || self.unknown.is_empty()
            || self.unknown.iter().any(|item| {
                !kind_matches(item)
                    || !known.contains(&item.object)
                    || started.elapsed() > std::time::Duration::from_secs(120)
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid sandbox retirement certificate",
            ));
        }
        Ok(())
    }

    /// Flushes the completed cleanup proof before the journal becomes an inert archive.
    pub(super) fn persist(&self, file: &mut File) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(&RetirementLine {
            retirement: self.clone(),
        })?;
        bytes.push(b'\n');
        // The reader's bound leaves room for a certificate that repeats every record.
        if file.metadata()?.len() + bytes.len() as u64 > super::FILE_BYTES {
            return Err(io::Error::other(
                "Retirement certificate exceeds safe bounds",
            ));
        }
        file.seek(SeekFrom::End(0))?;
        file.write_all(&bytes)?;
        file.sync_all()
    }
}

/// Keeps archived run names reserved even after their AppContainer profile is deleted.
pub(super) fn ensure_unused_profile(root: &Path, profile: &str) -> io::Result<()> {
    match fs::symlink_metadata(
        root.join("retired")
            .join(format!("{profile}.retired.jsonl")),
    ) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Retired sandbox identities cannot be reused",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Atomically archives an already flushed certificate without replacing earlier evidence.
pub(super) fn archive(path: &Path, profile: &str) -> io::Result<()> {
    let root = path
        .parent()
        .ok_or(io::ErrorKind::InvalidInput)?
        .join("retired");
    platform_fs::private_dir(&root, true)?;
    let destination = root.join(format!("{profile}.retired.jsonl"));
    let source: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
