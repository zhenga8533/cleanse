#![allow(clippy::disallowed_methods, reason = "tests build fixture files")]

use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime};

use cleanse_model::{EntryKind, FileIdentity};

use crate::{
    FakeProbe, FileFacts, FileOwner, KnownFolders, Probe, System, SystemFolders, process_name,
};

#[cfg(unix)]
fn link_dir(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

/// Junctions need no special privileges, unlike directory symlinks on Windows.
#[cfg(windows)]
fn link_dir(target: &Path, link: &Path) {
    junction::create(target, link).unwrap();
}

fn system() -> System {
    System::new().unwrap()
}

#[test]
fn inspects_a_plain_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.bin");
    // Large enough to be stored outside the NTFS file record, so it has real allocation.
    fs::write(&path, vec![1u8; 64 * 1024]).unwrap();

    let facts = system().inspect(&path).unwrap();

    assert_eq!(facts.kind, EntryKind::File);
    assert_eq!(facts.links, 1);
    assert!(
        facts.allocated >= 64 * 1024,
        "allocated {}",
        facts.allocated
    );
    assert!(!facts.cloud_synced);
    assert!(!facts.offline);
}

#[test]
fn identity_is_stable_and_distinguishes_files() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    fs::write(&a, "a").unwrap();
    fs::write(&b, "b").unwrap();
    let probe = system();

    let first = probe.inspect(&a).unwrap().identity;

    assert_eq!(probe.inspect(&a).unwrap().identity, first);
    assert_ne!(probe.inspect(&b).unwrap().identity, first);
}

#[test]
fn hard_links_share_an_identity_and_are_counted() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original");
    let link = dir.path().join("link");
    fs::write(&original, "shared").unwrap();
    fs::hard_link(&original, &link).unwrap();
    let probe = system();

    let original = probe.inspect(&original).unwrap();
    let link = probe.inspect(&link).unwrap();

    assert_eq!(original.identity, link.identity);
    assert_eq!(original.links, 2);
    assert_eq!(link.links, 2);
}

#[test]
fn inspects_directories() {
    let dir = tempfile::tempdir().unwrap();

    assert_eq!(system().inspect(dir.path()).unwrap().kind, EntryKind::Dir);
}

#[test]
fn describes_a_directory_link_rather_than_its_target() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    let link = dir.path().join("link");
    fs::create_dir(&target).unwrap();
    link_dir(&target, &link);
    let probe = system();

    let link = probe.inspect(&link).unwrap();

    assert_eq!(link.kind, EntryKind::Link);
    assert_ne!(link.identity, probe.inspect(&target).unwrap().identity);
}

#[test]
fn inspects_paths_longer_than_the_windows_max_path() {
    let dir = tempfile::tempdir().unwrap();
    let mut deep = dir.path().to_path_buf();
    for _ in 0..12 {
        deep.push("a-directory-name-thirty-chars-");
    }
    fs::create_dir_all(&deep).unwrap();
    let file = deep.join("bottom.txt");
    fs::write(&file, "x").unwrap();

    assert_eq!(system().inspect(&file).unwrap().kind, EntryKind::File);
}

// Tests can't create cloud placeholders, so these two run against real files from OneDrive (or
// another Cloud Files provider) when given them:
//
//     CLEANSE_TEST_ONLINE_ONLY_FILE=<path> CLEANSE_TEST_DOWNLOADED_FILE=<path> \
//         cargo test -p cleanse-platform -- --ignored

#[cfg(windows)]
#[test]
#[ignore = "needs CLEANSE_TEST_ONLINE_ONLY_FILE pointing at an online-only cloud file"]
fn recognizes_an_online_only_cloud_file_without_downloading_it() {
    let path = std::env::var_os("CLEANSE_TEST_ONLINE_ONLY_FILE")
        .expect("set CLEANSE_TEST_ONLINE_ONLY_FILE to an online-only cloud file");
    let probe = system();

    let facts = probe.inspect(Path::new(&path)).unwrap();

    assert!(facts.cloud_synced);
    assert!(facts.offline);
    assert_eq!(facts.allocated, 0);
    assert!(
        probe.inspect(Path::new(&path)).unwrap().offline,
        "inspecting must not download the file"
    );
}

