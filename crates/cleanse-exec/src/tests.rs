#![allow(clippy::disallowed_methods, reason = "tests build fixture trees")]

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use cleanse_core::rules::{Matcher, Os, RuleSet};
use cleanse_core::safety::{Assessor, ProtectedPolicy};
use cleanse_model::{CandidateId, Concern, Disposal, Outcome, Plan, PlanItem, Size, SkipReason};
use cleanse_platform::{Probe, System};
use serde_json::Value;
use tempfile::TempDir;

use super::{ApplyError, Executor, delete};

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

#[cfg(unix)]
fn link_dir(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

/// Junctions need no special privileges, unlike directory symlinks on Windows.
#[cfg(windows)]
fn link_dir(target: &Path, link: &Path) {
    junction::create(target, link).unwrap();
}

/// A JavaScript project under a temporary folder, with its own journal folder beside it.
struct Fixture {
    root: TempDir,
    journal: TempDir,
    rules: RuleSet,
    system: System,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        write_old(&root.path().join("web/package.json"), "{}");
        write_old(&root.path().join("web/src/index.js"), "x");
        write_old(
            &root.path().join("web/node_modules/left-pad/index.js"),
            "abcd",
        );
        Self {
            root,
            journal: tempfile::tempdir().unwrap(),
            rules: RuleSet::builtin().unwrap(),
            system: System::new().unwrap().elevated(false),
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.path().join(relative)
    }

    fn node_modules(&self) -> PathBuf {
        self.path("web/node_modules")
    }

    /// Approves everything the rules find that can be approved, as a person reviewing the scan
    /// would.
    fn plan(&self) -> Plan {
        let folders = self.system.known_folders();
        let matcher = Matcher::new(self.rules.rules(), folders, Os::current());
        let (found, _) = matcher.find(self.root.path()).unwrap();
        let policy = ProtectedPolicy::for_system(folders, &self.system);
        let roots = vec![self.root.path().to_path_buf()];
        let assessor = Assessor::new(&self.system, policy, &roots, SystemTime::now()).unwrap();
        let items: Vec<PlanItem> = found
            .iter()
            .map(|detection| assessor.assess(CandidateId(0), detection))
            .filter_map(|candidate| PlanItem::approve(&candidate))
            .collect();
        assert_eq!(items.len(), 1, "{items:#?}");
        Plan { roots, items }
    }

    fn executor(&self) -> Executor<'_> {
        Executor::new(&self.rules, &self.system, self.journal.path().to_path_buf())
    }

    fn apply(&self, plan: &Plan) -> Vec<Outcome> {
        self.executor()
            .apply(plan, &|_| {}, &AtomicBool::new(false))
            .unwrap()
            .outcomes
    }

    /// The events of the only journal written, in order.
    fn journal_events(&self) -> Vec<Value> {
        let files: Vec<_> = fs::read_dir(self.journal.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1, "{files:?}");
        fs::read_to_string(&files[0])
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn assert_web_untouched(&self) {
        assert!(self.path("web/node_modules/left-pad/index.js").exists());
        self.assert_no_tombstones();
    }

    fn assert_no_tombstones(&self) {
        let names: Vec<String> = fs::read_dir(self.path("web"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names
                .iter()
                .all(|name| !name.starts_with(".cleanse-tombstone-")),
            "{names:?}"
        );
    }
}

fn skipped(reason: SkipReason) -> Outcome {
    Outcome::Skipped { reason }
}

#[test]
fn deletes_an_approved_folder_and_journals_it() {
    let fixture = Fixture::new();
    let plan = fixture.plan();

    let outcomes = fixture.apply(&plan);

    assert!(
        matches!(&outcomes[..], [Outcome::Deleted { size }] if size.logical == 4),
        "{outcomes:?}"
    );
    assert!(!fixture.node_modules().exists());
    assert!(fixture.path("web/package.json").exists());
    assert!(fixture.path("web/src/index.js").exists());
    fixture.assert_no_tombstones();

    let events = fixture.journal_events();
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| event["event"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["started", "removing", "done", "finished"]);
    assert_eq!(events[0]["items"], 1);
    assert_eq!(events[1]["rule"], "javascript.node_modules");
    assert_eq!(events[1]["path"].as_str(), plan.items[0].path.to_str());
    assert_eq!(events[2]["outcome"]["kind"], "deleted");
}

#[test]
fn leaves_an_item_that_is_gone() {
    let fixture = Fixture::new();
    let plan = fixture.plan();
    fs::remove_dir_all(fixture.node_modules()).unwrap();

    assert_eq!(fixture.apply(&plan), [skipped(SkipReason::Gone)]);
}

#[test]
fn leaves_a_different_folder_at_the_same_path() {
    let fixture = Fixture::new();
    let plan = fixture.plan();
    fs::rename(fixture.node_modules(), fixture.path("web/old_modules")).unwrap();
    write_old(&fixture.path("web/node_modules/other/index.js"), "y");

    assert_eq!(fixture.apply(&plan), [skipped(SkipReason::Replaced)]);
    assert!(fixture.path("web/node_modules/other/index.js").exists());
}

#[test]
fn leaves_a_link_at_the_same_path_and_what_it_points_to() {
    let fixture = Fixture::new();
    let plan = fixture.plan();
    let elsewhere = fixture.path("elsewhere");
    fs::rename(fixture.node_modules(), &elsewhere).unwrap();
    link_dir(&elsewhere, &fixture.node_modules());

    assert_eq!(fixture.apply(&plan), [skipped(SkipReason::Replaced)]);
    assert!(elsewhere.join("left-pad/index.js").exists());
    assert!(fs::symlink_metadata(fixture.node_modules()).is_ok());
}

#[test]
fn removes_links_inside_without_following_them() {
    let fixture = Fixture::new();
    let canary = fixture.path("canary");
    write_old(&canary.join("keep.txt"), "important");
    link_dir(&canary, &fixture.path("web/node_modules/linked"));
    let plan = fixture.plan();

    let outcomes = fixture.apply(&plan);

    assert!(
        matches!(&outcomes[..], [Outcome::Deleted { .. }]),
        "{outcomes:?}"
    );
    assert!(!fixture.node_modules().exists());
    assert_eq!(
        fs::read_to_string(canary.join("keep.txt")).unwrap(),
        "important"
    );
}

#[test]
fn leaves_an_item_modified_after_approval() {
    let fixture = Fixture::new();
    let plan = fixture.plan();
    fs::write(fixture.path("web/node_modules/fresh.js"), "z").unwrap();

    let outcomes = fixture.apply(&plan);

    assert_eq!(
        outcomes,
        [skipped(SkipReason::Blocked {
            concerns: vec![Concern::RecentlyModified { minutes_ago: 0 }]
        })]
    );
    fixture.assert_web_untouched();
}

#[test]
fn leaves_an_item_its_rule_no_longer_recognizes() {
    let fixture = Fixture::new();
    let plan = fixture.plan();
    fs::remove_file(fixture.path("web/package.json")).unwrap();

    assert_eq!(fixture.apply(&plan), [skipped(SkipReason::NoLongerMatches)]);
    fixture.assert_web_untouched();
}

/// A hard link to a file outside means removing the folder frees less than approved.
#[cfg(unix)]
#[test]
fn leaves_an_item_with_new_kinds_of_warnings() {
    let fixture = Fixture::new();
    let plan = fixture.plan();
    write_old(&fixture.path("shared.bin"), "shared");
    fs::hard_link(
        fixture.path("shared.bin"),
        fixture.path("web/node_modules/shared.bin"),
    )
    .unwrap();
    File::options()
        .write(true)
        .open(fixture.path("web/node_modules/shared.bin"))
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_hours(24))
        .unwrap();

    assert_eq!(
        fixture.apply(&plan),
        [skipped(SkipReason::NewConcerns {
            concerns: vec![Concern::SharedData]
        })]
    );
    fixture.assert_web_untouched();
}

#[test]
fn leaves_items_for_disposals_that_arent_available_yet() {
    let fixture = Fixture::new();
    let mut plan = fixture.plan();
    plan.items[0].disposal = Disposal::Quarantine;

    assert_eq!(
        fixture.apply(&plan),
        [skipped(SkipReason::Unsupported {
            disposal: Disposal::Quarantine
        })]
    );
    fixture.assert_web_untouched();
}

#[test]
fn cancelling_leaves_the_items_not_yet_reached() {
    let fixture = Fixture::new();
    let plan = fixture.plan();

    let applied = fixture
        .executor()
        .apply(&plan, &|_| {}, &AtomicBool::new(true))
        .unwrap();

    assert_eq!(applied.outcomes, [skipped(SkipReason::Cancelled)]);
    fixture.assert_web_untouched();
}

#[test]
fn refuses_to_run_elevated() {
    let fixture = Fixture::new();
    let plan = fixture.plan();
    let elevated = System::new().unwrap().elevated(true);

    let result = Executor::new(&fixture.rules, &elevated, fixture.journal.path().into()).apply(
        &plan,
        &|_| {},
        &AtomicBool::new(false),
    );

    assert!(matches!(result, Err(ApplyError::Elevated)), "{result:?}");
    fixture.assert_web_untouched();
}

#[test]
fn removes_nothing_without_a_journal() {
    let fixture = Fixture::new();
    let plan = fixture.plan();
    let not_a_folder = fixture.journal.path().join("file");
    fs::write(&not_a_folder, "").unwrap();

    let result = Executor::new(&fixture.rules, &fixture.system, not_a_folder).apply(
        &plan,
        &|_| {},
        &AtomicBool::new(false),
    );

    assert!(matches!(result, Err(ApplyError::Journal(_))), "{result:?}");
    fixture.assert_web_untouched();
}

#[test]
fn puts_back_whatever_replaced_the_folder_before_it_moved() {
    let fixture = Fixture::new();
    let path = fixture.node_modules();
    let tombstone = delete::tombstone_for(&path).unwrap();
    let mut other = fixture.system.inspect(&path).unwrap().identity;
    other.file += 1;

    let outcome = delete::delete(&fixture.system, &path, &tombstone, other, Size::default());

    assert_eq!(outcome, skipped(SkipReason::Replaced));
    fixture.assert_web_untouched();
}

#[test]
fn deletes_read_only_files_inside() {
    let fixture = Fixture::new();
    let file = fixture.path("web/node_modules/left-pad/index.js");
    let mut permissions = fs::metadata(&file).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&file, permissions).unwrap();
    let plan = fixture.plan();

    let outcomes = fixture.apply(&plan);

    assert!(
        matches!(&outcomes[..], [Outcome::Deleted { .. }]),
        "{outcomes:?}"
    );
    assert!(!fixture.node_modules().exists());
}

/// Windows won't rename a folder while a file inside is open, so nothing is removed and the
/// folder stays whole.
#[cfg(windows)]
#[test]
fn leaves_a_folder_whole_while_a_file_inside_is_open() {
    let fixture = Fixture::new();
    let plan = fixture.plan();
    let open = File::open(fixture.path("web/node_modules/left-pad/index.js")).unwrap();

    let outcomes = fixture.apply(&plan);
    drop(open);

    assert!(
        matches!(&outcomes[..], [Outcome::Failed { .. }]),
        "{outcomes:?}"
    );
    fixture.assert_web_untouched();
}
