//! The journal: one append-only file per run, flushed to disk before each removal starts and
//! after it ends, so an interrupted run still shows what was in progress.

#![allow(
    clippy::disallowed_methods,
    reason = "cleanse-exec is the one crate that changes the filesystem (ADR 0002)"
)]

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use cleanse_model::{EntryOutcome, FileIdentity, Outcome, RuleId};
use serde::{Deserialize, Serialize};

pub(crate) struct Journal {
    file: File,
    path: PathBuf,
}

/// What a run did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Removed approved items: `cleanse clean`.
    Apply,
    /// Put quarantined items back.
    Restore,
    /// Deleted quarantined items for good.
    Purge,
    /// Removed what was left of removals that didn't finish.
    Finish,
}

/// One line of the journal. Times are seconds since the Unix epoch.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(crate) enum Record {
    Started {
        at: u64,
        version: String,
        action: Action,
        roots: Vec<String>,
        items: usize,
    },
    /// Written before the item is renamed to `tombstone`.
    Removing {
        path: String,
        tombstone: String,
        rule: RuleId,
        identity: FileIdentity,
    },
    /// Written before the item is moved into quarantine as `entry`.
    Quarantining {
        path: String,
        entry: String,
        rule: RuleId,
        identity: FileIdentity,
    },
    Done {
        path: String,
        outcome: Outcome,
    },
    /// Written before a quarantined item is moved back to `path`.
    Restoring {
        entry: String,
        path: String,
    },
    /// Written before a quarantined item, which was at `path`, is deleted.
    Purging {
        entry: String,
        path: String,
    },
    EntryDone {
        entry: String,
        outcome: EntryOutcome,
    },
    Finished {
        at: u64,
    },
}

impl Journal {
    /// Creates a new journal file in `dir` and records the start of a run that does `action`
    /// to `items` items found under `roots`.
    pub(crate) fn start(
        dir: &Path,
        action: Action,
        roots: &[PathBuf],
        items: usize,
    ) -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        fs::create_dir_all(dir)?;
        let millis = since_epoch().as_millis();
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("{millis}-{}-{sequence}.jsonl", process::id()));
        let mut options = OpenOptions::new();
        options.append(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut journal = Self {
            file: options.open(&path)?,
            path,
        };
        journal.record(&Record::Started {
            at: since_epoch().as_secs(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            action,
            roots: roots
                .iter()
                .map(|root| root.to_string_lossy().into_owned())
                .collect(),
            items,
        })?;
        Ok(journal)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Appends `record` as one line and waits until it's on disk.
    pub(crate) fn record(&mut self, record: &Record) -> io::Result<()> {
        let mut line = serde_json::to_vec(record).map_err(io::Error::other)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        self.file.sync_data()
    }

    pub(crate) fn finish(mut self) -> io::Result<()> {
        self.record(&Record::Finished {
            at: since_epoch().as_secs(),
        })
    }
}

/// The journal files in `dir`, oldest first. Their names start with the time the run began.
pub(crate) fn files(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = match fs::read_dir(dir) {
        Ok(entries) => entries
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<io::Result<Vec<_>>>()?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    files.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "jsonl")
    });
    files.sort_by_key(|path| started_millis(path));
    Ok(files)
}

fn started_millis(path: &Path) -> u128 {
    path.file_stem()
        .and_then(|stem| stem.to_str()?.split('-').next()?.parse().ok())
        .unwrap_or(0)
}

/// Reads a journal's records, stopping at the first line that can't be read, such as one a crash
/// cut short. `None` if the file has no readable start.
pub(crate) fn read(path: &Path) -> io::Result<Option<Vec<Record>>> {
    let mut records = Vec::new();
    for line in BufReader::new(File::open(path)?).lines() {
        match serde_json::from_str(&line?) {
            Ok(record) => records.push(record),
            Err(_) => break,
        }
    }
    Ok(matches!(records.first(), Some(Record::Started { .. })).then_some(records))
}

/// Seconds since the Unix epoch.
pub(crate) fn now() -> u64 {
    since_epoch().as_secs()
}

fn since_epoch() -> std::time::Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}
