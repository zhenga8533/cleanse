//! Read-only filesystem traversal.

mod walk;

#[cfg(test)]
mod tests;

pub use walk::{Descend, Entry, WalkError, WalkReport, walk};
