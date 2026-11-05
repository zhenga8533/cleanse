//! System cleanup on fixtures: system folders under a temporary folder, read through a probe
//! that reports them as the system's, since CI's Linux runners aren't root. The tests named
//! `when_elevated` use the real platform instead, and run where tests are elevated or root.

#![allow(clippy::disallowed_methods, reason = "tests build fixture trees")]
#![cfg_attr(
    target_os = "macos",
    allow(
        dead_code,
        unused_imports,
        reason = "system cleanup is for Windows and Linux, and so are its tests"
    )
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::SystemTime;

use cleanse_core::rules::{Matcher, Os, RuleSet};
use cleanse_core::safety::{Assessor, ProtectedPolicy, SystemRoots};
use cleanse_model::{
    CandidateId, Concern, Disposal, EntryOutcome, Outcome, Plan, PlanItem, QuarantineEntry,
    SkipReason,
};
use cleanse_platform::{Elevation, FileOwner, Probe, StagedSystem, System, SystemFolders};
use serde_json::Value;
use tempfile::TempDir;

use crate::journal::{Journal, Record};
use crate::tests::write_old;
use crate::{Action, ApplyError, Executor, Leftover, delete};

/// A system rule for each `.tmp` item directly in the Windows folder's `Temp`, and `.crash` item
/// in `/var/crash`.
const RULE: &str = r#"
[[rule]]
id = "test.system_files"
scope = "system"
name = "Files"
category = "temp"
risk = "safe"
explain = "Because."

[rule.match]
kind = "known_location"
entry = "any"
locations = [
    { os = "windows", base = "system_root", path = "Temp/*.tmp" },
    { os = "linux", base = "var", path = "crash/*.crash" },
]
"#;

/// System folders under a temporary folder, with one file a system rule matches in them.
struct Fixture<P> {
    root: TempDir,
    rules: RuleSet,
    probe: P,
}

impl Fixture<StagedSystem> {
    /// Its data folder exists, and the probe reports it as private.
    fn new() -> Self {
        let fixture = Self::with_probe(|_| StagedSystem::new().unwrap());
        let data = fixture.data();
        fs::create_dir_all(&data).unwrap();
        Fixture {
            probe: fixture.probe.private(&data),
            ..fixture
        }
    }
}

impl<P: Probe> Fixture<P> {
    fn with_probe(probe: impl FnOnce(&Path) -> P) -> Self {
        let root = tempfile::tempdir().unwrap();
        let root_path = std::path::absolute(root.path()).unwrap();
        let fixture = Self {
            probe: probe(&root_path),
            root,
            rules: RuleSet::system_from_sources([("test.toml", RULE)]).unwrap(),
        };
        write_old(&fixture.item(), "x");
        fixture
    }

    fn root(&self) -> PathBuf {
        std::path::absolute(self.root.path()).unwrap()
    }

    fn folders(&self) -> SystemFolders {
        let root = self.root();
        SystemFolders {
            system_root: Some(root.join("system_root")),
            program_data: Some(root.join("program_data")),
            var: Some(root.join("var")),
        }
    }

    fn data(&self) -> PathBuf {
        self.folders().cleanse_data().unwrap()
    }

    /// The folder [`RULE`] matches files directly inside on this OS.
    fn folder(&self) -> PathBuf {
        if cfg!(windows) {
            self.root().join("system_root").join("Temp")
        } else {
            self.root().join("var").join("crash")
        }
    }

    fn item(&self) -> PathBuf {
        self.folder().join(if cfg!(windows) {
            "setup.tmp"
        } else {
            "_usr_bin_app.0.crash"
        })
    }

