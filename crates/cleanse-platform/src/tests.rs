#![allow(clippy::disallowed_methods, reason = "tests build fixture files")]

use std::fs;
use std::io;
use std::path::Path;

use cleanse_model::{EntryKind, FileIdentity};

use crate::{FakeProbe, FileFacts, KnownFolders, Probe, System};

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

#[test]
fn the_fake_answers_only_what_it_was_told() {
    let facts = FileFacts {
        identity: FileIdentity { volume: 1, file: 2 },
        kind: EntryKind::File,
        links: 1,
        allocated: 0,
        cloud_synced: true,
        offline: true,
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
}
