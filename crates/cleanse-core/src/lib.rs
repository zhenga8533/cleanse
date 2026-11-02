//! Read-only cleanup analysis: scanning, rule matching, safety evaluation, and planning.
//!
//! This crate decides what could be removed and why; it never removes anything. Only
//! `cleanse-exec` may change the filesystem (ADR 0002).

pub mod config;
pub mod git;
pub mod rules;
pub mod safety;
pub mod scan;

/// The start of the name `cleanse-exec` gives an item while deleting it, beside where it was. No
/// rule matches one: what's left of a removal that stopped is only removed by finishing it,
/// which checks that it's what was approved.
pub const TOMBSTONE_PREFIX: &str = ".cleanse-tombstone-";
