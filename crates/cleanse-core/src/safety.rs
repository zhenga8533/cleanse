//! The safety checks: whether each match may be removed, and what to know first.
//!
//! [`Assessor::assess`] runs the safety model's gates over a rule's match and turns it into a
//! [`Candidate`]. Every concern found is recorded, so `explain` can show all of them, and any
//! blocking one makes the item Protected. System cleanup has gates of its own, in [`system`].

mod policy;
mod system;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{self, Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cleanse_model::{Candidate, CandidateId, Concern, EntryKind, FileIdentity, Match, Risk};
use cleanse_platform::{FileFacts, FileOwner, Probe};

pub(crate) use policy::key;
pub use policy::{Protected, ProtectedPolicy, Reach};
pub use system::{SystemRoots, scan_folders};

use crate::git::GitStatus;
use crate::rules::{Detection, Target};
use crate::scan::{Measurement, measure_with};

/// A file modified more recently than this suggests something is using the directory, such as
/// a running build or dev server pausing between writes.
const IN_USE_WINDOW: Duration = Duration::from_mins(10);

const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// Applies the safety gates to rule matches.
pub struct Assessor<'p> {
    probe: &'p dyn Probe,
    policy: ProtectedPolicy,
    cleanup: Cleanup,
    home: Option<PathBuf>,
    now: SystemTime,
    identities: Mutex<HashMap<PathBuf, Option<FileIdentity>>>,
    /// Listed the first time a match's rule has an owner. `None` if they couldn't be listed.
    running: OnceLock<Option<HashSet<String>>>,
    /// Read the first time a match's rule keeps items until they're idle. `None` if it couldn't
    /// be read.
    booted_at: OnceLock<Option<SystemTime>>,
    /// Once set, measuring goes no deeper. See [`Assessor::stopping_on`].
    stop: Option<&'p AtomicBool>,
}

/// Which cleanup an [`Assessor`] checks items for.
#[derive(Debug)]
enum Cleanup {
    /// The user's, of the folders being scanned, compared like [`policy::key`].
    User { roots: Vec<Vec<String>> },
    /// The system's, of items directly inside the system folders that checked out.
    /// `thorough` also reads who owns every entry inside each item.
    System { roots: SystemRoots, thorough: bool },
}

impl<'p> Assessor<'p> {
    /// Checks the user's items. `roots` are the folders being scanned. Recent activity is
    /// measured back from `now`.
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
            cleanup: Cleanup::User { roots },
            home: probe.known_folders().home.clone(),
            now,
            identities: Mutex::default(),
            running: OnceLock::new(),
            booted_at: OnceLock::new(),
            stop: None,
        })
    }

    /// Checks system cleanup's items, with `policy` from
    /// [`ProtectedPolicy::for_system_cleanup`] and the system folders `roots` that checked out.
    /// Recent activity is measured back from `now`.
    ///
    /// Only what folder listings show is checked inside each item: links and special files, and
    /// hard links on Unix. [`Assessor::thorough`] checks the rest.
    pub fn system(
        probe: &'p dyn Probe,
        policy: ProtectedPolicy,
        roots: SystemRoots,
        now: SystemTime,
    ) -> Self {
        Self {
            probe,
            policy,
            cleanup: Cleanup::System {
                roots,
                thorough: false,
            },
            home: None,
            now,
            identities: Mutex::default(),
            running: OnceLock::new(),
            booted_at: OnceLock::new(),
            stop: None,
        }
    }

    /// For system cleanup, also reads who owns every entry inside each item, and its hard links
    /// on Windows. That takes a file handle per entry, which can take minutes for an item with
    /// hundreds of thousands of files, so system cleanup does it right before removing an item
    /// rather than when scanning (ADR 0010).
    /// Measures each item only until `stop` is set, so an assessment made after then leaves out
    /// whatever it didn't reach, and mustn't be acted on.
    #[must_use]
    pub fn stopping_on(mut self, stop: &'p AtomicBool) -> Self {
        self.stop = Some(stop);
        self
    }

    #[must_use]
    pub fn thorough(mut self) -> Self {
        if let Cleanup::System { thorough, .. } = &mut self.cleanup {
            *thorough = true;
        }
        self
    }

    /// The system folders system cleanup checks `detection` against, and whether it reads who
    /// owns everything inside, or `None` if its own gates don't apply.
    fn system_gates(&self, detection: &Detection<'_>) -> Option<(&SystemRoots, bool)> {
        match &self.cleanup {
            // Nothing a `protected` rule matches is ever removed, so they're only for the rest.
            Cleanup::System { roots, thorough } if detection.rule.risk != Risk::Protected => {
                Some((roots, *thorough))
            }
            Cleanup::User { .. } | Cleanup::System { .. } => None,
        }
    }

    pub fn assess(&self, id: CandidateId, detection: &Detection<'_>) -> Candidate {
        let path = &detection.path;
        let system_gates = self.system_gates(detection);
        let system_roots = system_gates.map(|(roots, _)| roots);
        let measured = match system_gates {
            Some((_, thorough)) => measure_with(
                path,
                |entry| system::entry_concern(self.probe, entry, thorough),
                self.stop,
            ),
            None => measure_with(path, |_| None, self.stop),
        };
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
        match &self.cleanup {
            Cleanup::User { roots } => user_concerns(roots, detection, facts, &mut concerns),
            Cleanup::System { .. } => {
                if let Some(roots) = system_roots {
                    concerns.extend(system::item_concerns(roots, self.probe, path, facts));
                    concerns.extend(measurement.inside.iter().cloned());
                }
            }
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
            // Removing one name of a hard-linked file frees nothing, and in a system folder
            // the other name could be anywhere, so system cleanup keeps it.
            if system_roots.is_none() {
                concerns.push(Concern::SharedData);
            } else if !concerns.contains(&Concern::HardLinked) {
                concerns.push(Concern::HardLinked);
            }
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

/// The user's cleanup's own gates: the item must be inside the folders being scanned and belong
/// to the user, and Git mustn't track anything in it.
fn user_concerns(
    roots: &[Vec<String>],
    detection: &Detection<'_>,
    facts: Option<FileFacts>,
    concerns: &mut Vec<Concern>,
) {
    let key = policy::key(&detection.path);
    if !roots
        .iter()
        .any(|root| key.len() > root.len() && key.starts_with(root))
    {
        concerns.push(Concern::OutsideScanRoots);
    }
    // Without facts, the item is already Unverifiable.
    if let Some(facts) = facts {
        match facts.owner {
            None => concerns.push(Concern::OwnerUnknown),
            Some(FileOwner::System | FileOwner::Other) => {
                concerns.push(Concern::OwnedByAnotherUser);
            }
            // Administrators own what the user's own elevated programs make on Windows, and some
            // drives don't record owners at all.
            Some(FileOwner::CurrentUser | FileOwner::Administrators | FileOwner::Unrecorded) => {}
        }
    }
    match &detection.git {
        GitStatus::Known(git) if git.has_tracked_files => concerns.push(Concern::TrackedByGit),
        // Only system cleanup leaves Git unread, and its items are never the user's, so one that
        // is can't be shown free of tracked files.
        GitStatus::Unreadable { .. } | GitStatus::NotChecked => {
            concerns.push(Concern::RepositoryUnreadable);
        }
        GitStatus::Known(_) | GitStatus::NotInRepository => {}
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
