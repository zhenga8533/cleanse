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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

/// A plan saved to a file by `cleanse scan --save`, to review before `cleanse apply` removes its
/// selected items. Applying checks each item again, as `clean` does, so editing the file can't
/// make Cleanse remove anything its rules and safety checks wouldn't.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedPlan {
    /// The file format's version. The name marks the file as a Cleanse plan.
    pub cleanse_plan: u32,
    /// The version of Cleanse that saved it.
    pub saved_by: String,
    /// Seconds since the Unix epoch.
    pub saved_at: u64,
    pub roots: Vec<PathBuf>,
    pub items: Vec<SavedItem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedItem {
    /// Whether applying the plan removes it.
    pub selected: bool,
    #[serde(flatten)]
    pub item: PlanItem,
}

impl SavedPlan {
    /// The format this version of Cleanse saves and reads.
    pub const FORMAT: u32 = 1;

    /// Every candidate that can be approved, with the ones `clean` would preselect selected.
    /// Candidates whose paths aren't valid Unicode are left out, since the file can't hold their
    /// paths exactly.
    pub fn new(roots: &[PathBuf], candidates: &[Candidate], saved_at: u64) -> Self {
        let items = candidates
            .iter()
            .filter(|candidate| candidate.found.path.to_str().is_some())
            .filter_map(|candidate| {
                Some(SavedItem {
                    selected: candidate.preselected(),
                    item: PlanItem::approve(candidate)?,
                })
            })
            .collect();
        Self {
            cleanse_plan: Self::FORMAT,
            saved_by: env!("CARGO_PKG_VERSION").to_owned(),
            saved_at,
            roots: roots.to_vec(),
            items,
        }
    }

    /// The plan to apply: the selected items.
    pub fn selected(&self) -> Plan {
        Plan {
            roots: self.roots.clone(),
            items: self
                .items
                .iter()
                .filter(|saved| saved.selected)
                .map(|saved| saved.item.clone())
                .collect(),
        }
    }
}

/// What happened to one approved item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    /// Removed for good. `size` was measured just before.
    Deleted { size: Size },
    /// Moved into quarantine as `entry`, where it still takes `size` until it's purged.
    Quarantined { size: Size, entry: String },
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
            Self::Quarantined { entry, .. } => write!(f, "Quarantined as {entry}"),
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
    /// Its plan says to delete it, but items in its tier are only quarantined. Saved plans can
    /// be edited, so this is checked rather than trusted.
    NeedsQuarantine,
    /// Its path isn't valid Unicode, so the journal couldn't record it exactly.
    UnrecordablePath,
    /// This way of removing items isn't available yet.
    Unsupported { disposal: Disposal },
    /// It's on a different drive from Cleanse's quarantine, and moving it there would mean
    /// copying it.
    OtherDrive,
    /// The journal couldn't be written, so nothing more was removed.
    JournalUnavailable,
    /// The run was cancelled before reaching it.
    Cancelled,
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gone => f.write_str("It's no longer there"),
            Self::Replaced => f.write_str("Something else is there now"),
            Self::NoLongerMatches => f.write_str("No rule recognizes it anymore"),
            Self::Blocked { concerns } => write!(f, "{}", joined(concerns)),
            Self::Risen { from, to } => write!(f, "It's {to} now, not {from}"),
            Self::NewConcerns { concerns } => {
                write!(f, "It changed after you approved it: {}", joined(concerns))
            }
            Self::NeedsQuarantine => f.write_str("Review items are quarantined, not deleted"),
            Self::UnrecordablePath => {
                f.write_str("Its path isn't valid Unicode, so the journal can't record it")
            }
            Self::Unsupported {
                disposal: Disposal::Delete,
            } => f.write_str("Deleting isn't available"),
            Self::Unsupported {
                disposal: Disposal::Quarantine,
            } => f.write_str("Quarantine isn't available"),
            Self::Unsupported {
                disposal: Disposal::Trash,
            } => f.write_str("Moving to the trash isn't available yet"),
            Self::OtherDrive => f.write_str(
                "It's on a different drive from the quarantine, which can't hold it yet",
            ),
            Self::JournalUnavailable => {
                f.write_str("The journal couldn't be written, so nothing more was removed")
            }
            Self::Cancelled => f.write_str("Cancelled before it was reached"),
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
        warned(declared_risk, identity, Vec::new())
    }

    fn warned(
        declared_risk: Risk,
        identity: Option<FileIdentity>,
        concerns: Vec<Concern>,
    ) -> Candidate {
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
        Candidate::assess(CandidateId(0), found, identity, concerns)
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
    fn saved_plans_select_only_what_clean_would_preselect() {
        let candidates = [
            candidate(Risk::Regenerable, Some(IDENTITY)),
            warned(
                Risk::Regenerable,
                Some(IDENTITY),
                vec![Concern::UncommittedChanges],
            ),
            candidate(Risk::Review, Some(IDENTITY)),
            candidate(Risk::Protected, Some(IDENTITY)),
            candidate(Risk::Safe, None),
        ];

        let saved = SavedPlan::new(&[PathBuf::from("project")], &candidates, 7);

        let selection: Vec<(bool, Risk)> = saved
            .items
            .iter()
            .map(|saved| (saved.selected, saved.item.risk))
            .collect();
        assert_eq!(
            selection,
            [
                (true, Risk::Regenerable),
                (false, Risk::Regenerable),
                (false, Risk::Review)
            ]
        );
        let plan = saved.selected();
        assert_eq!(plan.roots, [PathBuf::from("project")]);
        assert_eq!(plan.items, [saved.items[0].item.clone()]);
    }

    #[test]
    fn saved_plans_are_flat_json_that_reads_back() {
        let saved = SavedPlan::new(
            &[PathBuf::from("project")],
            &[candidate(Risk::Review, Some(IDENTITY))],
            7,
        );

        let json = serde_json::to_value(&saved).unwrap();

        assert_eq!(json["cleanse_plan"], SavedPlan::FORMAT);
        assert_eq!(json["saved_by"], env!("CARGO_PKG_VERSION"));
        let item = &json["items"][0];
        assert_eq!(item["selected"], false);
        assert_eq!(item["path"], "project/node_modules");
        assert_eq!(item["disposal"], "quarantine");
        assert_eq!(serde_json::from_value::<SavedPlan>(json).unwrap(), saved);
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
        assert_eq!(risen.to_string(), "It's Review now, not Regenerable");
    }

    #[test]
    fn outcomes_serialize_with_stable_snake_case_names() {
        let outcome = Outcome::Skipped {
            reason: SkipReason::Unsupported {
                disposal: Disposal::Trash,
            },
        };

        let json = serde_json::to_value(&outcome).unwrap();

        assert_eq!(
            json,
            serde_json::json!({
                "kind": "skipped",
                "reason": { "kind": "unsupported", "disposal": "trash" },
            })
        );
        assert_eq!(serde_json::from_value::<Outcome>(json).unwrap(), outcome);
    }
}
