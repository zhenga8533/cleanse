use std::path::{Path, PathBuf};

use cleanse_model::{EntryKind, FileIdentity};
use cleanse_platform::{FakeProbe, FileFacts, FileOwner};

use super::{Unavailable, beside, installed_at};

fn facts(kind: EntryKind) -> FileFacts {
    FileFacts {
        identity: FileIdentity { volume: 1, file: 1 },
        kind,
        links: 1,
        allocated: 0,
        cloud_synced: false,
        offline: false,
        owner: Some(FileOwner::Administrators),
    }
}

/// A fake system with `files` and `folders`, of which only administrators or root can change
/// those in `protected`.
fn probe(folders: &[&str], files: &[&str], protected: &[&str]) -> FakeProbe {
    let mut probe = FakeProbe::default();
    for folder in folders {
        probe = probe.with_file(*folder, facts(EntryKind::Dir));
    }
    for file in files {
        probe = probe.with_file(*file, facts(EntryKind::File));
    }
    for path in protected {
        probe = probe.admins_only(*path);
    }
    probe
}

const PROGRAMS: &str = "/programs";
const FOLDER: &str = "/programs/Cleanse";
const APP: &str = "/programs/Cleanse/cleanse-desktop.exe";
const HELPER: &str = "/programs/Cleanse/cleanse-elevate.exe";

/// As the MSI installs it on Windows: beside the app, in Program Files.
fn installed_on_windows(protected: &[&str]) -> FakeProbe {
    probe(&[PROGRAMS, FOLDER], &[APP, HELPER], protected)
}

#[test]
fn finds_the_helper_beside_the_app_in_program_files() {
    let probe = installed_on_windows(&[PROGRAMS, FOLDER, HELPER]);

    let installed = beside(&probe, Path::new(PROGRAMS), Path::new(APP)).unwrap();

    assert_eq!(installed.program(), Path::new(HELPER));
}

/// Windows paths ignore case, and the app's path may differ from the registry's in it.
#[test]
fn program_files_matches_in_any_case() {
    let probe = installed_on_windows(&[PROGRAMS, FOLDER, HELPER]);

    assert!(beside(&probe, Path::new("/PROGRAMS"), Path::new(APP)).is_ok());
}

#[test]
fn an_app_outside_program_files_has_no_helper() {
    let probe = probe(
        &["/home", "/home/Cleanse"],
        &[
            "/home/Cleanse/cleanse-desktop.exe",
            "/home/Cleanse/cleanse-elevate.exe",
        ],
        &[
            "/home",
            "/home/Cleanse",
            "/home/Cleanse/cleanse-elevate.exe",
        ],
    );

    let error = beside(
        &probe,
        Path::new(PROGRAMS),
        Path::new("/home/Cleanse/cleanse-desktop.exe"),
    )
    .unwrap_err();

    assert!(matches!(error, Unavailable::NotInstalled), "{error:?}");
}

#[test]
fn an_app_installed_without_the_helper_has_none() {
    let probe = probe(&[PROGRAMS, FOLDER], &[APP], &[PROGRAMS, FOLDER]);

    let error = beside(&probe, Path::new(PROGRAMS), Path::new(APP)).unwrap_err();

    assert!(matches!(error, Unavailable::NotInstalled), "{error:?}");
}

/// Whoever can change any of them could put a program of their own where the helper is.
#[test]
fn the_helper_and_every_folder_down_to_it_must_be_only_admins() {
    for (unprotected, protected) in [
        (PROGRAMS, [FOLDER, HELPER]),
        (FOLDER, [PROGRAMS, HELPER]),
        (HELPER, [PROGRAMS, FOLDER]),
    ] {
        let probe = installed_on_windows(&protected);

        let error = beside(&probe, Path::new(PROGRAMS), Path::new(APP)).unwrap_err();

        assert!(
            matches!(&error, Unavailable::Unprotected(path) if path == Path::new(unprotected)),
            "{error:?}"
        );
    }
}

#[test]
fn parent_folder_names_dont_count_as_inside() {
    let probe = installed_on_windows(&[PROGRAMS, FOLDER, HELPER]);

    let error = beside(
        &probe,
        Path::new(PROGRAMS),
        Path::new("/programs/../home/cleanse-desktop.exe"),
    )
    .unwrap_err();

    assert!(matches!(error, Unavailable::NotInstalled), "{error:?}");
}

const PROGRAM: &str = "/usr/lib/cleanse/cleanse-elevate";
const POLICY: &str = "/usr/share/polkit-1/actions/io.github.zhenga8533.cleanse.policy";
const PKEXEC: &str = "/usr/bin/pkexec";

/// Everything from the root down to the helper and the policy, as the deb installs them on
/// Linux.
const LINUX_PATHS: [&str; 9] = [
    "/",
    "/usr",
    "/usr/lib",
    "/usr/lib/cleanse",
    PROGRAM,
    "/usr/share",
    "/usr/share/polkit-1",
    "/usr/share/polkit-1/actions",
    POLICY,
];

fn installed_on_linux(except: &str) -> FakeProbe {
    let folders: Vec<&str> = LINUX_PATHS
        .into_iter()
        .filter(|path| ![PROGRAM, POLICY].contains(path))
        .collect();
    let protected: Vec<&str> = LINUX_PATHS
        .into_iter()
        .filter(|path| *path != except)
        .collect();
    probe(&folders, &[PROGRAM, POLICY, PKEXEC], &protected)
}

fn find_on_linux(probe: &FakeProbe) -> Result<PathBuf, Unavailable> {
    installed_at(
        probe,
        Path::new(PROGRAM),
        Path::new(POLICY),
        Path::new(PKEXEC),
    )
    .map(|installed| installed.program().to_path_buf())
}

#[test]
fn finds_the_helper_where_the_deb_installs_it() {
    assert_eq!(
        find_on_linux(&installed_on_linux("")).unwrap(),
        Path::new(PROGRAM)
    );
}

/// The policy is what has pkexec ask for an administrator's password each time, so whoever can
/// change it could let anyone start the helper.
#[test]
fn the_helper_the_policy_and_every_folder_above_them_must_be_only_root() {
    for path in LINUX_PATHS {
        let error = find_on_linux(&installed_on_linux(path)).unwrap_err();

        assert!(
            matches!(&error, Unavailable::Unprotected(unprotected) if unprotected == Path::new(path)),
            "{path}: {error:?}"
        );
    }
}

#[test]
fn without_pkexec_or_the_policy_there_is_no_helper() {
    let without_pkexec = probe(&[], &[PROGRAM], &[]);
    let without_policy = probe(
        &["/", "/usr", "/usr/lib", "/usr/lib/cleanse"],
        &[PROGRAM, PKEXEC],
        &["/", "/usr", "/usr/lib", "/usr/lib/cleanse", PROGRAM],
    );

    assert!(matches!(
        find_on_linux(&without_pkexec),
        Err(Unavailable::NoPkexec)
    ));
    assert!(matches!(
        find_on_linux(&without_policy),
        Err(Unavailable::NotInstalled)
    ));
}
