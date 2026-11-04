#![allow(clippy::disallowed_methods, reason = "tests build fixture trees")]

use std::fs::{self, File};
use std::io::{BufReader, pipe};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, SystemTime};

use cleanse_elevate::server::serve;
use cleanse_engine::{Engine, KnownFolders, Session, System};
use serde_json::{Value, json};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder};
use tauri::webview::InvokeRequest;
use tauri::{App, Manager, WebviewWindow};

use crate::system::{Connection, Starter, SystemState};
use crate::{AppState, configure, context, is_own_content, open_window};

/// The app as it's built for real, with its capabilities, on a runtime that shows nothing.
struct TestApp {
    app: App<MockRuntime>,
    window: WebviewWindow<MockRuntime>,
}

impl TestApp {
    fn new() -> Self {
        Self::with_system(SystemState::installed())
    }

    /// With `system` for system cleanup's helper.
    fn with_system(system: Arc<SystemState>) -> Self {
        let app = configure(mock_builder(), system).build(context()).unwrap();
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

    for cmd in ["app_info", "scan", "clean", "restore", "purge", "finish"] {
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
    assert_eq!(again["categories"][0]["category"], "dependencies");
}

#[test]
fn shows_where_space_goes_folder_by_folder() {
    let root = tempfile::tempdir().unwrap();
    project(root.path());
    let app = TestApp::new();
    app.choose(root.path());
    let scan = app.invoke("scan", json!({ "progress": CHANNEL })).unwrap()["scan"].clone();
    let folder = |node: &Value, categories: Value| {
        app.invoke(
            "folder",
            json!({ "scan": scan, "node": node, "limit": 10, "categories": categories }),
        )
    };
    let named = |view: &Value, name: &str| -> Value {
        view["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("no {name} in {view}"))
    };

    let roots = folder(&Value::Null, json!([])).unwrap();
    assert_eq!(roots["trail"], json!([]));
    let top = folder(&roots["entries"][0]["node"], json!([])).unwrap();
    let project = named(&top, "app");
    let inside = folder(&project["node"], json!([])).unwrap();
    let node_modules = named(&inside, "node_modules");

    assert_eq!(inside["trail"].as_array().unwrap().len(), 2);
    assert_eq!(node_modules["kind"], "item");
    assert_eq!(node_modules["item"]["ruleName"], "Node.js dependencies");
    assert_eq!(project["removable"], node_modules["usage"]["onDisk"]);
    let logs_only = folder(&top["entries"][0]["node"], json!(["logs"])).unwrap();
    assert_eq!(logs_only["entries"][0]["removable"], 0);
    assert!(
        folder(&Value::Null, json!(["no_such_category"]))
            .unwrap_err()
            .to_string()
            .contains("no category")
    );
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
fn reads_the_quarantine_history_settings_and_rules() {
    let app = TestApp::new();

    // What's in them depends on what this computer's Cleanse has done, so only their shape is
    // checked.
    let quarantine = app.invoke("quarantined", json!({})).unwrap();
    assert!(quarantine["entries"].is_array());
    let history = app.invoke("history", json!({})).unwrap();
    assert!(history["runs"].is_array() && history["leftovers"].is_array());

    let settings = app.invoke("settings", json!({})).unwrap();
    assert_eq!(settings["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(settings["folders"][0]["name"], "Home");
    assert!(settings["largeFileSize"].as_u64().unwrap() > 0);

    let rules = app.invoke("rules", json!({})).unwrap();
    let node_modules = rules
        .as_array()
        .unwrap()
        .iter()
        .find(|rule| rule["id"] == "javascript.node_modules")
        .unwrap();
    assert_eq!(node_modules["matches"]["kind"], "projectArtifact");
    assert_eq!(node_modules["matches"]["names"], json!(["node_modules"]));
    assert_eq!(node_modules["risk"], "regenerable");
}

#[test]
fn restoring_or_purging_names_entries_by_id() {
    let app = TestApp::new();

    // Neither changes anything: there's nothing to do, or the ID is refused first.
    let nothing = app.invoke("restore", json!({ "ids": [] })).unwrap();
    assert_eq!(nothing["done"]["items"], 0);
    assert_eq!(nothing["journal"], Value::Null);
    let unknown = app
        .invoke("purge", json!({ "ids": ["no-such-entry"] }))
        .unwrap_err();
    assert!(
        unknown
            .to_string()
            .contains("Nothing in quarantine has the ID no-such-entry"),
        "{unknown}"
    );
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

/// Every system cleanup command.
const SYSTEM_COMMANDS: [&str; 13] = [
    "system_status",
    "system_scan",
    "system_cancel",
    "system_summary",
    "system_candidates",
    "system_explain",
    "system_clean",
    "system_quarantined",
    "system_restore",
    "system_purge",
    "system_history",
    "system_finish",
    "system_rules",
];

/// Serves the user's cleanup of `home`, as the helper serves system cleanup's: the protocol
/// doesn't tell them apart. It acts as if it isn't elevated, since CI runs tests as an
/// administrator on Windows.
struct InProcess {
    home: PathBuf,
    data: PathBuf,
    /// How long each helper waits for a request before it stops.
    idle: Duration,
    /// How many helpers it has started.
    started: Arc<AtomicUsize>,
}

impl InProcess {
    fn new(home: &Path, data: &Path, idle: Duration) -> Self {
        Self {
            home: home.to_path_buf(),
            data: data.to_path_buf(),
            idle,
            started: Arc::default(),
        }
    }
}

impl Starter for InProcess {
    fn unavailable(&self) -> Option<String> {
        None
    }

    fn start(&self, _: Option<usize>) -> Result<Connection, String> {
        self.started.fetch_add(1, Ordering::Relaxed);
        let folders = KnownFolders {
            home: Some(self.home.clone()),
            temp: self.home.join("temp"),
            ..KnownFolders::default()
        };
        let system = System::new()
            .unwrap()
            .elevated(false)
            .with_known_folders(folders);
        let engine = Engine::user_at(system, self.data.clone()).unwrap();
        let session = Arc::new(Session::new(engine));
        let (served, requests) = pipe().unwrap();
        let (answers, answering) = pipe().unwrap();
        let idle = self.idle;
        thread::spawn(move || serve(&session, BufReader::new(served), answering, idle));
        Ok(Connection {
            answers: Box::new(BufReader::new(answers)),
            requests: Box::new(requests),
            process: None,
        })
    }
}

#[test]
fn other_pages_cant_call_the_system_commands() {
    let app = TestApp::new();

    for cmd in SYSTEM_COMMANDS {
        let error = app
            .invoke_from(cmd, json!({}), Some("https://example.com/"))
            .unwrap_err();
        assert!(error.to_string().contains("not allowed"), "{cmd}: {error}");
    }
}

/// Tests don't run from where an installer puts the app, so it has no helper to start, and says
/// why. On macOS that's that system cleanup is for Windows and Linux.
#[test]
fn without_its_helper_the_app_says_why_and_starts_nothing() {
    let app = TestApp::new();

    let status = app.invoke("system_status", json!({})).unwrap();
    let why = status["unavailable"].as_str().unwrap();
    assert_eq!(status["supported"], cfg!(any(windows, target_os = "linux")));
    assert_eq!(status["running"], false);
    let error = app.invoke("system_history", json!({})).unwrap_err();
    assert_eq!(error, json!(why));
    // Nothing is running to cancel.
    app.invoke("system_cancel", json!({})).unwrap();
}

/// They're built in, so listing them needs no helper.
#[test]
fn lists_system_cleanups_rules() {
    let app = TestApp::new();

    let rules = app.invoke("system_rules", json!({})).unwrap();

    let ids: Vec<&str> = rules
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| rule["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"windows.memory_dumps"), "{ids:?}");
    assert!(ids.contains(&"linux.core_dumps"), "{ids:?}");
    assert!(!ids.contains(&"javascript.node_modules"), "{ids:?}");
}

#[test]
fn system_commands_go_to_the_helper_the_first_one_starts() {
    let root = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    project(&home);
    let app = TestApp::with_system(SystemState::with(Box::new(InProcess::new(
        &home,
        data.path(),
        Duration::from_secs(60),
    ))));
    let status = app.invoke("system_status", json!({})).unwrap();
    assert_eq!(status["unavailable"], Value::Null);
    assert_eq!(status["running"], false);

    let results = app
        .invoke("system_scan", json!({ "progress": CHANNEL }))
        .unwrap();
    let scan = &results["scan"];
    assert_eq!(results["sections"][0]["items"], 1);
    let status = app.invoke("system_status", json!({})).unwrap();
    assert_eq!(status["running"], true);
    let page = app
        .invoke(
            "system_candidates",
            json!({ "scan": scan, "section": "removable", "rule": null, "offset": 0, "limit": 10 }),
        )
        .unwrap();
    let item = &page["items"][0]["id"];
    let details = app
        .invoke("system_explain", json!({ "scan": scan, "item": item }))
        .unwrap();
    assert_eq!(details["ruleName"], "Node.js dependencies");

    let report = app
        .invoke(
            "system_clean",
            json!({ "scan": scan, "items": [item], "progress": CHANNEL }),
        )
        .unwrap();
    assert_eq!(report["deleted"]["items"], 1, "{report}");
    assert!(!project_is_whole(&home));
    let summary = app
        .invoke("system_summary", json!({ "scan": scan }))
        .unwrap();
    assert_eq!(summary["sections"][0]["items"], 0);
    let history = app.invoke("system_history", json!({})).unwrap();
    assert_eq!(history["runs"].as_array().unwrap().len(), 1, "{history}");
    let quarantine = app.invoke("system_quarantined", json!({})).unwrap();
    assert!(quarantine["entries"].is_array());

    // The helper's own errors come back as they are.
    let error = app
        .invoke("system_purge", json!({ "ids": ["no-such-entry"] }))
        .unwrap_err();
    assert!(error.to_string().contains("no-such-entry"), "{error}");
}

/// A helper that stopped, as one does after ten minutes with nothing asked, is replaced by the
/// next command after the one that found it gone.
#[test]
fn a_helper_that_stopped_is_started_again() {
    let root = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let helpers = InProcess::new(root.path(), data.path(), Duration::from_millis(100));
    let started = Arc::clone(&helpers.started);
    let app = TestApp::with_system(SystemState::with(Box::new(helpers)));
    app.invoke("system_history", json!({})).unwrap();

    thread::sleep(Duration::from_millis(500));
    let error = app.invoke("system_history", json!({})).unwrap_err();
    let status = app.invoke("system_status", json!({})).unwrap();
    let history = app.invoke("system_history", json!({}));

    assert!(
        error.to_string().contains("System cleanup stopped"),
        "{error}"
    );
    assert_eq!(status["running"], false);
    assert!(history.is_ok(), "{history:?}");
    assert_eq!(started.load(Ordering::Relaxed), 2);
}
