//! What Git knows about directories inside a repository.
//!
//! Repositories are read with gix and never by running `git`, because a repository's config can
//! make `git` run commands (`core.fsmonitor`, for example) and scanned repositories aren't
//! trusted (ADR 0008). Only the repository's own config is read, not the user's or the
//! system's. A global excludes file is therefore ignored, which can only make fewer directories
//! count as ignored.

#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use gix::index::entry::Mode;
use gix::worktree::stack::state::ignore::Source;

/// What Git says about a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitStatus {
    /// No repository contains the directory.
    NotInRepository,
    /// A repository contains the directory but couldn't be read, so nothing is known about it.
    Unreadable {
        repository: PathBuf,
        error: String,
    },
    Known(GitFacts),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GitFacts {
    /// The repository's ignore rules exclude the directory or one of its parents.
    pub ignored: bool,
    /// The index has files, or a submodule, inside the directory.
    pub has_tracked_files: bool,
}

/// Looks up Git facts, opening each repository once and sharing it between threads.
#[derive(Debug, Default)]
pub struct GitInfo {
    repositories: Mutex<HashMap<PathBuf, Arc<Result<Repository, String>>>>,
}

impl GitInfo {
    pub fn new() -> Self {
        Self::default()
    }

    /// `dir` must be absolute.
    pub fn status(&self, dir: &Path) -> GitStatus {
        let Some(root) = repository_root(dir) else {
            return GitStatus::NotInRepository;
        };
        let relative = dir
            .strip_prefix(&root)
            .expect("the repository root is an ancestor of the directory");
        let repository = self.open(&root);
        let facts = match &*repository {
            Ok(repository) => repository.facts(relative),
            Err(error) => Err(error.clone()),
        };
        match facts {
            Ok(facts) => GitStatus::Known(facts),
            Err(error) => GitStatus::Unreadable {
                repository: root,
                error,
            },
        }
    }

    fn open(&self, root: &Path) -> Arc<Result<Repository, String>> {
        let mut repositories = self
            .repositories
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        Arc::clone(
            repositories
                .entry(root.to_path_buf())
                .or_insert_with(|| Arc::new(Repository::open(root))),
        )
    }
}

/// The closest ancestor of `dir` with a `.git` directory or file. `dir` itself doesn't count:
/// a `.git` inside it makes it a separate repository, which says nothing about how the
/// repository around it treats it.
fn repository_root(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .skip(1)
        .find(|ancestor| fs::symlink_metadata(ancestor.join(".git")).is_ok())
        .map(Path::to_path_buf)
}

/// What's cached for one repository. gix's repository handles aren't thread-safe without its
/// `parallel` feature, so only plain data is kept, and each lookup opens its own handle.
#[derive(Debug)]
struct Repository {
    root: PathBuf,
    index: gix::index::File,
    /// Every directory holding tracked files, as `/`-separated paths relative to the worktree.
    tracked_dirs: HashSet<Vec<u8>>,
}

impl Repository {
    fn open(root: &Path) -> Result<Self, String> {
        let index = (**open_repository(root)?
            .index_or_empty()
            .map_err(|error| error.to_string())?)
        .clone();
        let tracked_dirs = tracked_dirs(&index);
        Ok(Self {
            root: root.to_path_buf(),
            index,
            tracked_dirs,
        })
    }

    fn facts(&self, relative: &Path) -> Result<GitFacts, String> {
        let repo = open_repository(&self.root)?;
        let mut excludes = repo
            .excludes(&self.index, None, Source::WorktreeThenIdMappingIfNotSkipped)
            .map_err(|error| error.to_string())?;
        // Checking each parent too: everything inside an ignored directory is ignored.
        let mut prefix = PathBuf::new();
        let mut ignored = false;
        for component in relative.components() {
            prefix.push(component);
            let platform = excludes
                .at_path(&prefix, Some(Mode::DIR))
                .map_err(|error| error.to_string())?;
            if platform.is_excluded() {
                ignored = true;
                break;
            }
        }
        Ok(GitFacts {
            ignored,
            has_tracked_files: self.tracked_dirs.contains(&*fold_case(&path_key(relative))),
        })
    }
}

fn open_repository(root: &Path) -> Result<gix::Repository, String> {
    gix::open_opts(root, gix::open::Options::isolated()).map_err(|error| error.to_string())
}

fn tracked_dirs(index: &gix::index::State) -> HashSet<Vec<u8>> {
    let mut dirs: HashSet<Vec<u8>> = HashSet::new();
    for entry in index.entries() {
        let path = fold_case(entry.path(index));
        for (end, byte) in path.iter().enumerate() {
            if *byte == b'/' && !dirs.contains(&path[..end]) {
                dirs.insert(path[..end].to_vec());
            }
        }
        // A submodule is a directory whose contents another repository tracks.
        if entry.mode == Mode::COMMIT {
            dirs.insert(path.into_owned());
        }
    }
    dirs
}

/// The index's form of a worktree-relative path: names joined with `/`.
fn path_key(relative: &Path) -> Vec<u8> {
    let mut key = Vec::new();
    for component in relative.components() {
        if !key.is_empty() {
            key.push(b'/');
        }
        key.extend_from_slice(&component_bytes(component.as_os_str()));
    }
    key
}

#[cfg(unix)]
fn component_bytes(name: &std::ffi::OsStr) -> Cow<'_, [u8]> {
    use std::os::unix::ffi::OsStrExt;
    Cow::Borrowed(name.as_bytes())
}

/// Git for Windows stores paths as UTF-8, so a name that isn't valid Unicode can't be in the
/// index, and the replacement characters never match.
#[cfg(windows)]
fn component_bytes(name: &std::ffi::OsStr) -> Cow<'_, [u8]> {
    match name.to_string_lossy() {
        Cow::Borrowed(name) => Cow::Borrowed(name.as_bytes()),
        Cow::Owned(name) => Cow::Owned(name.into_bytes()),
    }
}

/// Windows and macOS treat names case-insensitively by default, and Git there does too
/// (`core.ignoreCase`), so a directory renamed only in case is still the tracked one.
fn fold_case(path: &[u8]) -> Cow<'_, [u8]> {
    if cfg!(any(windows, target_os = "macos")) && path.iter().any(u8::is_ascii_uppercase) {
        Cow::Owned(path.to_ascii_lowercase())
    } else {
        Cow::Borrowed(path)
    }
}
