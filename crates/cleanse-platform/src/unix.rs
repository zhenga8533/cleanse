use std::fs::{self, Metadata};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use cleanse_model::{EntryKind, FileIdentity};

use crate::FileFacts;

/// `st_blocks` counts 512-byte units on Linux and macOS, whatever the filesystem's block size.
const STAT_BLOCK_BYTES: u64 = 512;

pub(crate) fn inspect(path: &Path) -> io::Result<FileFacts> {
    let metadata = fs::symlink_metadata(path)?;
    Ok(FileFacts {
        identity: FileIdentity {
            volume: metadata.dev(),
            file: u128::from(metadata.ino()),
        },
        kind: EntryKind::from(metadata.file_type()),
        links: metadata.nlink(),
        allocated: metadata.blocks() * STAT_BLOCK_BYTES,
        cloud_synced: false,
        offline: is_dataless(&metadata),
    })
}

/// Whether the data lives in iCloud Drive or another File Provider and is downloaded on access.
#[cfg(target_os = "macos")]
fn is_dataless(metadata: &Metadata) -> bool {
    use std::os::macos::fs::MetadataExt as _;

    /// `SF_DATALESS` from `<sys/stat.h>`.
    const SF_DATALESS: u32 = 0x4000_0000;
    metadata.st_flags() & SF_DATALESS != 0
}

/// Linux has no equivalent of macOS's dataless files; sync clients there keep full copies.
#[cfg(not(target_os = "macos"))]
fn is_dataless(_: &Metadata) -> bool {
    false
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "matches the Windows version, which can fail"
)]
pub(crate) fn is_elevated() -> io::Result<bool> {
    Ok(rustix::process::geteuid().is_root())
}
