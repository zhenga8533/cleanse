#![allow(
    clippy::disallowed_methods,
    reason = "tests build fixture trees and repositories, using the git command line"
)]

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use cleanse_model::{Candidate, CandidateId, Concern, EntryKind, FileIdentity, Risk};
use cleanse_platform::{FakeProbe, FileFacts, KnownFolders, Probe, System};
use proptest::prelude::*;

use super::{Assessor, Protected, ProtectedPolicy, Reach};
use crate::rules::{Matcher, Os, RuleSet, Target};

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "core.autocrlf=false"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
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

/// An absolute path made of `parts` below a fake root, such as `C:\fake\…` or `/fake/…`.
fn fake_path(parts: &[&str]) -> PathBuf {
    let mut path = PathBuf::from(if cfg!(windows) { r"C:\fake" } else { "/fake" });
    path.extend(parts);
    path
}

fn location(path: PathBuf, reach: Reach, what: &str) -> Protected {
    Protected {
        path,
        reach,
        what: what.into(),
        except: Vec::new(),
    }
}

fn policy(locations: Vec<Protected>) -> ProtectedPolicy {
    ProtectedPolicy::new(locations, &FakeProbe::default())
}

fn identity(file: u128) -> FileIdentity {
    FileIdentity { volume: 1, file }
}

#[test]
fn protects_a_location_and_whatever_contains_it() {
    let home = fake_path(&["home"]);
    let policy = policy(vec![location(
        home.clone(),
        Reach::Exact,
        "your home folder",
    )]);

    assert_eq!(policy.check(&home, None, &[]), Some("your home folder"));
    assert_eq!(
        policy.check(&fake_path(&[]), None, &[]),
        Some("your home folder")
    );
    assert_eq!(
        policy.check(&home.join("projects").join("node_modules"), None, &[]),
        None
    );
}