    fn executor(&self) -> Executor<'_> {
        Executor::system(&self.rules, &self.probe, self.folders()).unwrap()
    }

    /// Approves the file, as system cleanup's scan would find it.
    fn plan(&self, disposal: Disposal) -> Plan {
        let folders = self.folders();
        let matcher = Matcher::system(self.rules.rules(), &folders, Os::current());
        let (found, _) = matcher.find(&self.folder()).unwrap();
        let assessor = Assessor::system(
            &self.probe,
            ProtectedPolicy::for_system_cleanup(&folders, &self.probe),
            SystemRoots::verify(&self.rules, &folders, &self.probe),
            SystemTime::now(),
        );
        let items: Vec<PlanItem> = found
            .iter()
            .map(|detection| assessor.assess(CandidateId(0), detection))
            .filter_map(|candidate| PlanItem::approve(&candidate))
            .map(|mut item| {
                item.disposal = disposal;
                item
            })
            .collect();
        assert_eq!(items.len(), 1, "{items:#?}");
        Plan {
            roots: Vec::new(),
            items,
        }
    }

    fn apply(&self, plan: &Plan) -> Result<Vec<Outcome>, ApplyError> {
        self.executor()
            .apply(plan, &|_| {}, &AtomicBool::new(false))
            .map(|applied| applied.outcomes)
    }

    /// The events of each journal in the data folder, oldest first.
    fn journals(&self) -> Vec<Vec<Value>> {
        let mut files: Vec<_> = fs::read_dir(self.data().join("journal"))
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

    fn quarantine_one(&self) -> QuarantineEntry {
        let outcomes = self.apply(&self.plan(Disposal::Quarantine)).unwrap();
        assert!(
            matches!(&outcomes[..], [Outcome::Quarantined { .. }]),
            "{outcomes:?}"
        );
        let listing = self.executor().quarantined().unwrap();
        assert_eq!(listing.entries.len(), 1, "{listing:?}");
        listing.entries[0].clone()
    }
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn deletes_an_approved_item_and_journals_it_in_its_own_folder() {
    let fixture = Fixture::new();

    let outcomes = fixture.apply(&fixture.plan(Disposal::Delete)).unwrap();

    assert!(
        matches!(&outcomes[..], [Outcome::Deleted { .. }]),
        "{outcomes:?}"
    );
    assert!(!fixture.item().exists());
    let journals = fixture.journals();
    assert_eq!(journals.len(), 1);
    assert_eq!(journals[0][0]["roots"], Value::Array(Vec::new()));
    assert_eq!(journals[0][1]["event"], "removing");
}

/// The plan's roots don't widen what's removed, or narrow it: only the system folders count.
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn ignores_the_plans_roots() {
    let fixture = Fixture::new();
    let mut plan = fixture.plan(Disposal::Delete);
    plan.roots = vec![fixture.root().join("elsewhere")];
    let mut outside = plan.clone();
    outside.items[0].path = fixture.root().join("system_root/System32/setup.tmp");
    write_old(&outside.items[0].path, "x");
    outside.items[0].identity = fixture
        .probe
        .inspect(&outside.items[0].path)
        .unwrap()
        .identity;
    outside.roots = vec![fixture.root()];

    let kept = fixture.apply(&outside).unwrap();
    let removed = fixture.apply(&plan).unwrap();

    assert_eq!(
        kept,
        [Outcome::Skipped {
            reason: SkipReason::NoLongerMatches
        }]
    );
    assert!(outside.items[0].path.exists());
    assert!(
        matches!(&removed[..], [Outcome::Deleted { .. }]),
        "{removed:?}"
    );
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn quarantines_an_item_in_its_own_folder_and_restores_it() {
    let fixture = Fixture::new();

    let entry = fixture.quarantine_one();

    assert!(!fixture.item().exists());
    assert_eq!(entry.original, fixture.item());
    assert!(fixture.data().join("quarantine").join(&entry.id).exists());
    let handled = fixture.executor().restore(&[entry]).unwrap();
    assert_eq!(handled.outcomes, [EntryOutcome::Restored]);
    assert!(fixture.item().exists());
}

/// Only Administrators and SYSTEM, or root, can change the quarantine, but where a record points
/// is checked anyway.
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn wont_restore_to_where_an_edited_record_points() {
    let fixture = Fixture::new();
    let entry = fixture.quarantine_one();
    let record = fixture
        .data()
        .join("quarantine")
        .join(format!("{}.json", entry.id));
    let elsewhere = [
        fixture.root().join("system_root/System32/setup.tmp"),
        fixture.root().join("var/lib/_usr_bin_app.0.crash"),
        fixture.folder().join("setup.log"),
    ];

    for original in elsewhere {
        let edited = QuarantineEntry {
            original: original.clone(),
            ..entry.clone()
        };
        fs::write(&record, serde_json::to_vec(&edited).unwrap()).unwrap();
        let listed = fixture.executor().quarantined().unwrap().entries;

        let handled = fixture.executor().restore(&listed).unwrap();

        assert!(
            matches!(&handled.outcomes[..], [EntryOutcome::Failed { error }] if error.contains("isn't somewhere system cleanup")),
            "{}: {handled:?}",
            original.display()
        );
        assert!(!original.exists());
    }
    assert!(fixture.data().join("quarantine").join(&entry.id).exists());
}

/// A record pointing to the right place in a folder that no longer checks out isn't restored.
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn wont_restore_into_a_folder_that_isnt_the_systems() {
    let mut fixture = Fixture::new();
    let entry = fixture.quarantine_one();
    let folder = fixture.folder();
    fixture.probe = fixture.probe.owned(&folder, Some(FileOwner::Other));

    let handled = fixture.executor().restore(&[entry]).unwrap();

    assert!(
        matches!(&handled.outcomes[..], [EntryOutcome::Failed { .. }]),
        "{handled:?}"
    );
    assert!(!fixture.item().exists());
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn runs_however_the_account_is_elevated() {
    let mut fixture = Fixture::new();

    for elevation in [Elevation::Elevated, Elevation::AlwaysElevated] {
        fixture.probe = fixture.probe.with_elevation(elevation);
        let result = fixture.executor().check_can_change();

        assert!(result.is_ok(), "{elevation:?}: {result:?}");
    }
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn changes_nothing_unless_elevated() {
    let mut fixture = Fixture::new();
    let plan = fixture.plan(Disposal::Delete);
    let entry = fixture.quarantine_one();
    fixture.probe = fixture.probe.elevated(false);
    let executor = fixture.executor();

    assert!(matches!(
        executor.apply(&plan, &|_| {}, &AtomicBool::new(false)),
        Err(ApplyError::NotElevated)
    ));
    assert!(matches!(
        executor.restore(std::slice::from_ref(&entry)),
        Err(ApplyError::NotElevated)
    ));
    assert!(matches!(
        executor.purge(std::slice::from_ref(&entry)),
        Err(ApplyError::NotElevated)
    ));
    assert!(matches!(executor.finish(&[]), Err(ApplyError::NotElevated)));
    assert_eq!(fixture.executor().quarantined().unwrap().entries, [entry]);
}

/// A scan leaves who owns everything inside an item to the check right before it's removed.
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn checks_who_owns_whats_inside_right_before_removing_it() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.item()).unwrap();
    let folder = fixture.folder().join(if cfg!(windows) {
        "bundle.tmp"
    } else {
        "bundle.crash"
    });
    write_old(&folder.join("users.txt"), "x");
    let probe = StagedSystem::new()
        .unwrap()
        .private(&fixture.data())
        .owned(&folder.join("users.txt"), Some(FileOwner::Other));
    let fixture = Fixture { probe, ..fixture };
    let plan = fixture.plan(Disposal::Delete);
    assert_eq!(plan.items[0].path, folder);

    let outcomes = fixture.apply(&plan).unwrap();

    assert_eq!(
        outcomes,
        [Outcome::Skipped {
            reason: SkipReason::Blocked {
                concerns: vec![Concern::ContentsNotSystemOwned]
            }
        }]
    );
    assert!(folder.join("users.txt").exists());
}

/// Each cleanup uses only its own rules, so neither can act on the other's.
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn uses_only_system_rules() {
    let fixture = Fixture::new();
    let plan = fixture.plan(Disposal::Delete);
    let user_rules = RuleSet::builtin().unwrap();
    let config = cleanse_core::config::Config::default();
    let unelevated = System::new().unwrap().elevated(false);

    let system = Executor::system(&user_rules, &fixture.probe, fixture.folders()).unwrap();
    let user = Executor::new(&fixture.rules, &unelevated, &config, fixture.data());

    assert!(matches!(
        system.apply(&plan, &|_| {}, &AtomicBool::new(false)),
        Err(ApplyError::WrongRules)
    ));
    assert!(matches!(
        user.apply(&plan, &|_| {}, &AtomicBool::new(false)),
        Err(ApplyError::WrongRules)
    ));
    assert!(fixture.item().exists());
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn refuses_a_data_folder_others_can_change() {
    let fixture = Fixture::with_probe(|_| StagedSystem::new().unwrap());
    let plan = fixture.plan(Disposal::Delete);
    let reads_nothing = fixture.executor();
    assert_eq!(reads_nothing.quarantined().unwrap().entries, Vec::new());
    assert_eq!(reads_nothing.history().unwrap().runs.len(), 0);
    fs::create_dir_all(fixture.data()).unwrap();

    let applied = fixture.apply(&plan);

    assert!(
        matches!(applied, Err(ApplyError::DataFolder(_))),
        "{applied:?}"
    );
    assert!(fixture.item().exists());
    assert!(fixture.executor().quarantined().is_err());
    assert!(fixture.executor().history().is_err());
}

/// Interrupts a removal of `original` after its rename, as a crash would, leaving its journal in
/// the fixture's data folder.
fn interrupt_removal(fixture: &Fixture<StagedSystem>, original: &Path) -> Leftover {
    let plan = fixture.plan(Disposal::Delete);
    let item = &plan.items[0];
    let tombstone = delete::tombstone_for(original).unwrap();
    let mut journal =
        Journal::start(&fixture.data().join("journal"), Action::Apply, &[], 1).unwrap();
    journal
        .record(&Record::Removing {
            path: original.to_str().unwrap().to_owned(),
            tombstone: tombstone.to_str().unwrap().to_owned(),
            kind: item.kind,
            rule: item.rule.clone(),
            identity: item.identity,
        })
        .unwrap();
    fs::rename(&item.path, &tombstone).unwrap();
    Leftover {
        original: original.to_path_buf(),
        tombstone,
        kind: item.kind,
        rule: item.rule.clone(),
        identity: item.identity,
    }
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn finishes_a_removal_only_where_system_cleanup_removes_things() {
    let fixture = Fixture::new();
    let leftover = interrupt_removal(&fixture, &fixture.item());
    let history = fixture.executor().history().unwrap();
    assert_eq!(history.leftovers, std::slice::from_ref(&leftover));

    let finished = fixture.executor().finish(&[leftover]).unwrap();

    assert!(
        matches!(&finished.outcomes[..], [Outcome::Deleted { .. }]),
        "{finished:?}"
    );
    assert_eq!(fixture.executor().history().unwrap().leftovers, Vec::new());

    // A journal naming somewhere else, beside the tombstone it names.
    let other = Fixture::new();
    let mut misnamed = interrupt_removal(&other, &other.item());
    misnamed.original = misnamed.tombstone.with_file_name("setup.log");

    let kept = other.executor().finish(&[misnamed.clone()]).unwrap();

    assert_eq!(
        kept.outcomes,
        [Outcome::Skipped {
            reason: SkipReason::NoLongerMatches
        }]
    );
    assert!(misnamed.tombstone.exists());
}

/// The whole of system cleanup on the real platform: the data folder is created private, the
/// file is the system's for real, and it's removed. Where tests aren't elevated, this is skipped.
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn removes_a_system_file_when_elevated() {
    let probe = System::new().unwrap();
    if !probe.is_elevated() {
        let required = if cfg!(windows) {
            "CI"
        } else {
            "CLEANSE_TEST_AS_ROOT"
        };
        assert!(
            std::env::var_os(required).is_none(),
            "{required} is set, so this has to run as an administrator or root"
        );
        eprintln!("skipped: needs an administrator or root");
        return;
    }
    let fixture = Fixture::with_probe(|_| System::new().unwrap());
    // The data folder is made inside these, as it would be in `ProgramData` or `/var/lib`.
    fs::create_dir_all(fixture.data().parent().unwrap()).unwrap();
    // What elevated programs make belongs to Administrators, except from Git Bash, which makes
    // it its user's.
    #[cfg(windows)]
    {
        let status = std::process::Command::new("icacls")
            .arg(fixture.root())
            .args(["/setowner", "*S-1-5-32-544", "/T", "/C", "/Q"])
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    }

    let outcomes = fixture.apply(&fixture.plan(Disposal::Delete)).unwrap();

    assert!(
        matches!(&outcomes[..], [Outcome::Deleted { .. }]),
        "{outcomes:?}"
    );
    assert!(!fixture.item().exists());
    assert!(fixture.probe.is_private_dir(&fixture.data()).unwrap());
    assert_eq!(fixture.journals().len(), 1);
}
