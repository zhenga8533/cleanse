use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use crate::{FileFacts, KnownFolders, Probe, process_name};

/// A [`Probe`] that answers only what a test has told it.
#[derive(Debug, Default)]
pub struct FakeProbe {
    files: HashMap<PathBuf, FileFacts>,
    known_folders: KnownFolders,
    elevated: bool,
    running: HashSet<String>,
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
}
