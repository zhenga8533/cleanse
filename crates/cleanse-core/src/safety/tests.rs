#![allow(
    clippy::disallowed_methods,
    reason = "tests build fixture trees and repositories, using the git command line"
)]

use std::collections::HashSet;
use std::fs::{self, File, FileTimes};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use cleanse_model::{Candidate, CandidateId, Category, Concern, EntryKind, FileIdentity, Risk};
use cleanse_platform::{FakeProbe, FileFacts, FileOwner, KnownFolders, Probe, System};
use proptest::prelude::*;

use super::{Assessor, Protected, ProtectedPolicy, Reach};
use crate::rules::{Base, Entry, Matcher, Os, RuleSet, Target, base_folder};

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

#[test]
fn exceptions_can_name_folders_by_pattern() {
    let browser = fake_path(&["home", "browser"]);
    let mut profiles = location(browser.clone(), Reach::Subtree, "a browser's profiles");
    profiles.except.push("Profile */Cache".to_owned());
    let policy = policy(vec![profiles]);
    let check = |relative: &[&str]| {
        let mut path = browser.clone();
        path.extend(relative);
        policy.check(&path, None, &[])
    };

    assert_eq!(check(&["Profile 2", "Cache"]), None);
    assert_eq!(check(&["Profile 2", "Cache", "data_0"]), None);
    for protected in [
        &["Profile 2"][..],
        &["Profile 2", "History"],
        &["Profiles", "Cache"],
        &["Default", "Cache"],
    ] {
        assert_eq!(
            check(protected),
            Some("a browser's profiles"),
            "{protected:?}"
        );
    }
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
            owner: Some(FileOwner::CurrentUser),
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
    for file in ["pagefile.sys", "HIBERFIL.SYS", "swapfile.sys"] {
        assert_eq!(
            policy.check(&root.join(file), None, &[]),
            Some("Windows system files"),
            "{file}"
        );
    }
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
    assess_with(&RuleSet::builtin().unwrap(), root, probe, now)
}