#[cfg(windows)]
#[test]
#[ignore = "needs CLEANSE_TEST_DOWNLOADED_FILE pointing at a downloaded cloud file"]
fn recognizes_a_downloaded_cloud_file_as_synced() {
    let path = std::env::var_os("CLEANSE_TEST_DOWNLOADED_FILE")
        .expect("set CLEANSE_TEST_DOWNLOADED_FILE to a downloaded cloud file");

    let facts = system().inspect(Path::new(&path)).unwrap();

    assert!(facts.cloud_synced);
    assert!(!facts.offline);
}

#[test]
fn files_this_process_makes_belong_to_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mine");
    fs::write(&path, "x").unwrap();
    let probe = system();

    let owner = probe.inspect(&path).unwrap().owner;

    // An administrator's elevated programs make files the Administrators group owns, unless a
    // security policy gives them to the administrator instead.
    if cfg!(windows) && probe.is_elevated() {
        assert!(
            matches!(
                owner,
                Some(FileOwner::Administrators | FileOwner::CurrentUser)
            ),
            "{owner:?}"
        );
    } else {
        assert_eq!(owner, Some(FileOwner::CurrentUser));
    }
}

/// `NT SERVICE\TrustedInstaller` owns the Windows folder, and root owns `/`.
#[test]
fn the_operating_system_owns_its_own_folders() {
    let folder = if cfg!(windows) {
        std::env::var_os("SystemRoot").unwrap().into()
    } else {
        std::path::PathBuf::from("/")
    };

    assert_eq!(
        system().inspect(&folder).unwrap().owner,
        Some(FileOwner::System)
    );
}

/// Giving a file to another account, or to Administrators or root, takes an administrator or root.
/// CI's Windows runners are administrators, and CI runs the tests named `when_elevated` as root
/// on Linux, in a job of its own.
fn elevated_or_skip(probe: &System) -> bool {
    if probe.is_elevated() {
        return true;
    }
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
    false
}

#[test]
fn a_folder_anyone_made_isnt_private() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("folder");
    fs::create_dir(&folder).unwrap();

    assert!(!system().is_private_dir(&folder).unwrap());
    assert!(
        system()
            .is_private_dir(&dir.path().join("missing"))
            .is_err()
    );
}

#[test]
fn private_folders_when_elevated() {
    let probe = system();
    if !elevated_or_skip(&probe) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let private = dir.path().join("private");

    crate::privileged::create_private_dir(&private).unwrap();

    assert!(probe.is_private_dir(&private).unwrap());
    let again = crate::privileged::create_private_dir(&private).unwrap_err();
    assert_eq!(again.kind(), io::ErrorKind::AlreadyExists);
    // A folder made the usual way inherits permissions that let others in.
    let ordinary = dir.path().join("ordinary");
    fs::create_dir(&ordinary).unwrap();
    assert!(!probe.is_private_dir(&ordinary).unwrap());
    let link = dir.path().join("link");
    link_dir(&private, &link);
    assert!(!probe.is_private_dir(&link).unwrap());
    let given = dir.path().join("given");
    crate::privileged::create_private_dir(&given).unwrap();
    give_away(&given);
    assert!(!probe.is_private_dir(&given).unwrap());
}

/// Gives `path` to the Users group.
#[cfg(windows)]
fn give_away(path: &Path) {
    icacls(path, &["/setowner", "*S-1-5-32-545"]);
}

/// Gives `path` to `nobody`.
#[cfg(unix)]
fn give_away(path: &Path) {
    std::os::unix::fs::chown(path, Some(65534), None).unwrap();
}

