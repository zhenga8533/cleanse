#![allow(
    clippy::disallowed_methods,
    clippy::unwrap_used,
    reason = "integration tests spawn the cleanse binary, build fixture trees, and fail fast"
)]

use std::fs::{self, File};
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

use cleanse_engine::{Probe, System};

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

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

fn elevated() -> bool {
    System::new().unwrap().is_elevated()
}

fn node_modules_intact(root: &Path) -> bool {
    root.join("web/node_modules/left-pad/index.js").exists()
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

    assert!(report.contains("Rules: 40 built in"), "{report}");
    assert!(report.contains("Home"), "{report}");
}

#[test]
fn clean_without_a_terminal_asks_for_yes_and_removes_nothing() {
    let root = fixture();

    let output = cleanse(&["clean", arg(root.path())]);

    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("--yes"), "{}", stderr(&output));
    assert!(node_modules_intact(root.path()));
}

#[test]
fn clean_yes_needs_the_highest_tier_to_delete() {
    let root = fixture();

    let output = cleanse(&["clean", "--yes", arg(root.path())]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("--max-risk"),
        "{}",
        stderr(&output)
    );
    assert!(node_modules_intact(root.path()));
}

#[test]
fn clean_dry_run_lists_what_would_be_deleted_and_removes_nothing() {
    let root = fixture();

    let report = stdout(&cleanse(&["clean", "--dry-run", arg(root.path())]));

    assert!(report.contains("node_modules"), "{report}");
    assert!(report.contains("--yes would delete 1 item"), "{report}");
    assert!(node_modules_intact(root.path()));
}

#[test]
fn clean_dry_run_stays_within_the_highest_tier() {
    let root = fixture();

    let report = stdout(&cleanse(&[
        "clean",
        "--dry-run",
        "--max-risk",
        "safe",
        arg(root.path()),
    ]));

    assert!(report.contains("Nothing was selected"), "{report}");
}

/// Windows CI runs tests as an administrator, so that's where this gets checked.
#[test]
fn clean_refuses_to_delete_while_elevated() {
    if !elevated() {
        return;
    }
    let root = fixture();

    let output = cleanse(&[
        "clean",
        "--yes",
        "--max-risk",
        "regenerable",
        arg(root.path()),
    ]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("administrator or root"),
        "{}",
        stderr(&output)
    );
    assert!(node_modules_intact(root.path()));
}

/// Unix only, where moving the home folder also moves Cleanse's journal out of the real one.
#[cfg(unix)]
#[test]
fn clean_yes_deletes_the_preselected_items_and_journals_them() {
    if elevated() {
        return;
    }
    let root = fixture();
    let home = tempfile::tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_cleanse"))
        .args(["clean", "--yes", "--max-risk", "regenerable"])
        .arg(root.path())
        .env("HOME", home.path())
        .env_remove("XDG_DATA_HOME")
        .output()
        .unwrap();

    let report = stdout(&output);
    assert!(report.contains("Deleted 1 item"), "{report}");
    assert!(!root.path().join("web/node_modules").exists());
    assert!(root.path().join("web/package.json").exists());
    let journals = files_under(home.path());
    assert_eq!(journals.len(), 1, "{journals:?}");
    assert!(report.contains(arg(&journals[0])), "{report}");
}

#[cfg(unix)]
fn files_under(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[test]
fn restore_and_purge_need_to_be_told_what() {
    let restore = cleanse(&["restore"]);
    let purge = cleanse(&["quarantine", "purge"]);

    assert_eq!(restore.status.code(), Some(2));
    assert_eq!(purge.status.code(), Some(2));
}

/// Windows CI runs tests as an administrator, so that's where this gets checked.
#[test]
fn restore_and_purge_refuse_while_elevated() {
    if !elevated() {
        return;
    }

    let restore = cleanse(&["restore", "--all"]);
    let purge = cleanse(&["quarantine", "purge", "--all", "--yes"]);

    for output in [restore, purge] {
        assert_eq!(output.status.code(), Some(1));
        assert!(
            stderr(&output).contains("administrator or root"),
            "{}",
            stderr(&output)
        );
    }
}

/// Unix only, where moving the home folder also moves Cleanse's data and settings folders out
/// of the real ones.
#[cfg(unix)]
fn cleanse_at_home(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cleanse"))
        .args(args)
        .env("HOME", home)
        .env_remove("XDG_DATA_HOME")
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .unwrap()
}

/// Writes Cleanse's settings file for a home folder at `home`.
#[cfg(unix)]
fn write_settings(home: &Path, contents: &str) -> std::path::PathBuf {
    let settings = if cfg!(target_os = "macos") {
        "Library/Application Support"
    } else {
        ".config"
    };
    let path = home.join(settings).join("cleanse/config.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, contents).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn quarantine_says_when_its_empty() {
    let home = tempfile::tempdir().unwrap();

    let report = stdout(&cleanse_at_home(home.path(), &["quarantine"]));
    let json = stdout(&cleanse_at_home(
        home.path(),
        &["quarantine", "list", "--json"],
    ));

    assert!(report.contains("Nothing is in quarantine"), "{report}");
    assert_eq!(json.trim(), "[]");
}

#[cfg(unix)]
#[test]
fn restore_rejects_an_id_that_isnt_in_quarantine() {
    if elevated() {
        return;
    }
    let home = tempfile::tempdir().unwrap();

    let output = cleanse_at_home(home.path(), &["restore", "123"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("nothing in quarantine has the ID 123"),
        "{}",
        stderr(&output)
    );
}

#[cfg(unix)]
#[test]
fn settings_exclude_folders_and_turn_off_rules() {
    let root = fixture();
    let excluding = tempfile::tempdir().unwrap();
    let disabling = tempfile::tempdir().unwrap();
    write_settings(
        excluding.path(),
        &format!("exclude = [\"{}\"]\n", root.path().join("web").display()),
    );
    write_settings(
        disabling.path(),
        "disabled_rules = [\"javascript.node_modules\"]\n",
    );

    let excluded = stdout(&cleanse_at_home(
        excluding.path(),
        &["scan", "--show-protected", arg(root.path())],
    ));
    let unmatched = stdout(&cleanse_at_home(
        disabling.path(),
        &["scan", arg(root.path())],
    ));
    let doctor = stdout(&cleanse_at_home(disabling.path(), &["doctor"]));

    assert!(!excluded.contains("Can be removed"), "{excluded}");
    assert!(
        excluded.contains("a folder excluded in your settings"),
        "{excluded}"
    );
    assert!(!unmatched.contains("node_modules"), "{unmatched}");
    assert!(
        doctor.contains("turned off   javascript.node_modules"),
        "{doctor}"
    );
}

#[cfg(unix)]
#[test]
fn a_broken_settings_file_stops_cleanse_and_says_where_it_is() {
    let home = tempfile::tempdir().unwrap();
    let path = write_settings(home.path(), "exclude = [\"relative/path\"]\n");

    let output = cleanse_at_home(home.path(), &["scan"]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr(&output);
    assert!(stderr.contains(arg(&path)), "{stderr}");
    assert!(stderr.contains("isn't a full path"), "{stderr}");
}
