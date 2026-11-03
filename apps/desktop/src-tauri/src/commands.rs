//! What the web view can ask for. Each command is also listed in `build.rs` and allowed in
//! `capabilities/main.json`, or the web view can't call it.

#![allow(
    clippy::needless_pass_by_value,
    reason = "Tauri hands commands their arguments, `State` among them, by value"
)]

use cleanse_engine::Probe;
use tauri::State;

use crate::AppState;
use crate::views::AppInfo;

#[tauri::command]
pub(crate) fn app_info(state: State<'_, AppState>) -> Result<AppInfo, String> {
    let session = state.session()?;
    Ok(AppInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        elevated: session.engine().system().is_elevated(),
    })
}
