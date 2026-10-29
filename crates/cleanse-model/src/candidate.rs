use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize, Serializer};

use crate::{Category, Disposal, EntryKind, FileIdentity, Risk, Size};

/// Identifies the rule that matched an item, like `node.node_modules`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuleId(pub String);

impl fmt::Display for RuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identifies a candidate within one scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct CandidateId(pub u64);

/// What a rule found, before the safety checks have looked at it.
#[derive(Clone, Debug, Serialize)]
pub struct Match {
    #[serde(serialize_with = "serialize_path_lossy")]
    pub path: PathBuf,
    pub kind: EntryKind,
    pub rule: RuleId,
    pub category: Category,
    /// The tier the rule assigns. The safety checks can raise it but never lower it.
    pub declared_risk: Risk,
    pub size: Size,
    /// The number of files, counting the item itself if it is a file.
    pub files: u64,
    /// Facts the rule found that support removing the item.
    pub evidence: Vec<Evidence>,
}

/// A match the safety checks have assessed: whether it may be removed, and what to know first.
///
/// [`Candidate::assess`] is the only way to build one. It derives the risk from the match and
/// its concerns, so any blocking concern makes the item Protected.
#[derive(Clone, Debug, Serialize)]
pub struct Candidate {
    pub id: CandidateId,
    #[serde(flatten)]
    pub found: Match,
    /// Read when the item was scanned, and compared again before it's removed.
    #[serde(skip)]
    pub identity: Option<FileIdentity>,
    risk: Risk,
    concerns: Vec<Concern>,
}

impl Candidate {
    /// An item whose identity couldn't be read can't be re-checked before removal, so it's
    /// Protected, with [`Concern::Unverifiable`] explaining why.
    pub fn assess(
        id: CandidateId,
        found: Match,
        identity: Option<FileIdentity>,
        mut concerns: Vec<Concern>,
    ) -> Self {
        if identity.is_none() && found.declared_risk != Risk::Protected {
            concerns.push(Concern::Unverifiable);
        }
        let risk = if concerns.iter().any(Concern::blocks) {
            Risk::Protected
        } else {
            found.declared_risk
        };
        Self {
            id,
            found,
            identity,
            risk,
            concerns,
        }
    }

    pub fn risk(&self) -> Risk {
        self.risk
    }

    pub fn concerns(&self) -> &[Concern] {
        &self.concerns
    }

    /// Whether an interactive cleanup selects this item by default: only Safe and Regenerable
    /// items with nothing to warn about.
    pub fn preselected(&self) -> bool {
        matches!(self.risk, Risk::Safe | Risk::Regenerable) && self.concerns.is_empty()
    }

    pub fn default_disposal(&self) -> Option<Disposal> {
        self.risk.default_disposal()
    }
}

/// A fact supporting the removal of an item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Evidence {
    /// A file next to the item that marks the kind of project, like `Cargo.toml`.
    Marker { file: String },
    /// A file inside the item that identifies what it is, like a virtual environment's
    /// `pyvenv.cfg`.
    Contains { file: String },
    /// A `CACHEDIR.TAG` file inside, the standard way tools mark regenerable caches.
    CacheDirTag,
    /// The project's Git configuration ignores the item.
    IgnoredByGit,
}

impl fmt::Display for Evidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Marker { file } => write!(f, "Next to {file}"),
            Self::Contains { file } => write!(f, "Contains {file}"),
            Self::CacheDirTag => f.write_str("Contains CACHEDIR.TAG, the standard cache marker"),
            Self::IgnoredByGit => f.write_str("Ignored by Git"),
        }
    }
}

