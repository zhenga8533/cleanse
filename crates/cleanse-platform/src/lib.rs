//! Read-only, per-OS filesystem facts behind a single `Probe` trait.
//!
//! File identity, ownership, link and reparse-point detection, cloud-placeholder state,
//! allocated size, known folders, and elevation live here so that all `cfg` and `unsafe` code
//! sits in one audited place. Nothing in this crate may modify the filesystem, apart from the
//! [`privileged`] module, behind its feature.

#![warn(clippy::undocumented_unsafe_blocks)]

#[cfg(any(test, feature = "fake"))]
mod fake;
#[cfg(any(test, feature = "privileged"))]
pub mod privileged;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(test)]
mod tests;

#[cfg(not(any(unix, windows)))]
compile_error!("Cleanse supports Windows, macOS, and Linux");

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use cleanse_model::{EntryKind, FileIdentity};

#[cfg(any(test, feature = "fake"))]
pub use fake::{FakeProbe, StagedSystem};
#[cfg(unix)]
use unix as os;
#[cfg(windows)]
use windows as os;

/// Read-only questions about the filesystem and the running process.
///
/// [`System`] answers them for real. Tests can use `FakeProbe` (behind the `fake` feature) to
/// stage situations that are impractical to create, such as cloud placeholders.
pub trait Probe: Send + Sync {
    /// Facts about the file or directory at `path`, without following a link there.
    ///
    /// This takes a few system calls (plus opening a handle on Windows), so it's meant for
    /// candidates rather than every scanned entry.
    fn inspect(&self, path: &Path) -> io::Result<FileFacts>;

    fn known_folders(&self) -> &KnownFolders;

    /// Whether the process runs as an administrator (Windows) or root (Unix).
    fn is_elevated(&self) -> bool;

    /// The names of the programs running now, for every user, as [`process_name`] gives them.
    fn running_processes(&self) -> io::Result<HashSet<String>>;

    /// When the computer last started. No program running now started before then.
    fn booted_at(&self) -> io::Result<SystemTime>;

    /// Whether the folder at `path` is a real folder, not a link, that only Administrators and
    /// SYSTEM, or root, can change. System cleanup keeps its journal and quarantine only in one.
    fn is_private_dir(&self, path: &Path) -> io::Result<bool>;

    /// Whether the file or folder at `path` is a real one, not a link, that only administrators
    /// or root can change, though others may read and run it. On Windows, SYSTEM,
    /// Administrators, or a service such as `NT SERVICE\TrustedInstaller` owns it, and its
    /// access list lets only them change it. On Unix, root owns it and no one else may write it.
    fn only_admins_can_change(&self, path: &Path) -> io::Result<bool>;
}

/// A program's process name for comparing: lowercase, without a trailing `.exe`, so `Claude.exe`
/// on Windows, `Claude` on macOS, and `claude` on Linux all read `claude`.
pub fn process_name(name: &str) -> String {
    let name = name.to_lowercase();
    match name.strip_suffix(".exe") {
        Some(stem) => stem.to_owned(),
        None => name,
    }
}

/// What the platform reports about one file or directory, read without following links.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileFacts {
    pub identity: FileIdentity,
    pub kind: EntryKind,
    /// How many hard links point at the same data, counting this one. With more than one,
    /// removing this path doesn't free the data.
    pub links: u64,
    /// Bytes allocated on disk.
    pub allocated: u64,
    /// Inside a folder that a provider syncs through the Windows Cloud Files API (OneDrive,
    /// iCloud, Dropbox, and others), whether or not this item's data is downloaded. Removing it
    /// removes it from the cloud and the user's other devices too. Always `false` on macOS and
    /// Linux, where synced folders have to be recognized by location instead.
    pub cloud_synced: bool,
    /// Some or all of the data isn't stored on this device, and reading it would download it.
    pub offline: bool,
    /// Who owns it, or `None` if that couldn't be read.
    pub owner: Option<FileOwner>,
}

/// Who owns a file or directory, as far as deciding whether Cleanse may remove it goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileOwner {
    /// The operating system: root on Unix, and on Windows `SYSTEM`, `LOCAL SERVICE`,
    /// `NETWORK SERVICE`, or a service's own account, such as `NT SERVICE\TrustedInstaller`.
    System,
    /// The Administrators group, which by default owns what an administrator's elevated programs
    /// create on Windows.
    Administrators,
    /// The account this process runs as.
    CurrentUser,
    /// The volume doesn't record owners, as FAT and exFAT drives don't.
    Unrecorded,
    /// Any other account.
    Other,
}

/// What a directory listing's metadata says about an entry, without extra system calls. Meant
/// for every entry of a walk; [`Probe::inspect`] is the thorough version for single items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListedFacts {
    /// Bytes allocated on disk. Windows listings don't include allocation, so there it's the
    /// logical length, or 0 for cloud files whose data isn't on this device.
    pub on_disk: u64,
    /// How many hard links point at the same data, counting this one, where the listing says.
    /// Unix listings do; Windows listings don't.
    pub links: Option<u64>,
    /// Some or all of the data isn't stored on this device.
    pub offline: bool,
}

/// Reads [`ListedFacts`] from metadata a walk already has.
pub fn listed_facts(metadata: &std::fs::Metadata) -> ListedFacts {
    os::listed_facts(metadata)
}