#[test]
fn subtree_protection_covers_everything_inside() {
    let ssh = fake_path(&["home", ".ssh"]);
    let policy = policy(vec![location(ssh.clone(), Reach::Subtree, "SSH keys")]);

    assert_eq!(policy.check(&ssh, None, &[]), Some("SSH keys"));
    assert_eq!(policy.check(&ssh.join("keys"), None, &[]), Some("SSH keys"));
    assert_eq!(
        policy.check(&fake_path(&["home", ".sshx"]), None, &[]),
        None
    );
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn protection_ignores_case_where_the_os_does() {
    let policy = policy(vec![location(
        fake_path(&["home", ".ssh"]),
        Reach::Subtree,
        "SSH keys",
    )]);

    assert_eq!(
        policy.check(&fake_path(&["HOME", ".SSH", "keys"]), None, &[]),
        Some("SSH keys")
    );
}

#[test]
fn protection_follows_file_identity_through_other_spellings() {
    let protected = fake_path(&["home", ".ssh"]);
    let probe = FakeProbe::default().with_file(
        protected.clone(),
        FileFacts {
            identity: identity(7),
            kind: EntryKind::Dir,
            links: 1,
            allocated: 0,
            cloud_synced: false,
            offline: false,
        },
    );
    let policy = ProtectedPolicy::new(
        vec![location(protected, Reach::Subtree, "SSH keys")],
        &probe,
    );
    // Another spelling of the same folder, such as an 8.3 short name.
    let alias = fake_path(&["HOME~1", "SSH~1"]);

    assert_eq!(
        policy.check(&alias, Some(identity(7)), &[]),
        Some("SSH keys")
    );
    assert_eq!(
        policy.check(&alias.join("keys"), Some(identity(8)), &[identity(7)]),
        Some("SSH keys")
    );
    assert_eq!(
        policy.check(&fake_path(&["other"]), Some(identity(9)), &[identity(10)]),
        None
    );
}

#[test]
fn drive_roots_bundles_and_drive_system_folders_are_protected() {
    let policy = policy(Vec::new());
    let root = PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" });

    assert_eq!(
        policy.check(&root, None, &[]),
        Some("a drive's root folder")
    );
    assert_eq!(
        policy.check(&fake_path(&["Xcode.app", "Contents", "build"]), None, &[]),
        Some("the inside of an app bundle")
    );
    assert_eq!(
        policy.check(&fake_path(&["$Recycle.Bin", "x"]), None, &[]),
        Some("Windows system files")
    );
    assert_eq!(
        policy.check(&fake_path(&["repo", ".git", "lfs", "tmp"]), None, &[]),
        Some("a Git repository's internal data")
    );
    assert_eq!(
        policy.check(
            &fake_path(&["apps", "Chat", "resources", "app", "node_modules"]),
            None,
            &[]
        ),
        Some("an installed app")
    );
}

#[test]
fn the_system_policy_protects_the_os_and_the_home_folder() {
    let system = System::new().unwrap();
    let folders = system.known_folders();
    let policy = ProtectedPolicy::for_system(folders, &system, &[]);
    let os_file = if cfg!(windows) {
        PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("Temp")
    } else if cfg!(target_os = "macos") {
        PathBuf::from("/System/Library")
    } else {
        PathBuf::from("/usr/lib")
    };

    assert!(policy.check(&os_file, None, &[]).is_some());
    assert!(
        policy
            .check(folders.home.as_ref().unwrap(), None, &[])
            .is_some()
    );
}

#[test]
fn cleanse_protects_its_own_journal() {
    let root = tempfile::tempdir().unwrap();
    let folders = fake_folders(root.path());
    let policy = ProtectedPolicy::for_system(&folders, &FakeProbe::default(), &[]);
    let journal = folders.cleanse_data().unwrap().join("journal");

    assert_eq!(
        policy.check(&journal.join("run.jsonl"), None, &[]),
        Some("Cleanse's journal of what it removed")
    );
}

proptest! {
    #[test]
    fn nothing_that_is_or_contains_a_protected_location_is_allowed(
        protected in prop::collection::vec(prop::collection::vec("[ab]{1,2}", 1..4), 1..4),
        candidate in prop::collection::vec("[ab]{1,2}", 1..5),
        subtree in any::<bool>(),
    ) {
        let reach = if subtree { Reach::Subtree } else { Reach::Exact };
        let as_path = |parts: &Vec<String>| {
            fake_path(&parts.iter().map(String::as_str).collect::<Vec<_>>())
        };
        let policy = policy(
            protected.iter().map(|parts| location(as_path(parts), reach, "x")).collect(),
        );

        let blocked = policy.check(&as_path(&candidate), None, &[]).is_some();

        let contains = protected.iter().any(|parts| parts.starts_with(&candidate));
        let inside = subtree && protected.iter().any(|parts| candidate.starts_with(parts));
        prop_assert_eq!(blocked, contains || inside);
    }
}

/// Folders for a fake home under `root`, laid out like Windows on every OS.
fn fake_folders(root: &Path) -> KnownFolders {
    let home = root.join("home");
    KnownFolders {
        desktop: Some(home.join("Desktop")),
        documents: Some(home.join("Documents")),
        downloads: Some(home.join("Downloads")),
        cache: Some(home.join("AppData").join("Local")),
        config: Some(home.join("AppData").join("Roaming")),
        data_local: Some(home.join("AppData").join("Local")),
        temp: root.join("temp"),
        home: Some(home),
        ..KnownFolders::default()
    }
}

/// Scans `root` with the built-in rules and assesses every match, as an hour from now so files
/// written by the test don't count as recent activity.
fn assess_all(root: &Path, probe: &dyn Probe) -> Vec<Candidate> {
    assess_at(root, probe, SystemTime::now() + Duration::from_hours(1))
}

fn assess_at(root: &Path, probe: &dyn Probe, now: SystemTime) -> Vec<Candidate> {
    let rules = RuleSet::builtin().unwrap();
    let folders = fake_folders(root);
    let matcher = Matcher::new(rules.rules(), &folders, Os::current());
    let (found, _) = matcher.find(root).unwrap();
    let policy = ProtectedPolicy::for_system(&folders, probe, &[]);
    let assessor = Assessor::new(probe, policy, &[root.to_path_buf()], now).unwrap();
    (0u64..)
        .zip(&found)
        .map(|(id, detection)| assessor.assess(CandidateId(id), detection))
        .collect()
}

fn system() -> System {
    System::new().unwrap()
}

fn only(candidates: &[Candidate]) -> &Candidate {
    assert_eq!(candidates.len(), 1, "{candidates:#?}");
    &candidates[0]
}

#[test]
fn a_plain_project_artifact_is_removable() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("home/projects/web");
    write(&project.join("package.json"), "{}");
    write(&project.join("node_modules/left-pad/index.js"), "abcd");

    let candidates = assess_all(root.path(), &system());
    let candidate = only(&candidates);

    assert_eq!(candidate.risk(), Risk::Regenerable, "{candidate:#?}");
    assert!(candidate.concerns().is_empty(), "{candidate:#?}");
    assert!(candidate.preselected());
    assert!(candidate.identity.is_some());
    assert_eq!(candidate.found.size.logical, 4);
    assert_eq!(candidate.found.files, 1);
}

