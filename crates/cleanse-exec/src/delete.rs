//! Deleting through a tombstone: the item is first renamed to a hidden sibling, so a project
//! never sees a folder half-deleted, and what landed there is checked to be the approved item
//! before anything is removed.

#![allow(
    clippy::disallowed_methods,
    reason = "cleanse-exec is the one crate that changes the filesystem (ADR 0002)"
)]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use cleanse_model::{EntryKind, FileIdentity, Outcome, Size, SkipReason};
use cleanse_platform::Probe;

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

/// Deletes the `kind` of item at `path` by way of `tombstone`, provided it's still the item with
/// `identity`. `size` is what it's reported to free.
pub(crate) fn delete(
    probe: &dyn Probe,
    path: &Path,
    tombstone: &Path,
    kind: EntryKind,
    identity: FileIdentity,
    size: Size,
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
    finish(tombstone, kind, size)
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
/// reported to free.
pub(crate) fn finish(tombstone: &Path, kind: EntryKind, size: Size) -> Outcome {
    match remove(tombstone, kind) {
        Ok(()) => Outcome::Deleted { size },
        Err(error) => Outcome::Failed {
            error: format!("{error}. What's left of it is in {}", tombstone.display()),
        },
    }
}

/// Removes a folder with everything inside, or a file.
pub(crate) fn remove(path: &Path, kind: EntryKind) -> io::Result<()> {
    if kind == EntryKind::Dir {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
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
