#![allow(
    clippy::disallowed_methods,
    reason = "tests build fixture trees and repositories, using the git command line"
)]
#![cfg_attr(
    target_os = "macos",
    allow(
        dead_code,
        reason = "system cleanup is for Windows and Linux, and so are the tests that scan fixtures"
    )
)]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use cleanse_model::{Candidate, CandidateId, Concern, Risk};
use cleanse_platform::{FileFacts, FileOwner, KnownFolders, Probe, System, SystemFolders};

use super::SystemRoots;
use crate::rules::{Base, Matcher, Os, RuleSet, Target};
use crate::safety::policy::{SYSTEM_ADVISED, SYSTEM_DISPOSABLE};
use crate::safety::{Assessor, ProtectedPolicy};

/// Reports what's on disk, except that everything belongs to the system unless the test says
/// otherwise, and the computer started when the test says. Fixtures can't really belong to root
/// or SYSTEM unless the tests run as root or an administrator, which CI only does on Windows.
struct Owners {
    system: System,
    given: HashMap<PathBuf, Option<FileOwner>>,
    booted_at: SystemTime,
}

impl Owners {
    fn new() -> Self {
        Self {
            system: System::new().unwrap(),
            given: HashMap::new(),
            booted_at: SystemTime::now(),
        }
    }

    fn owned(mut self, path: &Path, owner: Option<FileOwner>) -> Self {
        self.given.insert(path.to_path_buf(), owner);
        self
    }
}

impl Probe for Owners {
    fn inspect(&self, path: &Path) -> io::Result<FileFacts> {
        let mut facts = self.system.inspect(path)?;
        facts.owner = self
            .given
            .get(path)
            .copied()
            .unwrap_or(Some(FileOwner::System));
        Ok(facts)
    }

    fn known_folders(&self) -> &KnownFolders {
        self.system.known_folders()
    }

    fn is_elevated(&self) -> bool {
        true
    }

    fn running_processes(&self) -> io::Result<HashSet<String>> {
        Ok(HashSet::new())
    }

    fn booted_at(&self) -> io::Result<SystemTime> {
        Ok(self.booted_at)
    }
}

/// System folders under `root`, each named after its base.
fn folders(root: &Path) -> SystemFolders {
    SystemFolders {
        system_root: Some(root.join("system_root")),
        program_data: Some(root.join("program_data")),
        var: Some(root.join("var")),
    }
}

fn write(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "x").unwrap();
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

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// Scans the fixture at `root` with `rules`, as system cleanup would, and assesses each match a
/// week from now, after the computer last started.
fn assess(rules: &RuleSet, policy: ProtectedPolicy, root: &Path, probe: &Owners) -> Vec<Candidate> {
    let folders = folders(root);
    let matcher = Matcher::system(rules.rules(), &folders, Os::current());
    let (found, _) = matcher.find(root).unwrap();
    let roots = SystemRoots::verify(rules, &folders, probe);
    let assessor = Assessor::system(
        probe,
        policy,
        roots,
        SystemTime::now() + Duration::from_hours(24 * 8),
    );
    (0u64..)
        .zip(&found)
        .map(|(id, detection)| assessor.assess(CandidateId(id), detection))
        .collect()
}

/// Each candidate's path relative to `root`, `/`-separated, with its blocking concerns.
fn blocked(candidates: &[Candidate], root: &Path) -> HashMap<String, Vec<Concern>> {
    candidates
        .iter()
        .map(|candidate| {
            let relative = candidate.found.path.strip_prefix(root).unwrap();
            let name = relative
                .iter()
                .map(|part| part.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let concerns = candidate
                .concerns()
                .iter()
                .filter(|concern| concern.blocks())
                .cloned()
                .collect();
            (name, concerns)
        })
        .collect()
}

/// A system rule for each item directly in `Temp` in the Windows folder, and in `/var/tmp`.
fn temp_rule() -> RuleSet {
    RuleSet::system_from_sources([(
        "test.toml",
        r#"
[[rule]]
id = "test.system_temp"
scope = "system"
name = "Temp"
category = "temp"
risk = "safe"
explain = "Because."

[rule.match]
kind = "known_location"
entry = "any"
locations = [
    { os = "windows", base = "system_root", path = "Temp/*" },
    { os = "linux", base = "var", path = "tmp/*" },
]
"#,
    )])
    .unwrap()
}

/// Where [`temp_rule`] finds items on this OS, relative to a fixture's root.
#[cfg(any(windows, target_os = "linux"))]
fn temp_folder() -> &'static str {
    if cfg!(windows) {
        "system_root/Temp"
    } else {
        "var/tmp"
    }
}

fn under(root: &Path, relative: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    path.extend(relative.split('/'));
    path
}