#[test]
fn recent_changes_mean_something_is_using_it() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join("app/package.json"), "{}");
    write(&root.path().join("app/node_modules/x/index.js"), "x");

    let candidates = assess_at(root.path(), &system(), SystemTime::now());

    assert_eq!(
        only(&candidates).concerns(),
        [Concern::RecentlyModified { minutes_ago: 0 }]
    );
    assert_eq!(only(&candidates).risk(), Risk::Protected);
}

#[test]
fn tracked_files_and_possible_secrets_block_ambiguous_folders() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join(".gitignore"), "dist/\n");
    write(&root.path().join("tracked/package.json"), "{}");
    write(&root.path().join("tracked/dist/app.js"), "x");
    write(&root.path().join("secret/package.json"), "{}");
    write(&root.path().join("secret/dist/.env.local"), "TOKEN=x");
    git(root.path(), &["init", "-q"]);
    git(
        root.path(),
        &["add", "--force", "--", "tracked/dist/app.js"],
    );

    let candidates = assess_all(root.path(), &system());

    assert_eq!(candidates.len(), 2, "{candidates:#?}");
    assert_eq!(
        candidates[0].concerns(),
        [Concern::PossibleSecrets {
            example: ".env.local".into()
        }]
    );
    assert_eq!(candidates[1].concerns(), [Concern::TrackedByGit]);
}

#[test]
fn files_that_merely_look_secret_dont_block_unambiguous_matches() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join("app/package.json"), "{}");
    write(&root.path().join("app/node_modules/certs/test.pem"), "x");

    let candidates = assess_all(root.path(), &system());

    assert_eq!(only(&candidates).concerns(), Vec::<Concern>::new());
}

#[test]
fn virtual_machine_disks_are_never_removable() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join("app/package.json"), "{}");
    write(&root.path().join("app/node_modules/vm/disk.vhdx"), "x");

    let candidates = assess_all(root.path(), &system());

    assert_eq!(
        only(&candidates).concerns(),
        [Concern::ProtectedLocation {
            what: "a virtual machine disk (disk.vhdx)".into()
        }]
    );
}

#[test]
fn matches_outside_the_scan_roots_are_blocked() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join("app/package.json"), "{}");
    write(&root.path().join("app/node_modules/x/index.js"), "x");
    let rules = RuleSet::builtin().unwrap();
    let folders = fake_folders(root.path());
    let (found, _) = Matcher::new(rules.rules(), &folders, Os::current())
        .find(root.path())
        .unwrap();
    let probe = system();
    let elsewhere = root.path().join("elsewhere");
    let assessor = Assessor::new(
        &probe,
        ProtectedPolicy::for_system(&folders, &probe, &[]),
        &[elsewhere],
        SystemTime::now() + Duration::from_hours(1),
    )
    .unwrap();

    let candidate = assessor.assess(CandidateId(0), &found[0]);

    assert_eq!(candidate.concerns(), [Concern::OutsideScanRoots]);
}

#[cfg(unix)]
#[test]
fn unreadable_contents_block_removal() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    write(&root.path().join("app/package.json"), "{}");
    let locked = root.path().join("app/node_modules/locked");
    write(&locked.join("index.js"), "x");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    // Root can read anything, so there's nothing to test when running as root.
    let running_as_root = fs::read_dir(&locked).is_ok();

    let candidates = assess_all(root.path(), &system());
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if running_as_root {
        return;
    }

    assert_eq!(only(&candidates).concerns(), [Concern::Incomplete]);
}

/// Real cloud placeholders and swapped-in links can't be staged reliably, so the probe reports
/// them instead.
fn probe_reporting(root: &Path, path: &Path, facts: FileFacts) -> FakeProbe {
    FakeProbe::new(fake_folders(root)).with_file(std::path::absolute(path).unwrap(), facts)
}

fn dir_facts() -> FileFacts {
    FileFacts {
        identity: identity(1),
        kind: EntryKind::Dir,
        links: 1,
        allocated: 0,
        cloud_synced: false,
        offline: false,
    }
}

#[test]
fn cloud_synced_folders_are_protected() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join("app/package.json"), "{}");
    let node_modules = root.path().join("app/node_modules");
    write(&node_modules.join("x/index.js"), "x");
    let probe = probe_reporting(
        root.path(),
        &node_modules,
        FileFacts {
            cloud_synced: true,
            ..dir_facts()
        },
    );

    let candidates = assess_all(root.path(), &probe);

    assert_eq!(
        only(&candidates).concerns(),
        [Concern::CloudSynced {
            provider: "a cloud storage service".into()
        }]
    );
}

