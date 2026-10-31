//! Past runs, read back from the journal, and removals that didn't finish.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use cleanse_model::{EntryKind, EntryOutcome, FileIdentity, Outcome, RuleId};
use cleanse_platform::Probe;

use crate::delete::TOMBSTONE_PREFIX;
use crate::journal::{self, Action, Record};

/// What the journal says Cleanse has done.
#[derive(Debug, Default)]
pub struct History {
    /// Oldest first.
    pub runs: Vec<Run>,
    /// Removals that started but whose folder is still on disk, under its tombstone name.
    pub leftovers: Vec<Leftover>,
    /// Journal files that couldn't be read.
    pub unreadable: Vec<PathBuf>,
}

/// One run that changed something.
#[derive(Debug)]
pub struct Run {
    pub journal: PathBuf,
    /// When it started, in seconds since the Unix epoch.
    pub at: u64,
    pub action: Action,
    /// How many items it set out to handle.
    pub items: usize,
    /// What happened to each item it reached, for runs that removed items.
    pub outcomes: Vec<Outcome>,
    /// What happened to each entry it reached, for runs that restored or purged.
    pub entry_outcomes: Vec<EntryOutcome>,
    /// It recorded its end. Otherwise it was cut short, such as by a crash.
    pub finished: bool,
}

/// What's left of a removal that didn't finish: the folder, renamed to its tombstone and
/// perhaps partly deleted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leftover {
    /// Where the folder was before it was renamed.
    pub original: PathBuf,
    pub tombstone: PathBuf,
    pub rule: RuleId,
    /// The approved folder's identity. What's at the tombstone is only removed if it matches.
    pub identity: FileIdentity,
}

pub(crate) fn read(journal_dir: &Path, probe: &dyn Probe) -> io::Result<History> {
    let mut history = History::default();
    let mut started = Vec::new();
    for file in journal::files(journal_dir)? {
        let Ok(Some(records)) = journal::read(&file) else {
            history.unreadable.push(file);
            continue;
        };
        let mut run = None;
        for record in records {
            match record {
                Record::Started {
                    at, action, items, ..
                } => {
                    run = Some(Run {
                        journal: file.clone(),
                        at,
                        action,
                        items,
                        outcomes: Vec::new(),
                        entry_outcomes: Vec::new(),
                        finished: false,
                    });
                }
                Record::Removing {
                    path,
                    tombstone,
                    rule,
                    identity,
                } => started.push(Leftover {
                    original: path.into(),
                    tombstone: tombstone.into(),
                    rule,
                    identity,
                }),
                Record::Done { outcome, .. } => {
                    if let Some(run) = &mut run {
                        run.outcomes.push(outcome);
                    }
                }
                Record::EntryDone { outcome, .. } => {
                    if let Some(run) = &mut run {
                        run.entry_outcomes.push(outcome);
                    }
                }
                Record::Finished { .. } => {
                    if let Some(run) = &mut run {
                        run.finished = true;
                    }
                }
                Record::Quarantining { .. } | Record::Restoring { .. } | Record::Purging { .. } => {
                }
            }
        }
        history.runs.extend(run);
    }

    // Every removal renames the folder to its tombstone first, so a tombstone still holding the
    // approved folder is what's left of a removal that didn't finish, whatever its journal says.
    // Anything else there was swapped in, isn't Cleanse's, and is left alone.
    let mut seen = HashSet::new();
    history.leftovers = started
        .into_iter()
        .filter(|leftover| seen.insert(leftover.tombstone.clone()) && still_there(probe, leftover))
        .collect();
    Ok(history)
}

/// Whether the leftover's tombstone still holds the folder that was approved. The tombstone must
/// also look like one Cleanse made, beside the original, so a damaged journal can't point
/// anywhere else.
pub(crate) fn still_there(probe: &dyn Probe, leftover: &Leftover) -> bool {
    let named_as_made = leftover
        .tombstone
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(TOMBSTONE_PREFIX))
        && leftover.tombstone.parent() == leftover.original.parent();
    named_as_made
        && probe
            .inspect(&leftover.tombstone)
            .is_ok_and(|facts| facts.kind == EntryKind::Dir && facts.identity == leftover.identity)
}
