use std::io;
use std::num::NonZeroUsize;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::Path;
#[cfg(unix)]
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use dua_core::{Options, Order};

use super::walk::root_metadata;
use super::{Entry, EntryKind, WalkError, WalkReport};

/// Walks everything below `root` using dua-core's work-stealing walker, calling `visit` for each
/// entry from the calling thread.
///
/// Behaves like [`super::walk_parallel`], except that dua-core doesn't say which directory a
/// read error came from, so [`WalkError::path`] is `None` for those.
pub fn walk_dua<F>(root: &Path, visit: F) -> io::Result<WalkReport>
where
    F: Fn(&Entry<'_>) + Sync,
{
    #[cfg_attr(
        not(unix),
        expect(unused_variables, reason = "only Unix compares devices")
    )]
    let root_metadata = root_metadata(root)?;
    let threads = thread::available_parallelism().map_or(1, NonZeroUsize::get);

    #[cfg(unix)]
    let other_filesystems = Arc::new(Mutex::new(Vec::new()));
    let descend = {
        #[cfg(unix)]
        let (root_device, other_filesystems) =
            (root_metadata.dev(), Arc::clone(&other_filesystems));
        move |entry: &dua_core::Entry| {
            #[cfg(unix)]
            if entry.depth > 0
                && let Some(Ok(metadata)) = &entry.metadata
                && metadata.dev() != root_device
            {
                other_filesystems
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(entry.path());
                return false;
            }
            #[cfg(not(unix))]
            let _ = entry;
            true
        }
    };

    let mut report = WalkReport::default();
    for result in dua_core::walk(
        root,
        threads,
        Order::Completion,
        Options::default(),
        descend,
    ) {
        let entry = match result {
            Ok(entry) if entry.depth == 0 => continue,
            Ok(entry) => entry,
            Err(error) => {
                report.errors.push(WalkError { path: None, error });
                continue;
            }
        };
        let path = entry.path();
        let kind = if entry.file_type.is_symlink() {
            EntryKind::Link
        } else if entry.file_type.is_dir() {
            EntryKind::Dir
        } else if entry.file_type.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        };
        let len = match entry.metadata {
            Some(Ok(metadata)) => metadata.len(),
            Some(Err(error)) => {
                report.errors.push(WalkError {
                    path: Some(path),
                    error,
                });
                continue;
            }
            // Only happens when metadata is skipped, which this walk never asks for.
            None => 0,
        };
        visit(&Entry {
            path: &path,
            kind,
            len,
        });
    }

    #[cfg(unix)]
    {
        report.other_filesystems = std::mem::take(
            &mut *other_filesystems
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
    }
    Ok(report)
}
