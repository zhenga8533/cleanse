use std::fs::{self, Metadata};
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use cleanse_model::{Concern, EntryKind, Size};
use cleanse_platform::listed_facts;

use super::{Descend, Entry, walk};

/// File names that suggest credentials, keys, or databases. An ambiguous build folder holding
/// one may be more than build output, and a large file named like one may be someone's only
/// copy. Compared case-insensitively; a leading `*` matches any start.
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
    // Outlook's own copy of mail, which can be the only one.
    "*.pst",
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
    /// When anything last changed: a file's contents, or the entries of a folder, counting the
    /// item itself. A file unpacked from an archive keeps the archive's modified time, but its
    /// folder still shows when it arrived. When files were last opened isn't counted: scanners
    /// open files, and especially programs, without anything needing them.
    pub newest_change: Option<SystemTime>,
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
    /// What [`measure_with`]'s check found wrong with entries inside, each once.
    pub inside: Vec<Concern>,
}

/// Walks everything below `path` without following links, or measures it alone if it's a file.
pub fn measure(path: &Path) -> io::Result<Measurement> {
    measure_with(path, |_| None, None)
}

/// Like [`measure`], and also asks `check` about each entry inside, from many threads at once,
/// recording what it says is wrong. Once `stop` is set, it goes no deeper, so what it returns
/// leaves out whatever it didn't reach.
pub fn measure_with<F>(path: &Path, check: F, stop: Option<&AtomicBool>) -> io::Result<Measurement>
where
    F: Fn(&Entry<'_>) -> Option<Concern> + Sync,
{
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_file() {
        let mut measurement = Measurement::default();
        measurement.add(path, &metadata, false);
        return Ok(measurement);
    }

    let mut started = Measurement::default();
    started.changed(metadata.modified().ok());
    let measurement = Mutex::new(started);
    let report = walk(path, |entry| {
        let concern = check(entry);
        let mut measurement = measurement.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(concern) = concern
            && !measurement.inside.contains(&concern)
        {
            measurement.inside.push(concern);
        }
        match entry.kind {
            EntryKind::File => {
                let in_store = cfg!(windows) && in_pnpm_store(entry.path, path);
                measurement.add(entry.path, entry.metadata, in_store);
            }
            EntryKind::Dir => measurement.changed(entry.metadata.modified().ok()),
            EntryKind::Link | EntryKind::Other => {}
        }
        if stop.is_some_and(|stop| stop.load(Ordering::Relaxed)) {
            Descend::No
        } else {
            Descend::Yes
        }
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
        self.changed(metadata.modified().ok());
        if self.secret.is_none() && is_secret_name(&name) {
            self.secret = Some(name.clone());
        }
        if self.vm_disk.is_none() && is_vm_disk(&name) {
            self.vm_disk = Some(name);
        }
    }

    /// Notes a time something changed, if the system records it.
    fn changed(&mut self, time: Option<SystemTime>) {
        self.newest_change = self.newest_change.max(time);
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
