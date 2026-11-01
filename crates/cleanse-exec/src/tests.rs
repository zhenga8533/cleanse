#![allow(clippy::disallowed_methods, reason = "tests build fixture trees")]

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use cleanse_core::rules::{Matcher, Os, RuleSet};
use cleanse_core::safety::{Assessor, ProtectedPolicy};
use cleanse_model::{
    CandidateId, Concern, Disposal, EntryKind, EntryOutcome, Outcome, Plan, PlanItem,
    QuarantineEntry, RuleId, SavedItem, SavedPlan, Size, SkipReason,
};
use cleanse_platform::{KnownFolders, Probe, System};
use serde_json::Value;
use tempfile::TempDir;

use super::journal::{self, Journal, Record};
use super::quarantine::Quarantine;
use super::{Action, ApplyError, Executor, Leftover, PlanFileError, delete, load_plan, save_plan};

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

/// A JavaScript project under a temporary folder, with its own Cleanse data folder beside it.
struct Fixture {
    root: TempDir,
    data: TempDir,
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
            data: tempfile::tempdir().unwrap(),
            rules: RuleSet::builtin().unwrap(),
            system: System::new().unwrap().elevated(false),
        }
    }

    /// A crash dump in an app data folder under a temporary folder, and a rule recognizing the
    /// files there, instead of the project.
    fn dump() -> Self {
        let root = tempfile::tempdir().unwrap();
        write_old(&root.path().join("data_local/Dumps/crash.dmp"), "abcd");
        let folders = KnownFolders {
            home: Some(root.path().join("home")),
            data_local: Some(root.path().join("data_local")),
            temp: root.path().join("temp"),
            ..KnownFolders::default()
        };
        let location =
            |os: &str| format!(r#"{{ os = "{os}", base = "data_local", path = "Dumps/*.dmp" }}"#);
        let toml = format!(
            r#"
[[rule]]
id = "test.dumps"
name = "Dumps"
category = "logs"
risk = "safe"
explain = "Because."

[rule.match]
kind = "known_location"
entry = "file"
locations = [{}, {}, {}]
"#,
            location("windows"),
            location("macos"),
            location("linux")
        );
        Self {
            root,
            data: tempfile::tempdir().unwrap(),
            rules: RuleSet::from_sources([("test.toml", toml.as_str())]).unwrap(),
            system: System::new()
                .unwrap()
                .elevated(false)
                .with_known_folders(folders),
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.path().join(relative)
    }

    fn crash_dump(&self) -> PathBuf {
        self.path("data_local/Dumps/crash.dmp")
    }

    /// Whether `Dumps` holds exactly the crash dump, if it's `there`, and nothing else.
    fn assert_dumps_hold(&self, there: bool) {
        let names: Vec<String> = fs::read_dir(self.path("data_local/Dumps"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        let expected: &[&str] = if there { &["crash.dmp"] } else { &[] };
        assert_eq!(names, expected);
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
        let policy = ProtectedPolicy::for_system(folders, &self.system, &[]);
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
        Executor::new(
            &self.rules,
            &self.system,
            &[],
            self.data.path().to_path_buf(),
        )
    }

    fn apply(&self, plan: &Plan) -> Vec<Outcome> {
        self.executor()
            .apply(plan, &|_| {}, &AtomicBool::new(false))
            .unwrap()
            .outcomes
    }

    /// The events of each journal written, oldest first.
    fn journals(&self) -> Vec<Vec<Value>> {
        let mut files: Vec<_> = fs::read_dir(self.data.path().join("journal"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        files.sort();
        files
            .iter()
            .map(|file| {
                fs::read_to_string(file)
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect()
            })
            .collect()
    }

    /// The events of the only journal written, in order.
    fn journal_events(&self) -> Vec<Value> {
        let mut journals = self.journals();
        assert_eq!(journals.len(), 1, "{journals:?}");
        journals.remove(0)
    }

    /// Quarantines the project's `node_modules` and returns its entry.
    fn quarantine_one(&self) -> QuarantineEntry {
        let mut plan = self.plan();
        plan.items[0].disposal = Disposal::Quarantine;
        let outcomes = self.apply(&plan);
        assert!(
            matches!(&outcomes[..], [Outcome::Quarantined { .. }]),
            "{outcomes:?}"
        );
        let listing = self.executor().quarantined().unwrap();
        assert_eq!(listing.entries.len(), 1, "{listing:?}");
        listing.entries[0].clone()
    }

    fn quarantine_is_empty(&self) -> bool {
        let listing = self.executor().quarantined().unwrap();
        listing.entries.is_empty() && listing.unreadable.is_empty()
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
    plan.items[0].disposal = Disposal::Trash;

    assert_eq!(
        fixture.apply(&plan),
        [skipped(SkipReason::Unsupported {
            disposal: Disposal::Trash
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

    let result = Executor::new(&fixture.rules, &elevated, &[], fixture.data.path().into()).apply(
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
    let not_a_folder = fixture.data.path().join("file");
    fs::write(&not_a_folder, "").unwrap();

    let result = Executor::new(&fixture.rules, &fixture.system, &[], not_a_folder).apply(
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

    let outcome = delete::delete(
        &fixture.system,
        &path,
        &tombstone,
        EntryKind::Dir,
        other,
        Size::default(),
    );

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

#[test]
fn quarantines_an_item_and_restores_it() {
    let fixture = Fixture::new();
    let original = fixture.plan().items[0].path.clone();

    let entry = fixture.quarantine_one();

    assert!(!fixture.node_modules().exists());
    assert_eq!(entry.original, original);
    assert_eq!(entry.rule.0, "javascript.node_modules");
    assert_eq!(entry.size.logical, 4);
    let events = fixture.journal_events();
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| event["event"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["started", "quarantining", "done", "finished"]);
    assert_eq!(events[1]["entry"].as_str(), Some(entry.id.as_str()));
    assert_eq!(events[2]["outcome"]["kind"], "quarantined");

    let handled = fixture.executor().restore(&[entry]).unwrap();

    assert_eq!(handled.outcomes, [EntryOutcome::Restored]);
    fixture.assert_web_untouched();
    assert!(fixture.quarantine_is_empty());
    let restore = &fixture.journals()[1];
    assert_eq!(restore[0]["action"], "restore");
    assert_eq!(restore[1]["event"], "restoring");
    assert_eq!(restore[2]["outcome"]["kind"], "restored");
}

#[test]
fn purging_deletes_a_quarantined_item_for_good() {
    let fixture = Fixture::new();
    let entry = fixture.quarantine_one();

    let handled = fixture
        .executor()
        .purge(std::slice::from_ref(&entry))
        .unwrap();

    assert_eq!(
        handled.outcomes,
        [EntryOutcome::Purged { size: entry.size }]
    );
    assert!(fixture.quarantine_is_empty());
    assert!(!fixture.node_modules().exists());
    assert!(
        !fixture
            .data
            .path()
            .join("quarantine")
            .join(&entry.id)
            .exists()
    );
}

#[test]
fn restoring_leaves_an_item_in_quarantine_when_its_place_is_taken() {
    let fixture = Fixture::new();
    let entry = fixture.quarantine_one();
    write_old(&fixture.path("web/node_modules/other/index.js"), "y");

    let handled = fixture.executor().restore(&[entry]).unwrap();

    assert!(
        matches!(&handled.outcomes[..], [EntryOutcome::Failed { error }] if error.starts_with("Something is at")),
        "{handled:?}"
    );
    assert_eq!(fixture.executor().quarantined().unwrap().entries.len(), 1);
    assert!(fixture.path("web/node_modules/other/index.js").exists());
}

#[test]
fn restoring_and_purging_refuse_to_run_elevated() {
    let fixture = Fixture::new();
    let entry = fixture.quarantine_one();
    let elevated = System::new().unwrap().elevated(true);
    let executor = Executor::new(&fixture.rules, &elevated, &[], fixture.data.path().into());

    assert!(matches!(
        executor.restore(std::slice::from_ref(&entry)),
        Err(ApplyError::Elevated)
    ));
    assert!(matches!(
        executor.purge(std::slice::from_ref(&entry)),
        Err(ApplyError::Elevated)
    ));
    assert_eq!(fixture.executor().quarantined().unwrap().entries, [entry]);
}

#[test]
fn quarantine_puts_back_whatever_replaced_the_folder_before_it_moved() {
    let fixture = Fixture::new();
    let mut other = fixture
        .system
        .inspect(&fixture.node_modules())
        .unwrap()
        .identity;
    other.file += 1;
    let entry = QuarantineEntry {
        id: Quarantine::new_id(),
        original: fixture.node_modules(),
        kind: EntryKind::Dir,
        rule: RuleId("javascript.node_modules".into()),
        size: Size::default(),
        at: 0,
    };

    let outcome = Quarantine::new(fixture.data.path()).admit(&fixture.system, &entry, other);

    assert_eq!(outcome, skipped(SkipReason::Replaced));
    fixture.assert_web_untouched();
    assert!(fixture.quarantine_is_empty());
}

/// Restoring or purging acts on the folder a record's ID names, so a record that names anything
/// but its own folder inside the quarantine is never listed.
#[test]
fn records_that_name_other_folders_arent_listed() {
    let fixture = Fixture::new();
    let quarantine = fixture.data.path().join("quarantine");
    let record = |id: &str| {
        serde_json::to_vec(&QuarantineEntry {
            id: id.into(),
            original: fixture.node_modules(),
            kind: EntryKind::Dir,
            rule: RuleId("javascript.node_modules".into()),
            size: Size::default(),
            at: 0,
        })
        .unwrap()
    };
    fs::create_dir_all(&quarantine).unwrap();
    fs::write(quarantine.join("...json"), record("..")).unwrap();
    fs::create_dir_all(quarantine.join("1-2-3")).unwrap();
    fs::write(quarantine.join("1-2-3.json"), record("4-5-6")).unwrap();

    let listing = fixture.executor().quarantined().unwrap();

    assert!(listing.entries.is_empty(), "{listing:?}");
    assert_eq!(listing.unreadable.len(), 2, "{listing:?}");
}

#[test]
fn leaves_an_item_excluded_after_approval() {
    let fixture = Fixture::new();
    let plan = fixture.plan();
    let excluded = [fixture.path("web")];

    let outcomes = Executor::new(
        &fixture.rules,
        &fixture.system,
        &excluded,
        fixture.data.path().into(),
    )
    .apply(&plan, &|_| {}, &AtomicBool::new(false))
    .unwrap()
    .outcomes;

    assert_eq!(
        outcomes,
        [skipped(SkipReason::Blocked {
            concerns: vec![Concern::ProtectedLocation {
                what: "a folder excluded in your settings".into()
            }]
        })]
    );
    fixture.assert_web_untouched();
}

#[test]
fn history_reads_back_each_run() {
    let fixture = Fixture::new();
    fixture.apply(&fixture.plan());

    let history = fixture.executor().history().unwrap();

    assert_eq!(history.runs.len(), 1, "{history:?}");
    let run = &history.runs[0];
    assert_eq!(run.action, Action::Apply);
    assert_eq!(run.items, 1);
    assert!(
        matches!(&run.outcomes[..], [Outcome::Deleted { .. }]),
        "{run:?}"
    );
    assert!(run.finished);
    assert!(history.leftovers.is_empty(), "{history:?}");
    assert!(history.unreadable.is_empty(), "{history:?}");
}

/// Starts removing the fixture's one item the way a run does, then stops as if the process died after
/// renaming it to its tombstone. `identity` is what the journal records as approved.
fn interrupt_removal(fixture: &Fixture, identity: cleanse_model::FileIdentity) -> Leftover {
    let plan = fixture.plan();
    let item = &plan.items[0];
    let tombstone = delete::tombstone_for(&item.path).unwrap();
    let mut journal = Journal::start(
        &fixture.data.path().join("journal"),
        Action::Apply,
        &plan.roots,
        1,
    )
    .unwrap();
    journal
        .record(&Record::Removing {
            path: item.path.to_str().unwrap().to_owned(),
            tombstone: tombstone.to_str().unwrap().to_owned(),
            kind: item.kind,
            rule: item.rule.clone(),
            identity,
        })
        .unwrap();
    fs::rename(&item.path, &tombstone).unwrap();
    Leftover {
        original: item.path.clone(),
        tombstone,
        kind: item.kind,
        rule: item.rule.clone(),
        identity,
    }
}

#[test]
fn runs_started_in_the_same_millisecond_stay_in_order() {
    let dir = tempfile::tempdir().unwrap();
    // Sorted as text, the folder would list `1000-5-10` before `1000-5-2`.
    let names = ["999-9-7.jsonl", "1000-5-2.jsonl", "1000-5-10.jsonl"];
    for name in names.iter().rev() {
        fs::write(dir.path().join(name), "").unwrap();
    }

    let files = journal::files(dir.path()).unwrap();

    let found: Vec<&str> = files
        .iter()
        .map(|path| path.file_name().unwrap().to_str().unwrap())
        .collect();
    assert_eq!(found, names);
}

#[test]
fn finds_and_finishes_a_removal_that_was_cut_short() {
    let fixture = Fixture::new();
    let identity = fixture
        .system
        .inspect(&fixture.node_modules())
        .unwrap()
        .identity;
    let leftover = interrupt_removal(&fixture, identity);

    let history = fixture.executor().history().unwrap();

    assert!(!history.runs[0].finished);
    assert_eq!(history.leftovers, std::slice::from_ref(&leftover));

    let applied = fixture.executor().finish(&history.leftovers).unwrap();

    assert!(
        matches!(&applied.outcomes[..], [Outcome::Deleted { size }] if size.logical == 4),
        "{applied:?}"
    );
    assert!(!leftover.tombstone.exists());
    assert!(fixture.path("web/package.json").exists());
    let history = fixture.executor().history().unwrap();
    assert!(history.leftovers.is_empty(), "{history:?}");
    assert_eq!(history.runs.len(), 2);
    assert_eq!(history.runs[1].action, Action::Finish);
}

#[test]
fn whatever_else_is_at_a_tombstone_is_left_alone() {
    let fixture = Fixture::new();
    let mut other = fixture
        .system
        .inspect(&fixture.node_modules())
        .unwrap()
        .identity;
    other.file += 1;
    let leftover = interrupt_removal(&fixture, other);

    let history = fixture.executor().history().unwrap();
    let applied = fixture
        .executor()
        .finish(std::slice::from_ref(&leftover))
        .unwrap();

    assert!(history.leftovers.is_empty(), "{history:?}");
    assert_eq!(applied.outcomes, [skipped(SkipReason::Gone)]);
    assert!(leftover.tombstone.join("left-pad/index.js").exists());
}

#[test]
fn finishing_refuses_to_run_elevated() {
    let fixture = Fixture::new();
    let identity = fixture
        .system
        .inspect(&fixture.node_modules())
        .unwrap()
        .identity;
    let leftover = interrupt_removal(&fixture, identity);
    let elevated = System::new().unwrap().elevated(true);

    let result = Executor::new(&fixture.rules, &elevated, &[], fixture.data.path().into())
        .finish(std::slice::from_ref(&leftover));

    assert!(matches!(result, Err(ApplyError::Elevated)), "{result:?}");
    assert!(leftover.tombstone.exists());
}

#[test]
fn a_journal_cut_short_ends_its_run_there() {
    let fixture = Fixture::new();
    fixture.apply(&fixture.plan());
    let journal_dir = fixture.data.path().join("journal");
    let journal = fs::read_dir(&journal_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let complete = fs::read_to_string(&journal).unwrap();
    let mut lines: Vec<&str> = complete.lines().collect();
    lines.truncate(2);
    let cut = format!("{}\n{{\"event\":\"do", lines.join("\n"));
    fs::write(&journal, cut).unwrap();
    fs::write(journal_dir.join("0-1-0.jsonl"), "not a journal").unwrap();

    let history = fixture.executor().history().unwrap();

    assert_eq!(history.runs.len(), 1, "{history:?}");
    assert!(!history.runs[0].finished);
    assert!(history.runs[0].outcomes.is_empty(), "{history:?}");
    assert_eq!(history.unreadable, [journal_dir.join("0-1-0.jsonl")]);
}

/// Cleanse only removes files and folders, so a journal saying it was removing a link, even one
/// that's really at a tombstone, has been damaged and is ignored.
#[test]
fn a_journal_naming_a_link_has_no_leftover() {
    let fixture = Fixture::new();
    let outside = tempfile::tempdir().unwrap();
    write_old(&outside.path().join("keep.txt"), "x");
    let tombstone = delete::tombstone_for(&fixture.node_modules()).unwrap();
    link_dir(outside.path(), &tombstone);
    let identity = fixture.system.inspect(&tombstone).unwrap().identity;
    let mut journal = Journal::start(
        &fixture.data.path().join("journal"),
        Action::Apply,
        &[fixture.root.path().to_path_buf()],
        1,
    )
    .unwrap();
    journal
        .record(&Record::Removing {
            path: fixture.node_modules().to_str().unwrap().to_owned(),
            tombstone: tombstone.to_str().unwrap().to_owned(),
            kind: EntryKind::Link,
            rule: RuleId("javascript.node_modules".into()),
            identity,
        })
        .unwrap();

    let history = fixture.executor().history().unwrap();

    assert!(history.leftovers.is_empty(), "{history:?}");
    assert!(outside.path().join("keep.txt").exists());
}

#[test]
fn removals_journaled_before_kinds_were_recorded_are_folders() {
    let fixture = Fixture::new();
    let identity = fixture
        .system
        .inspect(&fixture.node_modules())
        .unwrap()
        .identity;
    let leftover = interrupt_removal(&fixture, identity);
    let journal = fs::read_dir(fixture.data.path().join("journal"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let written = fs::read_to_string(&journal).unwrap();
    assert!(written.contains(r#""kind":"dir","#), "{written}");
    fs::write(&journal, written.replace(r#""kind":"dir","#, "")).unwrap();

    let history = fixture.executor().history().unwrap();

    assert_eq!(history.leftovers, [leftover]);
}

#[test]
fn deletes_an_approved_file_and_journals_it() {
    let fixture = Fixture::dump();
    let plan = fixture.plan();
    assert_eq!(plan.items[0].kind, EntryKind::File);

    let outcomes = fixture.apply(&plan);

    assert!(
        matches!(&outcomes[..], [Outcome::Deleted { size }] if size.logical == 4),
        "{outcomes:?}"
    );
    fixture.assert_dumps_hold(false);
    let events = fixture.journal_events();
    let removing = events
        .iter()
        .find(|event| event["event"] == "removing")
        .unwrap();
    assert_eq!(removing["kind"], "file");
}

#[test]
fn deletes_a_read_only_file() {
    let fixture = Fixture::dump();
    let mut permissions = fs::metadata(fixture.crash_dump()).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(fixture.crash_dump(), permissions).unwrap();
    let plan = fixture.plan();

    let outcomes = fixture.apply(&plan);

    assert!(
        matches!(&outcomes[..], [Outcome::Deleted { .. }]),
        "{outcomes:?}"
    );
    fixture.assert_dumps_hold(false);
}

#[test]
fn leaves_a_folder_where_an_approved_file_was() {
    let fixture = Fixture::dump();
    let plan = fixture.plan();
    fs::remove_file(fixture.crash_dump()).unwrap();
    write_old(&fixture.crash_dump().join("inside.txt"), "x");

    let outcomes = fixture.apply(&plan);

    assert_eq!(outcomes, [skipped(SkipReason::Replaced)]);
    assert!(fixture.crash_dump().join("inside.txt").exists());
}

/// Windows won't rename a file while a program has it open without allowing that, so it's left
/// in place.
#[cfg(windows)]
#[test]
fn leaves_a_file_a_program_has_open() {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_SHARE_READ | FILE_SHARE_WRITE, as most programs open files. Rust's default also
    // shares deleting, which lets the file be renamed and deleted while it's open.
    const SHARE_READ_WRITE: u32 = 0x1 | 0x2;

    let fixture = Fixture::dump();
    let plan = fixture.plan();
    let open = File::options()
        .read(true)
        .share_mode(SHARE_READ_WRITE)
        .open(fixture.crash_dump())
        .unwrap();

    let outcomes = fixture.apply(&plan);
    drop(open);

    assert!(
        matches!(&outcomes[..], [Outcome::Failed { .. }]),
        "{outcomes:?}"
    );
    fixture.assert_dumps_hold(true);
}

#[test]
fn quarantines_a_file_and_restores_it() {
    let fixture = Fixture::dump();
    let mut plan = fixture.plan();
    plan.items[0].disposal = Disposal::Quarantine;

    let outcomes = fixture.apply(&plan);

    assert!(
        matches!(&outcomes[..], [Outcome::Quarantined { .. }]),
        "{outcomes:?}"
    );
    fixture.assert_dumps_hold(false);
    let listing = fixture.executor().quarantined().unwrap();
    assert_eq!(listing.entries.len(), 1, "{listing:?}");
    assert_eq!(listing.entries[0].kind, EntryKind::File);

    let handled = fixture.executor().restore(&listing.entries).unwrap();

    assert_eq!(handled.outcomes, [EntryOutcome::Restored]);
    assert_eq!(fs::read_to_string(fixture.crash_dump()).unwrap(), "abcd");
    assert!(fixture.quarantine_is_empty());
}

#[test]
fn purging_deletes_a_quarantined_file_for_good() {
    let fixture = Fixture::dump();
    let mut plan = fixture.plan();
    plan.items[0].disposal = Disposal::Quarantine;
    fixture.apply(&plan);
    let entries = fixture.executor().quarantined().unwrap().entries;

    let handled = fixture.executor().purge(&entries).unwrap();

    assert!(
        matches!(&handled.outcomes[..], [EntryOutcome::Purged { size }] if size.logical == 4),
        "{handled:?}"
    );
    assert!(fixture.quarantine_is_empty());
    let left: Vec<_> = fs::read_dir(fixture.data.path().join("quarantine"))
        .unwrap()
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

/// Purging acts only on the kind of item its record says was quarantined, so a folder put where
/// a quarantined file was is left alone.
#[test]
fn purging_leaves_a_folder_where_a_quarantined_file_was() {
    let fixture = Fixture::dump();
    let mut plan = fixture.plan();
    plan.items[0].disposal = Disposal::Quarantine;
    fixture.apply(&plan);
    let entries = fixture.executor().quarantined().unwrap().entries;
    let item = fixture.data.path().join("quarantine").join(&entries[0].id);
    fs::remove_file(&item).unwrap();
    write_old(&item.join("keep.txt"), "x");

    let handled = fixture.executor().purge(&entries).unwrap();

    assert!(
        matches!(&handled.outcomes[..], [EntryOutcome::Failed { .. }]),
        "{handled:?}"
    );
    assert!(item.join("keep.txt").exists());
}

#[test]
fn finds_and_finishes_a_file_removal_that_was_cut_short() {
    let fixture = Fixture::dump();
    let identity = fixture
        .system
        .inspect(&fixture.crash_dump())
        .unwrap()
        .identity;
    let leftover = interrupt_removal(&fixture, identity);

    let history = fixture.executor().history().unwrap();

    assert_eq!(history.leftovers, std::slice::from_ref(&leftover));
    assert_eq!(leftover.kind, EntryKind::File);

    let applied = fixture.executor().finish(&history.leftovers).unwrap();

    assert!(
        matches!(&applied.outcomes[..], [Outcome::Deleted { size }] if size.logical == 4),
        "{applied:?}"
    );
    fixture.assert_dumps_hold(false);
}

/// The built-in rule for `node_modules`, but in the Review tier.
const REVIEW_NODE_MODULES: &str = r#"
[[rule]]
id = "test.node_modules"
name = "Dependencies to review"
category = "dependencies"
risk = "review"
explain = "For tests."

[rule.match]
kind = "project_artifact"
names = ["node_modules"]
markers = ["package.json"]
"#;

#[test]
fn a_plan_cant_delete_review_items() {
    let mut fixture = Fixture::new();
    fixture.rules = RuleSet::from_sources([("review.toml", REVIEW_NODE_MODULES)]).unwrap();
    let mut plan = fixture.plan();
    assert_eq!(plan.items[0].disposal, Disposal::Quarantine);
    plan.items[0].disposal = Disposal::Delete;

    let outcomes = fixture.apply(&plan);

    assert_eq!(outcomes, [skipped(SkipReason::NeedsQuarantine)]);
    assert!(fixture.node_modules().join("left-pad/index.js").exists());
}

/// `plan`, saved with every item selected.
fn saved(plan: &Plan) -> SavedPlan {
    SavedPlan {
        cleanse_plan: SavedPlan::FORMAT,
        saved_by: "test".into(),
        saved_at: 1,
        roots: plan.roots.clone(),
        items: plan
            .items
            .iter()
            .map(|item| SavedItem {
                selected: true,
                item: item.clone(),
            })
            .collect(),
    }
}

#[test]
fn saves_a_plan_and_reads_it_back() {
    let fixture = Fixture::new();
    let saved = saved(&fixture.plan());
    let file = fixture.data.path().join("plan.json");

    save_plan(&file, &saved).unwrap();
    // Saving again replaces the earlier plan.
    save_plan(&file, &saved).unwrap();

    assert_eq!(load_plan(&file).unwrap(), saved);
}

#[test]
fn saving_never_replaces_a_file_that_isnt_a_plan() {
    let fixture = Fixture::new();
    let file = fixture.data.path().join("notes.json");
    fs::write(&file, r#"{"notes": true}"#).unwrap();

    let result = save_plan(&file, &saved(&fixture.plan()));

    assert!(
        matches!(result, Err(PlanFileError::WouldReplace)),
        "{result:?}"
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), r#"{"notes": true}"#);
}

#[test]
fn reads_only_plans_it_understands() {
    let fixture = Fixture::new();
    let saved = saved(&fixture.plan());
    let file = fixture.data.path().join("plan.json");
    let load = |contents: &str| {
        fs::write(&file, contents).unwrap();
        load_plan(&file)
    };
    let mut json = serde_json::to_value(&saved).unwrap();

    json["cleanse_plan"] = 2.into();
    let newer = load(&json.to_string());
    json["cleanse_plan"] = SavedPlan::FORMAT.into();
    json["items"][0]["path"] = "web/node_modules".into();
    let relative = load(&json.to_string());
    let not_json = load("not json");
    let other_json = load(r#"{"notes": true}"#);
    let with_bom = load(&format!(
        "\u{feff}{}",
        serde_json::to_string(&saved).unwrap()
    ));

    assert!(
        matches!(newer, Err(PlanFileError::UnknownFormat(2))),
        "{newer:?}"
    );
    assert!(
        matches!(relative, Err(PlanFileError::RelativePath(_))),
        "{relative:?}"
    );
    assert!(
        matches!(not_json, Err(PlanFileError::NotAPlan(_))),
        "{not_json:?}"
    );
    assert!(
        matches!(other_json, Err(PlanFileError::NotAPlan(_))),
        "{other_json:?}"
    );
    assert_eq!(with_bom.unwrap(), saved);
}
