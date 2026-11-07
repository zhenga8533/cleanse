use std::env;
use std::fs::{self, Metadata};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use cleanse_model::{EntryKind, FileIdentity};

use crate::{Elevation, FileFacts, FileOwner, ListedFacts, SystemFolders};

#[cfg(any(test, feature = "privileged"))]
pub(crate) mod folder;

/// `st_blocks` counts 512-byte units on Linux and macOS, whatever the filesystem's block size.
const STAT_BLOCK_BYTES: u64 = 512;

/// The account this process runs as: its effective user ID, which owns the files it creates.
#[derive(Debug)]
pub(crate) struct User(u32);

#[expect(
    clippy::unnecessary_wraps,
    reason = "matches the Windows version, which can fail"
)]
pub(crate) fn current_user() -> io::Result<User> {
    Ok(User(rustix::process::geteuid().as_raw()))
}

pub(crate) fn inspect(path: &Path, user: &User) -> io::Result<FileFacts> {
    let metadata = fs::symlink_metadata(path)?;
    let owner = match metadata.uid() {
        0 => FileOwner::System,
        uid if uid == user.0 => FileOwner::CurrentUser,
        _ => FileOwner::Other,
    };
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
        owner: Some(owner),
    })
}

pub(crate) fn temp_dir() -> PathBuf {
    env::temp_dir()
}

/// Unix has no one folder that every account's home is made in.
pub(crate) fn user_profiles() -> Option<PathBuf> {
    None
}

/// Whether the folder at `path` is one only root can change: a real folder, not a link, that root
/// owns and no one else can open, inside folders no one else can change.
pub(crate) fn is_private_dir(path: &Path) -> io::Result<bool> {
    /// Group and others' permissions.
    const OTHERS: u32 = 0o077;
    /// Group and others' permission to write.
    const OTHERS_WRITE: u32 = 0o022;
    /// In a folder with the sticky bit, such as `/tmp`, only an entry's owner can move it.
    const STICKY: u32 = 0o1000;

    let metadata = fs::symlink_metadata(path)?;
    let private = metadata.is_dir() && metadata.uid() == 0 && metadata.mode() & OTHERS == 0;
    // Whoever can write a folder above it can move it away and put another in its place.
    let above_safe = path.ancestors().skip(1).all(|folder| {
        fs::symlink_metadata(folder).is_ok_and(|folder| {
            folder.is_dir()
                && folder.uid() == 0
                && (folder.mode() & OTHERS_WRITE == 0 || folder.mode() & STICKY != 0)
        })
    });
    Ok(private && above_safe)
}

/// Whether `path` is a real file or folder, not a link, that root owns and no one else may write.
pub(crate) fn only_admins_can_change(path: &Path) -> io::Result<bool> {
    /// Group and others' permission to write.
    const OTHERS_WRITE: u32 = 0o022;

    let metadata = fs::symlink_metadata(path)?;
    Ok(!metadata.file_type().is_symlink()
        && metadata.uid() == 0
        && metadata.mode() & OTHERS_WRITE == 0)
}

/// Creates the folder at `path`, which mustn't exist, so that only its owner can open it.
#[cfg(any(test, feature = "privileged"))]
#[expect(
    clippy::disallowed_methods,
    reason = "the privileged module's one change: creating system cleanup's data folder"
)]
pub(crate) fn create_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    fs::DirBuilder::new().mode(0o700).create(path)
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "matches the Windows version, which can fail"
)]
pub(crate) fn system_folders() -> io::Result<SystemFolders> {
    Ok(SystemFolders {
        system_root: None,
        program_data: None,
        var: cfg!(target_os = "linux").then(|| PathBuf::from("/var")),
    })
}

pub(crate) fn listed_facts(metadata: &Metadata) -> ListedFacts {
    ListedFacts {
        on_disk: metadata.blocks() * STAT_BLOCK_BYTES,
        links: Some(metadata.nlink()),
        offline: is_dataless(metadata),
    }
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

/// Root is always [`Elevation::Elevated`]: whoever runs Cleanse as root has an account of their
/// own to run it as, and `sudo` is a choice made each time.
#[expect(
    clippy::unnecessary_wraps,
    reason = "matches the Windows version, which can fail"
)]
pub(crate) fn elevation(user: &User) -> io::Result<Elevation> {
    Ok(if user.0 == 0 {
        Elevation::Elevated
    } else {
        Elevation::None
    })
}