/// Someone who can change a folder above a private one can move it away and put another there.
#[cfg(unix)]
#[test]
fn a_private_folder_inside_one_others_can_change_isnt_private_when_elevated() {
    use std::os::unix::fs::PermissionsExt;

    let probe = system();
    if !elevated_or_skip(&probe) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let exposed = dir.path().join("exposed");
    fs::create_dir(&exposed).unwrap();
    fs::set_permissions(&exposed, fs::Permissions::from_mode(0o777)).unwrap();
    let private = exposed.join("private");

    crate::privileged::create_private_dir(&private).unwrap();

    assert!(!probe.is_private_dir(&private).unwrap());
}

#[cfg(windows)]
fn icacls(path: &Path, args: &[&str]) {
    let status = std::process::Command::new("icacls")
        .arg(path)
        .args(args)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "icacls {args:?}");
}

#[cfg(windows)]
#[test]
fn reads_owners_an_administrator_set() {
    let probe = system();
    if !elevated_or_skip(&probe) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // SYSTEM, LOCAL SERVICE, TrustedInstaller, and the Users group.
    let owners = [
        ("S-1-5-18", FileOwner::System),
        ("S-1-5-19", FileOwner::System),
        (
            "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
            FileOwner::System,
        ),
        ("S-1-5-32-545", FileOwner::Other),
    ];

    for (sid, owner) in owners {
        let path = dir.path().join(sid);
        fs::write(&path, "x").unwrap();
        icacls(&path, &["/setowner", &format!("*{sid}")]);

        assert_eq!(probe.inspect(&path).unwrap().owner, Some(owner), "{sid}");
    }
}

#[cfg(windows)]
#[test]
fn an_owner_that_cant_be_read_is_unknown() {
    let probe = system();
    if !elevated_or_skip(&probe) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("private");
    fs::write(&path, "x").unwrap();
    // Owners may always read their own file's permissions, so the file goes to SYSTEM first, and
    // then everyone else is denied reading them.
    icacls(&path, &["/setowner", "*S-1-5-18"]);
    icacls(&path, &["/deny", "*S-1-1-0:(RC)"]);

    let facts = probe.inspect(&path).unwrap();

    assert_eq!(facts.kind, EntryKind::File);
    // Git Bash turns on the backup privilege, which lets an administrator read any file's
    // permissions, and processes started from it inherit that.
    let expected = if backup_privilege_on() {
        Some(FileOwner::System)
    } else {
        None
    };
    assert_eq!(facts.owner, expected);
}

#[cfg(windows)]
fn backup_privilege_on() -> bool {
    let output = std::process::Command::new("whoami")
        .arg("/priv")
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).lines().any(|line| {
        let mut words = line.split_whitespace();
        words.next() == Some("SeBackupPrivilege") && words.last() == Some("Enabled")
    })
}

#[test]
fn finds_the_system_folders() {
    let folders = SystemFolders::current().unwrap();

    if cfg!(windows) {
        let system_root = folders.system_root.unwrap();
        assert!(system_root.join("System32").is_dir(), "{system_root:?}");
        assert!(folders.program_data.unwrap().is_dir());
        assert_eq!(folders.var, None);
    } else if cfg!(target_os = "linux") {
        assert_eq!(folders.var.as_deref(), Some(Path::new("/var")));
        assert_eq!((folders.system_root, folders.program_data), (None, None));
    } else {
        assert_eq!(folders, SystemFolders::default());
    }
}

/// A user can set these for the elevated programs they start, through `HKCU\Environment`.
#[cfg(windows)]
const SYSTEM_FOLDER_VARIABLES: [&str; 5] = [
    "SystemRoot",
    "windir",
    "SystemDrive",
    "ProgramData",
    "ALLUSERSPROFILE",
];

#[cfg(windows)]
#[test]
fn system_folders_dont_come_from_the_environment() {
    let fake = std::env::temp_dir().join("cleanse-fake-windows");
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tests::print_system_folders",
            "--ignored",
            "--nocapture",
        ])
        .envs(SYSTEM_FOLDER_VARIABLES.map(|name| (name, &fake)))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "{stdout}");
    let expected = format!("{:?}", SystemFolders::current().unwrap());
    assert!(stdout.contains(&expected), "{expected} isn't in {stdout}");
}

