#![allow(clippy::disallowed_methods, reason = "tests build fixture trees")]

use std::fs::{self, File, FileTimes};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use cleanse_model::EntryKind;

use super::{
    Descend, DirFiles, WalkReport, distinct_roots, measure, measure_with, walk, walk_with,
};

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
fn measures_contents_and_notes_what_the_safety_checks_need() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.txt"), "ab").unwrap();
    fs::create_dir(root.path().join("sub")).unwrap();
    fs::write(root.path().join("sub/b.txt"), "cde").unwrap();
    fs::write(root.path().join("sub/.env.production"), "x").unwrap();
    fs::write(root.path().join("sub/Disk.VMDK"), "").unwrap();

    let measurement = measure(root.path()).unwrap();

    assert_eq!(measurement.files, 4);
    assert_eq!(measurement.size.logical, 6);
    assert_eq!(measurement.size.guaranteed, measurement.size.on_disk);
    assert_eq!(measurement.secret.as_deref(), Some(".env.production"));
    assert_eq!(measurement.vm_disk.as_deref(), Some("disk.vmdk"));
    assert!(measurement.newest_modified.is_some());
    assert_eq!((measurement.unreadable, measurement.mounts), (0, 0));
    assert!(!measurement.shared);
}

/// Stopping cleanup cuts measuring the item it's on short, so it goes no deeper.
#[test]
fn measuring_goes_no_deeper_once_stopped() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.txt"), "ab").unwrap();
    fs::create_dir(root.path().join("sub")).unwrap();
    fs::write(root.path().join("sub/b.txt"), "cde").unwrap();

    let whole = measure_with(root.path(), |_| None, Some(&AtomicBool::new(false))).unwrap();
    let stopped = measure_with(root.path(), |_| None, Some(&AtomicBool::new(true))).unwrap();

    assert_eq!((whole.files, whole.size.logical), (2, 5));
    assert_eq!((stopped.files, stopped.size.logical), (1, 2));
}

/// Sets when a file or folder was last modified and opened.
fn set_times(path: &Path, modified: SystemTime, accessed: SystemTime) {
    let mut options = File::options();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_WRITE_ATTRIBUTES, and FILE_FLAG_BACKUP_SEMANTICS, which opening a folder needs.
        options.access_mode(0x100).custom_flags(0x0200_0000);
    }
    #[cfg(unix)]
    options.read(true);
    let times = FileTimes::new()
        .set_modified(modified)
        .set_accessed(accessed);
    options.open(path).unwrap().set_times(times).unwrap();
}

fn days_ago(days: u64) -> SystemTime {
    SystemTime::now() - Duration::from_hours(24 * days)
}

/// Filesystems store times less precisely than `SystemTime`.
fn assert_near(actual: Option<SystemTime>, expected: SystemTime) {
    let actual = actual.unwrap();
    let apart = actual
        .duration_since(expected)
        .unwrap_or_else(|error| error.duration());
    assert!(apart < Duration::from_secs(2), "{actual:?} vs {expected:?}");
}

/// Scanners open files without anything needing them, so only changes count.
#[test]
fn notes_when_anything_inside_last_changed_but_not_when_it_was_opened() {
    let root = tempfile::tempdir().unwrap();
    let sub = root.path().join("sub");
    fs::create_dir(&sub).unwrap();
    let opened = sub.join("opened.txt");
    fs::write(&opened, "x").unwrap();
    set_times(&opened, days_ago(9), days_ago(1));
    set_times(&sub, days_ago(5), days_ago(5));
    set_times(root.path(), days_ago(9), days_ago(9));

    let measurement = measure(root.path()).unwrap();

    assert_near(measurement.newest_modified, days_ago(9));
    assert_near(measurement.newest_change, days_ago(5));
}

/// A file unpacked from an archive keeps the archive's dates, but the folder it landed in
/// changed when it arrived.
#[test]
fn a_folder_changed_since_its_files_counts_as_used_then() {
    let root = tempfile::tempdir().unwrap();
    let unpacked = root.path().join("unpacked.exe");
    fs::write(&unpacked, "x").unwrap();
    set_times(&unpacked, days_ago(400), days_ago(400));
    set_times(root.path(), days_ago(1), days_ago(1));

    let measurement = measure(root.path()).unwrap();

    assert_near(measurement.newest_modified, days_ago(400));
    assert_near(measurement.newest_change, days_ago(1));
}

