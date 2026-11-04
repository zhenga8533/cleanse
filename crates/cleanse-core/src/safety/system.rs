//! What system cleanup checks on top of the user's cleanup (ADR 0010). Anyone can create files
//! in some system folders, such as `C:\Windows\Temp` and `/var/crash`, so an item there is only
//! removed if it, and everything in it, is the system's own, and it's directly inside a system
//! folder that was checked.

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cleanse_model::{Concern, EntryKind, FileIdentity, Risk};
use cleanse_platform::{FileFacts, FileOwner, Probe, SystemFolders};

use super::policy;
use crate::rules::{Os, RuleSet, Scope, Target, system_base_folder};
use crate::scan::Entry;

/// The folders system rules find removable items directly inside, each checked to be a real
/// folder the system owns, as is every folder from its base down to it. Folders that aren't, or
/// don't exist, are left out, so nothing in them is removed.
#[derive(Debug, Default)]
pub struct SystemRoots {
    /// Each folder's path, compared like [`policy::key`], and its identity when it was checked.
    verified: Vec<(Vec<String>, FileIdentity)>,
}

impl SystemRoots {
    /// Checks the folders that the locations of `rules` that remove things, on this OS, are
    /// directly inside.
    pub fn verify(rules: &RuleSet, folders: &SystemFolders, probe: &dyn Probe) -> Self {
        let mut checked = HashMap::new();
        let mut verified: Vec<(Vec<String>, FileIdentity)> = Vec::new();
        for (base, names) in fixed_folders(rules, folders) {
            let mut folder = base.to_path_buf();
            let mut identity = system_folder(probe, &mut checked, &folder);
            for name in names {
                folder.push(name);
                identity = identity.and(system_folder(probe, &mut checked, &folder));
            }
            let key = policy::key(&folder);
            if let Some(identity) = identity
                && !verified.iter().any(|(known, _)| *known == key)
            {
                verified.push((key, identity));
            }
        }
        Self { verified }
    }

    /// Whether `path` is directly inside one of the folders, and that's still the folder that
    /// was checked.
    pub(super) fn holds(&self, path: &Path, probe: &dyn Probe) -> bool {
        let Some(parent) = path.parent() else {
            return false;
        };
        let key = policy::key(parent);
        self.verified.iter().any(|(folder, identity)| {
            *folder == key
                && probe
                    .inspect(parent)
                    .is_ok_and(|facts| facts.kind == EntryKind::Dir && facts.identity == *identity)
        })
    }
}

/// Each folder that a location of a system rule that removes things, on this OS, is directly
/// inside: its base folder, and the names below that.
fn fixed_folders<'a>(
    rules: &'a RuleSet,
    folders: &'a SystemFolders,
) -> impl Iterator<Item = (&'a Path, Vec<&'a str>)> {
    rules
        .rules()
        .iter()
        .filter(|rule| rule.scope == Scope::System && rule.risk != Risk::Protected)
        .filter_map(|rule| match &rule.target {
            Target::KnownLocation { locations, .. } => Some(locations),
            Target::ProjectArtifact { .. } | Target::LargeFile {} => None,
        })
        .flatten()
        .filter(|location| location.os == Os::current())
        .filter_map(|location| {
            let base = system_base_folder(folders, location.base)?;
            let mut names: Vec<&str> = location.path.split('/').collect();
            names.pop();
            Some((base, names))
        })
}

/// The identity of the folder at `path`, if it's a real folder, not a link, that the system owns.
fn system_folder(
    probe: &dyn Probe,
    checked: &mut HashMap<PathBuf, Option<FileIdentity>>,
    path: &Path,
) -> Option<FileIdentity> {
    *checked.entry(path.to_path_buf()).or_insert_with(|| {
        probe
            .inspect(path)
            .ok()
            .filter(|facts| facts.kind == EntryKind::Dir && is_system(facts.owner))
            .map(|facts| facts.identity)
    })
}

/// Whether system cleanup counts `owner` as the operating system's. Administrators own what
/// elevated programs make on Windows, and only an elevated program can make something theirs.
fn is_system(owner: Option<FileOwner>) -> bool {
    matches!(owner, Some(FileOwner::System | FileOwner::Administrators))
}

/// What's wrong with an item that system cleanup could remove, itself: where it is and who owns
/// it. `facts` are its own, if they could be read.
pub(super) fn item_concerns(
    roots: &SystemRoots,
    probe: &dyn Probe,
    path: &Path,
    facts: Option<FileFacts>,
) -> Vec<Concern> {
    let mut concerns = Vec::new();
    if !roots.holds(path, probe) {
        concerns.push(Concern::OutsideSystemFolders);
    }
    // Without facts, the item is already Unverifiable.
    if let Some(facts) = facts {
        if facts.owner.is_none() {
            concerns.push(Concern::OwnerUnknown);
        } else if !is_system(facts.owner) {
            concerns.push(Concern::NotSystemOwned);
        }
    }
    concerns
}

/// What's wrong with `entry`, inside an item that system cleanup could remove, if anything.
pub(super) fn entry_concern(probe: &dyn Probe, entry: &Entry<'_>) -> Option<Concern> {
    if !matches!(entry.kind, EntryKind::File | EntryKind::Dir) {
        return Some(Concern::LinksInside);
    }
    let Ok(facts) = probe.inspect(entry.path) else {
        return Some(Concern::ContentsNotSystemOwned);
    };
    match facts.kind {
        EntryKind::Link | EntryKind::Other => return Some(Concern::LinksInside),
        // It changed since the folder was listed.
        kind if kind != entry.kind => return Some(Concern::Incomplete),
        EntryKind::File | EntryKind::Dir => {}
    }
    if !is_system(facts.owner) {
        return Some(Concern::ContentsNotSystemOwned);
    }
    (facts.kind == EntryKind::File && facts.links > 1).then_some(Concern::HardLinked)
}
