//! The safety checks: whether each match may be removed, and what to know first.
//!
//! [`Assessor::assess`] runs the safety model's gates over a rule's match and turns it into a
//! [`Candidate`]. Every concern found is recorded, so `explain` can show all of them, and any
//! blocking one makes the item Protected.

mod policy;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{self, Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cleanse_model::{Candidate, CandidateId, Concern, EntryKind, FileIdentity, Match};
use cleanse_platform::{FileOwner, Probe};

pub(crate) use policy::key;
pub use policy::{Protected, ProtectedPolicy, Reach};

use crate::git::GitStatus;
use crate::rules::{Detection, Target};
use crate::scan::{Measurement, measure};

/// A file modified more recently than this suggests something is using the directory, such as
/// a running build or dev server pausing between writes.
const IN_USE_WINDOW: Duration = Duration::from_mins(10);

const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// Applies the safety gates to rule matches.
pub struct Assessor<'p> {
    probe: &'p dyn Probe,
    policy: ProtectedPolicy,
    roots: Vec<Vec<String>>,
    home: Option<PathBuf>,
    now: SystemTime,
    identities: Mutex<HashMap<PathBuf, Option<FileIdentity>>>,
    /// Listed the first time a match's rule has an owner. `None` if they couldn't be listed.
    running: OnceLock<Option<HashSet<String>>>,
    /// Read the first time a match's rule keeps items until they're idle. `None` if it couldn't
    /// be read.
    booted_at: OnceLock<Option<SystemTime>>,
}

impl<'p> Assessor<'p> {
    /// `roots` are the folders being scanned. Recent activity is measured back from `now`.
    pub fn new(
        probe: &'p dyn Probe,
        policy: ProtectedPolicy,
        roots: &[PathBuf],
        now: SystemTime,
    ) -> io::Result<Self> {
        let roots = roots
            .iter()
            .map(|root| path::absolute(root).map(|root| policy::key(&root)))
            .collect::<io::Result<_>>()?;
        Ok(Self {
            probe,
            policy,
            roots,
            home: probe.known_folders().home.clone(),
            now,
            identities: Mutex::default(),
            running: OnceLock::new(),
            booted_at: OnceLock::new(),
        })
    }