/// Standard per-user folders, following any redirection, such as OneDrive moving Documents.
/// Folders the system doesn't define are `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KnownFolders {
    pub home: Option<PathBuf>,
    pub desktop: Option<PathBuf>,
    pub documents: Option<PathBuf>,
    pub downloads: Option<PathBuf>,
    pub pictures: Option<PathBuf>,
    pub music: Option<PathBuf>,
    pub videos: Option<PathBuf>,
    /// `%LOCALAPPDATA%` on Windows, `~/Library/Caches` on macOS, `~/.cache` on Linux.
    pub cache: Option<PathBuf>,
    /// `%APPDATA%` on Windows, `~/Library/Application Support` on macOS, `~/.config` on Linux.
    pub config: Option<PathBuf>,
    /// `%LOCALAPPDATA%` on Windows, `~/Library/Application Support` on macOS, and
    /// `~/.local/share` on Linux.
    pub data_local: Option<PathBuf>,
    pub temp: PathBuf,
}

impl KnownFolders {
    /// The current user's folders.
    pub fn current() -> Self {
        Self {
            home: dirs::home_dir(),
            desktop: dirs::desktop_dir(),
            documents: dirs::document_dir(),
            downloads: dirs::download_dir(),
            pictures: dirs::picture_dir(),
            music: dirs::audio_dir(),
            videos: dirs::video_dir(),
            cache: dirs::cache_dir(),
            config: dirs::config_dir(),
            data_local: dirs::data_local_dir(),
            temp: os::temp_dir(),
        }
    }

    /// Where Cleanse keeps its own data, such as the journal of what it removed.
    pub fn cleanse_data(&self) -> Option<PathBuf> {
        self.data_local
            .as_ref()
            .map(|folder| folder.join("cleanse"))
    }
}

/// The folders the operating system keeps its own files in, where system cleanup works. Folders
/// an OS doesn't have are `None`, and macOS has none of them.
///
/// They come from the operating system, never from environment variables such as `%SystemRoot%`,
/// which a user can set for the elevated programs they start through `HKCU\Environment`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemFolders {
    /// The Windows folder, usually `C:\Windows`.
    pub system_root: Option<PathBuf>,
    /// Where Windows programs keep data for every user, usually `C:\ProgramData`.
    pub program_data: Option<PathBuf>,
    /// `/var` on Linux.
    pub var: Option<PathBuf>,
}

/// Where Windows installs programs for every user, usually `C:\Program Files`, as the registry
/// records it, which the environment doesn't change.
#[cfg(windows)]
pub fn program_files() -> io::Result<PathBuf> {
    os::program_files()
}

impl SystemFolders {
    pub fn current() -> io::Result<Self> {
        os::system_folders()
    }

    /// Where system cleanup keeps its journal and quarantine: `ProgramData\Cleanse` on Windows and
    /// `/var/lib/cleanse` on Linux.
    pub fn cleanse_data(&self) -> Option<PathBuf> {
        self.program_data
            .as_ref()
            .map(|folder| folder.join("Cleanse"))
            .or_else(|| {
                self.var
                    .as_ref()
                    .map(|folder| folder.join("lib").join("cleanse"))
            })
    }
}

/// The real platform.
#[derive(Debug)]
pub struct System {
    known_folders: KnownFolders,
    elevated: bool,
    user: os::User,
}

impl System {
    /// Reads the known folders, the process's elevation, and the account it runs as once, up
    /// front.
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            known_folders: KnownFolders::current(),
            elevated: os::is_elevated()?,
            user: os::current_user()?,
        })
    }
}

#[cfg(any(test, feature = "fake"))]
impl System {
    /// Overrides the elevation read at startup, for tests on CI runners, which run as an
    /// administrator on Windows.
    #[must_use]
    pub fn elevated(mut self, elevated: bool) -> Self {
        self.elevated = elevated;
        self
    }

    /// Replaces the known folders read at startup, so known locations resolve inside a test's
    /// own folder while files are still read for real.
    #[must_use]
    pub fn with_known_folders(mut self, known_folders: KnownFolders) -> Self {
        self.known_folders = known_folders;
        self
    }
}

impl Probe for System {
    fn inspect(&self, path: &Path) -> io::Result<FileFacts> {
        os::inspect(path, &self.user)
    }

    fn known_folders(&self) -> &KnownFolders {
        &self.known_folders
    }

    fn is_elevated(&self) -> bool {
        self.elevated
    }

    fn running_processes(&self) -> io::Result<HashSet<String>> {
        let mut system = sysinfo::System::new().map_err(io::Error::other)?;
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::All,
            true,
            sysinfo::ProcessRefreshKind::nothing(),
        );
        Ok(system
            .processes()
            .values()
            .map(|process| process_name(&process.name().to_string_lossy()))
            .collect())
    }

    fn booted_at(&self) -> io::Result<SystemTime> {
        let seconds = sysinfo::System::boot_time().map_err(io::Error::other)?;
        Ok(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
    }

    fn is_private_dir(&self, path: &Path) -> io::Result<bool> {
        os::is_private_dir(path)
    }

    fn only_admins_can_change(&self, path: &Path) -> io::Result<bool> {
        os::only_admins_can_change(path)
    }
}
