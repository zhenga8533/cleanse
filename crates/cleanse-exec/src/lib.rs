//! The only crate permitted to change the filesystem.
//!
//! Executes an approved plan item by item, re-validating each item through the safety gates
//! immediately before acting, and records every outcome in an append-only journal.

mod delete;
mod journal;

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
use cleanse_model::{
    Candidate, CandidateId, Concern, Disposal, EntryKind, Outcome, Plan, PlanItem, Risk, SkipReason,
};
use cleanse_platform::Probe;

use journal::{Journal, Record};

/// Removes approved items, checking each one again first.
pub struct Executor<'a> {
    rules: &'a RuleSet,
    probe: &'a dyn Probe,
    journal_dir: PathBuf,
}

/// What applying a plan did.
#[derive(Debug)]
pub struct Applied {
    /// The journal file recording this run.
    pub journal: PathBuf,
    /// One per plan item, in the plan's order.
    pub outcomes: Vec<Outcome>,
}

/// Why a plan couldn't be applied at all. Nothing was removed.
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
                "Cleanse doesn't remove anything while running as an administrator or root, \
                 because that could remove files your own account can't. Run it again \
                 without elevation.",
            ),
            Self::Journal(error) => {
                write!(
                    f,
                    "couldn't start the journal, so nothing was removed: {error}"
                )
            }
            Self::Io(error) => write!(
                f,
                "couldn't set up the safety checks, so nothing was removed: {error}"
            ),
        }
    }
}

impl Error for ApplyError {}

impl<'a> Executor<'a> {
    /// Each run writes its journal to a new file in `journal_dir`.
    pub fn new(rules: &'a RuleSet, probe: &'a dyn Probe, journal_dir: PathBuf) -> Self {
        Self {
            rules,
            probe,
            journal_dir,
        }
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
        let policy = ProtectedPolicy::for_system(folders, self.probe);
        // Files modified after this count as modified moments ago, however long the run takes.
        let assessor = Assessor::new(self.probe, policy, &plan.roots, SystemTime::now())
            .map_err(ApplyError::Io)?;
        let mut journal = Journal::start(&self.journal_dir, plan).map_err(ApplyError::Journal)?;

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
        if item.disposal != Disposal::Delete {
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
        // The protected policy blocks drive roots, so anything that passed the check has a
        // parent folder to hold the tombstone.
        let Some(tombstone) = delete::tombstone_for(&item.path) else {
            return Ok(Outcome::Failed {
                error: "it has no parent folder".into(),
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
                    error: format!("couldn't check it: {error}"),
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
