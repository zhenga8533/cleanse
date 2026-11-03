#![allow(clippy::disallowed_methods, reason = "tests build fixture trees")]

use std::fs::{self, File};
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder};
use tauri::webview::InvokeRequest;
use tauri::{App, Manager, WebviewWindow};

use crate::{AppState, configure, context, is_own_content, open_window};

/// The app as it's built for real, with its capabilities, on a runtime that shows nothing.
struct TestApp {
    app: App<MockRuntime>,
    window: WebviewWindow<MockRuntime>,
}

impl TestApp {
    fn new() -> Self {
        let app = configure(mock_builder()).build(context()).unwrap();
        let window = open_window(&app).unwrap();
        Self { app, window }
    }

    /// Calls `cmd` from the window's own content.
    fn invoke(&self, cmd: &str, args: Value) -> Result<Value, Value> {
        self.invoke_from(cmd, args, None)
    }

    /// Calls `cmd` from the window, as if from `from` if given.
    fn invoke_from(&self, cmd: &str, args: Value, from: Option<&str>) -> Result<Value, Value> {
        let url = match from {
            Some(url) => url.parse().unwrap(),
            None => self.window.url().unwrap(),
        };
        get_ipc_response(
            &self.window,
            InvokeRequest {
                cmd: cmd.to_owned(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url,
                body: InvokeBody::Json(args),
                headers: tauri::http::HeaderMap::default(),
                invoke_key: INVOKE_KEY.to_owned(),
            },
        )
        .map(|response| response.deserialize().unwrap())
    }

    /// Has scans cover `folder`, as if it had been chosen in the folder picker.
    fn choose(&self, folder: &Path) {
        self.app.state::<AppState>().choose([folder.to_path_buf()]);
    }
}

/// How the web view passes a channel. Progress sent on it goes nowhere on the mock runtime.
const CHANNEL: &str = "__CHANNEL__:1";

/// A JavaScript project whose dependencies can be removed.
fn project(root: &Path) {
    write_old(&root.join("app/package.json"), "{}");
    write_old(
        &root.join("app/node_modules/dep/index.js"),
        &"x".repeat(5000),
    );
}

/// Writes a file dated a day ago, since something modified moments ago is probably in use and
/// is kept.
fn write_old(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_hours(24))
        .unwrap();
}

#[test]
fn the_window_can_call_the_apps_commands() {
    let app = TestApp::new();

    let info = app.invoke("app_info", json!({})).unwrap();
    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
    assert!(info["elevated"].is_boolean());
    let folders = app.invoke("folders", json!({})).unwrap();
    assert_eq!(folders["home"], true);
}

#[test]
fn the_window_cant_call_tauris_own_commands_or_plugins() {
    let app = TestApp::new();

    for cmd in [
        "plugin:event|listen",
        "plugin:path|resolve_directory",
        "plugin:webview|create_webview_window",
        "plugin:dialog|open",
        "plugin:fs|read_file",
        "plugin:fs|remove",
        "plugin:shell|execute",
        "no_such_command",
    ] {
        assert!(app.invoke(cmd, json!({})).is_err(), "{cmd} was allowed");
    }
}

#[test]
fn other_pages_cant_call_the_apps_commands() {
    let app = TestApp::new();

    for cmd in ["app_info", "scan", "clean"] {
        let error = app
            .invoke_from(cmd, json!({}), Some("https://example.com/"))
            .unwrap_err();
        assert!(error.to_string().contains("not allowed"), "{cmd}: {error}");
    }
}

#[test]
fn scans_the_chosen_folders_and_reads_the_results() {
    let root = tempfile::tempdir().unwrap();
    project(root.path());
    let app = TestApp::new();
    app.choose(root.path());

    let folders = app.invoke("folders", json!({})).unwrap();
    assert_eq!(folders["home"], false);
    assert_eq!(folders["folders"], json!([root.path().to_string_lossy()]));

    let results = app.invoke("scan", json!({ "progress": CHANNEL })).unwrap();
    let scan = &results["scan"];
    let sections: Vec<&Value> = results["sections"].as_array().unwrap().iter().collect();
    let names: Vec<&Value> = sections.iter().map(|section| &section["section"]).collect();
    assert_eq!(names, ["removable", "review", "advice", "kept"]);
    assert_eq!(sections[0]["items"], 1);
    assert_eq!(sections[0]["rules"][0]["name"], "Node.js dependencies");

    let page = app
        .invoke(
            "candidates",
            json!({ "scan": scan, "section": "removable", "rule": null, "offset": 0, "limit": 10 }),
        )
        .unwrap();
    assert_eq!(page["total"], 1);
    let item = &page["items"][0];
    assert!(item["path"].as_str().unwrap().ends_with("node_modules"));
    assert_eq!(item["disposal"], "delete");

    let details = app
        .invoke("explain", json!({ "scan": scan, "item": item["id"] }))
        .unwrap();
    assert_eq!(details["ruleName"], "Node.js dependencies");
    assert_eq!(details["evidence"], json!(["Next to package.json"]));

    let again = app.invoke("summary", json!({ "scan": scan })).unwrap();
    assert_eq!(again["sections"][0]["items"], 1);
}

#[test]
fn a_cleanup_names_items_by_the_current_scans_ids() {
    let root = tempfile::tempdir().unwrap();
    project(root.path());
    let app = TestApp::new();
    app.choose(root.path());
    let scan = app.invoke("scan", json!({ "progress": CHANNEL })).unwrap()["scan"].clone();
    let clean = |args: Value| app.invoke("clean", args).unwrap_err().to_string();

    // Refused before anything is touched, so these never remove anything.
    let old = scan.as_u64().unwrap() + 1;
    assert!(
        clean(json!({ "scan": old, "items": [0], "progress": CHANNEL })).contains("earlier scan")
    );
    assert!(
        clean(json!({ "scan": scan, "items": [999], "progress": CHANNEL }))
            .contains("has no item 999")
    );
    assert!(project_is_whole(root.path()));
}

#[test]
fn only_chosen_folders_can_be_removed_from_the_list() {
    let app = TestApp::new();

    assert!(
        app.invoke("remove_folder", json!({ "index": 0 }))
            .unwrap_err()
            .to_string()
            .contains("no chosen folder")
    );

    let root = tempfile::tempdir().unwrap();
    app.choose(root.path());
    app.choose(root.path());
    let folders = app.invoke("folders", json!({})).unwrap();
    assert_eq!(folders["folders"].as_array().unwrap().len(), 1);
    let folders = app.invoke("remove_folder", json!({ "index": 0 })).unwrap();
    assert_eq!(folders["home"], true);
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

fn project_is_whole(root: &Path) -> bool {
    root.join("app/node_modules/dep/index.js").is_file()
}
