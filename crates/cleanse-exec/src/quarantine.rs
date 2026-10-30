//! Quarantine: approved folders moved into Cleanse's data folder on the same drive, each beside
//! a record of where it came from, so it can be put back until it's purged.

#![allow(
    clippy::disallowed_methods,
    reason = "cleanse-exec is the one crate that changes the filesystem (ADR 0002)"
)]

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use cleanse_model::{EntryKind, EntryOutcome, FileIdentity, Outcome, QuarantineEntry, SkipReason};
use cleanse_platform::Probe;

use crate::delete;

/// The extension of the record saved beside each quarantined folder.
const RECORD: &str = "json";

/// What's in quarantine.
#[derive(Debug, Default)]
pub struct Listing {
    /// Oldest first.
    pub entries: Vec<QuarantineEntry>,
    /// Records that couldn't be read, so whatever they describe isn't listed.
    pub unreadable: Vec<PathBuf>,
}

/// The `quarantine` folder in Cleanse's data folder. Each entry is a folder named by its ID,
/// with `<id>.json` beside it.
pub(crate) struct Quarantine {
    dir: PathBuf,
}

impl Quarantine {
    pub(crate) fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("quarantine"),
        }
    }

    /// An ID no other entry uses.
    pub(crate) fn new_id() -> String {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis());
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        format!("{millis}-{}-{sequence}", process::id())
    }

    pub(crate) fn list(&self) -> io::Result<Listing> {
        let mut listing = Listing::default();
        let records = match fs::read_dir(&self.dir) {
            Ok(records) => records,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(listing),
            Err(error) => return Err(error),
        };
        for record in records {
            let path = record?.path();
            if path.extension().is_none_or(|extension| extension != RECORD) {
                continue;
            }
            let entry = fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<QuarantineEntry>(&bytes).ok())
                // The ID names the folder that restoring or purging acts on, so it has to be one
                // Cleanse made, and the record's own.
                .filter(|entry| {
                    is_id(&entry.id)
                        && path.file_stem().is_some_and(|stem| stem == entry.id.as_str())
                });
            match entry {
                // A record without its folder is left over from a restore or purge that couldn't
                // remove it, and describes nothing.
                Some(entry) if fs::symlink_metadata(self.folder(&entry.id)).is_err() => {}
                Some(entry) => listing.entries.push(entry),
                None => listing.unreadable.push(path),
            }
        }
        listing
            .entries
            .sort_by(|a, b| (a.at, &a.id).cmp(&(b.at, &b.id)));
        Ok(listing)
    }

    /// Moves the folder at `entry.original`, approved as `identity`, into quarantine.
    pub(crate) fn admit(
        &self,
        probe: &dyn Probe,
        entry: &QuarantineEntry,
        identity: FileIdentity,
    ) -> Outcome {
        let failed = |what: &str, error: io::Error| Outcome::Failed {
            error: format!("{what}: {error}"),
        };
        if let Err(error) = fs::create_dir_all(&self.dir) {
            return failed("Couldn't create the quarantine folder", error);
        }
        // Moving within a drive is a rename. Across drives it would be a copy, which could fail
        // halfway and doubles the space used while it runs.
        match probe.inspect(&self.dir) {
            Ok(facts) if facts.identity.volume == identity.volume => {}
            Ok(_) => {
                return Outcome::Skipped {
                    reason: SkipReason::OtherDrive,
                };
            }
            Err(error) => return failed("Couldn't check the quarantine folder", error),
        }

        // Written first, so a folder in quarantine always has a record of where it came from.
        let record = self.record(&entry.id);
        if let Err(error) = write_record(&record, entry) {
            return failed("Couldn't save where it came from", error);
        }
        let folder = self.folder(&entry.id);
        // On Windows this fails while a program has a file inside open.
        if let Err(error) = fs::rename(&entry.original, &folder) {
            // A record without its folder isn't listed, so a leftover one changes nothing.
            let _ = fs::remove_file(&record);
            return failed("Couldn't move it into quarantine", error);
        }

        // If something replaced the folder after it was checked, that's what moved, so it goes
        // back. If it can't, it stays listed in quarantine, where it can be restored.
        let moved = probe.inspect(&folder);
        if !moved.is_ok_and(|facts| facts.kind == EntryKind::Dir && facts.identity == identity) {
            let outcome = delete::put_back(&entry.original, &folder);
            if matches!(outcome, Outcome::Skipped { .. }) {
                let _ = fs::remove_file(&record);
            }
            return outcome;
        }
        Outcome::Quarantined {
            size: entry.size,
            entry: entry.id.clone(),
        }
    }

    /// Moves an entry back to where it was, unless something is there now.
    pub(crate) fn restore(&self, entry: &QuarantineEntry) -> EntryOutcome {
        let original = &entry.original;
        if fs::symlink_metadata(original).is_ok() {
            return failed(format!("Something is at {} now", original.display()));
        }
        // Moving into a link would put it wherever the link points.
        if let Some(parent) = original.parent()
            && !fs::symlink_metadata(parent).is_ok_and(|metadata| metadata.is_dir())
        {
            return failed(format!("{} isn't a folder anymore", parent.display()));
        }
        if let Err(error) = fs::rename(self.folder(&entry.id), original) {
            return failed(format!("Couldn't move it back: {error}"));
        }
        // A record without its folder isn't listed, so a leftover one changes nothing.
        let _ = fs::remove_file(self.record(&entry.id));
        EntryOutcome::Restored
    }

    /// Deletes an entry for good.
    pub(crate) fn purge(&self, entry: &QuarantineEntry) -> EntryOutcome {
        let folder = self.folder(&entry.id);
        // Cleanse only ever quarantines folders, so anything else here isn't its own.
        if !fs::symlink_metadata(&folder).is_ok_and(|metadata| metadata.is_dir()) {
            return failed(format!(
                "{} isn't a folder Cleanse quarantined",
                folder.display()
            ));
        }
        if let Err(error) = fs::remove_dir_all(&folder) {
            return failed(format!(
                "{error}. What's left of it is in {}",
                folder.display()
            ));
        }
        let _ = fs::remove_file(self.record(&entry.id));
        EntryOutcome::Purged { size: entry.size }
    }

    fn folder(&self, id: &str) -> PathBuf {
        self.dir.join(id)
    }

    fn record(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.{RECORD}"))
    }
}

/// Whether `id` looks like one [`Quarantine::new_id`] makes: digits and dashes, so it can only
/// name a folder directly inside the quarantine.
fn is_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit() || byte == b'-')
}

fn write_record(path: &Path, entry: &QuarantineEntry) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    file.write_all(&serde_json::to_vec_pretty(entry).map_err(io::Error::other)?)?;
    file.sync_all()
}

fn failed(error: String) -> EntryOutcome {
    EntryOutcome::Failed { error }
}