    pub fn assess(&self, id: CandidateId, detection: &Detection<'_>) -> Candidate {
        let path = &detection.path;
        let measured = measure(path);
        let mut measurement = measured.as_ref().cloned().unwrap_or_default();
        // Anything other than the kind of item the rule matched means it changed since the scan.
        let facts = self
            .probe
            .inspect(path)
            .ok()
            .filter(|facts| facts.kind == detection.kind);
        let identity = facts.map(|facts| facts.identity);
        // Windows listings don't count a file's hard links, but its own facts do. A folder's
        // link count means something else.
        if facts.is_some_and(|facts| facts.kind == EntryKind::File && facts.links > 1) {
            measurement.shared = true;
            measurement.size.guaranteed = 0;
        }

        let mut concerns = Vec::new();
        if let Some(what) = self
            .policy
            .check(path, identity, &self.ancestor_identities(path))
        {
            concerns.push(Concern::ProtectedLocation { what: what.into() });
        }
        if let Some(name) = &measurement.vm_disk {
            concerns.push(Concern::ProtectedLocation {
                what: format!("a virtual machine disk ({name})"),
            });
        }
        if measurement.mounts > 0 {
            concerns.push(Concern::ProtectedLocation {
                what: "another filesystem mounted inside".into(),
            });
        }
        if !self.inside_roots(path) {
            concerns.push(Concern::OutsideScanRoots);
        }
        // Without facts, the item is already Unverifiable.
        if let Some(facts) = facts {
            match facts.owner {
                None => concerns.push(Concern::OwnerUnknown),
                Some(FileOwner::System | FileOwner::Other) => {
                    concerns.push(Concern::OwnedByAnotherUser);
                }
                // Administrators own what the user's own elevated programs make on Windows, and
                // some drives don't record owners at all.
                Some(
                    FileOwner::CurrentUser | FileOwner::Administrators | FileOwner::Unrecorded,
                ) => {}
            }
        }
        match &detection.git {
            GitStatus::Known(git) if git.has_tracked_files => concerns.push(Concern::TrackedByGit),
            GitStatus::Unreadable { .. } => concerns.push(Concern::RepositoryUnreadable),
            GitStatus::Known(_) | GitStatus::NotInRepository => {}
        }
        if is_ambiguous(detection)
            && let Some(name) = &measurement.secret
        {
            concerns.push(Concern::PossibleSecrets {
                example: name.clone(),
            });
        }
        if measured.is_err() || measurement.unreadable > 0 {
            concerns.push(Concern::Incomplete);
        }
        if let Some(concern) = self.recent_activity(&measurement) {
            concerns.push(concern);
        }
        if let Some(provider) = self.cloud_provider(path, facts.is_some_and(|f| f.cloud_synced)) {
            concerns.push(Concern::CloudSynced { provider });
        }
        if measurement.shared {
            concerns.push(Concern::SharedData);
        }
        let rule = detection.rule;
        if let Some(days) = rule.idle_days {
            concerns.extend(self.not_idle(&measurement, days));
        }
        if let Some(owner) = &rule.owner {
            let app = owner.app.clone();
            match self.running() {
                Some(running) if owner.processes.iter().any(|name| running.contains(name)) => {
                    concerns.push(Concern::OwnerRunning { app });
                }
                Some(_) => {}
                None => concerns.push(Concern::OwnerUnchecked { app }),
            }
        }

        let found = Match {
            path: path.clone(),
            kind: detection.kind,
            rule: rule.id.clone(),
            category: rule.category,
            declared_risk: rule.risk,
            size: measurement.size,
            files: measurement.files,
            modified: measurement
                .newest_modified
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|since| since.as_secs()),
            evidence: detection.evidence.clone(),
        };
        Candidate::assess(id, found, identity, concerns)
    }

    fn running(&self) -> Option<&HashSet<String>> {
        self.running
            .get_or_init(|| self.probe.running_processes().ok())
            .as_ref()
    }

    /// Why an item whose rule keeps it until it's been unchanged for `days`, and since the
    /// computer last started, isn't there yet.
    fn not_idle(&self, measurement: &Measurement, days: u32) -> Vec<Concern> {
        let Some(last_change) = measurement.newest_change else {
            return Vec::new();
        };
        let mut concerns = Vec::new();
        // Changed in the future: a skewed clock, or something changing it right now.
        let unchanged_for = self
            .now
            .duration_since(last_change)
            .unwrap_or(Duration::ZERO);
        if unchanged_for < Duration::from_hours(24) * days {
            concerns.push(Concern::ChangedRecently {
                days_ago: unchanged_for.as_secs() / SECONDS_PER_DAY,
                keep_days: days,
            });
        }
        match self.booted_at.get_or_init(|| self.probe.booted_at().ok()) {
            Some(booted_at) if last_change > *booted_at => {
                concerns.push(Concern::ChangedSinceStartup);
            }
            Some(_) => {}
            None => concerns.push(Concern::StartupUnchecked),
        }
        concerns
    }

    /// The identities of `path`'s parent folders that could be read, nearest first. Many
    /// candidates share parents, so they're remembered.
    fn ancestor_identities(&self, path: &Path) -> Vec<FileIdentity> {
        let mut known = self
            .identities
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        path.ancestors()
            .skip(1)
            .filter_map(|ancestor| {
                *known.entry(ancestor.to_path_buf()).or_insert_with(|| {
                    self.probe
                        .inspect(ancestor)
                        .ok()
                        .map(|facts| facts.identity)
                })
            })
            .collect()
    }

    fn inside_roots(&self, path: &Path) -> bool {
        let path = policy::key(path);
        self.roots
            .iter()
            .any(|root| path.len() > root.len() && path.starts_with(root))
    }

    fn recent_activity(&self, measurement: &Measurement) -> Option<Concern> {
        let newest = measurement.newest_modified?;
        let age = match self.now.duration_since(newest) {
            Ok(age) if age >= IN_USE_WINDOW => return None,
            Ok(age) => age,
            // Modified in the future: a skewed clock, or a file being written right now.
            Err(_) => Duration::ZERO,
        };
        Some(Concern::RecentlyModified {
            minutes_ago: age.as_secs() / 60,
        })
    }

    /// Windows reports sync roots directly. Elsewhere, synced folders are recognized by where
    /// macOS keeps them.
    fn cloud_provider(&self, path: &Path, in_sync_root: bool) -> Option<String> {
        if in_sync_root {
            return Some("a cloud storage service".into());
        }
        let inside = path
            .strip_prefix(self.home.as_deref()?.join("Library"))
            .ok()?;
        let mut parts = inside
            .components()
            .map(|part| part.as_os_str().to_string_lossy());
        match parts.next()?.as_ref() {
            "Mobile Documents" => Some("iCloud Drive".into()),
            // Folders here are named like `OneDrive-Personal` or `GoogleDrive-name@example.com`.
            "CloudStorage" => {
                let folder = parts.next()?;
                Some(folder.split('-').next().unwrap_or(&folder).to_owned())
            }
            _ => None,
        }
    }
}

/// Rules that need Git to ignore a directory exist for names too common to trust on their
/// own, and a large file could be anything, so those matches also get checked for files that
/// look like secrets.
fn is_ambiguous(detection: &Detection<'_>) -> bool {
    matches!(
        detection.rule.target,
        Target::ProjectArtifact {
            require_git_ignored: true,
            ..
        } | Target::LargeFile {}
    )
}
