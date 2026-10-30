//! Session API over the scan, plan, and apply pipeline.
//!
//! The CLI and desktop app call only this crate. It owns progress reporting, cancellation, and
//! the in-memory scan results, so front-ends never touch the filesystem directly.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{self, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use cleanse_core::config::ConfigError;
use cleanse_core::rules::{Detection, Matcher};
use cleanse_core::safety::{Assessor, ProtectedPolicy};
use cleanse_core::scan::{Descend, walk};
use cleanse_exec::Executor;
use cleanse_model::{Candidate, CandidateId, EntryKind, Plan, QuarantineEntry};
use rayon::prelude::*;

pub use cleanse_core::config::Config;
pub use cleanse_core::rules::{Base, Location, Os, Rule, RuleError, RuleSet, RuleTest, Target};
pub use cleanse_exec::{Action, Applied, ApplyError, Handled, History, Leftover, Listing, Run};
pub use cleanse_platform::{KnownFolders, Probe, System};

/// Walk progress is reported every this many entries.
const PROGRESS_INTERVAL: u64 = 1024;

/// The rules and platform a session works with.
#[derive(Debug)]
pub struct Engine {
    /// The built-in rules, minus those the settings turn off.
    rules: RuleSet,
    system: System,
    config: Config,
    /// Where the settings file is, if the system has a folder for app settings.
    config_path: Option<PathBuf>,
    /// Cleanse's data folder, which holds the journal and the quarantine, if the system has a
    /// folder for app data.
    data_dir: Option<PathBuf>,
}

#[derive(Debug)]
pub enum EngineError {
    Rules(RuleError),
    Config(ConfigError),
    Io(io::Error),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rules(error) => write!(f, "a built-in rule is invalid: {error}"),
            Self::Config(error) => write!(f, "your settings file has a problem: {error}"),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl Error for EngineError {}

impl From<RuleError> for EngineError {
    fn from(error: RuleError) -> Self {
        Self::Rules(error)
    }
}

impl From<ConfigError> for EngineError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl From<io::Error> for EngineError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// Looking through folders for things rules recognize.
    Walking { entries: u64 },
    /// Measuring and checking what the rules found.
    Assessing { done: usize, total: usize },
}

#[derive(Debug)]
pub struct Scan {
    pub roots: Vec<PathBuf>,
    /// Sorted by path.
    pub candidates: Vec<Candidate>,
    /// Files, folders, and links looked at, not counting what's inside the matches.
    pub entries: u64,
    /// Folders the scan couldn't read, so anything inside them wasn't considered.
    pub unreadable: Vec<PathBuf>,
    pub elapsed: Duration,
    /// The scan was cancelled, so the results are partial.
    pub cancelled: bool,
}

#[derive(Debug)]
pub enum Explanation<'e> {
    /// A rule recognizes the path, or a folder containing it, which is the candidate.
    Matched {
        candidate: Box<Candidate>,
        rule: &'e Rule,
    },
    /// No rule recognizes the path or anything containing it, so Cleanse leaves it alone.
    Unmatched {
        /// Why the path is protected as well, if it is.
        protected: Option<String>,
    },
}

impl Engine {
    /// Reads this system's folders and elevation and the user's settings, and loads the
    /// built-in rules the settings don't turn off.
    pub fn new() -> Result<Self, EngineError> {
        let system = System::new()?;
        let folders = system.known_folders();
        let config_path = Config::path(folders);
        let config = match &config_path {
            Some(path) => Config::load(path, folders.home.as_deref())?,
            None => Config::default(),
        };
        let rules = RuleSet::builtin()?
            .without(&config.disabled_rules)
            .map_err(|message| ConfigError {
                path: config_path.clone().unwrap_or_default(),
                message,
            })?;
        let data_dir = folders.cleanse_data();
        Ok(Self {
            rules,
            system,
            config,
            config_path,
            data_dir,
        })
    }

    pub fn rules(&self) -> &RuleSet {
        &self.rules
    }

    pub fn system(&self) -> &System {
        &self.system
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }

    /// What's scanned when no folders are given: the home folder, which holds the user's
    /// projects as well as the caches the rules know about.
    pub fn default_roots(&self) -> Vec<PathBuf> {
        self.system.known_folders().home.iter().cloned().collect()
    }

