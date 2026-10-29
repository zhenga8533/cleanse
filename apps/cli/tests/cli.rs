#![allow(
    clippy::disallowed_methods,
    clippy::unwrap_used,
    reason = "integration tests spawn the cleanse binary, build fixture trees, and fail fast"
)]

use std::fs::{self, File};
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

fn cleanse(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cleanse"))
        .args(args)
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

/// Writes a file dated a day ago, so it doesn't count as recent activity.
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

/// A project with removable dependencies and some source code.
fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    write_old(&root.path().join("web/package.json"), "{}");
    write_old(
        &root.path().join("web/node_modules/left-pad/index.js"),
        "abcd",
    );
    write_old(&root.path().join("web/src/index.js"), "x");
    root
}

fn arg(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn version_flag_prints_name_and_version() {
    let output = cleanse(&["--version"]);

    assert_eq!(
        stdout(&output).trim(),
        concat!("cleanse ", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn bare_invocation_shows_help_instead_of_doing_anything() {
    let output = cleanse(&[]);

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("Usage: cleanse"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn scan_lists_what_can_be_removed_and_removes_nothing() {
    let root = fixture();

    let report = stdout(&cleanse(&["scan", arg(root.path())]));

    assert!(report.contains("Can be removed: 1 item"), "{report}");
    assert!(report.contains("node_modules"), "{report}");
    assert!(report.contains("Regenerable"), "{report}");
    assert!(report.contains("Nothing was removed"), "{report}");
    assert!(
        root.path()
            .join("web/node_modules/left-pad/index.js")
            .exists()
    );
}

#[test]
fn scan_json_has_the_candidates_and_their_risk() {
    let root = fixture();

    let output = stdout(&cleanse(&["scan", "--json", arg(root.path())]));
    let json: serde_json::Value = serde_json::from_str(&output).unwrap();

    let candidates = json["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0]["rule"], "javascript.node_modules");
    assert_eq!(candidates[0]["risk"], "regenerable");
    assert_eq!(candidates[0]["size"]["logical"], 4);
}

#[test]
fn scan_of_a_missing_folder_fails_and_says_which() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing");

    let output = cleanse(&["scan", arg(&missing)]);

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("missing"), "{stderr}");
}

#[test]
fn explain_describes_the_folder_a_file_belongs_to() {
    let root = fixture();
    let file = root.path().join("web/node_modules/left-pad/index.js");

    let report = stdout(&cleanse(&["explain", arg(&file)]));

    assert!(report.contains("javascript.node_modules"), "{report}");
    assert!(report.contains("Regenerable, can be removed"), "{report}");
    assert!(report.contains("Next to package.json"), "{report}");
    assert!(report.contains("npm install"), "{report}");
}

#[test]
fn explain_says_when_nothing_recognizes_a_path() {
    let root = fixture();

    let report = stdout(&cleanse(&["explain", arg(&root.path().join("web/src"))]));

    assert!(report.contains("No rule recognizes this"), "{report}");
}

#[test]
fn rules_lists_and_shows_rules() {
    let list = stdout(&cleanse(&["rules"]));
    let show = stdout(&cleanse(&["rules", "show", "python.venv"]));
    let missing = cleanse(&["rules", "show", "no.such.rule"]);

    assert!(list.contains("javascript.node_modules"), "{list}");
    assert!(show.contains("pyvenv.cfg"), "{show}");
    assert!(!missing.status.success());
}

#[test]
fn doctor_reports_rules_and_folders() {
    let report = stdout(&cleanse(&["doctor"]));

    assert!(report.contains("Rules: 29 built in"), "{report}");
    assert!(report.contains("Home"), "{report}");
}