/// A reason to be careful about removing an item, found by the safety checks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Concern {
    /// The item is, is inside, or contains a location the protected policy covers.
    ProtectedLocation { what: String },
    /// The item isn't inside any of the folders the user chose to scan.
    OutsideScanRoots,
    /// Git tracks files inside the item.
    TrackedByGit,
    /// The item is inside a Git repository that couldn't be read, so tracked files can't be
    /// ruled out.
    RepositoryUnreadable,
    /// The item contains files that look like they hold secrets, such as `.env`.
    PossibleSecrets { example: String },
    /// Part of the item couldn't be read, so its contents aren't fully known.
    Incomplete,
    /// Something modified the item moments ago, so it's probably in use.
    RecentlyModified { minutes_ago: u64 },
    /// The app that owns the item is running.
    OwnerRunning { app: String },
    /// The item is in a cloud-synced folder, so removing it would remove it on every device.
    CloudSynced { provider: String },
    /// The item belongs to another user.
    OwnedByAnotherUser,
    /// The item's file identity couldn't be read, so it can't be re-checked before removal.
    Unverifiable,
    /// The project the item belongs to has uncommitted changes.
    UncommittedChanges,
    /// The project the item belongs to was worked on recently.
    RecentProjectActivity { days_ago: u64 },
    /// Some of the item's data is shared with files elsewhere, through hard links or clones.
    SharedData,
}

impl Concern {
    /// Whether this concern makes the item Protected. The others are warnings: the item can
    /// still be removed, but it isn't selected by default.
    pub fn blocks(&self) -> bool {
        // Listing the warnings rather than the blocks means a newly added concern blocks until
        // someone decides otherwise.
        !matches!(
            self,
            Self::UncommittedChanges | Self::RecentProjectActivity { .. } | Self::SharedData
        )
    }
}

impl fmt::Display for Concern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProtectedLocation { what } => write!(f, "Protected location: {what}"),
            Self::OutsideScanRoots => f.write_str("It's outside the folders chosen for the scan"),
            Self::TrackedByGit => f.write_str("Git tracks files inside it"),
            Self::RepositoryUnreadable => f.write_str(
                "Its Git repository couldn't be read, so tracked files can't be ruled out",
            ),
            Self::PossibleSecrets { example } => {
                write!(f, "Contains files that may hold secrets, such as {example}")
            }
            Self::Incomplete => f.write_str("Part of it couldn't be read"),
            Self::RecentlyModified { minutes_ago: 0 } => {
                f.write_str("Modified less than a minute ago")
            }
            Self::RecentlyModified { minutes_ago: 1 } => f.write_str("Modified a minute ago"),
            Self::RecentlyModified { minutes_ago } => {
                write!(f, "Modified {minutes_ago} minutes ago")
            }
            Self::OwnerRunning { app } => write!(f, "{app} is running"),
            Self::CloudSynced { provider } => write!(
                f,
                "Synced by {provider}, so removing it would remove it on every device"
            ),
            Self::OwnedByAnotherUser => f.write_str("Belongs to another user"),
            Self::Unverifiable => f.write_str(
                "Its file identity couldn't be read, so it can't be re-checked before removal",
            ),
            Self::UncommittedChanges => f.write_str("The project has uncommitted changes"),
            Self::RecentProjectActivity { days_ago: 0 } => {
                f.write_str("The project was worked on today")
            }
            Self::RecentProjectActivity { days_ago: 1 } => {
                f.write_str("The project was worked on yesterday")
            }
            Self::RecentProjectActivity { days_ago } => {
                write!(f, "The project was worked on {days_ago} days ago")
            }
            Self::SharedData => {
                f.write_str("Shares data with other files, so removing it frees less")
            }
        }
    }
}

