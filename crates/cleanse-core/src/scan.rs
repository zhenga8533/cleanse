//! Read-only filesystem traversal.

mod walk;
mod walk_dua;

#[cfg(test)]
mod tests;

pub use walk::{Entry, EntryKind, WalkError, WalkReport, walk_parallel};
pub use walk_dua::walk_dua;
