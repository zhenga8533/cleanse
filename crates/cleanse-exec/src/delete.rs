//! Deleting through a tombstone: the item is first renamed to a hidden sibling, so a project
//! never sees a folder half-deleted, and what landed there is checked to be the approved item
//! before anything is removed.

#![allow(
    clippy::disallowed_methods,
    reason = "cleanse-exec is the one crate that changes the filesystem (ADR 0002)"
)]

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use cleanse_model::{EntryKind, FileIdentity, Outcome, Size, SkipReason};
use cleanse_platform::Probe;
use cleanse_platform::privileged::Folder;
use rayon::prelude::*;
use rayon::{ThreadPool, ThreadPoolBuilder};

pub(crate) use cleanse_core::TOMBSTONE_PREFIX;

/// A sibling of `path` with a name no other removal uses, or `None` for a drive's root.
pub(crate) fn tombstone_for(path: &Path) -> Option<PathBuf> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent()?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    Some(parent.join(format!(
        "{TOMBSTONE_PREFIX}{}-{nanos}-{sequence}",
        process::id()
    )))
}

/// Never set, for removals that always run to the end.
pub(crate) static NEVER_STOP: AtomicBool = AtomicBool::new(false);

/// Why removing something ended before all of it was gone.
#[derive(Debug)]
pub(crate) enum Unfinished {
    /// Stopping was asked for.
    Stopped,
    Failed(io::Error),
}

impl From<io::Error> for Unfinished {
    fn from(error: io::Error) -> Self {
        Self::Failed(error)
    }
}

impl fmt::Display for Unfinished {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stopped => f.write_str("Stopped before all of it was removed"),
            Self::Failed(error) => error.fmt(f),
        }
    }
}

/// Deletes the `kind` of item at `path` by way of `tombstone`, provided it's still the item with
/// `identity`. `size` is what it's reported to free. Once `stop` is set, nothing more is removed.
pub(crate) fn delete(
    probe: &dyn Probe,
    path: &Path,
    tombstone: &Path,
    kind: EntryKind,
    identity: FileIdentity,
    size: Size,
    stop: &AtomicBool,
) -> Outcome {
    // On Unix, renaming a folder onto an empty one replaces it, so the name has to be free.
    if fs::symlink_metadata(tombstone).is_ok() {
        return Outcome::Failed {
            error: format!("{} is already taken", tombstone.display()),
        };
    }
    // On Windows this fails while a program has the file, or a file inside the folder, open,
    // before anything is removed.
    if let Err(error) = fs::rename(path, tombstone) {
        return Outcome::Failed {
            error: not_moved("Couldn't start removing it", &error),
        };
    }

    // Whatever was at `path` is at `tombstone` now. If something replaced the item after it was
    // checked, that's what moved, so it goes back untouched.
    let moved = probe.inspect(tombstone);
    if !moved.is_ok_and(|facts| facts.kind == kind && facts.identity == identity) {
        return put_back(path, tombstone);
    }
    finish(tombstone, kind, size, stop)
}

/// Why moving an item failed, after `what` couldn't be done.
///
/// Windows won't move a file, or a folder, while a program has it or anything inside it open,
/// and says only that access is denied, or that it's in use. Its own users are rarely denied
/// moving their own files, so a program having it open is what that almost always means.
pub(crate) fn not_moved(what: &str, error: &io::Error) -> String {
    /// `ERROR_ACCESS_DENIED` and `ERROR_SHARING_VIOLATION`.
    const IN_USE: [i32; 2] = [5, 32];
    if cfg!(windows)
        && error
            .raw_os_error()
            .is_some_and(|code| IN_USE.contains(&code))
    {
        format!(
            "{what}: Windows wouldn't move it, which usually means a program has it, or a file \
             inside it, open, such as an editor, a dev server or a build. Close that program and \
             try again ({error})"
        )
    } else {
        format!("{what}: {error}")
    }
}