fn assess_with(rules: &RuleSet, root: &Path, probe: &dyn Probe, now: SystemTime) -> Vec<Candidate> {
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
fn large_files_named_like_secrets_or_mail_are_kept() {
    let root = tempfile::tempdir().unwrap();
    let documents = root.path().join("home/Documents");
    let big = "x".repeat(100_000);
    write(&documents.join("Outlook.pst"), &big);
    write(&documents.join("raw.mov"), &big);
    let folders = fake_folders(root.path());
    let rules = RuleSet::builtin().unwrap();
    let (found, _) = Matcher::new(rules.rules(), &folders, Os::current())
        .with_large_files(64 * 1024)
        .find(root.path())
        .unwrap();
    let probe = system();
    let policy = ProtectedPolicy::for_system(&folders, &probe, &[]);
    let now = SystemTime::now() + Duration::from_hours(1);
    let assessor = Assessor::new(&probe, policy, &[root.path().to_path_buf()], now).unwrap();

    let candidates: Vec<Candidate> = found
        .iter()
        .map(|detection| assessor.assess(CandidateId(0), detection))
        .collect();

    assert_eq!(candidates.len(), 2, "{candidates:#?}");
    assert_eq!(
        candidates[0].concerns(),
        [Concern::PossibleSecrets {
            example: "outlook.pst".into()
        }]
    );
    assert_eq!(candidates[1].risk(), Risk::Review, "{:#?}", candidates[1]);
    assert_eq!(candidates[1].found.category, Category::LargeFiles);
    assert!(candidates[1].found.modified.is_some());
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

/// Matches any file in `Dumps` under the app data folder.
fn dump_rules() -> RuleSet {
    let location =
        |os: &str| format!(r#"{{ os = "{os}", base = "data_local", path = "Dumps/*" }}"#);
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
    RuleSet::from_sources([("test.toml", toml.as_str())]).unwrap()
}

fn dumps(root: &Path) -> PathBuf {
    fake_folders(root).data_local.unwrap().join("Dumps")
}

#[test]
fn a_file_a_rule_recognizes_is_removable_on_its_own() {
    let root = tempfile::tempdir().unwrap();
    let dump = dumps(root.path()).join("crash.dmp");
    write(&dump, "abcd");

    let candidates = assess_with(
        &dump_rules(),
        root.path(),
        &system(),
        SystemTime::now() + Duration::from_hours(1),
    );
    let candidate = only(&candidates);

    assert_eq!(candidate.found.path, dump);
    assert_eq!(candidate.found.kind, EntryKind::File);
    assert_eq!(
        (candidate.found.files, candidate.found.size.logical),
        (1, 4)
    );
    assert_eq!(candidate.risk(), Risk::Safe, "{candidate:#?}");
    assert!(candidate.concerns().is_empty(), "{candidate:#?}");
    assert!(candidate.identity.is_some());
}

#[test]
fn a_file_modified_moments_ago_is_in_use() {
    let root = tempfile::tempdir().unwrap();
    write(&dumps(root.path()).join("crash.dmp"), "abcd");

    let candidates = assess_with(&dump_rules(), root.path(), &system(), SystemTime::now());
    let candidate = only(&candidates);

    assert_eq!(candidate.risk(), Risk::Protected);
    assert!(
        matches!(candidate.concerns(), [Concern::RecentlyModified { .. }]),
        "{candidate:#?}"
    );
}

#[test]
fn a_virtual_machine_disk_is_protected_wherever_a_rule_finds_it() {
    let root = tempfile::tempdir().unwrap();
    write(&dumps(root.path()).join("Ubuntu.vhdx"), "");

    let candidates = assess_with(
        &dump_rules(),
        root.path(),
        &system(),
        SystemTime::now() + Duration::from_hours(1),
    );

    assert_eq!(
        only(&candidates).concerns(),
        [Concern::ProtectedLocation {
            what: "a virtual machine disk (ubuntu.vhdx)".into()
        }]
    );
}

#[test]
fn a_file_with_other_hard_links_frees_nothing_for_certain() {
    let root = tempfile::tempdir().unwrap();
    let dump = dumps(root.path()).join("crash.dmp");
    write(&dump, "abcd");
    let probe = probe_reporting(
        root.path(),
        &dump,
        FileFacts {
            kind: EntryKind::File,
            links: 2,
            ..dir_facts()
        },
    );

    let candidates = assess_with(
        &dump_rules(),
        root.path(),
        &probe,
        SystemTime::now() + Duration::from_hours(1),
    );
    let candidate = only(&candidates);

    assert_eq!(candidate.concerns(), [Concern::SharedData]);
    assert_eq!(candidate.found.size.guaranteed, 0);
    assert_eq!(candidate.found.size.logical, 4);
}

#[test]
fn a_file_replaced_by_a_folder_cant_be_verified() {
    let root = tempfile::tempdir().unwrap();
    let dump = dumps(root.path()).join("crash.dmp");
    write(&dump, "abcd");
    let probe = probe_reporting(root.path(), &dump, dir_facts());

    let candidates = assess_with(
        &dump_rules(),
        root.path(),
        &probe,
        SystemTime::now() + Duration::from_hours(1),
    );
    let candidate = only(&candidates);

    assert_eq!(candidate.risk(), Risk::Protected);
    assert!(
        candidate.concerns().contains(&Concern::Unverifiable),
        "{candidate:#?}"
    );
}

#[test]
fn a_file_git_tracks_is_protected() {
    let root = tempfile::tempdir().unwrap();
    let home = fake_folders(root.path()).home.unwrap();
    let dump = dumps(root.path()).join("crash.dmp");
    write(&dump, "abcd");
    git(&home, &["init", "-q"]);
    git(&home, &["add", "--", "AppData/Local/Dumps/crash.dmp"]);

    let candidates = assess_with(
        &dump_rules(),
        root.path(),
        &system(),
        SystemTime::now() + Duration::from_hours(1),
    );

    assert_eq!(only(&candidates).concerns(), [Concern::TrackedByGit]);
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
        owner: Some(FileOwner::CurrentUser),
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
fn only_what_the_user_or_administrators_own_can_be_removed() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join("app/package.json"), "{}");
    let node_modules = root.path().join("app/node_modules");
    write(&node_modules.join("x/index.js"), "x");
    let owners = [
        (Some(FileOwner::CurrentUser), vec![]),
        (Some(FileOwner::Administrators), vec![]),
        (Some(FileOwner::Unrecorded), vec![]),
        (Some(FileOwner::System), vec![Concern::OwnedByAnotherUser]),
        (Some(FileOwner::Other), vec![Concern::OwnedByAnotherUser]),
        (None, vec![Concern::OwnerUnknown]),
    ];

    for (owner, concerns) in owners {
        let probe = probe_reporting(
            root.path(),
            &node_modules,
            FileFacts {
                owner,
                ..dir_facts()
            },
        );

        let candidates = assess_all(root.path(), &probe);

        assert_eq!(only(&candidates).concerns(), concerns, "{owner:?}");
    }
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
/// listed. On a developer's machine Claude Code may well be running. The computer started when
/// the test says, or when it really did.
struct Running {
    system: System,
    programs: Option<HashSet<String>>,
    booted_at: Option<io::Result<SystemTime>>,
}

impl Running {
    fn programs(names: &[&str]) -> Self {
        Self {
            system: system(),
            programs: Some(names.iter().map(|name| (*name).to_owned()).collect()),
            booted_at: None,
        }
    }

    fn booted(mut self, at: io::Result<SystemTime>) -> Self {
        self.booted_at = Some(at);
        self
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
        self.programs
            .clone()
            .ok_or_else(|| io::Error::other("can't list processes"))
    }

    fn booted_at(&self) -> io::Result<SystemTime> {
        match &self.booted_at {
            Some(Ok(at)) => Ok(*at),
            Some(Err(error)) => Err(io::Error::new(error.kind(), error.to_string())),
            None => self.system.booted_at(),
        }
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
        let Target::KnownLocation { locations, .. } = &rule.target else {
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

/// Makes an item for every location of these rules on this OS, filling in each `*`, and returns
/// their paths.
fn fill_locations(folders: &KnownFolders, ids: &[&str]) -> Vec<PathBuf> {
    let rules = RuleSet::builtin().unwrap();
    let mut items = Vec::new();
    for id in ids {
        let rule = rules.get(&cleanse_model::RuleId((*id).into())).unwrap();
        let Target::KnownLocation { locations, entry } = &rule.target else {
            panic!("{id} isn't a known location");
        };
        for location in locations
            .iter()
            .filter(|location| location.os == Os::current())
        {
            let mut path = base_folder(folders, location.base).unwrap().to_path_buf();
            path.extend(location.path.replace('*', "x").split('/'));
            if *entry == Entry::File {
                write(&path, "x");
            } else {
                write(&path.join("x.log"), "x");
            }
            items.push(path);
        }
    }
    items
}

/// Sets when a file or folder was last modified and opened.
fn set_times(path: &Path, at: SystemTime) {
    let mut options = File::options();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_WRITE_ATTRIBUTES, and FILE_FLAG_BACKUP_SEMANTICS, which opening a folder needs.
        options.access_mode(0x100).custom_flags(0x0200_0000);
    }
    #[cfg(unix)]
    options.read(true);
    let times = FileTimes::new().set_modified(at).set_accessed(at);
    options.open(path).unwrap().set_times(times).unwrap();
}

fn days_ago(days: u64) -> SystemTime {
    SystemTime::now() - Duration::from_hours(24 * days)
}

/// Matches everything directly in the temp folder, kept until unchanged for 7 days and since the
/// computer started.
fn idle_rules() -> RuleSet {
    let location = |os: &str| format!(r#"{{ os = "{os}", base = "temp", path = "*" }}"#);
    let toml = format!(
        r#"
[[rule]]
id = "test.temp"
name = "Temp"
category = "temp"
risk = "safe"
explain = "Because."
idle_days = 7

[rule.match]
kind = "known_location"
entry = "any"
locations = [{}, {}, {}]
"#,
        location("windows"),
        location("macos"),
        location("linux")
    );
    RuleSet::from_sources([("test.toml", toml.as_str())]).unwrap()
}

/// A file and a folder in the temp folder, both last changed `days` ago.
fn temp_items(root: &Path, days: u64) -> [PathBuf; 2] {
    let temp = fake_folders(root).temp;
    let file = temp.join("setup.tmp");
    let folder = temp.join("unpacked");
    write(&file, "x");
    write(&folder.join("app.exe"), "x");
    for path in [&file, &folder.join("app.exe"), &folder] {
        set_times(path, days_ago(days));
    }
    [file, folder]
}

fn assess_idle(root: &Path, probe: &dyn Probe) -> Vec<Candidate> {
    assess_with(&idle_rules(), root, probe, SystemTime::now())
}

#[test]
fn items_unchanged_for_long_enough_and_since_startup_are_removable() {
    let root = tempfile::tempdir().unwrap();
    let items = temp_items(root.path(), 8);

    let candidates = assess_idle(root.path(), &Running::programs(&[]).booted(Ok(days_ago(2))));

    assert_eq!(candidates.len(), 2, "{candidates:#?}");
    for (candidate, item) in candidates.iter().zip(&items) {
        assert_eq!(&candidate.found.path, item);
        assert_eq!(candidate.risk(), Risk::Safe, "{candidate:#?}");
        assert!(candidate.concerns().is_empty(), "{candidate:#?}");
    }
}

#[test]
fn items_changed_too_recently_are_kept() {
    let root = tempfile::tempdir().unwrap();
    temp_items(root.path(), 3);

    // Started since, so only the week is missing.
    let candidates = assess_idle(root.path(), &Running::programs(&[]).booted(Ok(days_ago(1))));

    assert_eq!(candidates.len(), 2, "{candidates:#?}");
    for candidate in &candidates {
        assert_eq!(candidate.risk(), Risk::Protected);
        assert_eq!(
            candidate.concerns(),
            [Concern::ChangedRecently {
                days_ago: 3,
                keep_days: 7
            }]
        );
    }
}

#[test]
fn items_changed_since_the_computer_started_are_kept() {
    let root = tempfile::tempdir().unwrap();
    temp_items(root.path(), 8);

    let candidates = assess_idle(
        root.path(),
        &Running::programs(&[]).booted(Ok(days_ago(10))),
    );

    assert_eq!(candidates.len(), 2, "{candidates:#?}");
    for candidate in &candidates {
        assert_eq!(candidate.concerns(), [Concern::ChangedSinceStartup]);
    }
}

#[test]
fn a_folder_whose_contents_just_changed_is_kept_however_old_its_files() {
    let root = tempfile::tempdir().unwrap();
    let [_, folder] = temp_items(root.path(), 400);
    write(&folder.join("new.dat"), "x");
    set_times(&folder.join("new.dat"), days_ago(400));

    let candidates = assess_idle(root.path(), &Running::programs(&[]).booted(Ok(days_ago(2))));

    let candidate = candidates
        .iter()
        .find(|candidate| candidate.found.path == folder)
        .unwrap();
    assert!(
        candidate.concerns().contains(&Concern::ChangedRecently {
            days_ago: 0,
            keep_days: 7
        }),
        "{candidate:#?}"
    );
    assert!(candidate.concerns().contains(&Concern::ChangedSinceStartup));
}

#[test]
fn items_are_kept_when_startup_cant_be_read() {
    let root = tempfile::tempdir().unwrap();
    temp_items(root.path(), 8);

    let candidates = assess_idle(
        root.path(),
        &Running::programs(&[]).booted(Err(io::Error::other("unknown"))),
    );

    assert_eq!(candidates.len(), 2, "{candidates:#?}");
    for candidate in &candidates {
        assert_eq!(candidate.concerns(), [Concern::StartupUnchecked]);
    }
}

/// Windows only: elsewhere the temp folder has no rule yet.
#[test]
fn windows_temp_items_are_safe_once_idle() {
    if !cfg!(windows) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let items = temp_items(root.path(), 8);

    let candidates = assess_all(root.path(), &Running::programs(&[]).booted(Ok(days_ago(2))));

    assert_eq!(candidates.len(), 2, "{candidates:#?}");
    for (candidate, item) in candidates.iter().zip(&items) {
        assert_eq!(&candidate.found.path, item);
        assert_eq!(candidate.found.rule.0, "windows.temp");
        assert_eq!(candidate.risk(), Risk::Safe, "{candidate:#?}");
        assert!(candidate.concerns().is_empty(), "{candidate:#?}");
        assert!(candidate.preselected());
    }
}

#[test]
fn developer_tool_logs_are_safe_items_nothing_protects() {
    let root = tempfile::tempdir().unwrap();
    let logs = fill_locations(
        &fake_folders(root.path()),
        &[
            "npm.logs",
            "vscode.logs",
            "jetbrains.logs",
            "gradle.daemon_logs",
        ],
    );

    let candidates = assess_all(root.path(), &Running::programs(&[]));

    assert_eq!(candidates.len(), logs.len(), "{candidates:#?}");
    for log in &logs {
        let candidate = candidates
            .iter()
            .find(|candidate| &candidate.found.path == log)
            .unwrap_or_else(|| panic!("{} wasn't matched", log.display()));
        assert_eq!(candidate.risk(), Risk::Safe, "{candidate:#?}");
        assert!(candidate.concerns().is_empty(), "{candidate:#?}");
    }

    let candidates = assess_all(root.path(), &Running::programs(&["code"]));
    let vscode = candidates
        .iter()
        .find(|candidate| candidate.found.rule.0 == "vscode.logs")
        .unwrap();
    assert_eq!(
        vscode.concerns(),
        [Concern::OwnerRunning {
            app: "VS Code".into()
        }]
    );
}

#[test]
fn crash_dumps_are_review_items_nothing_protects() {
    let root = tempfile::tempdir().unwrap();
    let folders = fake_folders(root.path());
    let dump = if cfg!(windows) {
        folders
            .data_local
            .unwrap()
            .join("CrashDumps/app.exe.1234.dmp")
    } else if cfg!(target_os = "macos") {
        let reports = folders.home.unwrap().join("Library/Logs/DiagnosticReports");
        reports.join("App-2026-10-30-101500.ips")
    } else {
        // Linux keeps crashes only in system folders.
        return;
    };
    write(&dump, "abcd");

    let candidates = assess_all(root.path(), &system());
    let candidate = only(&candidates);

    assert_eq!(candidate.found.path, dump);
    assert_eq!(candidate.found.kind, EntryKind::File);
    assert_eq!(candidate.risk(), Risk::Review, "{candidate:#?}");
    assert!(candidate.concerns().is_empty(), "{candidate:#?}");
    assert!(!candidate.preselected());
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
            programs: None,
            booted_at: None,
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

/// A fake home with a folder at every location a browser cache rule matches on this OS, with
/// `*` read as `1`, and beside each, in its profile, an extension that looks like a JavaScript
/// project.
fn browser_home(root: &Path) -> Vec<PathBuf> {
    let rules = RuleSet::builtin().unwrap();
    let folders = fake_folders(root);
    let mut caches = Vec::new();
    for rule in rules
        .rules()
        .iter()
        .filter(|rule| rule.category == Category::BrowserCaches)
    {
        let Target::KnownLocation { locations, .. } = &rule.target else {
            panic!("{} isn't a known location", rule.id);
        };
        for location in locations
            .iter()
            .filter(|location| location.os == Os::current())
        {
            let base = match location.base {
                Base::Home => &folders.home,
                Base::Cache => &folders.cache,
                Base::Config => &folders.config,
                Base::DataLocal => &folders.data_local,
                Base::Temp | Base::Downloads => {
                    panic!("{} keeps a cache in a folder everyone shares", rule.id)
                }
                Base::SystemRoot | Base::ProgramData | Base::Var => {
                    panic!("{} keeps a cache in a system folder", rule.id)
                }
            };
            let cache = base.as_ref().unwrap().join(location.path.replace('*', "1"));
            write(&cache.join("x"), "x");
            let profile = cache.parent().unwrap();
            write(&profile.join("Extensions/e/package.json"), "{}");
            write(&profile.join("Extensions/e/node_modules/a/index.js"), "x");
            caches.push(cache);
        }
    }
    caches
}

#[test]
fn browser_caches_are_removable_and_the_rest_of_each_profile_stays_protected() {
    let root = tempfile::tempdir().unwrap();
    let caches = browser_home(root.path());

    let candidates = assess_all(root.path(), &Running::programs(&[]));

    for cache in &caches {
        let candidate = candidates
            .iter()
            .find(|candidate| &candidate.found.path == cache)
            .unwrap_or_else(|| panic!("{} wasn't matched", cache.display()));
        assert_eq!(
            candidate.risk(),
            candidate.found.declared_risk,
            "{candidate:#?}"
        );
    }
    let extensions: Vec<&Candidate> = candidates
        .iter()
        .filter(|candidate| !caches.contains(&candidate.found.path))
        .collect();
    assert!(!extensions.is_empty());
    for extension in extensions {
        assert!(
            extension.found.path.ends_with("e/node_modules"),
            "{extension:#?}"
        );
        assert_eq!(extension.risk(), Risk::Protected);
        let Concern::ProtectedLocation { what } = &extension.concerns()[0] else {
            panic!("{extension:#?}");
        };
        assert!(what.ends_with("'s profiles"), "{what}");
    }
}

#[test]
fn an_exception_doesnt_lift_another_protection_of_the_same_folder() {
    let claude = fake_path(&["home", ".claude"]);
    let debug = claude.join("debug");
    let mut documented = location(claude.clone(), Reach::Subtree, "Claude Code's data");
    documented.except.push("debug".to_owned());
    let excluded = location(claude, Reach::Subtree, "a folder excluded in your settings");

    let alone = policy(vec![documented.clone()]);
    let both = policy(vec![documented, excluded]);

    assert_eq!(alone.check(&debug, None, &[]), None);
    assert_eq!(
        both.check(&debug, None, &[]),
        Some("a folder excluded in your settings")
    );
}