/// For display only: names that aren't valid Unicode get replacement characters, so the result
/// can't always be turned back into the original path.
fn serialize_path_lossy<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&path.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDENTITY: Option<FileIdentity> = Some(FileIdentity {
        volume: 7,
        file: 42,
    });

    fn node_modules(declared_risk: Risk) -> Match {
        Match {
            path: PathBuf::from("project/node_modules"),
            kind: EntryKind::Dir,
            rule: RuleId("node.node_modules".into()),
            category: Category::Dependencies,
            declared_risk,
            size: Size {
                logical: 1000,
                on_disk: 4096,
                guaranteed: 4096,
            },
            files: 3,
            evidence: vec![Evidence::Marker {
                file: "package.json".into(),
            }],
        }
    }

    fn assess(declared_risk: Risk, concerns: Vec<Concern>) -> Candidate {
        Candidate::assess(
            CandidateId(1),
            node_modules(declared_risk),
            IDENTITY,
            concerns,
        )
    }

    #[test]
    fn without_concerns_the_declared_risk_stands() {
        let candidate = assess(Risk::Regenerable, vec![]);

        assert_eq!(candidate.risk(), Risk::Regenerable);
        assert!(candidate.preselected());
        assert_eq!(candidate.default_disposal(), Some(Disposal::Delete));
    }

    #[test]
    fn review_items_are_never_preselected() {
        let candidate = assess(Risk::Review, vec![]);

        assert!(!candidate.preselected());
        assert_eq!(candidate.default_disposal(), Some(Disposal::Quarantine));
    }

    #[test]
    fn a_blocking_concern_makes_any_item_protected() {
        for declared in [Risk::Safe, Risk::Regenerable, Risk::Review] {
            let candidate = assess(declared, vec![Concern::TrackedByGit]);

            assert_eq!(candidate.risk(), Risk::Protected, "{declared:?}");
            assert!(!candidate.preselected());
            assert_eq!(candidate.default_disposal(), None);
        }
    }

    #[test]
    fn a_warning_keeps_the_tier_but_deselects_the_item() {
        let candidate = assess(Risk::Regenerable, vec![Concern::UncommittedChanges]);

        assert_eq!(candidate.risk(), Risk::Regenerable);
        assert!(!candidate.preselected());
    }

    #[test]
    fn an_item_without_an_identity_is_protected_and_says_why() {
        let candidate = Candidate::assess(CandidateId(1), node_modules(Risk::Safe), None, vec![]);

        assert_eq!(candidate.risk(), Risk::Protected);
        assert_eq!(candidate.concerns(), [Concern::Unverifiable]);
    }

    #[test]
    fn a_declared_protected_item_needs_no_identity() {
        let candidate =
            Candidate::assess(CandidateId(1), node_modules(Risk::Protected), None, vec![]);

        assert_eq!(candidate.risk(), Risk::Protected);
        assert_eq!(candidate.concerns(), Vec::<Concern>::new());
    }

    #[test]
    fn only_project_state_and_shared_data_are_warnings() {
        let warnings = [
            Concern::UncommittedChanges,
            Concern::RecentProjectActivity { days_ago: 3 },
            Concern::SharedData,
        ];
        let blocks = [
            Concern::ProtectedLocation {
                what: "SSH keys".into(),
            },
            Concern::OutsideScanRoots,
            Concern::TrackedByGit,
            Concern::RepositoryUnreadable,
            Concern::PossibleSecrets {
                example: ".env".into(),
            },
            Concern::Incomplete,
            Concern::RecentlyModified { minutes_ago: 2 },
            Concern::OwnerRunning {
                app: "Claude Code".into(),
            },
            Concern::CloudSynced {
                provider: "OneDrive".into(),
            },
            Concern::OwnedByAnotherUser,
            Concern::Unverifiable,
        ];

        assert!(warnings.iter().all(|concern| !concern.blocks()));
        assert!(blocks.iter().all(Concern::blocks));
    }

    #[test]
    fn evidence_and_concerns_read_as_sentences() {
        assert_eq!(
            Evidence::Marker {
                file: "Cargo.toml".into()
            }
            .to_string(),
            "Next to Cargo.toml"
        );
        assert_eq!(
            Concern::RecentlyModified { minutes_ago: 1 }.to_string(),
            "Modified a minute ago"
        );
        assert_eq!(
            Concern::RecentProjectActivity { days_ago: 12 }.to_string(),
            "The project was worked on 12 days ago"
        );
    }

    #[test]
    fn serializes_with_stable_snake_case_names() {
        let candidate = assess(Risk::Regenerable, vec![Concern::UncommittedChanges]);

        assert_eq!(
            serde_json::to_value(&candidate).unwrap(),
            serde_json::json!({
                "id": 1,
                "path": "project/node_modules",
                "kind": "dir",
                "rule": "node.node_modules",
                "category": "dependencies",
                "declared_risk": "regenerable",
                "size": { "logical": 1000, "on_disk": 4096, "guaranteed": 4096 },
                "files": 3,
                "evidence": [{ "kind": "marker", "file": "package.json" }],
                "risk": "regenerable",
                "concerns": [{ "kind": "uncommitted_changes" }],
            })
        );
    }
}