    /// Finds what the rules recognize under `roots` and runs the safety checks on it. Nothing
    /// is modified. Setting `cancel` stops the scan early with partial results.
    pub fn scan(
        &self,
        roots: &[PathBuf],
        progress: &(dyn Fn(Progress) + Sync),
        cancel: &AtomicBool,
    ) -> io::Result<Scan> {
        let started = Instant::now();
        let roots: Vec<PathBuf> = roots
            .iter()
            .map(path::absolute)
            .collect::<io::Result<_>>()?;
        let matcher = Matcher::new(
            self.rules.rules(),
            self.system.known_folders(),
            Os::current(),
        );

        let entries = AtomicU64::new(0);
        // Keyed by path, so overlapping roots don't produce duplicates.
        let found: Mutex<BTreeMap<PathBuf, Detection<'_>>> = Mutex::default();
        let mut unreadable = Vec::new();
        for root in &roots {
            let report = walk(root, |entry| {
                if cancel.load(Ordering::Relaxed) {
                    return Descend::No;
                }
                let seen = entries.fetch_add(1, Ordering::Relaxed) + 1;
                if seen.is_multiple_of(PROGRESS_INTERVAL) {
                    progress(Progress::Walking { entries: seen });
                }
                if entry.kind != EntryKind::Dir {
                    return Descend::Yes;
                }
                match matcher.match_dir(entry.path) {
                    Some(detection) => {
                        found
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .insert(detection.path.clone(), detection);
                        Descend::No
                    }
                    None => Descend::Yes,
                }
            })
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("couldn't scan {}: {error}", root.display()),
                )
            })?;
            unreadable.extend(report.errors.into_iter().map(|error| error.path));
        }

        let detections: Vec<Detection<'_>> = found
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
            .into_values()
            .collect();
        let policy = ProtectedPolicy::for_system(
            self.system.known_folders(),
            &self.system,
            &self.config.exclude,
        );
        let assessor = Assessor::new(&self.system, policy, &roots, SystemTime::now())?;
        let done = AtomicUsize::new(0);
        let total = detections.len();
        let candidates: Vec<Candidate> = detections
            .par_iter()
            .enumerate()
            .filter_map(|(index, detection)| {
                if cancel.load(Ordering::Relaxed) {
                    return None;
                }
                let id = CandidateId(u64::try_from(index).expect("fewer than 2^64 matches"));
                let candidate = assessor.assess(id, detection);
                let done = done.fetch_add(1, Ordering::Relaxed) + 1;
                progress(Progress::Assessing { done, total });
                Some(candidate)
            })
            .collect();

        Ok(Scan {
            roots,
            candidates,
            entries: entries.into_inner(),
            unreadable,
            elapsed: started.elapsed(),
            cancelled: cancel.load(Ordering::Relaxed),
        })
    }

    /// Explains what Cleanse would make of `path`: the rule that recognizes it or the folder
    /// containing it, and every safety concern, or why it's left alone.
    pub fn explain(&self, path: &Path) -> io::Result<Explanation<'_>> {
        let path = path::absolute(path)?;
        fs::symlink_metadata(&path)?;
        let folders = self.system.known_folders();
        let matcher = Matcher::new(self.rules.rules(), folders, Os::current());
        let policy = ProtectedPolicy::for_system(folders, &self.system, &self.config.exclude);

        let is_dir = |path: &Path| fs::symlink_metadata(path).is_ok_and(|m| m.is_dir());
        let detection = path
            .ancestors()
            .filter(|ancestor| is_dir(ancestor))
            .find_map(|ancestor| matcher.match_dir(ancestor));
        let Some(detection) = detection else {
            let identity = self.system.inspect(&path).ok().map(|facts| facts.identity);
            return Ok(Explanation::Unmatched {
                protected: policy.check(&path, identity, &[]).map(str::to_owned),
            });
        };

        // The scan root would be the folder around the match.
        let root = detection.path.parent().unwrap_or(&detection.path);
        let assessor = Assessor::new(
            &self.system,
            policy,
            &[root.to_path_buf()],
            SystemTime::now(),
        )?;
        Ok(Explanation::Matched {
            candidate: Box::new(assessor.assess(CandidateId(0), &detection)),
            rule: detection.rule,
        })
    }

    /// Removes the items in `plan` that are still what was approved and still pass every
    /// safety check, recording each one in the journal. `progress` gets the number of items
    /// done so far. Setting `cancel` skips the items not yet reached.
    pub fn apply(
        &self,
        plan: &Plan,
        progress: &dyn Fn(usize),
        cancel: &AtomicBool,
    ) -> Result<Applied, ApplyError> {
        self.executor()?.apply(plan, progress, cancel)
    }

    /// What Cleanse has done before, and what's left of removals that didn't finish.
    pub fn history(&self) -> io::Result<History> {
        match self.executor() {
            Ok(executor) => executor.history(),
            // Without a data folder, there's no journal.
            Err(_) => Ok(History::default()),
        }
    }

    /// Removes what's left of removals that didn't finish, if it's still what was approved.
    pub fn finish(&self, leftovers: &[Leftover]) -> Result<Applied, ApplyError> {
        self.executor()?.finish(leftovers)
    }

    /// What's in quarantine.
    pub fn quarantined(&self) -> io::Result<Listing> {
        match self.executor() {
            Ok(executor) => executor.quarantined(),
            // Without a data folder, nothing can have been quarantined.
            Err(_) => Ok(Listing::default()),
        }
    }

    /// Moves quarantined items back to where they were, unless something is there now.
    pub fn restore(&self, entries: &[QuarantineEntry]) -> Result<Handled, ApplyError> {
        self.executor()?.restore(entries)
    }

    /// Deletes quarantined items for good.
    pub fn purge(&self, entries: &[QuarantineEntry]) -> Result<Handled, ApplyError> {
        self.executor()?.purge(entries)
    }

    fn executor(&self) -> Result<Executor<'_>, ApplyError> {
        let data_dir = self.data_dir.clone().ok_or_else(|| {
            ApplyError::Journal(io::Error::new(
                io::ErrorKind::NotFound,
                "this system has no folder for app data",
            ))
        })?;
        Ok(Executor::new(
            &self.rules,
            &self.system,
            &self.config.exclude,
            data_dir,
        ))
    }
}
