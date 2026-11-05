//! Session API over the scan, plan, and apply pipeline.
//!
//! The CLI and desktop app call only this crate. It owns progress reporting, cancellation, and
//! the in-memory scan results, so front-ends never touch the filesystem directly.

mod session;
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

use cleanse_core::analyze::Leaf;
use cleanse_core::config::ConfigError;
use cleanse_core::rules::{Detection, Listed, Matcher};
use cleanse_core::safety::{Assessor, ProtectedPolicy, SystemRoots, scan_folders};
use cleanse_core::scan::{Descend, DirFiles, distinct_roots, walk_with};
use cleanse_exec::Executor;
use cleanse_model::{Candidate, CandidateId, Concern, Plan, QuarantineEntry, Risk};
use rayon::prelude::*;

pub use cleanse_core::analyze::{Node, NodeId, NodeKind, SizeTree, Usage, removable_under};
pub use cleanse_core::config::Config;
pub use cleanse_core::rules::{
    Base, Entry, Location, Os, Rule, RuleError, RuleSet, RuleTest, Target,
};
pub use cleanse_exec::{
    Action, Applied, ApplyError, Handled, History, Leftover, Listing, PlanFileError, Run, helper,
    load_plan, save_plan,
};
pub use cleanse_platform::{Elevation, KnownFolders, Probe, System, SystemFolders};
pub use session::{
    Cleaned, Explained, Finished, FolderEntry, FolderView, HandledEntries, Page, RuleSummary,
    ScanId, SectionSummary, Session, SessionError, Summary,
};

/// Walk progress is reported every this many entries.
const PROGRESS_INTERVAL: u64 = 1024;

/// Where a candidate belongs in a scan's results.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Section {
    /// Safe and Regenerable items.
    Removable,
    /// Review items, which only a person can decide about.
    Review,
    /// Kept, because their own tool cleans them better: see [`advice`].
    Advice,
    /// Kept by a safety check or their rule.
    Kept,
}

impl Section {
    /// In the order results are shown.
    pub const ALL: [Self; 4] = [Self::Removable, Self::Review, Self::Advice, Self::Kept];

    pub fn of(rules: &RuleSet, candidate: &Candidate) -> Self {
        match candidate.risk() {
            Risk::Safe | Risk::Regenerable => Self::Removable,
            Risk::Review => Self::Review,
            Risk::Protected if advice(rules, candidate).is_some() => Self::Advice,
            Risk::Protected => Self::Kept,
        }
    }
}

/// The command that cleans `candidate` better than removing it would, from its rule. An item a
/// safety check keeps for another reason, such as a folder excluded in the settings, has none,
/// and is kept with that reason instead.
pub fn advice<'r>(rules: &'r RuleSet, candidate: &Candidate) -> Option<&'r str> {
    if candidate.concerns().iter().any(Concern::blocks) {
        return None;
    }
    rules.get(&candidate.found.rule)?.advise.as_deref()
}

/// How this build recognizes folders a cloud service syncs, which are kept.
pub fn cloud_sync_detection() -> &'static str {
    if cfg!(windows) {
        "Windows Cloud Files API (OneDrive, iCloud, Dropbox, and other providers)"
    } else if cfg!(target_os = "macos") {
        "by location (~/Library/CloudStorage and iCloud Drive)"
    } else {
        "not available; synced folders aren't recognized on Linux yet"
    }
}

/// Which cleanup an [`Engine`] is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The user's own files. Nothing is changed while elevated, unless the account has no way
    /// to run without, as on Windows with UAC off (ADR 0011).
    User,
    /// The operating system's folders, changed only while elevated (ADR 0010).
    System,
}

/// The rules and platform a session works with.
#[derive(Debug)]
pub struct Engine {
    /// The built-in rules, minus those the settings turn off. For system cleanup, the built-in
    /// system rules.
    rules: RuleSet,
    system: System,
    /// For system cleanup, the defaults: it reads no settings file.
    config: Config,
    /// Where the settings file is, if the system has a folder for app settings.
    config_path: Option<PathBuf>,
    /// Cleanse's data folder, which holds the journal and the quarantine, if the system has a
    /// folder for app data. System cleanup has its own.
    data_dir: Option<PathBuf>,
    cleanup: Cleanup,
}

