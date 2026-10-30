//! The only crate permitted to change the filesystem.
//!
//! Executes an approved plan item by item, re-validating each item through the safety gates
//! immediately before acting, and records every outcome in an append-only journal.

mod delete;
mod history;
mod journal;
mod quarantine;

#[cfg(test)]
mod tests;

use std::error::Error;
use std::fmt;
use std::io;
use std::mem;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use cleanse_core::rules::{Matcher, Os, RuleSet};
use cleanse_core::safety::{Assessor, ProtectedPolicy};
use cleanse_core::scan::measure;
use cleanse_model::{
    Candidate, CandidateId, Concern, Disposal, EntryKind, EntryOutcome, Outcome, Plan, PlanItem,
    QuarantineEntry, Risk, SkipReason,
};
use cleanse_platform::Probe;

use journal::{Journal, Record};
use quarantine::Quarantine;

pub use history::{History, Leftover, Run};
pub use journal::Action;
pub use quarantine::Listing;

/// Removes approved items, checking each one again first, and looks after the quarantine.
pub struct Executor<'a> {
    rules: &'a RuleSet,
    probe: &'a dyn Probe,
    excluded: &'a [PathBuf],
    data_dir: PathBuf,
}

/// What applying a plan did.
#[derive(Debug)]
pub struct Applied {
    /// The journal file recording this run.
    pub journal: PathBuf,
    /// One per plan item, in the plan's order.
    pub outcomes: Vec<Outcome>,
}

/// What restoring or purging quarantined items did.
#[derive(Debug)]
pub struct Handled {
    /// The journal file recording this run.
    pub journal: PathBuf,
    /// One per entry, in the order given.
    pub outcomes: Vec<EntryOutcome>,
}

/// Why a run couldn't start at all. Nothing was changed.
#[derive(Debug)]
pub enum ApplyError {
    /// Elevation would let Cleanse remove files the user's own account couldn't, such as other
    /// users' files, so it refuses.
    Elevated,
    /// The journal couldn't be started.
    Journal(io::Error),
    /// The safety checks couldn't be set up.
    Io(io::Error),
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Elevated => f.write_str(
                "Cleanse doesn't change anything while running as an administrator or root, \
                 because that could remove files your own account can't. Run it again \
                 without elevation.",
            ),
            Self::Journal(error) => {
                write!(
                    f,
                    "couldn't start the journal, so nothing was changed: {error}"
                )
            }
            Self::Io(error) => write!(
                f,
                "couldn't set up the safety checks, so nothing was changed: {error}"
            ),
        }
    }
}

impl Error for ApplyError {}

impl<'a> Executor<'a> {
    /// `excluded` are the folders the user's settings exclude. `data_dir` is Cleanse's data
    /// folder: each run writes its journal to a new file in its `journal` folder, and
    /// quarantined items go in its `quarantine` folder.
    pub fn new(
        rules: &'a RuleSet,
        probe: &'a dyn Probe,
        excluded: &'a [PathBuf],
        data_dir: PathBuf,
    ) -> Self {
        Self {
            rules,
            probe,
            excluded,
            data_dir,
        }
    }

    /// Refuses while elevated, then starts the journal for a run.
    fn start(
        &self,
        action: Action,
        roots: &[PathBuf],
        items: usize,
    ) -> Result<Journal, ApplyError> {
        if self.probe.is_elevated() {
            return Err(ApplyError::Elevated);
        }
        Journal::start(&self.data_dir.join("journal"), action, roots, items)
            .map_err(ApplyError::Journal)
    }

    fn quarantine(&self) -> Quarantine {
        Quarantine::new(&self.data_dir)
    }

