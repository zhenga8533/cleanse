#![allow(clippy::disallowed_methods, reason = "tests build fixture trees")]

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cleanse_model::{CandidateId, Category, EntryOutcome, Outcome, Risk, RuleId};
use tempfile::TempDir;

use super::{ScanId, Session, SessionError};
use crate::tests::{project, write_old};
use crate::{Engine, KnownFolders, NodeKind, RuleSet, Section, System};

/// A session over a temporary folder, whose home folder is `home` inside it, with its own data
/// folder, acting as if it isn't elevated, since CI runs tests as an administrator on Windows.
struct Fixture {
    root: TempDir,
    _data: TempDir,
    session: Session,
}

impl Fixture {
    fn new() -> Self {
        Self::with_rules(RuleSet::builtin().unwrap())
    }

    fn with_rules(rules: RuleSet) -> Self {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let folders = KnownFolders {
            home: Some(root.path().join("home")),
            temp: root.path().join("temp"),
            ..KnownFolders::default()
        };
        let engine = Engine {
            rules,
            system: System::new()
                .unwrap()
                .elevated(false)
                .with_known_folders(folders),
            data_dir: Some(data.path().to_path_buf()),
            ..Engine::new().unwrap()
        };
        Self {
            root,
            _data: data,
            session: Session::new(engine),
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.path().join(relative)
    }

    fn scan(&self) -> ScanId {
        self.session
            .scan(&[self.root.path().to_path_buf()], &|_| {})
            .unwrap()
    }

    /// The IDs of everything in `section`, largest first.
    fn ids(&self, scan: ScanId, section: Section) -> Vec<CandidateId> {
        self.session
            .candidates(scan, section, None, 0, usize::MAX)
            .unwrap()
            .items
            .iter()
            .map(|candidate| candidate.id)
            .collect()
    }
}

/// A JavaScript project whose dependencies take `bytes`.
fn project_of(root: &Path, name: &str, bytes: usize) {
    write_old(&root.join(name).join("package.json"), "{}");
    write_old(
        &root.join(name).join("node_modules/dep/index.js"),
        &"x".repeat(bytes),
    );
}

#[test]
fn summarizes_every_section_and_rule() {
    let fixture = Fixture::new();
    project(fixture.root.path());
    write_old(&fixture.path("home/go/pkg/mod/cache/x.zip"), "zip");

    let scan = fixture.scan();
    let summary = fixture.session.summary(scan).unwrap();

    let sections: Vec<(Section, usize, usize)> = summary
        .sections
        .iter()
        .map(|section| (section.section, section.items, section.preselected))
        .collect();
    assert_eq!(
        sections,
        [
            (Section::Removable, 1, 1),
            (Section::Review, 0, 0),
            (Section::Advice, 1, 0),
            (Section::Kept, 0, 0),
        ]
    );
    assert_eq!(
        summary.sections[0].rules[0].rule,
        RuleId("javascript.node_modules".into())
    );
    assert_eq!(summary.sections[0].size.logical, 4);
    assert!(!summary.cancelled);
}

#[test]
fn lists_candidates_largest_first_a_page_at_a_time() {
    let fixture = Fixture::new();
    project_of(fixture.root.path(), "small", 10_000);
    project_of(fixture.root.path(), "large", 30_000);
    project_of(fixture.root.path(), "middle", 20_000);
    let scan = fixture.scan();

    let first = fixture
        .session
        .candidates(scan, Section::Removable, None, 0, 2)
        .unwrap();
    let rest = fixture
        .session
        .candidates(scan, Section::Removable, None, 2, 2)
        .unwrap();
    let other_rule = fixture
        .session
        .candidates(
            scan,
            Section::Removable,
            Some(&RuleId("python.venv".into())),
            0,
            10,
        )
        .unwrap();

    let names = |page: &super::Page<cleanse_model::Candidate>| -> Vec<String> {
        page.items
            .iter()
            .map(|candidate| {
                let project = candidate.found.path.parent().unwrap();
                project.file_name().unwrap().to_string_lossy().into_owned()
            })
            .collect()
    };
    assert_eq!(names(&first), ["large", "middle"]);
    assert_eq!((first.total, first.offset), (3, 0));
    assert_eq!(names(&rest), ["small"]);
    assert_eq!(other_rule.total, 0);
}

#[test]
fn ids_from_an_earlier_scan_are_refused() {
    let fixture = Fixture::new();
    project(fixture.root.path());
    let earlier = fixture.scan();
    let id = fixture.ids(earlier, Section::Removable)[0];

    let later = fixture.scan();

    assert_ne!(earlier, later);
    assert!(matches!(
        fixture.session.summary(earlier),
        Err(SessionError::OldScan)
    ));
    assert!(matches!(
        fixture.session.explain(earlier, id),
        Err(SessionError::OldScan)
    ));
    assert!(matches!(
        fixture.session.clean(earlier, &[id], &|_| {}),
        Err(SessionError::OldScan)
    ));
    assert!(fixture.path("web/node_modules").exists());
    assert!(fixture.session.summary(later).is_ok());
}

#[test]
fn explains_an_item_by_id() {
    let fixture = Fixture::new();
    project(fixture.root.path());
    let scan = fixture.scan();
    let id = fixture.ids(scan, Section::Removable)[0];

    let explained = fixture.session.explain(scan, id).unwrap();

    assert_eq!(explained.rule.id.0, "javascript.node_modules");
    assert_eq!(explained.section, Section::Removable);
    assert!(explained.candidate.found.path.ends_with("web/node_modules"));
    assert!(matches!(
        fixture.session.explain(scan, CandidateId(999)),
        Err(SessionError::UnknownCandidate(CandidateId(999)))
    ));
}

#[test]
fn cleans_by_id_and_drops_what_it_removed() {
    let fixture = Fixture::new();
    project(fixture.root.path());
    let scan = fixture.scan();
    let ids = fixture.ids(scan, Section::Removable);
    let progress = Mutex::new(Vec::new());

    let cleaned = fixture
        .session
        .clean(scan, &ids, &|done| progress.lock().unwrap().push(done))
        .unwrap();

    assert!(
        matches!(&cleaned.outcomes[..], [(id, Outcome::Deleted { .. })] if *id == ids[0]),
        "{cleaned:?}"
    );
    assert!(cleaned.journal.is_some());
    assert_eq!(*progress.lock().unwrap(), [1]);
    assert!(!fixture.path("web/node_modules").exists());
    let summary = fixture.session.summary(scan).unwrap();
    assert_eq!(summary.sections[0].items, 0);
    assert_eq!(summary.removed.logical, 4);
}

#[test]
fn kept_and_unknown_items_cant_be_cleaned() {
    let fixture = Fixture::new();
    project(fixture.root.path());
    write_old(&fixture.path("home/go/pkg/mod/cache/x.zip"), "zip");
    let scan = fixture.scan();
    let advised = fixture.ids(scan, Section::Advice);
    let removable = fixture.ids(scan, Section::Removable);

    let kept = fixture
        .session
        .clean(scan, &[removable[0], advised[0]], &|_| {});
    let unknown = fixture.session.clean(scan, &[CandidateId(999)], &|_| {});
    let nothing = fixture.session.clean(scan, &[], &|_| {}).unwrap();

    assert!(
        matches!(kept, Err(SessionError::NotRemovable(id)) if id == advised[0]),
        "{kept:?}"
    );
    assert!(matches!(unknown, Err(SessionError::UnknownCandidate(_))));
    assert!(nothing.outcomes.is_empty() && nothing.journal.is_none());
    // Refusing one item refuses the whole cleanup.
    assert!(fixture.path("web/node_modules").exists());
    assert!(fixture.path("home/go/pkg/mod").exists());
}

/// The built-in rule for `node_modules`, but in the Review tier, so it's quarantined.
const REVIEW_NODE_MODULES: &str = r#"
[[rule]]
id = "test.node_modules"
name = "Dependencies to review"
category = "dependencies"
risk = "review"
explain = "Because."

[rule.match]
kind = "project_artifact"
names = ["node_modules"]
markers = ["package.json"]
"#;

#[test]
fn quarantines_review_items_and_restores_them_by_entry_id() {
    let fixture =
        Fixture::with_rules(RuleSet::from_sources([("test.toml", REVIEW_NODE_MODULES)]).unwrap());
    project(fixture.root.path());
    let scan = fixture.scan();
    let ids = fixture.ids(scan, Section::Review);

    let cleaned = fixture.session.clean(scan, &ids, &|_| {}).unwrap();

    assert!(
        matches!(&cleaned.outcomes[..], [(_, Outcome::Quarantined { .. })]),
        "{cleaned:?}"
    );
    let entries = fixture.session.quarantined().unwrap().entries;
    assert_eq!(entries.len(), 1);
    assert!(matches!(
        fixture.session.restore(&["no-such-entry".into()]),
        Err(SessionError::UnknownEntry(_))
    ));

    let restored = fixture
        .session
        .restore(&[entries[0].id.clone(), entries[0].id.clone()])
        .unwrap();

    assert!(restored.journal.is_some());
    let [(entry, outcome)] = &restored.outcomes[..] else {
        panic!("{restored:?}")
    };
    assert_eq!(entry.id, entries[0].id);
    assert_eq!(*outcome, EntryOutcome::Restored);
    assert!(fixture.path("web/node_modules/left-pad/index.js").exists());
}

#[test]
fn purges_by_entry_id_saying_which_is_which() {
    let fixture =
        Fixture::with_rules(RuleSet::from_sources([("test.toml", REVIEW_NODE_MODULES)]).unwrap());
    project(fixture.root.path());
    let scan = fixture.scan();
    let ids = fixture.ids(scan, Section::Review);
    fixture.session.clean(scan, &ids, &|_| {}).unwrap();
    let entries = fixture.session.quarantined().unwrap().entries;

    let purged = fixture.session.purge(&[entries[0].id.clone()]).unwrap();

    let [(entry, EntryOutcome::Purged { .. })] = &purged.outcomes[..] else {
        panic!("{purged:?}")
    };
    assert_eq!(entry.original, entries[0].original);
    assert_eq!(fixture.session.quarantined().unwrap().entries, Vec::new());
}

#[test]
fn nothing_to_restore_purge_or_finish_writes_no_journal() {
    let fixture = Fixture::new();

    let restored = fixture.session.restore(&[]).unwrap();
    let purged = fixture.session.purge(&[]).unwrap();
    let finished = fixture.session.finish().unwrap();

    assert!(restored.journal.is_none() && restored.outcomes.is_empty());
    assert!(purged.journal.is_none() && purged.outcomes.is_empty());
    assert!(finished.journal.is_none() && finished.outcomes.is_empty());
    assert!(fixture.session.history().unwrap().runs.is_empty());
}

#[test]
fn shows_folders_with_what_can_be_removed_under_each() {
    let fixture = Fixture::new();
    project(fixture.root.path());
    let scan = fixture.scan();

    let roots = fixture.session.folder(scan, None, 10, &[]).unwrap();
    let [root] = &roots.entries[..] else {
        panic!("{roots:?}")
    };
    let top = fixture
        .session
        .folder(scan, Some(root.node.index()), 10, &[])
        .unwrap();
    let web = top
        .entries
        .iter()
        .find(|entry| entry.name == "web")
        .unwrap();
    let inside_web = fixture
        .session
        .folder(scan, Some(web.node.index()), 1, &[])
        .unwrap();

    assert!(roots.trail.is_empty());
    assert_eq!(root.path, std::path::absolute(fixture.root.path()).unwrap());
    assert_eq!(top.trail.len(), 1);
    assert_eq!(web.usage.logical, 2 + 4 + 1);
    assert_eq!(inside_web.trail.len(), 2);
    assert_eq!(inside_web.entries.len(), 1);
    assert_eq!(inside_web.hidden, 1);
    assert_eq!(inside_web.own_files, 1);
    let node_modules = &inside_web.entries[0];
    assert_eq!(node_modules.kind, NodeKind::Item);
    assert!(node_modules.candidate.is_some());
    assert_eq!(web.removable, node_modules.usage.on_disk);
    assert_eq!(node_modules.removable, node_modules.usage.on_disk);
    assert!(matches!(
        fixture.session.folder(scan, Some(u32::MAX), 10, &[]),
        Err(SessionError::UnknownFolder(_))
    ));
}

#[test]
fn counts_only_the_chosen_categories_as_removable() {
    let fixture = Fixture::new();
    project(fixture.root.path());
    let scan = fixture.scan();
    let root = fixture.session.folder(scan, None, 10, &[]).unwrap().entries[0].node;
    let web = |categories: &[Category]| {
        let top = fixture
            .session
            .folder(scan, Some(root.index()), 10, categories)
            .unwrap();
        top.entries
            .into_iter()
            .find(|entry| entry.name == "web")
            .unwrap()
    };

    let all = web(&[]);
    let dependencies = web(&[Category::Dependencies]);
    let logs = web(&[Category::Logs, Category::Temp]);

    assert!(all.removable > 0);
    assert_eq!(dependencies.removable, all.removable);
    assert_eq!(logs.removable, 0);
    // Sizes count everything whatever is chosen.
    assert_eq!(logs.usage, all.usage);
}

#[test]
fn only_one_scan_or_change_runs_at_a_time() {
    let fixture = Fixture::new();
    project(fixture.root.path());
    let during = Mutex::new(None);

    fixture
        .session
        .scan(&[fixture.root.path().to_path_buf()], &|_| {
            let mut during = during.lock().unwrap();
            if during.is_none() {
                *during = Some(fixture.session.scan(&[], &|_| {}));
            }
        })
        .unwrap();

    assert!(matches!(
        during.into_inner().unwrap(),
        Some(Err(SessionError::Busy))
    ));
    assert!(fixture.scan().0 > 0);
}

#[test]
fn a_cancelled_scan_keeps_what_it_found_so_far() {
    let fixture = Fixture::new();
    project(fixture.root.path());

    let scan = fixture
        .session
        .scan(&[fixture.root.path().to_path_buf()], &|_| {
            fixture.session.cancel();
        })
        .unwrap();

    assert!(fixture.session.summary(scan).unwrap().cancelled);
    // The next scan starts uncancelled.
    let again = fixture.scan();
    assert!(!fixture.session.summary(again).unwrap().cancelled);
}

#[test]
fn sections_follow_the_tier_and_advice() {
    let fixture = Fixture::new();
    project(fixture.root.path());
    write_old(&fixture.path("home/go/pkg/mod/cache/x.zip"), "zip");
    let scan = fixture.scan();
    let rules = fixture.session.engine().rules();

    let risks: Vec<(Section, Risk)> = Section::ALL
        .into_iter()
        .flat_map(|section| {
            fixture
                .session
                .candidates(scan, section, None, 0, 10)
                .unwrap()
                .items
                .into_iter()
                .map(move |candidate| (section, candidate.risk()))
        })
        .collect();

    assert_eq!(
        risks,
        [
            (Section::Removable, Risk::Regenerable),
            (Section::Advice, Risk::Protected)
        ]
    );
    for (section, _) in risks {
        let candidates = fixture
            .session
            .candidates(scan, section, None, 0, 10)
            .unwrap();
        assert_eq!(Section::of(rules, &candidates.items[0]), section);
    }
}
