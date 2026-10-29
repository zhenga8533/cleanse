//! Read-only filesystem traversal.

mod measure;
mod walk;

#[cfg(test)]
mod tests;

pub use measure::{Measurement, measure};
pub use walk::{Descend, Entry, WalkError, WalkReport, walk};
