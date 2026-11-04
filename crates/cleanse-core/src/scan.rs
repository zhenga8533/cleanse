//! Read-only filesystem traversal.

mod measure;
mod roots;
mod walk;

#[cfg(test)]
mod tests;

pub use measure::{Measurement, measure, measure_with};
pub use roots::distinct_roots;
pub use walk::{Descend, DirFiles, Entry, WalkError, WalkReport, walk, walk_with};
