#![allow(clippy::disallowed_methods, reason = "tests build fixture trees")]

use std::fs::{self, File};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use cleanse_model::Risk;

use super::{Engine, Explanation, Probe, Progress};

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

fn project(root: &Path) {
    write_old(&root.join("web/package.json"), "{}");
    write_old(&root.join("web/node_modules/left-pad/index.js"), "abcd");
    write_old(&root.join("web/src/index.js"), "x");
}

#[test]
fn scans_finds_and_assesses_what_the_rules_recognize() {
    let root = tempfile::tempdir().unwrap();
    project(root.path());
    let engine = Engine::new().unwrap();
    let reports = Mutex::new(Vec::new());

    let scan = engine
        .scan(
            &[root.path().to_path_buf()],
            &|progress| reports.lock().unwrap().push(progress),
            &AtomicBool::new(false),
        )
        .unwrap();

    assert_eq!(scan.candidates.len(), 1);
    let candidate = &scan.candidates[0];
    assert!(candidate.found.path.ends_with("web/node_modules"));
    assert_eq!(candidate.risk(), Risk::Regenerable, "{candidate:#?}");
    assert_eq!(scan.unreadable.len(), 0, "{:?}", scan.unreadable);
    assert!(!scan.cancelled);
    assert!(
        reports
            .lock()
            .unwrap()
            .contains(&Progress::Assessing { done: 1, total: 1 })
    );
}

#[test]
fn overlapping_roots_dont_duplicate_matches() {
    let root = tempfile::tempdir().unwrap();
    project(root.path());
    let engine = Engine::new().unwrap();

    let scan = engine
        .scan(
            &[root.path().to_path_buf(), root.path().join("web")],
            &|_| {},
            &AtomicBool::new(false),
        )
        .unwrap();

    assert_eq!(scan.candidates.len(), 1);
}

#[test]
fn a_cancelled_scan_stops_with_partial_results() {
    let root = tempfile::tempdir().unwrap();
    project(root.path());
    let engine = Engine::new().unwrap();

    let scan = engine
        .scan(
            &[root.path().to_path_buf()],
            &|_| {},
            &AtomicBool::new(true),
        )
        .unwrap();

    assert!(scan.cancelled);
    assert!(scan.candidates.is_empty());
}

#[test]
fn a_missing_root_is_an_error_naming_it() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing");
    let engine = Engine::new().unwrap();

    let error = engine
        .scan(
            std::slice::from_ref(&missing),
            &|_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();

    assert!(error.to_string().contains("missing"), "{error}");
}

#[test]
fn explains_the_folder_a_path_belongs_to() {
    let root = tempfile::tempdir().unwrap();
    project(root.path());
    let engine = Engine::new().unwrap();

    let explanation = engine
        .explain(&root.path().join("web/node_modules/left-pad/index.js"))
        .unwrap();

    let Explanation::Matched { candidate, rule } = explanation else {
        panic!("expected a match: {explanation:?}");
    };
    assert_eq!(rule.id.0, "javascript.node_modules");
    assert!(candidate.found.path.ends_with("web/node_modules"));
    assert_eq!(candidate.risk(), Risk::Regenerable, "{candidate:#?}");
}

#[test]
fn explains_that_unrecognized_paths_are_left_alone() {
    let root = tempfile::tempdir().unwrap();
    project(root.path());
    let engine = Engine::new().unwrap();

    let explanation = engine.explain(&root.path().join("web/src")).unwrap();

    assert!(
        matches!(explanation, Explanation::Unmatched { protected: None }),
        "{explanation:?}"
    );
}

#[test]
fn explains_when_an_unrecognized_path_is_also_protected() {
    let engine = Engine::new().unwrap();
    let home = engine.system().known_folders().home.clone().unwrap();

    let explanation = engine.explain(&home).unwrap();

    assert!(
        matches!(&explanation, Explanation::Unmatched { protected: Some(what) } if what == "your home folder"),
        "{explanation:?}"
    );
}
