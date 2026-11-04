use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::{FileFacts, FileOwner, KnownFolders, Probe, System, process_name};

/// A [`Probe`] that answers only what a test has told it.
#[derive(Debug, Default)]
pub struct FakeProbe {
    files: HashMap<PathBuf, FileFacts>,
    known_folders: KnownFolders,
    elevated: bool,
    running: HashSet<String>,
    booted_at: Option<SystemTime>,
}

impl FakeProbe {
    pub fn new(known_folders: KnownFolders) -> Self {
        Self {
            known_folders,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn with_file(mut self, path: impl Into<PathBuf>, facts: FileFacts) -> Self {
        self.files.insert(path.into(), facts);
        self
    }

    #[must_use]
    pub fn elevated(mut self, elevated: bool) -> Self {
        self.elevated = elevated;
        self
    }

    /// Reports a process with this name as running.
    #[must_use]
    pub fn with_running(mut self, name: &str) -> Self {
        self.running.insert(process_name(name));
        self
    }

    /// Reports the computer as started at `time`.
    #[must_use]
    pub fn with_boot_time(mut self, time: SystemTime) -> Self {
        self.booted_at = Some(time);
        self
    }
}

impl Probe for FakeProbe {
    /// Paths the test didn't set up are reported as missing.
    fn inspect(&self, path: &Path) -> io::Result<FileFacts> {
        self.files.get(path).copied().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("FakeProbe has no facts for {}", path.display()),
            )
        })
    }

    fn known_folders(&self) -> &KnownFolders {
        &self.known_folders
    }

    fn is_elevated(&self) -> bool {
        self.elevated
    }

    fn running_processes(&self) -> io::Result<HashSet<String>> {
        Ok(self.running.clone())
    }

    /// Unknown unless the test said when the computer started.
    fn booted_at(&self) -> io::Result<SystemTime> {
        self.booted_at
            .ok_or_else(|| io::Error::other("FakeProbe wasn't told when the computer started"))
    }

    fn is_private_dir(&self, _: &Path) -> io::Result<bool> {
        Ok(false)
    }
}

/// The real filesystem, as an elevated process sees it when the system owns everything there,
/// for testing system cleanup on fixtures. Fixtures only really belong to the system when tests
/// run as an administrator or root, which CI's Linux runners don't.
///
/// Every owner reads as the system's unless the test gave it another, folders read as private
/// only when the test says, no programs are running, and the computer started when this was made.
#[derive(Debug)]
pub struct StagedSystem {
    system: System,
    owners: HashMap<PathBuf, Option<FileOwner>>,
    private: HashSet<PathBuf>,
    elevated: bool,
    booted_at: SystemTime,
}

impl StagedSystem {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            system: System::new()?,
            owners: HashMap::new(),
            private: HashSet::new(),
            elevated: true,
            booted_at: SystemTime::now(),
        })
    }

    /// Reports `path` as owned by `owner`, or as having an owner that can't be read.
    #[must_use]
    pub fn owned(mut self, path: &Path, owner: Option<FileOwner>) -> Self {
        self.owners.insert(path.to_path_buf(), owner);
        self
    }

    /// Reports the folder at `path` as private, if it's a real folder.
    #[must_use]
    pub fn private(mut self, path: &Path) -> Self {
        self.private.insert(path.to_path_buf());
        self
    }

    #[must_use]
    pub fn elevated(mut self, elevated: bool) -> Self {
        self.elevated = elevated;
        self
    }
}

impl Probe for StagedSystem {
    fn inspect(&self, path: &Path) -> io::Result<FileFacts> {
        let mut facts = self.system.inspect(path)?;
        facts.owner = self
            .owners
            .get(path)
            .copied()
            .unwrap_or(Some(FileOwner::System));
        Ok(facts)
    }

    fn known_folders(&self) -> &KnownFolders {
        self.system.known_folders()
    }

    fn is_elevated(&self) -> bool {
        self.elevated
    }

    fn running_processes(&self) -> io::Result<HashSet<String>> {
        Ok(HashSet::new())
    }

    fn booted_at(&self) -> io::Result<SystemTime> {
        Ok(self.booted_at)
    }

    fn is_private_dir(&self, path: &Path) -> io::Result<bool> {
        let metadata = std::fs::symlink_metadata(path)?;
        Ok(metadata.is_dir() && self.private.contains(path))
    }
}
