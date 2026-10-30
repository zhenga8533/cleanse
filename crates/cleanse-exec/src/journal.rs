//! The journal: one append-only file per run, flushed to disk before each removal starts and
//! after it ends, so an interrupted run still shows what was in progress.

#![allow(
    clippy::disallowed_methods,
    reason = "cleanse-exec is the one crate that changes the filesystem (ADR 0002)"
)]

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use cleanse_model::{FileIdentity, Outcome, Plan, RuleId};
use serde::Serialize;

pub(crate) struct Journal {
    file: File,
    path: PathBuf,
}

/// One line of the journal. Times are seconds since the Unix epoch.
#[derive(Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(crate) enum Record {
    Started {
        at: u64,
        version: &'static str,
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
    Done {
        path: String,
        outcome: Outcome,
    },
    Finished {
        at: u64,
    },
}

impl Journal {
    /// Creates a new journal file in `dir` and records the start of the run.
    pub(crate) fn start(dir: &Path, plan: &Plan) -> io::Result<Self> {
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
            version: env!("CARGO_PKG_VERSION"),
            roots: plan
                .roots
                .iter()
                .map(|root| root.to_string_lossy().into_owned())
                .collect(),
            items: plan.items.len(),
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

fn since_epoch() -> std::time::Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}
