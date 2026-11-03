//! What commands return, shaped for the web view. `ts-rs` writes a TypeScript type for each to
//! `apps/desktop/src/bindings/` when the tests run, and CI fails if those are out of date.

use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct AppInfo {
    pub version: String,
    /// Cleanse runs as an administrator or root, so it won't change anything.
    pub elevated: bool,
}
