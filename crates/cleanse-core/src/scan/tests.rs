#![allow(clippy::disallowed_methods, reason = "tests build fixture trees")]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cleanse_model::EntryKind;

use super::{Descend, WalkReport, walk};

/// Walks `root`, returning every entry relative to it, sorted. Directory sizes are zeroed
/// because they differ between filesystems.
fn collect(root: &Path) -> (Vec<(PathBuf, EntryKind, u64)>, WalkReport) {
    collect_pruning(root, |_| false)
}

/// Like [`collect`], but doesn't enter directories for which `prune` returns true.
fn collect_pruning(
    root: &Path,
    prune: impl Fn(&Path) -> bool + Sync,
) -> (Vec<(PathBuf, EntryKind, u64)>, WalkReport) {
    let seen = Mutex::new(Vec::new());
    let report = walk(root, |entry| {
        let relative = entry.path.strip_prefix(root).unwrap().to_path_buf();
        let len = if entry.kind == EntryKind::Dir {
            0
        } else {
            entry.len
        };
        let descend = if prune(&relative) {
            Descend::No
        } else {
            Descend::Yes
        };
        seen.lock().unwrap().push((relative, entry.kind, len));
        descend
    })
    .unwrap();
    let mut seen = seen.into_inner().unwrap();
    seen.sort();
    (seen, report)
}

fn entry(path: &str, kind: EntryKind, len: u64) -> (PathBuf, EntryKind, u64) {
    (PathBuf::from_iter(path.split('/')), kind, len)
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

#[test]
fn reports_every_entry_below_the_root_including_hidden_ones() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.txt"), "abc").unwrap();
    fs::create_dir_all(root.path().join("sub/deeper")).unwrap();
    fs::write(root.path().join("sub/b.txt"), "hello").unwrap();
    fs::create_dir(root.path().join(".hidden")).unwrap();
    fs::write(root.path().join(".hidden/c"), "x").unwrap();

    let (seen, report) = collect(root.path());

    assert_eq!(
        seen,
        [
            entry(".hidden", EntryKind::Dir, 0),
            entry(".hidden/c", EntryKind::File, 1),
            entry("a.txt", EntryKind::File, 3),
            entry("sub", EntryKind::Dir, 0),
            entry("sub/b.txt", EntryKind::File, 5),
            entry("sub/deeper", EntryKind::Dir, 0),
        ]
    );
    assert!(report.errors.is_empty(), "{:?}", report.errors);
}

#[test]
fn reports_a_pruned_directory_but_not_its_contents() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("app/node_modules/pkg")).unwrap();
    fs::write(root.path().join("app/node_modules/pkg/index.js"), "x").unwrap();
    fs::write(root.path().join("app/main.js"), "x").unwrap();

    let (seen, _) = collect_pruning(root.path(), |path| path.ends_with("node_modules"));

    assert_eq!(
        seen,
        [
            entry("app", EntryKind::Dir, 0),
            entry("app/main.js", EntryKind::File, 1),
            entry("app/node_modules", EntryKind::Dir, 0),
        ]
    );
}

#[test]
fn reports_directory_links_without_following_them() {
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret.txt"), "keep me").unwrap();
    let root = tempfile::tempdir().unwrap();
    link_dir(outside.path(), &root.path().join("link"));

    let (seen, _) = collect(root.path());

    let kinds: Vec<_> = seen
        .iter()
        .map(|(path, kind, _)| (path.clone(), *kind))
        .collect();
    assert_eq!(kinds, [(PathBuf::from("link"), EntryKind::Link)]);
}

#[test]
fn rejects_a_root_that_is_not_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("file.txt");
    fs::write(&file, "x").unwrap();
    let target = dir.path().join("target");
    fs::create_dir(&target).unwrap();
    let link = dir.path().join("link");
    link_dir(&target, &link);

    for root in [&file, &link] {
        let error = walk(root, |_| Descend::Yes).unwrap_err();
        assert_eq!(
            error.kind(),
            io::ErrorKind::NotADirectory,
            "{}",
            root.display()
        );
    }
    let missing = walk(&dir.path().join("missing"), |_| Descend::Yes).unwrap_err();
    assert_eq!(missing.kind(), io::ErrorKind::NotFound);
}

#[test]
fn walks_paths_longer_than_the_windows_max_path() {
    let root = tempfile::tempdir().unwrap();
    let segment = "a-directory-name-thirty-chars-";
    let mut deep = root.path().to_path_buf();
    for _ in 0..12 {
        deep.push(segment);
    }
    assert!(deep.as_os_str().len() > 260);
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("bottom.txt"), "found").unwrap();

    let (seen, report) = collect(root.path());

    assert_eq!(seen.len(), 13);
    assert!(
        seen.iter()
            .any(|(path, kind, len)| path.ends_with("bottom.txt")
                && *kind == EntryKind::File
                && *len == 5)
    );
    assert!(report.errors.is_empty(), "{:?}", report.errors);
}

#[cfg(any(windows, all(unix, not(target_os = "macos"))))]
#[test]
fn keeps_file_names_that_are_not_valid_unicode() {
    #[cfg(windows)]
    let name = {
        use std::os::windows::ffi::OsStringExt;
        // An unpaired surrogate: valid on NTFS, impossible in UTF-8.
        std::ffi::OsString::from_wide(&[u16::from(b'a'), 0xD800])
    };
    #[cfg(unix)]
    let name = {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::OsStr::from_bytes(b"bad\xff").to_owned()
    };
    assert!(name.to_str().is_none());
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join(&name), "x").unwrap();

    let (seen, _) = collect(root.path());

    assert_eq!(seen, [(PathBuf::from(&name), EntryKind::File, 1)]);
}

#[cfg(unix)]
#[test]
fn reports_unreadable_directories_by_path_and_keeps_going() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let locked = root.path().join("locked");
    fs::create_dir(&locked).unwrap();
    fs::write(locked.join("hidden.txt"), "x").unwrap();
    fs::write(root.path().join("visible.txt"), "x").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    // Root can read anything, so there's nothing to test when running as root.
    let running_as_root = fs::read_dir(&locked).is_ok();

    let (seen, report) = collect(root.path());
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if running_as_root {
        return;
    }

    assert!(
        seen.iter()
            .any(|(path, ..)| path == Path::new("visible.txt"))
    );
    assert!(!seen.iter().any(|(path, ..)| path.ends_with("hidden.txt")));
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    assert_eq!(report.errors[0].path, locked);
}