#[test]
fn a_folder_replaced_by_a_link_cant_be_verified() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join("app/package.json"), "{}");
    let node_modules = root.path().join("app/node_modules");
    write(&node_modules.join("x/index.js"), "x");
    let probe = probe_reporting(
        root.path(),
        &node_modules,
        FileFacts {
            kind: EntryKind::Link,
            ..dir_facts()
        },
    );

    let candidates = assess_all(root.path(), &probe);

    assert_eq!(only(&candidates).concerns(), [Concern::Unverifiable]);
}

#[cfg(unix)]
#[test]
fn hard_linked_data_is_a_warning_and_isnt_guaranteed() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join("app/package.json"), "{}");
    write(&root.path().join("store/pkg.js"), &"x".repeat(8192));
    let linked = root.path().join("app/node_modules/pkg.js");
    fs::create_dir_all(linked.parent().unwrap()).unwrap();
    fs::hard_link(root.path().join("store/pkg.js"), &linked).unwrap();

    let candidates = assess_all(root.path(), &system());
    let candidate = only(&candidates);

    assert_eq!(candidate.concerns(), [Concern::SharedData]);
    assert_eq!(candidate.risk(), Risk::Regenerable);
    assert_eq!(candidate.found.size.guaranteed, 0);
    assert!(candidate.found.size.on_disk > 0);
}

#[cfg(windows)]
#[test]
fn pnpm_stores_count_as_shared_on_windows() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join("app/package.json"), "{}");
    write(
        &root
            .path()
            .join("app/node_modules/.pnpm/left-pad@1.3.0/node_modules/left-pad/index.js"),
        "abcd",
    );

    let candidates = assess_all(root.path(), &system());
    let candidate = only(&candidates);

    assert_eq!(candidate.concerns(), [Concern::SharedData]);
    assert_eq!(candidate.found.size.guaranteed, 0);
    assert_eq!(candidate.found.size.on_disk, 4);
}

/// A fake home full of things that must never become removable, next to ordinary build output
/// that should. Nothing is removed in M1; this checks that no removable candidate is, or
/// contains, a canary.
#[test]
fn canaries_are_never_removable() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let mut canaries = Vec::new();
    let mut canary = |relative: &str| {
        let path = home.join(relative);
        write(&path, "canary");
        canaries.push(path);
    };

    canary(".ssh/id_ed25519");
    canary("Documents/report.docx");
    // An editor extension and an installed app both ship node_modules beside a package.json.
    canary(".vscode/extensions/ms-python/package.json");
    canary(".vscode/extensions/ms-python/node_modules/x/index.js");
    canary("AppData/Local/Programs/App/resources/app/package.json");
    canary("AppData/Local/Programs/App/resources/app/node_modules/x/index.js");
    // The same layout unpacked somewhere no OS reserves for apps, as Linux apps often are.
    canary("apps/Chat/resources/app/package.json");
    canary("apps/Chat/resources/app/node_modules/x/index.js");
    canary("projects/vm/node_modules/disks/ubuntu.vhdx");
    canary("projects/site/dist/.env");
    canary("projects/committed/dist/app.js");
    write(&home.join("projects/vm/package.json"), "{}");
    write(&home.join("projects/site/package.json"), "{}");
    write(&home.join("projects/committed/package.json"), "{}");
    write(&home.join("projects/.gitignore"), "dist/\n");
    git(&home.join("projects"), &["init", "-q"]);
    git(
        &home.join("projects"),
        &["add", "--force", "--", "committed/dist/app.js"],
    );
    // Ordinary build output, with a link inside it pointing at the Documents canary.
    write(&home.join("projects/web/package.json"), "{}");
    write(
        &home.join("projects/web/node_modules/left-pad/index.js"),
        "abcd",
    );
    link_dir(
        &home.join("Documents"),
        &home.join("projects/web/node_modules/documents-link"),
    );

    let candidates = assess_all(root.path(), &system());

    let removable: Vec<&Candidate> = candidates
        .iter()
        .filter(|candidate| candidate.risk() != Risk::Protected)
        .collect();
    assert_eq!(removable.len(), 1, "{candidates:#?}");
    assert!(removable[0].found.path.ends_with("web/node_modules"));
    // The link is part of the folder, but what it points to isn't measured or included.
    assert_eq!(removable[0].found.size.logical, 4);
    for candidate in &removable {
        for canary in &canaries {
            assert!(
                !canary.starts_with(&candidate.found.path),
                "{} would take {}",
                candidate.found.path.display(),
                canary.display()
            );
        }
    }
    assert_eq!(candidates.len(), 7, "{candidates:#?}");
}

