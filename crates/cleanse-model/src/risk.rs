use std::fmt;

use serde::{Deserialize, Serialize};

/// How much is at stake if an item is removed, ordered from least to most.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// The owning app recreates it on its own: temp files, logs, HTTP caches.
    Safe,
    /// Recreatable at a cost in time, bandwidth, or a command: `node_modules`, `target/`.
    Regenerable,
    /// Might matter, so a person has to decide: large files, old downloads.
    Review,
    /// Shown so the user can see it, but never removed.
    Protected,
}

impl Risk {
    /// How an approved item at this tier is removed unless the user chooses otherwise. Protected
    /// items can't be approved, so they have none.
    pub fn default_disposal(self) -> Option<Disposal> {
        match self {
            Self::Safe | Self::Regenerable => Some(Disposal::Delete),
            Self::Review => Some(Disposal::Quarantine),
            Self::Protected => None,
        }
    }
}

impl fmt::Display for Risk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Safe => "Safe",
            Self::Regenerable => "Regenerable",
            Self::Review => "Review",
            Self::Protected => "Protected",
        })
    }
}

/// How an approved item is removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposal {
    /// Removed for good. Regenerable items come back by rebuilding them.
    Delete,
    /// Moved into Cleanse's quarantine on the same drive, restorable until it's purged.
    Quarantine,
    /// Moved to the operating system's trash or Recycle Bin.
    Trash,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_are_ordered_from_least_to_most_at_stake() {
        assert!(Risk::Safe < Risk::Regenerable);
        assert!(Risk::Regenerable < Risk::Review);
        assert!(Risk::Review < Risk::Protected);
    }

    #[test]
    fn default_disposal_follows_the_tier() {
        assert_eq!(Risk::Safe.default_disposal(), Some(Disposal::Delete));
        assert_eq!(Risk::Regenerable.default_disposal(), Some(Disposal::Delete));
        assert_eq!(Risk::Review.default_disposal(), Some(Disposal::Quarantine));
        assert_eq!(Risk::Protected.default_disposal(), None);
    }
}