#[derive(Debug)]
enum Cleanup {
    User,
    /// In the folders the operating system reports.
    System {
        folders: SystemFolders,
    },
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
    /// The folders scanned: absolute, and none inside another.
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

/// A scan, and how much space every folder under its roots takes.
#[derive(Debug)]
pub struct Analysis {
    pub scan: Scan,
    pub tree: SizeTree,
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
            cleanup: Cleanup::User,
        })
    }

    /// System cleanup's engine (ADR 0010): the built-in system rules, in the folders the
    /// operating system reports. It reads no settings file, scans only what's directly in the
    /// folders the rules name, and changes nothing unless elevated.
    pub fn new_system() -> Result<Self, EngineError> {
        let system = System::new()?;
        let folders = SystemFolders::current()?;
        Ok(Self {
            rules: RuleSet::builtin_system()?,
            system,
            config: Config::default(),
            config_path: None,
            data_dir: folders.cleanse_data(),
            cleanup: Cleanup::System { folders },
        })
    }

    /// The user's cleanup, with the built-in rules and the default settings, over `system`'s
    /// known folders, keeping its journal and quarantine in `data_dir`. For tests of what uses an
    /// engine, on fixtures.
    #[cfg(feature = "fake")]
    pub fn user_at(system: System, data_dir: PathBuf) -> Result<Self, EngineError> {
        Ok(Self {
            rules: RuleSet::builtin()?,
            system,
            config: Config::default(),
            config_path: None,
            data_dir: Some(data_dir),
            cleanup: Cleanup::User,
        })
    }

    /// System cleanup over `folders`, as if the operating system reported them. For tests of what
    /// uses an engine, on fixtures.
    #[cfg(feature = "fake")]
    pub fn system_at(system: System, folders: SystemFolders) -> Result<Self, EngineError> {
        Ok(Self {
            rules: RuleSet::builtin_system()?,
            system,
            config: Config::default(),
            config_path: None,
            data_dir: folders.cleanse_data(),
            cleanup: Cleanup::System { folders },
        })
    }

    pub fn mode(&self) -> Mode {
        match self.cleanup {
            Cleanup::User => Mode::User,
            Cleanup::System { .. } => Mode::System,
        }
    }

    /// Why nothing can be changed now, if so: the user's cleanup changes nothing while elevated
    /// when it could run without, and system cleanup only while elevated.
    pub fn check_can_change(&self) -> Result<(), ApplyError> {
        self.executor()?.check_can_change()
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
    /// projects as well as the caches the rules know about. For system cleanup, the folders the
    /// system rules name items directly inside, where they exist.
    pub fn default_roots(&self) -> Vec<PathBuf> {
        match &self.cleanup {
            Cleanup::User => self.system.known_folders().home.iter().cloned().collect(),
            Cleanup::System { folders } => scan_folders(&self.rules, folders)
                .into_iter()
                .filter(|folder| {
                    fs::symlink_metadata(folder).is_ok_and(|metadata| metadata.is_dir())
                })
                .collect(),
        }
    }

    /// Finds what the rules recognize under `roots` and runs the safety checks on it. Nothing
    /// is modified. Setting `cancel` stops the scan early with partial results. System cleanup
    /// only looks directly inside each root, and never deeper apart from measuring what it finds.
    pub fn scan(
        &self,
        roots: &[PathBuf],
        progress: &(dyn Fn(Progress) + Sync),
        cancel: &AtomicBool,
    ) -> io::Result<Scan> {
        let (scan, _) = self.scan_with(roots, progress, cancel, |_, _| {})?;
        Ok(scan)
    }

    /// Scans `roots` as [`Engine::scan`] does, and also adds up how much space every folder
    /// under them takes, which costs a little more time and memory.
    pub fn analyze(
        &self,
        roots: &[PathBuf],
        progress: &(dyn Fn(Progress) + Sync),
        cancel: &AtomicBool,
    ) -> io::Result<Analysis> {
        let folders = Mutex::new(Vec::new());
        let (scan, other_filesystems) = self.scan_with(roots, progress, cancel, |dir, files| {
            folders
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((dir, files));
        })?;
        let leaves = scan
            .candidates
            .iter()
            .filter_map(Leaf::of)
            .chain(other_filesystems.into_iter().map(Leaf::other_filesystem))
            .collect();
        let tree = SizeTree::build(
            &scan.roots,
            folders.into_inner().unwrap_or_else(PoisonError::into_inner),
            leaves,
            &scan.unreadable,
            scan.cancelled,
        );
        Ok(Analysis { scan, tree })
    }

    /// Scans, calling `listed` for each folder read, and returns the scan and the folders on
    /// other filesystems it didn't enter.
    fn scan_with<L>(
        &self,
        roots: &[PathBuf],
        progress: &(dyn Fn(Progress) + Sync),
        cancel: &AtomicBool,
        listed: L,
    ) -> io::Result<(Scan, Vec<PathBuf>)>
    where
        L: Fn(PathBuf, DirFiles) + Sync,
    {
        let started = Instant::now();
        let roots = match self.cleanup {
            Cleanup::User => distinct_roots(roots)?,
            // A folder inside another is listed as well, since nothing below the other is.
            Cleanup::System { .. } => {
                let mut distinct: Vec<PathBuf> = Vec::new();
                for root in roots {
                    let root = path::absolute(root)?;
                    if !distinct.contains(&root) {
                        distinct.push(root);
                    }
                }
                distinct
            }
        };
        let descend = match self.cleanup {
            Cleanup::User => Descend::Yes,
            Cleanup::System { .. } => Descend::No,
        };
        let matcher = self.matcher();

        let entries = AtomicU64::new(0);
        // Sorted by path, which `removable_under` relies on.
        let found: Mutex<BTreeMap<PathBuf, Detection<'_>>> = Mutex::default();
        let mut unreadable = Vec::new();
        let mut other_filesystems = Vec::new();
        for root in &roots {
            let visit = |entry: &cleanse_core::scan::Entry<'_>| {
                if cancel.load(Ordering::Relaxed) {
                    return Descend::No;
                }
                let seen = entries.fetch_add(1, Ordering::Relaxed) + 1;
                if seen.is_multiple_of(PROGRESS_INTERVAL) {
                    progress(Progress::Walking { entries: seen });
                }
                match matcher.match_entry(entry.path, Listed::of(entry.metadata)) {
                    Some(detection) => {
                        found
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .insert(detection.path.clone(), detection);
                        Descend::No
                    }
                    None => descend,
                }
            };
            let report = walk_with(root, visit, &listed).map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("couldn't scan {}: {error}", root.display()),
                )
            })?;
            unreadable.extend(report.errors.into_iter().map(|error| error.path));
            other_filesystems.extend(report.other_filesystems);
        }

        let policy = self.policy();
        // A large file is only a match because of its size, so one where nothing may be removed,
        // like Windows' own folders or a folder excluded in the settings, isn't worth listing as
        // kept.
        let detections: Vec<Detection<'_>> = found
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
            .into_values()
            .filter(|detection| {
                !matches!(detection.rule.target, Target::LargeFile {})
                    || policy
                        .check_on_disk(&detection.path, &self.system)
                        .is_none()
            })
            .collect();
        // Checking who owns everything inside every item would make a system scan take minutes
        // where an item holds hundreds of thousands of files, so that waits until removal.
        let assessor = self.assessor(policy, &roots, false)?;
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

        let scan = Scan {
            roots,
            candidates,
            entries: entries.into_inner(),
            unreadable,
            elapsed: started.elapsed(),
            cancelled: cancel.load(Ordering::Relaxed),
        };
        Ok((scan, other_filesystems))
    }

    /// Explains what Cleanse would make of `path`: the rule that recognizes it or the folder
    /// containing it, and every safety concern, or why it's left alone.
    pub fn explain(&self, path: &Path) -> io::Result<Explanation<'_>> {
        let path = path::absolute(path)?;
        fs::symlink_metadata(&path)?;
        let matcher = self.matcher();
        let policy = self.policy();

        let detection = path.ancestors().find_map(|ancestor| {
            let metadata = fs::symlink_metadata(ancestor).ok()?;
            matcher.match_entry(ancestor, Listed::of(&metadata))
        });
        let Some(detection) = detection else {
            return Ok(Explanation::Unmatched {
                protected: policy.check_on_disk(&path, &self.system).map(str::to_owned),
            });
        };

        // The scan root would be the folder around the match.
        let root = detection.path.parent().unwrap_or(&detection.path);
        let assessor = self.assessor(policy, &[root.to_path_buf()], true)?;
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
        if let Cleanup::System { folders } = &self.cleanup {
            return Executor::system(&self.rules, &self.system, folders.clone());
        }
        let data_dir = self.data_dir.clone().ok_or_else(|| {
            ApplyError::Journal(io::Error::new(
                io::ErrorKind::NotFound,
                "this system has no folder for app data",
            ))
        })?;
        Ok(Executor::new(
            &self.rules,
            &self.system,
            &self.config,
            data_dir,
        ))
    }

    fn matcher(&self) -> Matcher<'_> {
        match &self.cleanup {
            Cleanup::User => Matcher::new(
                self.rules.rules(),
                self.system.known_folders(),
                Os::current(),
            )
            .with_large_files(self.config.large_file_size),
            Cleanup::System { folders } => {
                Matcher::system(self.rules.rules(), folders, Os::current())
            }
        }
    }

    fn policy(&self) -> ProtectedPolicy {
        match &self.cleanup {
            Cleanup::User => ProtectedPolicy::for_system(
                self.system.known_folders(),
                &self.system,
                &self.config.exclude,
            ),
            Cleanup::System { folders } => {
                ProtectedPolicy::for_system_cleanup(folders, &self.system)
            }
        }
    }

    /// The user's cleanup checks that items are inside `roots`. System cleanup checks the
    /// system folders instead, now, and, if `thorough`, who owns everything inside each item.
    fn assessor(
        &self,
        policy: ProtectedPolicy,
        roots: &[PathBuf],
        thorough: bool,
    ) -> io::Result<Assessor<'_>> {
        let now = SystemTime::now();
        match &self.cleanup {
            Cleanup::User => Assessor::new(&self.system, policy, roots, now),
            Cleanup::System { folders } => {
                let roots = SystemRoots::verify(&self.rules, folders, &self.system);
                let assessor = Assessor::system(&self.system, policy, roots, now);
                Ok(if thorough {
                    assessor.thorough()
                } else {
                    assessor
                })
            }
        }
    }
}