/// The real filesystem, with the running programs a test chooses: `None` when they can't be
/// listed. On a developer's machine Claude Code may well be running.
struct Running {
    system: System,
    running: Option<HashSet<String>>,
}

impl Running {
    fn programs(names: &[&str]) -> Self {
        Self {
            system: system(),
            running: Some(names.iter().map(|name| (*name).to_owned()).collect()),
        }
    }
}

impl Probe for Running {
    fn inspect(&self, path: &Path) -> io::Result<FileFacts> {
        self.system.inspect(path)
    }

    fn known_folders(&self) -> &KnownFolders {
        self.system.known_folders()
    }

    fn is_elevated(&self) -> bool {
        false
    }

    fn running_processes(&self) -> io::Result<HashSet<String>> {
        self.running
            .clone()
            .ok_or_else(|| io::Error::other("can't list processes"))
    }
}

/// A fake home whose `.claude` holds every folder a Claude Code rule matches, plus data that
/// must stay: a transcript, and a plugin that looks like a JavaScript project.
fn claude_home(root: &Path) -> Vec<PathBuf> {
    let rules = RuleSet::builtin().unwrap();
    let home = root.join("home");
    let mut folders = Vec::new();
    for rule in rules
        .rules()
        .iter()
        .filter(|rule| rule.id.0.starts_with("claude-code."))
    {
        let Target::KnownLocation { locations } = &rule.target else {
            panic!("{} isn't a known location", rule.id);
        };
        for location in locations
            .iter()
            .filter(|location| location.os == Os::current())
        {
            let folder = home.join(&location.path);
            write(&folder.join("x"), "x");
            folders.push(folder);
        }
    }
    write(&home.join(".claude/projects/p/session.jsonl"), "{}");
    write(&home.join(".claude/plugins/p/package.json"), "{}");
    write(&home.join(".claude/plugins/p/node_modules/a/index.js"), "x");
    folders
}

#[test]
fn claude_codes_documented_folders_are_removable_and_the_rest_stays_protected() {
    let root = tempfile::tempdir().unwrap();
    let removable = claude_home(root.path());

    let candidates = assess_all(root.path(), &Running::programs(&[]));

    for folder in &removable {
        let candidate = candidates
            .iter()
            .find(|candidate| &candidate.found.path == folder)
            .unwrap_or_else(|| panic!("{} wasn't matched", folder.display()));
        assert_eq!(
            candidate.risk(),
            candidate.found.declared_risk,
            "{candidate:#?}"
        );
    }
    let plugin = candidates
        .iter()
        .find(|candidate| candidate.found.path.ends_with("p/node_modules"))
        .unwrap();
    assert_eq!(plugin.risk(), Risk::Protected);
    assert_eq!(
        plugin.concerns()[0],
        Concern::ProtectedLocation {
            what: "Claude Code's data".into()
        }
    );
    assert_eq!(candidates.len(), removable.len() + 1, "{candidates:#?}");
}

#[test]
fn an_apps_folders_are_kept_while_it_runs() {
    let root = tempfile::tempdir().unwrap();
    claude_home(root.path());

    let running = assess_all(root.path(), &Running::programs(&["claude"]));
    let unknown = assess_all(
        root.path(),
        &Running {
            system: system(),
            running: None,
        },
    );

    let debug = |candidates: &[Candidate]| {
        candidates
            .iter()
            .find(|candidate| candidate.found.path.ends_with(".claude/debug"))
            .unwrap()
            .clone()
    };
    for (candidate, concern) in [
        (
            debug(&running),
            Concern::OwnerRunning {
                app: "Claude Code".into(),
            },
        ),
        (
            debug(&unknown),
            Concern::OwnerUnchecked {
                app: "Claude Code".into(),
            },
        ),
    ] {
        assert_eq!(candidate.risk(), Risk::Protected);
        assert!(candidate.concerns().contains(&concern), "{candidate:#?}");
    }
}

#[test]
fn an_exception_doesnt_lift_another_protection_of_the_same_folder() {
    let claude = fake_path(&["home", ".claude"]);
    let debug = claude.join("debug");
    let mut documented = location(claude.clone(), Reach::Subtree, "Claude Code's data");
    documented.except.push(debug.clone());
    let excluded = location(claude, Reach::Subtree, "a folder excluded in your settings");

    let alone = policy(vec![documented.clone()]);
    let both = policy(vec![documented, excluded]);

    assert_eq!(alone.check(&debug, None, &[]), None);
    assert_eq!(
        both.check(&debug, None, &[]),
        Some("a folder excluded in your settings")
    );
}