/// Without a protected policy, only system cleanup's own gates apply.
fn no_policy(probe: &Owners) -> ProtectedPolicy {
    ProtectedPolicy::new(Vec::new(), probe)
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn only_what_the_system_owns_throughout_can_be_removed() {
    let fixture = tempfile::tempdir().unwrap();
    let root = std::path::absolute(fixture.path()).unwrap();
    let temp = under(&root, temp_folder());
    for file in [
        "ok.tmp",
        "folder/inner.txt",
        "admins.tmp",
        "users.tmp",
        "mine.tmp",
        "unknown.tmp",
        "has-users-file/users.txt",
        "has-link/x",
        "has-hard-link/x",
    ] {
        write(&temp.join(file));
    }
    write(&root.join("elsewhere/original"));
    write(&root.join("elsewhere/second"));
    write(&under(&root, "system_root/System32/drivers/etc/hosts"));
    fs::hard_link(root.join("elsewhere/original"), temp.join("linked.tmp")).unwrap();
    fs::hard_link(
        root.join("elsewhere/second"),
        temp.join("has-hard-link/linked"),
    )
    .unwrap();
    link_dir(&root.join("elsewhere"), &temp.join("has-link/link"));
    link_dir(
        &under(&root, "system_root/System32"),
        &temp.join("System32"),
    );
    // System cleanup reads no Git, so a repository above the folder that tracks an item in it
    // changes nothing.
    git(&root, &["init", "-q"]);
    git(&root, &["add", "--", &format!("{}/ok.tmp", temp_folder())]);
    let probe = Owners::new()
        .owned(&temp.join("admins.tmp"), Some(FileOwner::Administrators))
        .owned(&temp.join("users.tmp"), Some(FileOwner::Other))
        .owned(&temp.join("mine.tmp"), Some(FileOwner::CurrentUser))
        .owned(&temp.join("unknown.tmp"), None)
        .owned(
            &temp.join("has-users-file/users.txt"),
            Some(FileOwner::Other),
        );

    let candidates = assess(&temp_rule(), no_policy(&probe), &root, &probe);

    let item = |name: &str| format!("{}/{name}", temp_folder());
    let expected = HashMap::from([
        (item("ok.tmp"), vec![]),
        (item("folder"), vec![]),
        (item("admins.tmp"), vec![]),
        (item("users.tmp"), vec![Concern::NotSystemOwned]),
        (item("mine.tmp"), vec![Concern::NotSystemOwned]),
        (item("unknown.tmp"), vec![Concern::OwnerUnknown]),
        (
            item("has-users-file"),
            vec![Concern::ContentsNotSystemOwned],
        ),
        (item("has-link"), vec![Concern::LinksInside]),
        (item("has-hard-link"), vec![Concern::HardLinked]),
        (item("linked.tmp"), vec![Concern::HardLinked]),
    ]);
    assert_eq!(blocked(&candidates, &root), expected);
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn nothing_is_removed_from_a_folder_that_isnt_the_systems() {
    let fixture = tempfile::tempdir().unwrap();
    let root = std::path::absolute(fixture.path()).unwrap();
    let temp = under(&root, temp_folder());
    write(&temp.join("ok.tmp"));
    let base = temp.parent().unwrap();

    for (folder, owner) in [
        (&temp, Some(FileOwner::Other)),
        (&temp, Some(FileOwner::CurrentUser)),
        (&temp, None),
        (&base.to_path_buf(), Some(FileOwner::Other)),
    ] {
        let probe = Owners::new().owned(folder, owner);

        let candidates = assess(&temp_rule(), no_policy(&probe), &root, &probe);

        assert_eq!(
            blocked(&candidates, &root),
            HashMap::from([(
                format!("{}/ok.tmp", temp_folder()),
                vec![Concern::OutsideSystemFolders]
            )]),
            "{} owned by {owner:?}",
            folder.display()
        );
    }
}

/// A folder swapped for another after it was checked isn't the folder that was checked.
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn a_system_folder_must_still_be_the_one_checked() {
    let fixture = tempfile::tempdir().unwrap();
    let root = std::path::absolute(fixture.path()).unwrap();
    let temp = under(&root, temp_folder());
    write(&temp.join("ok.tmp"));
    let rules = temp_rule();
    let folders = folders(&root);
    let probe = Owners::new();
    let roots = SystemRoots::verify(&rules, &folders, &probe);
    assert!(roots.holds(&temp.join("ok.tmp"), &probe));

    fs::rename(&temp, root.join("moved")).unwrap();
    write(&temp.join("ok.tmp"));

    assert!(!roots.holds(&temp.join("ok.tmp"), &probe));
    assert!(!roots.holds(&temp.join("ok.tmp/deeper"), &probe));
}

#[test]
fn system_exceptions_are_exactly_the_system_rules_locations() {
    let rules = RuleSet::builtin_system().unwrap();
    let listed = |exceptions: &[(Base, &[&str])]| -> HashSet<(Base, String)> {
        exceptions
            .iter()
            .flat_map(|(base, paths)| paths.iter().map(|path| (*base, (*path).to_owned())))
            .collect()
    };
    let located = |removable: bool| -> HashSet<(Base, String)> {
        rules
            .rules()
            .iter()
            .filter(|rule| (rule.risk != Risk::Protected) == removable)
            .flat_map(|rule| {
                let Target::KnownLocation { locations, .. } = &rule.target else {
                    panic!("{} isn't a known location", rule.id);
                };
                locations
                    .iter()
                    .map(|location| (location.base, location.path.clone()))
            })
            .collect()
    };

    assert_eq!(listed(SYSTEM_DISPOSABLE), located(true));
    assert_eq!(listed(SYSTEM_ADVISED), located(false));
    for rule in rules.rules() {
        assert_eq!(
            rule.risk == Risk::Protected,
            rule.advise.is_some(),
            "{}",
            rule.id
        );
    }
}

#[test]
fn the_system_policy_protects_system_folders_but_not_whats_disposable_in_them() {
    let root = if cfg!(windows) { r"C:\fake" } else { "/fake" };
    let root = Path::new(root);
    let policy = ProtectedPolicy::for_system_cleanup(&folders(root), &Owners::new());
    let check = |relative: &str| policy.check(&under(root, relative), None, &[]);

    for removable in [
        "system_root/Temp/setup.log",
        "system_root/Temp/folder/inside",
        "system_root/MEMORY.DMP",
        "system_root/Minidump/103026-12345-01.dmp",
        "system_root/SoftwareDistribution/Download/0a1b2c",
        "program_data/Microsoft/Windows/WER/ReportArchive/AppCrash_x",
        "var/crash/_usr_bin_app.0.crash",
        "var/lib/systemd/coredump/core.app.1000",
        "var/cache/apt/archives",
        "var/log/journal",
    ] {
        assert_eq!(check(removable), None, "{removable}");
    }
    for (protected, what) in [
        ("system_root/Temp", "Windows"),
        ("system_root", "Windows"),
        ("system_root/System32/drivers/etc/hosts", "Windows"),
        ("system_root/Temp/../System32", "Windows"),
        ("system_root/Minidump", "Windows"),
        ("system_root/Minidump/notes.txt", "Windows"),
        ("system_root/SoftwareDistribution/DataStore", "Windows"),
        (
            "program_data/Microsoft/Windows/WER/ReportQueue/AppCrash_x",
            "shared app data",
        ),
        ("program_data/Cleanse", "shared app data"),
        ("var/crash", "system files"),
        ("var/crash/notes.txt", "system files"),
        ("var/lib/dpkg/status", "system files"),
        ("var/cache/apt", "system files"),
    ] {
        assert_eq!(check(protected), Some(what), "{protected}");
    }
}

#[cfg(windows)]
#[test]
fn the_windows_rules_on_a_fixture() {
    let fixture = tempfile::tempdir().unwrap();
    let root = std::path::absolute(fixture.path()).unwrap();
    let delivery = "system_root/ServiceProfiles/NetworkService/AppData/Local/Microsoft/Windows/\
                    DeliveryOptimization/Cache";
    for file in [
        "system_root/Temp/setup.log",
        "system_root/Temp/users.log",
        "system_root/System32/drivers/etc/hosts",
        "system_root/MEMORY.DMP",
        "system_root/Minidump/103026-12345-01.dmp",
        "system_root/SoftwareDistribution/Download/0a1b2c/windows11.0-kb5031455.cab",
        "program_data/Microsoft/Windows/WER/ReportArchive/AppCrash_app.exe_1a2b/Report.wer",
        "program_data/Microsoft/Windows/WER/ReportQueue/AppCrash_app.exe_3c4d/Report.wer",
        &format!("{delivery}/0a1b/content"),
    ] {
        write(&under(&root, file));
    }
    link_dir(
        &under(&root, "system_root/System32"),
        &under(&root, "system_root/Temp/System32"),
    );
    let rules = RuleSet::builtin_system().unwrap();
    let probe = Owners::new()
        .owned(
            &under(&root, "system_root/Temp/users.log"),
            Some(FileOwner::Other),
        )
        .owned(
            &under(&root, &format!("{delivery}/0a1b/content")),
            Some(FileOwner::Other),
        );
    let policy = ProtectedPolicy::for_system_cleanup(&folders(&root), &probe);

    let candidates = assess(&rules, policy, &root, &probe);

    assert_eq!(
        blocked(&candidates, &root),
        HashMap::from([
            ("system_root/Temp/setup.log".to_owned(), vec![]),
            (
                "system_root/Temp/users.log".to_owned(),
                vec![Concern::NotSystemOwned]
            ),
            ("system_root/MEMORY.DMP".to_owned(), vec![]),
            (
                "system_root/Minidump/103026-12345-01.dmp".to_owned(),
                vec![]
            ),
            (
                "system_root/SoftwareDistribution/Download/0a1b2c".to_owned(),
                vec![]
            ),
            (
                "program_data/Microsoft/Windows/WER/ReportArchive/AppCrash_app.exe_1a2b".to_owned(),
                vec![]
            ),
            // Its advice is shown: nothing it matches is removed, so what's inside isn't checked.
            (delivery.to_owned(), vec![]),
        ])
    );
    let advised = candidates
        .iter()
        .find(|candidate| candidate.found.rule.0 == "windows.delivery_optimization")
        .unwrap();
    assert_eq!(advised.risk(), Risk::Protected);
}

#[cfg(target_os = "linux")]
#[test]
fn the_linux_rules_on_a_fixture() {
    let fixture = tempfile::tempdir().unwrap();
    let root = std::path::absolute(fixture.path()).unwrap();
    for file in [
        "var/crash/_usr_bin_app.0.crash",
        "var/crash/_usr_bin_app.1000.crash",
        "var/crash/_usr_bin_app.0.upload",
        "var/lib/systemd/coredump/core.app.0.0123.42.1761800000000000.zst",
        "var/cache/apt/archives/curl_8.5.0_amd64.deb",
        "var/cache/apt/archives/partial/x",
        "var/log/journal/0123/system.journal",
        "var/lib/dpkg/status",
    ] {
        write(&under(&root, file));
    }
    let rules = RuleSet::builtin_system().unwrap();
    let probe = Owners::new()
        .owned(
            &under(&root, "var/crash/_usr_bin_app.1000.crash"),
            Some(FileOwner::Other),
        )
        // apt's downloads in progress belong to its own `_apt` account.
        .owned(
            &under(&root, "var/cache/apt/archives/partial"),
            Some(FileOwner::Other),
        );
    let policy = ProtectedPolicy::for_system_cleanup(&folders(&root), &probe);

    let candidates = assess(&rules, policy, &root, &probe);

    assert_eq!(
        blocked(&candidates, &root),
        HashMap::from([
            ("var/crash/_usr_bin_app.0.crash".to_owned(), vec![]),
            (
                "var/crash/_usr_bin_app.1000.crash".to_owned(),
                vec![Concern::NotSystemOwned]
            ),
            (
                "var/lib/systemd/coredump/core.app.0.0123.42.1761800000000000.zst".to_owned(),
                vec![]
            ),
            ("var/cache/apt/archives".to_owned(), vec![]),
            ("var/log/journal".to_owned(), vec![]),
        ])
    );
}

/// The real folders, read only. On Windows, reading their owners takes an administrator, which
/// CI's Windows runners are.
#[test]
fn the_real_system_folders_check_out() {
    let probe = System::new().unwrap();
    if cfg!(windows) && !probe.is_elevated() {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI's Windows runners run as an administrator"
        );
        eprintln!("skipped: needs an administrator");
        return;
    }
    let folders = SystemFolders::current().unwrap();
    let rules = RuleSet::builtin_system().unwrap();
    let roots = SystemRoots::verify(&rules, &folders, &probe);
    let policy = ProtectedPolicy::for_system_cleanup(&folders, &probe);
    let (fixed, protected): (Vec<PathBuf>, Vec<PathBuf>) = if cfg!(windows) {
        let windows = folders.system_root.clone().unwrap();
        let program_data = folders.program_data.clone().unwrap();
        (
            vec![
                windows.join("Temp"),
                windows.join("Minidump"),
                windows.join(r"SoftwareDistribution\Download"),
                program_data.join(r"Microsoft\Windows\WER\ReportArchive"),
            ],
            vec![
                windows.join("Temp"),
                windows.join("System32"),
                windows.join(r"System32\config\SAM"),
                windows.join(r"Temp\..\System32"),
                program_data.join(r"Microsoft\Windows\WER\ReportQueue"),
            ],
        )
    } else if cfg!(target_os = "linux") {
        (
            vec!["/var/crash".into(), "/var/lib/systemd/coredump".into()],
            vec![
                "/var/crash".into(),
                "/var/lib/dpkg".into(),
                "/var/crash/../lib".into(),
            ],
        )
    } else {
        (Vec::new(), Vec::new())
    };

    assert!(
        fixed
            .iter()
            .all(|folder| !folder.is_dir() || roots.holds(&folder.join("x"), &probe))
    );
    if cfg!(windows) {
        assert!(
            roots.holds(&fixed[0].join("x"), &probe),
            "the Windows temp folder"
        );
    }
    for path in protected {
        assert!(
            policy.check(&path, None, &[]).is_some(),
            "{}",
            path.display()
        );
    }
}
