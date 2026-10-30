use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{RuleId, Size};

/// An item in Cleanse's quarantine. This is also what's saved beside it, so it can be listed and
/// put back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarantineEntry {
    pub id: String,
    /// Where it was, and where restoring it puts it back.
    pub original: PathBuf,
    pub rule: RuleId,
    /// What it takes up, and what purging it frees.
    pub size: Size,
    /// When it was quarantined, in seconds since the Unix epoch.
    pub at: u64,
}

/// What happened to a quarantined item when it was restored or purged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EntryOutcome {
    /// Moved back to where it was.
    Restored,
    /// Deleted for good.
    Purged { size: Size },
    /// Left in quarantine, because of `error`.
    Failed { error: String },
}