#[cfg(windows)]
#[test]
#[ignore = "system_folders_dont_come_from_the_environment runs this with the environment changed"]
fn print_system_folders() {
    for name in SYSTEM_FOLDER_VARIABLES {
        assert!(
            std::env::var_os(name).is_some_and(|value| value.to_string_lossy().contains("fake")),
            "{name} isn't changed"
        );
    }
    println!("{:?}", SystemFolders::current().unwrap());
}

#[test]
fn missing_paths_are_not_found() {
    let dir = tempfile::tempdir().unwrap();

    let error = system().inspect(&dir.path().join("missing")).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::NotFound);
}

#[test]
fn finds_the_home_and_temp_folders() {
    let folders = KnownFolders::current();

    assert!(folders.home.as_deref().is_some_and(Path::is_dir));
    assert!(folders.temp.is_dir());
}

/// GitHub's Windows runners set `%TEMP%` with short names, like `C:\Users\RUNNER~1\...`.
#[test]
fn the_temp_folder_is_given_by_its_long_names() {
    let temp = KnownFolders::current().temp;

    assert_eq!(
        system().inspect(&temp).unwrap().identity,
        system().inspect(&std::env::temp_dir()).unwrap().identity
    );
    if cfg!(windows) {
        let short = temp.iter().find(|name| {
            let name = name.to_string_lossy();
            name.find('~')
                .is_some_and(|tilde| name[tilde + 1..].starts_with(|c: char| c.is_ascii_digit()))
        });
        assert_eq!(short, None, "{}", temp.display());
    }
}

#[test]
fn knows_when_the_computer_started() {
    let booted_at = system().booted_at().unwrap();

    assert!(booted_at <= SystemTime::now());
    assert!(booted_at > SystemTime::UNIX_EPOCH + Duration::from_hours(24 * 365 * 30));
}

#[test]
fn the_fake_answers_only_what_it_was_told() {
    let facts = FileFacts {
        identity: FileIdentity { volume: 1, file: 2 },
        kind: EntryKind::File,
        links: 1,
        allocated: 0,
        cloud_synced: true,
        offline: true,
        owner: Some(FileOwner::Other),
    };
    let folders = KnownFolders {
        temp: "/fake/temp".into(),
        ..KnownFolders::default()
    };
    let probe = FakeProbe::new(folders.clone())
        .with_file("/fake/onedrive/report.docx", facts)
        .elevated(true);

    assert_eq!(
        probe
            .inspect(Path::new("/fake/onedrive/report.docx"))
            .unwrap(),
        facts
    );
    assert_eq!(
        probe.inspect(Path::new("/elsewhere")).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
    assert_eq!(probe.known_folders(), &folders);
    assert!(probe.is_elevated());
    assert!(probe.booted_at().is_err());
    let started = SystemTime::UNIX_EPOCH + Duration::from_secs(7);
    assert_eq!(probe.with_boot_time(started).booted_at().unwrap(), started);
}

#[test]
fn process_names_ignore_case_and_the_exe_extension() {
    assert_eq!(process_name("Claude.exe"), "claude");
    assert_eq!(process_name("Claude"), "claude");
    assert_eq!(process_name("claude"), "claude");
    assert_eq!(process_name("setup.exe.old"), "setup.exe.old");
}

#[test]
fn lists_running_programs_including_this_one() {
    let this = std::env::current_exe().unwrap();
    let name = this.file_name().unwrap().to_string_lossy();

    let running = system().running_processes().unwrap();

    // Linux truncates process names to 15 bytes.
    let name = process_name(&name);
    assert!(
        running
            .iter()
            .any(|process| name.starts_with(process.as_str())),
        "{name} isn't among {running:?}"
    );
}

#[test]
fn the_fake_reports_only_the_programs_it_was_told_are_running() {
    let probe = FakeProbe::default().with_running("Claude.exe");

    let running = probe.running_processes().unwrap();

    assert_eq!(running.into_iter().collect::<Vec<_>>(), ["claude"]);
}