#[test]
fn measures_a_file_on_its_own() {
    let root = tempfile::tempdir().unwrap();
    let dump = root.path().join("crash.dmp");
    fs::write(&dump, "abcd").unwrap();
    let disk = root.path().join("Ubuntu.vhdx");
    fs::write(&disk, "").unwrap();

    let measurement = measure(&dump).unwrap();

    assert_eq!(measurement.files, 1);
    assert_eq!(measurement.size.logical, 4);
    assert_eq!(measurement.size.guaranteed, measurement.size.on_disk);
    assert!(measurement.newest_modified.is_some());
    assert_eq!(measurement.vm_disk, None);
    assert_eq!(
        measure(&disk).unwrap().vm_disk.as_deref(),
        Some("ubuntu.vhdx")
    );
}

#[cfg(unix)]
#[test]
fn a_hard_linked_file_counts_as_shared() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("a.log");
    fs::write(&file, "abcd").unwrap();
    fs::hard_link(&file, root.path().join("b.log")).unwrap();

    let measurement = measure(&file).unwrap();

    assert!(measurement.shared);
    assert_eq!(measurement.size.guaranteed, 0);
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

/// Walks `root` and returns each folder it listed with its files' totals, relative to `root`.
fn listed(root: &Path) -> Vec<(PathBuf, DirFiles)> {
    let listed = Mutex::new(Vec::new());
    walk_with(
        root,
        |_| Descend::Yes,
        |dir, files| {
            let relative = dir.strip_prefix(root).unwrap().to_path_buf();
            listed.lock().unwrap().push((relative, files));
        },
    )
    .unwrap();
    let mut listed = listed.into_inner().unwrap();
    listed.sort_by(|a, b| a.0.cmp(&b.0));
    listed
}

#[test]
fn adds_up_the_files_directly_in_each_folder() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.txt"), "ab").unwrap();
    fs::create_dir_all(root.path().join("sub/deeper")).unwrap();
    fs::write(root.path().join("sub/b.txt"), "cde").unwrap();
    fs::write(root.path().join("sub/c.txt"), "f").unwrap();

    let listed = listed(root.path());

    let totals: Vec<(PathBuf, u64, u64)> = listed
        .iter()
        .map(|(dir, files)| (dir.clone(), files.files, files.logical))
        .collect();
    assert_eq!(
        totals,
        [
            (PathBuf::new(), 1, 2),
            (PathBuf::from("sub"), 2, 4),
            (PathBuf::from_iter(["sub", "deeper"]), 0, 0),
        ]
    );
    assert!(listed.iter().all(|(_, files)| !files.incomplete));
}

#[cfg(unix)]
#[test]
fn a_folder_that_cant_be_listed_has_no_totals() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let locked = root.path().join("locked");
    fs::create_dir(&locked).unwrap();
    fs::write(locked.join("hidden.txt"), "x").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    // Root can read anything, so there's nothing to test when running as root.
    let running_as_root = fs::read_dir(&locked).is_ok();

    let listed = listed(root.path());
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if running_as_root {
        return;
    }

    let dirs: Vec<&PathBuf> = listed.iter().map(|(dir, _)| dir).collect();
    assert_eq!(dirs, [&PathBuf::new()]);
}

#[test]
fn roots_inside_others_or_repeated_are_walked_once() {
    let root = tempfile::tempdir().unwrap();
    let outer = std::path::absolute(root.path()).unwrap();
    let inner = outer.join("inner");
    let other = tempfile::tempdir().unwrap();
    let other = std::path::absolute(other.path()).unwrap();
    let mut trailing = other.clone().into_os_string();
    trailing.push(std::path::MAIN_SEPARATOR_STR);

    let roots = distinct_roots(&[
        inner.clone(),
        outer.clone(),
        PathBuf::from(trailing),
        other.clone(),
    ])
    .unwrap();

    assert_eq!(roots, [outer, other]);
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn roots_differing_only_in_case_are_the_same_folder() {
    let root = tempfile::tempdir().unwrap();
    let root = std::path::absolute(root.path()).unwrap();
    let shouted = PathBuf::from(root.to_string_lossy().to_uppercase());

    assert_eq!(distinct_roots(&[root.clone(), shouted]).unwrap(), [root]);
}
