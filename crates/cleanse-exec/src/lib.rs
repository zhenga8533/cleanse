//! The only crate permitted to change the filesystem.
//!
//! Executes an approved plan item by item, re-validating each item through the safety gates
//! immediately before acting, and records every outcome in an append-only journal. The user's
//! cleanup and system cleanup each have an executor of their own (ADR 0010).

mod delete;
pub mod helper;
mod history;
mod journal;
mod plan_file;
mod quarantine;
mod system;

#[cfg(test)]
mod tests;

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::mem;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use cleanse_core::config::Config;
use cleanse_core::rules::{Listed, Matcher, Os, RuleSet, Scope};
use cleanse_core::safety::{Assessor, ProtectedPolicy, SystemRoots};
use cleanse_core::scan::measure;
use cleanse_model::{
    Candidate, CandidateId, Concern, Disposal, EntryOutcome, Outcome, Plan, PlanItem,
    QuarantineEntry, Risk, SkipReason,
};
use cleanse_platform::{Probe, SystemFolders, privileged};

use journal::{Journal, Record};
use quarantine::Quarantine;
use system::Targets;

pub use history::{History, Leftover, Run};
pub use journal::Action;
pub use plan_file::{PlanFileError, load_plan, save_plan};
pub use quarantine::Listing;

/// Removes approved items, checking each one again first, and looks after the quarantine.
pub struct Executor<'a> {
    rules: &'a RuleSet,
    probe: &'a dyn Probe,
    cleanup: Cleanup<'a>,
    data_dir: PathBuf,
}

