//! Shared data model for Cleanse: candidates, risk tiers, plans, and journal records.
//!
//! Every other crate and both front-ends speak in these types, and the desktop app gets
//! TypeScript bindings generated from them, so they stay plain data with no I/O.

mod candidate;
mod category;
mod entry;
mod identity;
mod risk;
mod size;

pub use candidate::{Candidate, CandidateId, Concern, Evidence, Match, RuleId};
pub use category::Category;
pub use entry::EntryKind;
pub use identity::FileIdentity;
pub use risk::{Disposal, Risk};
pub use size::Size;
