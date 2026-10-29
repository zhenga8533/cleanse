//! The safety checks: whether each match may be removed, and what to know first.
//!
//! [`Assessor::assess`] runs the safety model's gates over a rule's match and turns it into a
//! [`Candidate`]. Every concern found is recorded, so `explain` can show all of them, and any
//! blocking one makes the item Protected.

mod policy;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::io;
use std::path::{self, Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use cleanse_model::{Candidate, CandidateId, Concern, EntryKind, FileIdentity, Match};
use cleanse_platform::Probe;

pub use policy::{Protected, ProtectedPolicy, Reach};

use crate::git::GitStatus;
use crate::rules::{Detection, Target};
use crate::scan::{Measurement, measure};

/// A file modified more recently than this suggests something is using the directory, such as
/// a running build or dev server pausing between writes.
const IN_USE_WINDOW: Duration = Duration::from_mins(10);

/// Applies the safety gates to rule matches.
pub struct Assessor<'p> {
    probe: &'p dyn Probe,
    policy: ProtectedPolicy,
    roots: Vec<Vec<String>>,
    home: Option<PathBuf>,
    now: SystemTime,
    identities: Mutex<HashMap<PathBuf, Option<FileIdentity>>>,
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
        })
    }

    pub fn assess(&self, id: CandidateId, detection: &Detection<'_>) -> Candidate {
        let path = &detection.path;
        let measured = measure(path);
        let measurement = measured.as_ref().cloned().unwrap_or_default();
        // Rules only match directories, so anything else means it changed since the scan.
        let facts = self
            .probe
            .inspect(path)
            .ok()
            .filter(|facts| facts.kind == EntryKind::Dir);
        let identity = facts.map(|facts| facts.identity);

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
        let found = Match {
            path: path.clone(),
            kind: EntryKind::Dir,
            rule: rule.id.clone(),
            category: rule.category,
            declared_risk: rule.risk,
            size: measurement.size,
            files: measurement.files,
            evidence: detection.evidence.clone(),
        };
        Candidate::assess(id, found, identity, concerns)
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
/// own, so those matches also get checked for files that look like secrets.
fn is_ambiguous(detection: &Detection<'_>) -> bool {
    matches!(
        detection.rule.target,
        Target::ProjectArtifact {
            require_git_ignored: true,
            ..
        }
    )
}
