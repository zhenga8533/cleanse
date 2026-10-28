//! The only crate permitted to change the filesystem.
//!
//! Executes an approved plan item by item, re-validating each item through the safety gates
//! immediately before acting, and records every outcome in an append-only journal.