    /// Removes each item in `plan` that is still what was approved and still passes every
    /// safety check, in order. `progress` gets the number of items done so far. Setting
    /// `cancel` skips the items not yet reached.
    pub fn apply(
        &self,
        plan: &Plan,
        progress: &dyn Fn(usize),
        cancel: &AtomicBool,
    ) -> Result<Applied, ApplyError> {
        if self.probe.is_elevated() {
            return Err(ApplyError::Elevated);
        }
        let folders = self.probe.known_folders();
        let matcher = Matcher::new(self.rules.rules(), folders, Os::current());
        let policy = ProtectedPolicy::for_system(folders, self.probe, self.excluded);
        // Files modified after this count as modified moments ago, however long the run takes.
        let assessor = Assessor::new(self.probe, policy, &plan.roots, SystemTime::now())
            .map_err(ApplyError::Io)?;
        let mut journal = self.start(Action::Apply, &plan.roots, plan.items.len())?;

        let mut journal_ok = true;
        let mut outcomes = Vec::with_capacity(plan.items.len());
        for item in &plan.items {
            let outcome = if !journal_ok {
                skipped(SkipReason::JournalUnavailable)
            } else if cancel.load(Ordering::Relaxed) {
                skipped(SkipReason::Cancelled)
            } else if let Ok(outcome) = self.remove(item, &matcher, &assessor, &mut journal) {
                outcome
            } else {
                // The journal failed before anything was touched.
                journal_ok = false;
                skipped(SkipReason::JournalUnavailable)
            };
            if journal_ok && let Some(path) = item.path.to_str() {
                let done = Record::Done {
                    path: path.to_owned(),
                    outcome: outcome.clone(),
                };
                journal_ok = journal.record(&done).is_ok();
            }
            outcomes.push(outcome);
            progress(outcomes.len());
        }
        let journal_path = journal.path().to_owned();
        if journal_ok {
            // Every outcome is already on disk, so a missing end marker loses nothing.
            let _ = journal.finish();
        }

        Ok(Applied {
            journal: journal_path,
            outcomes,
        })
    }

    /// Fails only if the journal can't record the removal, in which case nothing was touched.
    fn remove(
        &self,
        item: &PlanItem,
        matcher: &Matcher<'_>,
        assessor: &Assessor<'_>,
        journal: &mut Journal,
    ) -> io::Result<Outcome> {
        if item.disposal == Disposal::Trash {
            return Ok(skipped(SkipReason::Unsupported {
                disposal: item.disposal,
            }));
        }
        let Some(path) = item.path.to_str() else {
            return Ok(skipped(SkipReason::UnrecordablePath));
        };
        let candidate = match self.check(item, matcher, assessor) {
            Ok(candidate) => candidate,
            Err(outcome) => return Ok(outcome),
        };

        if item.disposal == Disposal::Quarantine {
            let entry = QuarantineEntry {
                id: Quarantine::new_id(),
                original: item.path.clone(),
                rule: item.rule.clone(),
                size: candidate.found.size,
                at: journal::now(),
            };
            journal.record(&Record::Quarantining {
                path: path.to_owned(),
                entry: entry.id.clone(),
                rule: item.rule.clone(),
                identity: item.identity,
            })?;
            return Ok(self.quarantine().admit(self.probe, &entry, item.identity));
        }

        // The protected policy blocks drive roots, so anything that passed the check has a
        // parent folder to hold the tombstone.
        let Some(tombstone) = delete::tombstone_for(&item.path) else {
            return Ok(Outcome::Failed {
                error: "It has no parent folder".into(),
            });
        };

        journal.record(&Record::Removing {
            path: path.to_owned(),
            tombstone: tombstone.to_string_lossy().into_owned(),
            rule: item.rule.clone(),
            identity: item.identity,
        })?;
        Ok(delete::delete(
            self.probe,
            &item.path,
            &tombstone,
            item.identity,
            candidate.found.size,
        ))
    }

    /// What the journal says Cleanse has done, and what's left of removals that didn't finish.
    pub fn history(&self) -> io::Result<History> {
        history::read(&self.data_dir.join("journal"), self.probe)
    }

    /// Removes what's left of removals that didn't finish, each only if its tombstone still
    /// holds the folder that was approved.
    pub fn finish(&self, leftovers: &[Leftover]) -> Result<Applied, ApplyError> {
        let mut journal = self.start(Action::Finish, &[], leftovers.len())?;
        let mut journal_ok = true;
        let mut outcomes = Vec::with_capacity(leftovers.len());
        for leftover in leftovers {
            let removing = Record::Removing {
                path: leftover.original.to_string_lossy().into_owned(),
                tombstone: leftover.tombstone.to_string_lossy().into_owned(),
                rule: leftover.rule.clone(),
                identity: leftover.identity,
            };
            let outcome = if !journal_ok {
                skipped(SkipReason::JournalUnavailable)
            } else if !history::still_there(self.probe, leftover) {
                skipped(SkipReason::Gone)
            } else if journal.record(&removing).is_err() {
                journal_ok = false;
                skipped(SkipReason::JournalUnavailable)
            } else {
                let size = measure(&leftover.tombstone)
                    .map(|measurement| measurement.size)
                    .unwrap_or_default();
                delete::finish(&leftover.tombstone, size)
            };
            if journal_ok {
                let done = Record::Done {
                    path: leftover.original.to_string_lossy().into_owned(),
                    outcome: outcome.clone(),
                };
                journal_ok = journal.record(&done).is_ok();
            }
            outcomes.push(outcome);
        }
        let journal_path = journal.path().to_owned();
        if journal_ok {
            // Every outcome is already on disk, so a missing end marker loses nothing.
            let _ = journal.finish();
        }
        Ok(Applied {
            journal: journal_path,
            outcomes,
        })
    }

