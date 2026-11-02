#![allow(clippy::disallowed_methods, reason = "tests build fixture trees")]

use std::fs::{self, File};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use cleanse_model::{Outcome, Plan, PlanItem, Risk};

use cleanse_model::{Concern, RuleId};

use super::{
    Config, Engine, Explanation, KnownFolders, NodeKind, Probe, Progress, RuleSet, System,
    removable_under,
};

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
fn analyzes_where_space_goes_under_the_roots() {
    let root = tempfile::tempdir().unwrap();
    project(root.path());
    let engine = Engine::new().unwrap();

    let analysis = engine
        .analyze(
            &[root.path().to_path_buf()],
            &|_| {},
            &AtomicBool::new(false),
        )
        .unwrap();

    let tree = &analysis.tree;
    let [top] = tree.roots() else {
        panic!("{:?}", tree.roots())
    };
    assert_eq!(tree.path(*top), std::path::absolute(root.path()).unwrap());
    let named = |parent, name: &str| {
        *tree
            .children(parent)
            .iter()
            .find(|&&id| tree.node(id).name.to_string_lossy() == name)
            .unwrap_or_else(|| panic!("no {name}"))
    };
    let web = named(*top, "web");
    // package.json, then node_modules measured as a whole, then src.
    assert_eq!(tree.node(web).usage.logical, 2 + 4 + 1);
    assert_eq!(tree.node(web).files, 3);
    assert_eq!(tree.node(named(web, "node_modules")).kind, NodeKind::Item);
    assert_eq!(tree.node(named(web, "src")).kind, NodeKind::Folder);
    let removable: Vec<_> = removable_under(&analysis.scan.candidates, &tree.path(web)).collect();
    assert_eq!(removable.len(), 1);
    assert!(!tree.partial && !tree.node(*top).incomplete);
}