/// Removes the approved `kind` of item at `tombstone`, or what's left of it after a removal that
/// didn't finish. The caller has checked that it's the approved item. `size` is what it's
/// reported to free. Once `stop` is set, nothing more is removed, and what's left stays in the
/// tombstone, where History lists it.
pub(crate) fn finish(tombstone: &Path, kind: EntryKind, size: Size, stop: &AtomicBool) -> Outcome {
    match remove(tombstone, kind, stop) {
        Ok(()) => Outcome::Deleted { size },
        Err(unfinished @ Unfinished::Stopped) => Outcome::Failed {
            error: format!(
                "{unfinished}. What's left of it is in {}, and History can remove the rest",
                tombstone.display()
            ),
        },
        Err(unfinished @ Unfinished::Failed(_)) => Outcome::Failed {
            error: format!(
                "{unfinished}. What's left of it is in {}",
                tombstone.display()
            ),
        },
    }
}

/// Removes a folder with everything inside, or a file. A folder's contents go from many threads
/// at once, through each folder's handle, so a link inside is only ever removed as itself. Once
/// `stop` is set, nothing more is removed.
pub(crate) fn remove(path: &Path, kind: EntryKind, stop: &AtomicBool) -> Result<(), Unfinished> {
    if stop.load(Ordering::Relaxed) {
        return Err(Unfinished::Stopped);
    }
    if kind != EntryKind::Dir {
        return Ok(fs::remove_file(path)?);
    }
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "It has no parent folder").into());
    };
    let parent = Folder::open(parent)?;
    let remove = || remove_inside(&parent, name, true, stop);
    match pool() {
        Some(pool) => pool.install(remove),
        None => remove(),
    }
}

/// Removes `name` from inside `folder`, and if it's a folder, everything inside it first.
fn remove_inside(
    folder: &Folder,
    name: &OsStr,
    is_folder: bool,
    stop: &AtomicBool,
) -> Result<(), Unfinished> {
    if stop.load(Ordering::Relaxed) {
        return Err(Unfinished::Stopped);
    }
    // A folder that turns out to be a link put in its place goes as a link.
    if is_folder && let Some(inside) = folder.open_folder(name)? {
        inside
            .children()?
            .into_par_iter()
            .try_for_each(|child| remove_inside(&inside, &child.name, child.is_folder, stop))?;
        drop(inside);
        return Ok(folder.remove_folder(name)?);
    }
    Ok(folder.remove_file(name)?)
}

/// The threads removals run on, or `None` if they couldn't be started, when they run on rayon's
/// own.
///
/// Removing a file mostly waits on the filesystem, and any antivirus checking it, rather than the
/// processor, so having more removals going than there are processors keeps it busy. On a laptop
/// with 12 logical processors and endpoint protection, 16 to 24 threads removed small files about
/// five times as fast as std's `remove_dir_all`, and 32 no faster.
fn pool() -> Option<&'static ThreadPool> {
    static POOL: OnceLock<Option<ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let processors = thread::available_parallelism().map_or(4, NonZero::get);
        ThreadPoolBuilder::new()
            .num_threads((processors * 2).clamp(4, 32))
            .thread_name(|index| format!("cleanse-remove-{index}"))
            .build()
            .ok()
    })
    .as_ref()
}

/// Moves whatever is at `moved` back to `path`, where it was before something swapped it in
/// for the approved item.
pub(crate) fn put_back(path: &Path, moved: &Path) -> Outcome {
    let result = if fs::symlink_metadata(path).is_ok() {
        Err(format!("{} was recreated", path.display()))
    } else {
        fs::rename(moved, path).map_err(|error| error.to_string())
    };
    match result {
        Ok(()) => Outcome::Skipped {
            reason: SkipReason::Replaced,
        },
        Err(error) => Outcome::Failed {
            error: format!(
                "Something replaced it while it was being removed, and that couldn't be put \
                 back ({error}). It's in {}",
                moved.display()
            ),
        },
    }
}