    /// What's in quarantine.
    pub fn quarantined(&self) -> io::Result<Listing> {
        self.quarantine().list()
    }

    /// Moves each entry back to where it was, unless something is there now.
    pub fn restore(&self, entries: &[QuarantineEntry]) -> Result<Handled, ApplyError> {
        self.each_entry(Action::Restore, entries, Quarantine::restore)
    }

    /// Deletes each entry for good.
    pub fn purge(&self, entries: &[QuarantineEntry]) -> Result<Handled, ApplyError> {
        self.each_entry(Action::Purge, entries, Quarantine::purge)
    }

    fn each_entry(
        &self,
        action: Action,
        entries: &[QuarantineEntry],
        handle: fn(&Quarantine, &QuarantineEntry) -> EntryOutcome,
    ) -> Result<Handled, ApplyError> {
        let mut journal = self.start(action, &[], entries.len())?;
        let quarantine = self.quarantine();
        let mut journal_ok = true;
        let mut outcomes = Vec::with_capacity(entries.len());
        for entry in entries {
            let id = entry.id.clone();
            let path = entry.original.to_string_lossy().into_owned();
            let before = if matches!(action, Action::Restore) {
                Record::Restoring { entry: id, path }
            } else {
                Record::Purging { entry: id, path }
            };
            journal_ok = journal_ok && journal.record(&before).is_ok();
            let outcome = if journal_ok {
                handle(&quarantine, entry)
            } else {
                EntryOutcome::Failed {
                    error: "The journal couldn't be written, so it was left alone".into(),
                }
            };
            if journal_ok {
                let done = Record::EntryDone {
                    entry: entry.id.clone(),
                    outcome: outcome.clone(),
                };
                journal_ok = journal.record(&done).is_ok();
            }
            outcomes.push(outcome);
        }
        let journal_path = journal.path().to_owned();
        if journal_ok {
            // Every outcome is already on disk, so a missing end marker loses nothing.
            let _ = journal.finish();
        }
        Ok(Handled {
            journal: journal_path,
            outcomes,
        })
    }

    /// Checks that `item` is still the folder that was approved, that its rule still recognizes
    /// it, and that the safety checks find nothing new, returning their assessment.
    fn check(
        &self,
        item: &PlanItem,
        matcher: &Matcher<'_>,
        assessor: &Assessor<'_>,
    ) -> Result<Candidate, Outcome> {
        let facts = match self.probe.inspect(&item.path) {
            Ok(facts) => facts,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(skipped(SkipReason::Gone));
            }
            Err(error) => {
                return Err(Outcome::Failed {
                    error: format!("Couldn't check it: {error}"),
                });
            }
        };
        if facts.kind != EntryKind::Dir || facts.identity != item.identity {
            return Err(skipped(SkipReason::Replaced));
        }
        let Some(detection) = matcher
            .match_dir(&item.path)
            .filter(|detection| detection.rule.id == item.rule)
        else {
            return Err(skipped(SkipReason::NoLongerMatches));
        };

        let candidate = assessor.assess(CandidateId(0), &detection);
        if candidate.risk() == Risk::Protected {
            let concerns = candidate
                .concerns()
                .iter()
                .filter(|concern| concern.blocks())
                .cloned()
                .collect();
            return Err(skipped(SkipReason::Blocked { concerns }));
        }
        if candidate.identity != Some(item.identity) {
            return Err(skipped(SkipReason::Replaced));
        }
        if candidate.risk() > item.risk {
            return Err(skipped(SkipReason::Risen {
                from: item.risk,
                to: candidate.risk(),
            }));
        }
        // Compared by kind, since details like how many days ago a project was worked on change
        // on their own.
        let new: Vec<Concern> = candidate
            .concerns()
            .iter()
            .filter(|concern| {
                !item
                    .concerns
                    .iter()
                    .any(|seen| mem::discriminant(seen) == mem::discriminant(*concern))
            })
            .cloned()
            .collect();
        if !new.is_empty() {
            return Err(skipped(SkipReason::NewConcerns { concerns: new }));
        }
        Ok(candidate)
    }
}

fn skipped(reason: SkipReason) -> Outcome {
    Outcome::Skipped { reason }
}
