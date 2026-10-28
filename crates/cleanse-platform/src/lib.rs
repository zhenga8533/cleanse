//! Read-only, per-OS filesystem facts behind a single `Probe` trait.
//!
//! File identity, link and reparse-point detection, cloud-placeholder state, allocated size,
//! mounts, elevation, and process queries live here so that all `cfg` and `unsafe` code sits in
//! one audited place. Nothing in this crate may modify the filesystem.
