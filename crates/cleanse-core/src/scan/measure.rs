use std::fs::{self, Metadata};
use std::io;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use cleanse_model::{EntryKind, Size};
use cleanse_platform::listed_facts;

use super::{Descend, walk};

/// File names that suggest credentials or keys. An ambiguous build folder holding one may be
/// more than build output. Compared case-insensitively; a leading `*` matches any start.
const SECRET_NAMES: &[&str] = &[
    ".env",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "*.kdbx",
    "id_rsa",
    "id_ecdsa",
    "id_ed25519",
    "credentials",
    "credentials.json",
    "*.sqlite",
    "*.sqlite3",
    "*.db",
];

/// Virtual machine and container disks: losing one loses an entire system, so they're never
/// part of a cleanup no matter where they turn up.
const VM_DISK_EXTENSIONS: &[&str] = &["vhd", "vhdx", "vmdk", "qcow2", "vdi"];

/// pnpm's virtual store. On Windows the listing can't show hard links, and pnpm hard-links
/// the files in here to its global store, so removing them frees little.
const PNPM_STORE: &str = ".pnpm";

/// Everything the safety checks need to know about a file, or a directory's contents from one
/// walk.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Measurement {
    pub size: Size,
    pub files: u64,
    /// When the most recently changed file was modified.
    pub newest_modified: Option<SystemTime>,
    /// Entries inside that couldn't be read.
    pub unreadable: usize,
    /// Directories on other filesystems mounted inside, which a removal could reach.
    pub mounts: usize,
    /// The name of a file that looks like it holds secrets, if any.
    pub secret: Option<String>,
    /// The name of a virtual machine disk, if any.
    pub vm_disk: Option<String>,
    /// Some of the data is shared with files elsewhere through hard links.
    pub shared: bool,
}

/// Walks everything below `path` without following links, or measures it alone if it's a file.
pub fn measure(path: &Path) -> io::Result<Measurement> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_file() {
        let mut measurement = Measurement::default();
        measurement.add(path, &metadata, false);
        return Ok(measurement);
    }

    let measurement = Mutex::new(Measurement::default());
    let report = walk(path, |entry| {
        if entry.kind != EntryKind::File {
            return Descend::Yes;
        }
        let in_store = cfg!(windows) && in_pnpm_store(entry.path, path);
        measurement
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .add(entry.path, entry.metadata, in_store);
        Descend::Yes
    })?;

    let mut measurement = measurement
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner);
    measurement.unreadable = report.errors.len();
    measurement.mounts = report.other_filesystems.len();
    Ok(measurement)
}

impl Measurement {
    /// Counts one file, which is `hard_linked` even if its listing doesn't say so.
    fn add(&mut self, path: &Path, metadata: &Metadata, hard_linked: bool) {
        let listed = listed_facts(metadata);
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        self.files += 1;
        self.size.logical += metadata.len();
        self.size.on_disk += listed.on_disk;
        if hard_linked || listed.links.is_some_and(|links| links > 1) {
            self.shared = true;
        } else {
            self.size.guaranteed += listed.on_disk;
        }
        if let Ok(modified) = metadata.modified() {
            self.newest_modified = self.newest_modified.max(Some(modified));
        }
        if self.secret.is_none() && is_secret_name(&name) {
            self.secret = Some(name.clone());
        }
        if self.vm_disk.is_none() && is_vm_disk(&name) {
            self.vm_disk = Some(name);
        }
    }
}

fn is_secret_name(name: &str) -> bool {
    SECRET_NAMES
        .iter()
        .any(|pattern| match pattern.strip_prefix('*') {
            Some(suffix) => name.ends_with(suffix),
            // `.env.local`, `.env.production`, and so on hold the same kind of values as `.env`.
            None => name == *pattern || (*pattern == ".env" && name.starts_with(".env.")),
        })
}

fn is_vm_disk(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| VM_DISK_EXTENSIONS.contains(&extension))
}

fn in_pnpm_store(path: &Path, dir: &Path) -> bool {
    path.strip_prefix(dir).is_ok_and(|inside| {
        inside
            .components()
            .any(|part| part.as_os_str() == PNPM_STORE)
    })
}