/// Which cleanup an [`Executor`] carries out.
enum Cleanup<'a> {
    /// The user's, with their settings.
    User { config: &'a Config },
    /// The system's, in the folders the operating system reports.
    System { folders: SystemFolders },
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
    /// System cleanup changes things only as an administrator or root.
    NotElevated,
    /// The rules are for the other cleanup: system cleanup uses only system rules, and the user's
    /// cleanup only the user's.
    WrongRules,
    /// System cleanup's data folder couldn't be made, or someone other than Administrators and
    /// SYSTEM, or root, can change it.
    DataFolder(io::Error),
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
            Self::NotElevated => f.write_str(
                "System cleanup changes things only as an administrator or root. Run it again \
                 from an administrator terminal, or with sudo.",
            ),
            Self::WrongRules => {
                f.write_str("these rules are for the other kind of cleanup, so nothing was changed")
            }
            Self::DataFolder(error) => write!(
                f,
                "system cleanup's data folder can't be used, so nothing was changed: {error}"
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
    /// `config` is the user's settings, which items are checked against as the scan was.
    /// `data_dir` is Cleanse's data folder: each run writes its journal to a new file in its
    /// `journal` folder, and quarantined items go in its `quarantine` folder.
    pub fn new(
        rules: &'a RuleSet,
        probe: &'a dyn Probe,
        config: &'a Config,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            rules,
            probe,
            cleanup: Cleanup::User { config },
            data_dir,
        }
    }

    /// System cleanup's executor (ADR 0010), for system `rules` in the system's `folders`. Its
    /// journal and quarantine are in a data folder of its own there, which only Administrators
    /// and SYSTEM, or root, can change. It reads no settings, and only the items of a plan.
    pub fn system(
        rules: &'a RuleSet,
        probe: &'a dyn Probe,
        folders: SystemFolders,
    ) -> Result<Self, ApplyError> {
        let data_dir = folders.cleanse_data().ok_or_else(|| {
            ApplyError::DataFolder(io::Error::new(
                io::ErrorKind::Unsupported,
                "this system has no folder for system cleanup's data",
            ))
        })?;
        Ok(Self {
            rules,
            probe,
            cleanup: Cleanup::System { folders },
            data_dir,
        })
    }

    /// Why nothing can be changed now, if so: the user's cleanup refuses to run elevated, and
    /// system cleanup only runs elevated, and each only with its own rules.
    pub fn check_can_change(&self) -> Result<(), ApplyError> {
        let (scope, elevation) = match self.cleanup {
            Cleanup::User { .. } => (Scope::User, !self.probe.is_elevated()),
            Cleanup::System { .. } => (Scope::System, self.probe.is_elevated()),
        };
        if !elevation {
            return Err(match self.cleanup {
                Cleanup::User { .. } => ApplyError::Elevated,
                Cleanup::System { .. } => ApplyError::NotElevated,
            });
        }
        if self.rules.scope() != scope {
            return Err(ApplyError::WrongRules);
        }
        Ok(())
    }

    /// Checks that a run may change anything, makes sure system cleanup's data folder is
    /// private, then starts the journal for the run.
    fn start(
        &self,
        action: Action,
        roots: &[PathBuf],
        items: usize,
    ) -> Result<Journal, ApplyError> {
        self.check_can_change()?;
        if let Cleanup::System { .. } = self.cleanup {
            self.prepare_data_folder()?;
        }
        Journal::start(&self.data_dir.join("journal"), action, roots, items)
            .map_err(ApplyError::Journal)
    }

    /// Creates system cleanup's data folder if it's missing, then checks that only
    /// Administrators and SYSTEM, or root, can change it. Anyone can create folders in
    /// `ProgramData`, so one someone else made first is refused, never repaired.
    fn prepare_data_folder(&self) -> Result<(), ApplyError> {
        #[expect(
            clippy::disallowed_methods,
            reason = "cleanse-exec creates system cleanup's data folder (ADR 0010)"
        )]
        let created = privileged::create_private_dir(&self.data_dir);
        match created {
            Err(error) if error.kind() != io::ErrorKind::AlreadyExists => {
                Err(ApplyError::DataFolder(error))
            }
            Ok(()) | Err(_) => self.check_data_folder().map_err(ApplyError::DataFolder),
        }
    }

    /// For system cleanup, whether its data folder exists, refusing one that others can change.
    fn data_folder_exists(&self) -> io::Result<bool> {
        match self.cleanup {
            Cleanup::User { .. } => Ok(true),
            Cleanup::System { .. } => match self.check_data_folder() {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
                checked => checked.map(|()| true),
            },
        }
    }

    fn check_data_folder(&self) -> io::Result<()> {
        if self.probe.is_private_dir(&self.data_dir)? {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "accounts other than Administrators and SYSTEM, or root, can change {}, so \
                 system cleanup won't use it. If no one you trust made it, remove it.",
                self.data_dir.display()
            )))
        }
    }

    /// For system cleanup, the places it may put back or finish removing items.
    fn targets(&self) -> Option<Targets<'a>> {
        match &self.cleanup {
            Cleanup::User { .. } => None,
            Cleanup::System { folders } => Some(Targets::new(self.rules, folders, self.probe)),
        }
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
        self.check_can_change()?;
        // Files modified after this count as modified moments ago, however long the run takes.
        let now = SystemTime::now();
        let (matcher, assessor, roots) = match &self.cleanup {
            Cleanup::User { config } => {
                let folders = self.probe.known_folders();
                let matcher = Matcher::new(self.rules.rules(), folders, Os::current())
                    .with_large_files(config.large_file_size);
                let policy = ProtectedPolicy::for_system(folders, self.probe, &config.exclude);
                let assessor =
                    Assessor::new(self.probe, policy, &plan.roots, now).map_err(ApplyError::Io)?;
                (matcher, assessor, plan.roots.as_slice())
            }
            // The plan's roots are ignored: system cleanup only removes what's directly inside
            // the system folders it checks, which it checks again now.
            Cleanup::System { folders } => {
                let matcher = Matcher::system(self.rules.rules(), folders, Os::current());
                let policy = ProtectedPolicy::for_system_cleanup(folders, self.probe);
                let roots = SystemRoots::verify(self.rules, folders, self.probe);
                let assessor = Assessor::system(self.probe, policy, roots, now).thorough();
                (matcher, assessor, &[][..])
            }
        };
        let mut journal = self.start(Action::Apply, roots, plan.items.len())?;

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
        if item.disposal == Disposal::Delete
            && candidate.default_disposal() != Some(Disposal::Delete)
        {
            return Ok(skipped(SkipReason::NeedsQuarantine));
        }

        if item.disposal == Disposal::Quarantine {
            let entry = QuarantineEntry {
                id: Quarantine::new_id(),
                original: item.path.clone(),
                kind: item.kind,
                rule: item.rule.clone(),
                size: candidate.found.size,
                at: journal::now(),
            };
            journal.record(&Record::Quarantining {
                path: path.to_owned(),
                entry: entry.id.clone(),
                kind: item.kind,
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
            kind: item.kind,
            rule: item.rule.clone(),
            identity: item.identity,
        })?;
        Ok(delete::delete(
            self.probe,
            &item.path,
            &tombstone,
            item.kind,
            item.identity,
            candidate.found.size,
        ))
    }

    /// What the journal says Cleanse has done, and what's left of removals that didn't finish.
    pub fn history(&self) -> io::Result<History> {
        if !self.data_folder_exists()? {
            return Ok(History::default());
        }
        history::read(&self.data_dir.join("journal"), self.probe)
    }

    /// Removes what's left of removals that didn't finish, each only if its tombstone still
    /// holds the item that was approved.
    pub fn finish(&self, leftovers: &[Leftover]) -> Result<Applied, ApplyError> {
        let mut journal = self.start(Action::Finish, &[], leftovers.len())?;
        let targets = self.targets();
        let mut journal_ok = true;
        let mut outcomes = Vec::with_capacity(leftovers.len());
        for leftover in leftovers {
            let removing = Record::Removing {
                path: leftover.original.to_string_lossy().into_owned(),
                tombstone: leftover.tombstone.to_string_lossy().into_owned(),
                kind: leftover.kind,
                rule: leftover.rule.clone(),
                identity: leftover.identity,
            };
            let outcome = if !journal_ok {
                skipped(SkipReason::JournalUnavailable)
            } else if !history::still_there(self.probe, leftover) {
                skipped(SkipReason::Gone)
            } else if targets
                .as_ref()
                .is_some_and(|targets| !targets.allow(&leftover.original, leftover.kind))
            {
                skipped(SkipReason::NoLongerMatches)
            } else if journal.record(&removing).is_err() {
                journal_ok = false;
                skipped(SkipReason::JournalUnavailable)
            } else {
                let size = measure(&leftover.tombstone)
                    .map(|measurement| measurement.size)
                    .unwrap_or_default();
                delete::finish(&leftover.tombstone, leftover.kind, size)
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
        if !self.data_folder_exists()? {
            return Ok(Listing::default());
        }
        self.quarantine().list()
    }

    /// Moves each entry back to where it was, unless something is there now. System cleanup
    /// also puts an item back only where a system rule names one, directly inside a system
    /// folder that checks out.
    pub fn restore(&self, entries: &[QuarantineEntry]) -> Result<Handled, ApplyError> {
        let targets = self.targets();
        self.each_entry(Action::Restore, entries, |quarantine, entry| {
            if let Some(targets) = &targets
                && !targets.allow(&entry.original, entry.kind)
            {
                return EntryOutcome::Failed {
                    error: format!(
                        "{} isn't somewhere system cleanup removes things from, so it was left \
                         in quarantine",
                        entry.original.display()
                    ),
                };
            }
            quarantine.restore(entry)
        })
    }

    /// Deletes each entry for good.
    pub fn purge(&self, entries: &[QuarantineEntry]) -> Result<Handled, ApplyError> {
        self.each_entry(Action::Purge, entries, Quarantine::purge)
    }

    fn each_entry(
        &self,
        action: Action,
        entries: &[QuarantineEntry],
        handle: impl Fn(&Quarantine, &QuarantineEntry) -> EntryOutcome,
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

    /// Checks that `item` is still the file or folder that was approved, that its rule still
    /// recognizes it, and that the safety checks find nothing new, returning their assessment.
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
        if facts.kind != item.kind || facts.identity != item.identity {
            return Err(skipped(SkipReason::Replaced));
        }
        // Read the way the scan's walk read it, so a large file is measured the same way.
        let listed = fs::symlink_metadata(&item.path)
            .map(|metadata| Listed::of(&metadata))
            .map_err(|error| Outcome::Failed {
                error: format!("Couldn't check it: {error}"),
            })?;
        let Some(detection) = matcher
            .match_entry(&item.path, listed)
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
