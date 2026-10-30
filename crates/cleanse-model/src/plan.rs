use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Candidate, Concern, Disposal, FileIdentity, Risk, RuleId, Size};

/// The items a person approved for removal, and the folders the scan covered.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    /// Every item must still be inside one of these when it's removed.
    pub roots: Vec<PathBuf>,
    pub items: Vec<PlanItem>,
}

/// One approved item, as it was when the person approved it. Before removing it, Cleanse checks
/// that it's still the same item in the same state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanItem {
    pub path: PathBuf,
    pub rule: RuleId,
    pub identity: FileIdentity,
    pub risk: Risk,
    /// The warnings the person saw. A new kind of concern means the item has changed since.
    pub concerns: Vec<Concern>,
    pub size: Size,
    pub disposal: Disposal,
}

impl PlanItem {
    /// Approves a candidate for its tier's default disposal. Protected items, and items whose
    /// identity couldn't be read, can't be approved.
    pub fn approve(candidate: &Candidate) -> Option<Self> {
        Some(Self {
            path: candidate.found.path.clone(),
            rule: candidate.found.rule.clone(),
            identity: candidate.identity?,
            risk: candidate.risk(),
            concerns: candidate.concerns().to_vec(),
            size: candidate.found.size,
            disposal: candidate.default_disposal()?,
        })
    }
}

/// What happened to one approved item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    /// Removed for good. `size` was measured just before.
    Deleted { size: Size },
    /// Left alone because something about it changed since it was approved, or because the run
    /// stopped first.
    Skipped { reason: SkipReason },
    /// Removing it was attempted and didn't succeed.
    Failed { error: String },
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Deleted { .. } => f.write_str("Deleted"),
            Self::Skipped { reason } => write!(f, "Skipped: {reason}"),
            Self::Failed { error } => write!(f, "Failed: {error}"),
        }
    }
}

/// Why an approved item was left alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkipReason {
    /// Nothing is at its path anymore.
    Gone,
    /// Something else is at its path now: a different folder, or a link.
    Replaced,
    /// Its rule no longer recognizes it.
    NoLongerMatches,
    /// A safety check blocks it now.
    Blocked { concerns: Vec<Concern> },
    /// Its rule puts it in a higher tier than when it was approved.
    Risen { from: Risk, to: Risk },
    /// New kinds of warnings appeared after it was approved.
    NewConcerns { concerns: Vec<Concern> },
    /// Its path isn't valid Unicode, so the journal couldn't record it exactly.
    UnrecordablePath,
    /// This way of removing items isn't available yet.
    Unsupported { disposal: Disposal },
    /// The journal couldn't be written, so nothing more was removed.
    JournalUnavailable,
    /// The run was cancelled before reaching it.
    Cancelled,
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gone => f.write_str("it's no longer there"),
            Self::Replaced => f.write_str("something else is there now"),
            Self::NoLongerMatches => f.write_str("no rule recognizes it anymore"),
            Self::Blocked { concerns } => write!(f, "{}", joined(concerns)),
            Self::Risen { from, to } => write!(f, "it's {to} now, not {from}"),
            Self::NewConcerns { concerns } => {
                write!(f, "it changed after you approved it: {}", joined(concerns))
            }
            Self::UnrecordablePath => {
                f.write_str("its path isn't valid Unicode, so the journal can't record it")
            }
            Self::Unsupported {
                disposal: Disposal::Delete,
            } => f.write_str("deleting isn't available"),
            Self::Unsupported {
                disposal: Disposal::Quarantine,
            } => f.write_str("quarantine isn't available yet"),
            Self::Unsupported {
                disposal: Disposal::Trash,
            } => f.write_str("moving to the trash isn't available yet"),
            Self::JournalUnavailable => {
                f.write_str("the journal couldn't be written, so nothing more was removed")
            }
            Self::Cancelled => f.write_str("cancelled before it was reached"),
        }
    }
}

fn joined(concerns: &[Concern]) -> String {
    concerns
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CandidateId, Category, EntryKind, Match};

    fn candidate(declared_risk: Risk, identity: Option<FileIdentity>) -> Candidate {
        let found = Match {
            path: PathBuf::from("project/node_modules"),
            kind: EntryKind::Dir,
            rule: RuleId("javascript.node_modules".into()),
            category: Category::Dependencies,
            declared_risk,
            size: Size::default(),
            files: 0,
            evidence: Vec::new(),
        };
        Candidate::assess(CandidateId(0), found, identity, Vec::new())
    }

    const IDENTITY: FileIdentity = FileIdentity { volume: 1, file: 2 };

    #[test]
    fn approving_keeps_what_the_person_saw() {
        let item = PlanItem::approve(&candidate(Risk::Regenerable, Some(IDENTITY))).unwrap();

        assert_eq!(item.identity, IDENTITY);
        assert_eq!(item.risk, Risk::Regenerable);
        assert_eq!(item.disposal, Disposal::Delete);
    }

    #[test]
    fn protected_and_unverifiable_items_cant_be_approved() {
        assert_eq!(
            PlanItem::approve(&candidate(Risk::Protected, Some(IDENTITY))),
            None
        );
        assert_eq!(PlanItem::approve(&candidate(Risk::Safe, None)), None);
    }

    #[test]
    fn outcomes_read_as_sentences() {
        let blocked = Outcome::Skipped {
            reason: SkipReason::Blocked {
                concerns: vec![Concern::RecentlyModified { minutes_ago: 1 }],
            },
        };
        assert_eq!(blocked.to_string(), "Skipped: Modified a minute ago");
        let risen = SkipReason::Risen {
            from: Risk::Regenerable,
            to: Risk::Review,
        };
        assert_eq!(risen.to_string(), "it's Review now, not Regenerable");
    }

    #[test]
    fn outcomes_serialize_with_stable_snake_case_names() {
        let outcome = Outcome::Skipped {
            reason: SkipReason::Unsupported {
                disposal: Disposal::Quarantine,
            },
        };

        let json = serde_json::to_value(&outcome).unwrap();

        assert_eq!(
            json,
            serde_json::json!({
                "kind": "skipped",
                "reason": { "kind": "unsupported", "disposal": "quarantine" },
            })
        );
        assert_eq!(serde_json::from_value::<Outcome>(json).unwrap(), outcome);
    }
}
