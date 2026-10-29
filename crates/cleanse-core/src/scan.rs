//! Read-only filesystem traversal.

mod walk;

#[cfg(test)]
mod tests;

pub use walk::{Entry, EntryKind, WalkError, WalkReport, walk};
