//! Session API over the scan, plan, and apply pipeline.
//!
//! The CLI and desktop app call only this crate. It owns progress reporting, cancellation, and
//! the in-memory scan index, so front-ends never touch the filesystem directly.
