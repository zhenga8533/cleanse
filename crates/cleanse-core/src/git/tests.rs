#![allow(
    clippy::disallowed_methods,
    reason = "tests build fixture repositories, using the git command line"
)]

use std::fs;
use std::path::Path;
use std::process::Command;

use cleanse_model::EntryKind;

use super::{GitFacts, GitInfo, GitStatus};

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "core.autocrlf=false"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A repository whose `.gitignore` ignores `dist/`, with `src/main.js` tracked and
/// `dist/app.js` untracked.
fn repository() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join(".gitignore"), "dist/\n");
    write(&root.path().join("src/main.js"), "");
    write(&root.path().join("dist/app.js"), "");
    git(root.path(), &["init", "-q"]);
    git(root.path(), &["add", "--", ".gitignore", "src/main.js"]);
    root
}

fn facts(ignored: bool, has_tracked_files: bool) -> GitStatus {
    GitStatus::Known(GitFacts {
        ignored,
        has_tracked_files,
    })
}

#[test]
fn reports_ignored_and_tracked_directories() {
    let repo = repository();
    let git = GitInfo::new();

    assert_eq!(
        git.status(&repo.path().join("dist"), EntryKind::Dir),
        facts(true, false)
    );
    assert_eq!(
        git.status(&repo.path().join("src"), EntryKind::Dir),
        facts(false, true)
    );
}

#[test]
fn reports_tracked_and_untracked_files() {
    let repo = repository();
    let git = GitInfo::new();

    assert_eq!(
        git.status(&repo.path().join("src/main.js"), EntryKind::File),
        facts(false, true)
    );
    assert_eq!(
        git.status(&repo.path().join("dist/app.js"), EntryKind::File),
        facts(true, false)
    );
}

/// `dist/` in `.gitignore` matches only directories.
#[test]
fn a_file_named_like_an_ignored_directory_isnt_ignored() {
    let repo = repository();
    write(&repo.path().join("notes/dist"), "");

    let status = GitInfo::new().status(&repo.path().join("notes/dist"), EntryKind::File);

    assert_eq!(status, facts(false, false));
}

#[test]
fn a_directory_inside_an_ignored_one_is_ignored() {
    let repo = repository();
    fs::create_dir_all(repo.path().join("dist/assets")).unwrap();

    let status = GitInfo::new().status(&repo.path().join("dist/assets"), EntryKind::Dir);

    assert_eq!(status, facts(true, false));
}

#[test]
fn ignored_files_that_were_committed_anyway_still_count_as_tracked() {
    let repo = repository();
    git(repo.path(), &["add", "--force", "--", "dist/app.js"]);

    let status = GitInfo::new().status(&repo.path().join("dist"), EntryKind::Dir);

    assert_eq!(status, facts(true, true));
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn tracked_directories_match_regardless_of_case() {
    let repo = repository();

    let status = GitInfo::new().status(&repo.path().join("SRC"), EntryKind::Dir);

    assert_eq!(status, facts(false, true));
}

#[test]
fn outside_a_repository_there_is_nothing_to_report() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("dist")).unwrap();

    let status = GitInfo::new().status(&dir.path().join("dist"), EntryKind::Dir);

    assert_eq!(status, GitStatus::NotInRepository);
}

#[test]
fn a_nested_repository_is_judged_by_the_one_around_it() {
    let repo = repository();
    let nested = repo.path().join("dist");
    git(&nested, &["init", "-q"]);

    let status = GitInfo::new().status(&nested, EntryKind::Dir);

    assert_eq!(status, facts(true, false));
}

#[test]
fn a_repository_that_cant_be_read_is_reported_as_such() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join(".git"), "this is not a repository\n");
    fs::create_dir(dir.path().join("dist")).unwrap();

    let status = GitInfo::new().status(&dir.path().join("dist"), EntryKind::Dir);

    assert!(
        matches!(&status, GitStatus::Unreadable { repository, .. } if repository == dir.path()),
        "{status:?}"
    );
}
