//! Read-only cleanup analysis: scanning, rule matching, safety evaluation, and planning.
//!
//! This crate decides what could be removed and why; it never removes anything. Only
//! `cleanse-exec` may change the filesystem (ADR 0002).

pub mod git;
pub mod rules;
pub mod scan;