#[test]
fn a_folder_given_twice_or_inside_another_is_scanned_once() {
    let root = tempfile::tempdir().unwrap();
    project(root.path());
    let engine = Engine::new().unwrap();
    let mut trailing = root.path().as_os_str().to_owned();
    trailing.push(std::path::MAIN_SEPARATOR_STR);

    let scan = engine
        .scan(
            &[
                root.path().join("web"),
                root.path().to_path_buf(),
                trailing.into(),
            ],
            &|_| {},
            &AtomicBool::new(false),
        )
        .unwrap();

    assert_eq!(scan.roots, [std::path::absolute(root.path()).unwrap()]);
    assert_eq!(scan.candidates.len(), 1, "{:#?}", scan.candidates);
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
    // The project is in the temp folder, where on Windows the temp rule recognizes every item, so
    // the temp folder is somewhere else here.
    let mut folders = KnownFolders::current();
    folders.temp = root.path().join("temp");
    let engine = Engine {
        system: System::new().unwrap().with_known_folders(folders),
        ..Engine::new().unwrap()
    };

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

/// An engine whose data folder is `data`, acting as if it isn't elevated, since CI runs tests as
/// an administrator on Windows.
fn engine_for_apply(data: &Path) -> Engine {
    Engine {
        system: System::new().unwrap().elevated(false),
        data_dir: Some(data.to_path_buf()),
        ..Engine::new().unwrap()
    }
}

#[test]
fn applies_what_was_approved_from_a_scan() {
    let root = tempfile::tempdir().unwrap();
    project(root.path());
    let journal = tempfile::tempdir().unwrap();
    let engine = engine_for_apply(journal.path());
    let scan = engine
        .scan(
            &[root.path().to_path_buf()],
            &|_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
    let plan = Plan {
        roots: scan.roots.clone(),
        items: scan
            .candidates
            .iter()
            .filter_map(PlanItem::approve)
            .collect(),
    };
    let progress = Mutex::new(Vec::new());

    let applied = engine
        .apply(
            &plan,
            &|done| progress.lock().unwrap().push(done),
            &AtomicBool::new(false),
        )
        .unwrap();

    assert!(
        matches!(&applied.outcomes[..], [Outcome::Deleted { .. }]),
        "{applied:?}"
    );
    assert!(!root.path().join("web/node_modules").exists());
    assert!(root.path().join("web/package.json").exists());
    assert!(applied.journal.starts_with(journal.path()));
    assert_eq!(*progress.lock().unwrap(), [1]);
}

#[test]
fn large_files_where_nothing_may_be_removed_arent_listed() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let big = "x".repeat(100_000);
    write_old(&home.join("Videos/raw.mov"), &big);
    write_old(&home.join("archive/old.mov"), &big);
    write_old(&home.join("Videos/small.mov"), "x");
    let folders = KnownFolders {
        home: Some(home.clone()),
        temp: root.path().join("temp"),
        ..KnownFolders::default()
    };
    let engine = Engine {
        system: System::new().unwrap().with_known_folders(folders),
        config: Config {
            exclude: vec![home.join("archive")],
            large_file_size: 64 * 1024,
            ..Config::default()
        },
        ..Engine::new().unwrap()
    };

    let scan = engine
        .scan(
            &[root.path().to_path_buf()],
            &|_| {},
            &AtomicBool::new(false),
        )
        .unwrap();

    let [candidate] = &scan.candidates[..] else {
        panic!("{:#?}", scan.candidates)
    };
    assert!(candidate.found.path.ends_with("raw.mov"), "{candidate:#?}");
    assert_eq!(candidate.found.rule.0, "files.large");
    assert_eq!(candidate.risk(), Risk::Review, "{candidate:#?}");
}

#[test]
fn what_a_tool_cleans_itself_is_kept_and_cant_be_approved() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    write_old(&home.join("go/pkg/mod/cache/download/x.zip"), "zip");
    let folders = KnownFolders {
        home: Some(home.clone()),
        temp: root.path().join("temp"),
        ..KnownFolders::default()
    };
    let engine = Engine {
        system: System::new().unwrap().with_known_folders(folders),
        ..Engine::new().unwrap()
    };

    let scan = engine
        .scan(
            &[root.path().to_path_buf()],
            &|_| {},
            &AtomicBool::new(false),
        )
        .unwrap();

    let [candidate] = &scan.candidates[..] else {
        panic!("{:#?}", scan.candidates)
    };
    assert_eq!(candidate.found.rule.0, "go.module_cache");
    assert_eq!(candidate.risk(), Risk::Protected, "{candidate:#?}");
    assert_eq!(PlanItem::approve(candidate), None);
    assert_eq!(removable_under(&scan.candidates, root.path()).count(), 0);
    let rule = engine.rules().get(&candidate.found.rule).unwrap();
    assert_eq!(rule.advise.as_deref(), Some("go clean -modcache"));
}

#[test]
fn excluded_folders_are_kept_and_turned_off_rules_match_nothing() {
    let root = tempfile::tempdir().unwrap();
    project(root.path());
    let roots = [root.path().to_path_buf()];
    let excluding = Engine {
        config: Config {
            exclude: vec![root.path().join("web")],
            ..Config::default()
        },
        ..Engine::new().unwrap()
    };
    let without_rule = Engine {
        rules: RuleSet::builtin()
            .unwrap()
            .without(&[RuleId("javascript.node_modules".into())])
            .unwrap(),
        ..Engine::new().unwrap()
    };

    let excluded = excluding
        .scan(&roots, &|_| {}, &AtomicBool::new(false))
        .unwrap();
    let unmatched = without_rule
        .scan(&roots, &|_| {}, &AtomicBool::new(false))
        .unwrap();

    assert_eq!(excluded.candidates.len(), 1);
    let candidate = &excluded.candidates[0];
    assert_eq!(candidate.risk(), Risk::Protected);
    assert!(
        candidate.concerns().contains(&Concern::ProtectedLocation {
            what: "a folder excluded in your settings".into()
        }),
        "{candidate:#?}"
    );
    assert!(
        unmatched.candidates.is_empty(),
        "{:#?}",
        unmatched.candidates
    );
}
