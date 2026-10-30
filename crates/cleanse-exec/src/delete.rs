//! Deleting through a tombstone: the folder is first renamed to a hidden sibling, so a project
//! never sees it half-deleted, and what landed there is checked to be the approved folder before
//! anything is removed.

#![allow(
    clippy::disallowed_methods,
    reason = "cleanse-exec is the one crate that changes the filesystem (ADR 0002)"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use cleanse_model::{EntryKind, FileIdentity, Outcome, Size, SkipReason};
use cleanse_platform::Probe;

const TOMBSTONE_PREFIX: &str = ".cleanse-tombstone-";

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

/// Deletes the folder at `path` by way of `tombstone`, provided it's still the folder with
/// `identity`. `size` is what it's reported to free.
pub(crate) fn delete(
    probe: &dyn Probe,
    path: &Path,
    tombstone: &Path,
    identity: FileIdentity,
    size: Size,
) -> Outcome {
    // On Unix, renaming a folder onto an empty one replaces it, so the name has to be free.
    if fs::symlink_metadata(tombstone).is_ok() {
        return Outcome::Failed {
            error: format!("{} is already taken", tombstone.display()),
        };
    }
    // On Windows this fails while a program has a file inside open, before anything is removed.
    if let Err(error) = fs::rename(path, tombstone) {
        return Outcome::Failed {
            error: format!("couldn't start removing it: {error}"),
        };
    }

    // Whatever was at `path` is at `tombstone` now. If something replaced the folder after it
    // was checked, that's what moved, so it goes back untouched.
    let moved = probe.inspect(tombstone);
    if !moved.is_ok_and(|facts| facts.kind == EntryKind::Dir && facts.identity == identity) {
        return put_back(path, tombstone);
    }
    match fs::remove_dir_all(tombstone) {
        Ok(()) => Outcome::Deleted { size },
        Err(error) => Outcome::Failed {
            error: format!("{error}. What's left of it is in {}", tombstone.display()),
        },
    }
}

fn put_back(path: &Path, tombstone: &Path) -> Outcome {
    let result = if fs::symlink_metadata(path).is_ok() {
        Err(format!("{} was recreated", path.display()))
    } else {
        fs::rename(tombstone, path).map_err(|error| error.to_string())
    };
    match result {
        Ok(()) => Outcome::Skipped {
            reason: SkipReason::Replaced,
        },
        Err(error) => Outcome::Failed {
            error: format!(
                "something replaced it while it was being removed, and that couldn't be put \
                 back ({error}). It's in {}",
                tombstone.display()
            ),
        },
    }
}
