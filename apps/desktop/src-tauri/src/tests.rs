use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponseBody};
use tauri::test::{INVOKE_KEY, get_ipc_response, mock_builder};
use tauri::webview::InvokeRequest;

use crate::{configure, is_own_content, open_window};

/// Calls `cmd` from the app's window, in the app as it's built for real, with its capabilities,
/// on a runtime that shows nothing. The call comes from the window's own content unless `from`
/// says otherwise.
fn invoke_from(cmd: &str, from: Option<&str>) -> Result<InvokeResponseBody, serde_json::Value> {
    let app = configure(mock_builder())
        .build(tauri::generate_context!())
        .unwrap();
    let window = open_window(&app).unwrap();
    let url = match from {
        Some(url) => url.parse().unwrap(),
        None => window.url().unwrap(),
    };
    get_ipc_response(
        &window,
        InvokeRequest {
            cmd: cmd.to_owned(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url,
            body: InvokeBody::default(),
            headers: tauri::http::HeaderMap::default(),
            invoke_key: INVOKE_KEY.to_owned(),
        },
    )
}

fn invoke(cmd: &str) -> Result<InvokeResponseBody, serde_json::Value> {
    invoke_from(cmd, None)
}

#[test]
fn the_window_can_call_the_apps_commands() {
    let info: serde_json::Value = invoke("app_info").unwrap().deserialize().unwrap();

    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
    assert!(info["elevated"].is_boolean());
}

#[test]
fn the_window_cant_call_tauris_own_commands_or_plugins() {
    for cmd in [
        "plugin:event|listen",
        "plugin:path|resolve_directory",
        "plugin:webview|create_webview_window",
        "plugin:fs|read_file",
        "plugin:shell|execute",
        "no_such_command",
    ] {
        assert!(invoke(cmd).is_err(), "{cmd} was allowed");
    }
}

#[test]
fn other_pages_cant_call_the_apps_commands() {
    let error = invoke_from("app_info", Some("https://example.com/")).unwrap_err();

    assert!(error.to_string().contains("not allowed"), "{error}");
}

#[test]
fn the_window_shows_only_the_apps_own_content() {
    let own = |url: &str| is_own_content(&url.parse().unwrap());

    assert!(own("tauri://localhost/"));
    assert!(own("http://tauri.localhost/index.html"));
    assert!(!own("https://example.com/"));
    assert!(!own("http://tauri.localhost.example.com/"));
    assert!(!own("file:///C:/Windows/"));
    assert!(!own("http://localhost:8080/"));
    // Only development builds load the dev server, and tests are built that way.
    assert_eq!(own("http://localhost:1420/"), cfg!(dev));
}
