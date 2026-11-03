//! The desktop app: a window onto the engine's [`Session`]. Its commands name everything by an ID
//! the session gave out and never take a path to remove (ADR 0009).

mod commands;
#[cfg(test)]
mod tests;
mod views;

use cleanse_engine::{Engine, Session};
use tauri::webview::NewWindowResponse;
use tauri::{App, Builder, Runtime, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// The session, or why the engine couldn't start, such as a mistake in the settings file. The
/// app shows that rather than closing.
pub(crate) struct AppState(Result<Session, String>);

impl AppState {
    fn session(&self) -> Result<&Session, String> {
        self.0.as_ref().map_err(Clone::clone)
    }
}

/// Opens the app's window and runs until it's closed.
pub fn run() {
    configure(Builder::default())
        .setup(|app| {
            open_window(app)?;
            Ok(())
        })
        .run(context())
        .expect("the app couldn't start");
}

/// The app's configuration and assets. On macOS, `generate_context!` embeds the app's
/// Info.plist under a fixed symbol, so a program can only expand it once.
pub(crate) fn context<R: Runtime>() -> tauri::Context<R> {
    tauri::generate_context!()
}

/// The app's state and commands, for any runtime, so tests can use a mock one.
pub(crate) fn configure<R: Runtime>(builder: Builder<R>) -> Builder<R> {
    let state = Engine::new()
        .map(Session::new)
        .map_err(|error| error.to_string());
    builder
        .manage(AppState(state))
        .invoke_handler(tauri::generate_handler![commands::app_info])
}

/// The app's one window, which `capabilities/main.json` names.
pub(crate) fn open_window<R: Runtime>(app: &App<R>) -> tauri::Result<WebviewWindow<R>> {
    let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
        .title("Cleanse")
        .inner_size(1100.0, 720.0)
        .min_inner_size(720.0, 480.0)
        .on_navigation(is_own_content)
        .on_new_window(|_, _| NewWindowResponse::Deny);
    #[cfg(all(windows, debug_assertions))]
    let window = with_debug_port(window);
    window.build()
}

/// Opens WebView2's debugging port when `CLEANSE_WEBVIEW_DEBUG_PORT` names one, so the window
/// can be inspected from another program while developing. Release builds never have this: a
/// debugging port lets any local program drive the window.
#[cfg(all(windows, debug_assertions))]
fn with_debug_port<R: Runtime, M: tauri::Manager<R>>(
    window: WebviewWindowBuilder<'_, R, M>,
) -> WebviewWindowBuilder<'_, R, M> {
    match std::env::var("CLEANSE_WEBVIEW_DEBUG_PORT").map(|port| port.parse::<u16>()) {
        // Setting arguments replaces Tauri's own, so they're repeated.
        Ok(Ok(port)) => window.additional_browser_args(&format!(
            "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
             --remote-debugging-port={port}"
        )),
        _ => window,
    }
}

/// Whether `url` is the app's own content, the only thing its window may show. Tauri serves the
/// built frontend from `tauri://localhost`, or `http://tauri.localhost` on Windows.
fn is_own_content(url: &Url) -> bool {
    let built = url.scheme() == "tauri"
        || (url.scheme() == "http" && url.host_str() == Some("tauri.localhost"));
    // `devUrl` in tauri.conf.json, which only development builds load.
    let dev_server = cfg!(dev) && url.host_str() == Some("localhost") && url.port() == Some(1420);
    built || dev_server
}
