use std::fs::{self, Metadata};
use std::io;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use cleanse_model::EntryKind;

/// One filesystem entry found below a walk's root.
#[derive(Debug)]
pub struct Entry<'a> {
    pub path: &'a Path,
    pub kind: EntryKind,
    /// Logical size in bytes.
    pub len: u64,
}

#[derive(Debug, Default)]
pub struct WalkReport {
    /// Entries and directories that couldn't be read. Everything above them is incomplete.
    pub errors: Vec<WalkError>,
    /// Directories on a different filesystem from the root. They're reported but not entered.
    pub other_filesystems: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct WalkError {
    /// The entry or directory that couldn't be read.
    pub path: PathBuf,
    pub error: io::Error,
}

/// Walks everything below `root` using `std::fs::read_dir` on a rayon thread pool, calling
/// `visit` for each entry from many threads at once.
///
/// Links are reported but never followed, directories on other filesystems are reported but
/// not entered, and unreadable entries are collected in the report instead of ending the walk.
/// ADR 0007 explains why this isn't built on a walker crate.
pub fn walk<F>(root: &Path, visit: F) -> io::Result<WalkReport>
where
    F: Fn(&Entry<'_>) + Sync,
{
    let state = State {
        visit,
        filesystem: Filesystem::of(&root_metadata(root)?),
        report: Mutex::default(),
    };
    rayon::scope(|scope| read_dir(scope, root.to_path_buf(), &state));
    Ok(state
        .report
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner))
}

/// Returns the root's metadata, rejecting anything that isn't a directory. A link to a
/// directory is rejected too, because walks never follow links.
fn root_metadata(root: &Path) -> io::Result<Metadata> {
    let metadata = fs::symlink_metadata(root)?;
    if metadata.is_dir() {
        Ok(metadata)
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            format!(
                "{} is not a directory (links are not followed)",
                root.display()
            ),
        ))
    }
}

struct State<F> {
    visit: F,
    filesystem: Filesystem,
    report: Mutex<WalkReport>,
}

impl<F> State<F> {
    fn report(&self) -> MutexGuard<'_, WalkReport> {
        self.report.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn error(&self, path: PathBuf, error: io::Error) {
        self.report().errors.push(WalkError { path, error });
    }
}

/// The filesystem a directory lives on. On Windows, mount points are reparse points, which are
/// never followed, so there is nothing to compare.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Filesystem {
    #[cfg(unix)]
    device: u64,
}

impl Filesystem {
    #[cfg_attr(
        not(unix),
        expect(unused_variables, reason = "only Unix compares devices")
    )]
    fn of(metadata: &Metadata) -> Self {
        Self {
            #[cfg(unix)]
            device: metadata.dev(),
        }
    }
}

fn read_dir<'scope, F>(scope: &rayon::Scope<'scope>, dir: PathBuf, state: &'scope State<F>)
where
    F: Fn(&Entry<'_>) + Sync,
{
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) => return state.error(dir, error),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                state.error(dir.clone(), error);
                continue;
            }
        };
        let path = entry.path();
        // Windows fills this in from the directory listing itself. On Unix it's an lstat, which
        // describes a link rather than its target.
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                state.error(path, error);
                continue;
            }
        };
        let kind = EntryKind::from(metadata.file_type());
        (state.visit)(&Entry {
            path: &path,
            kind,
            len: metadata.len(),
        });
        if kind == EntryKind::Dir {
            if Filesystem::of(&metadata) == state.filesystem {
                scope.spawn(move |scope| read_dir(scope, path, state));
            } else {
                state.report().other_filesystems.push(path);
            }
        }
    }
}
