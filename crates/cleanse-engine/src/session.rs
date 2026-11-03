//! One front end's scan at a time, with everything it changes named by IDs the session holds.
//!
//! A front end such as the desktop app never gives a path to remove. It reads candidates,
//! folders, and quarantine entries from the session and names them back by ID, and an ID from a
//! scan that's since been replaced is refused, so it can never name something else.

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use cleanse_model::{
    Candidate, CandidateId, Category, EntryKind, EntryOutcome, Outcome, Plan, PlanItem,
    QuarantineEntry, RuleId, Size,
};

use crate::{
    Analysis, ApplyError, Engine, Handled, History, Leftover, Listing, NodeId, NodeKind, Progress,
    Rule, Section, Usage, removable_under,
};

/// Names one scan. Every candidate and folder a session gives out is named with the scan it's
/// from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScanId(pub u64);

/// The engine, the latest scan's results, and the one scan or removal that may run at a time.
#[derive(Debug)]
pub struct Session {
    engine: Engine,
    current: Mutex<Option<Current>>,
    /// Set while a scan or a change runs.
    busy: AtomicBool,
    /// Asks the scan or removal running now to stop.
    cancel: AtomicBool,
    last_scan: AtomicU64,
}

#[derive(Debug)]
struct Current {
    id: ScanId,
    analysis: Analysis,
    /// What's been removed since the scan. Folder sizes still count it.
    removed: Size,
}

/// What a scan found, section by section.
#[derive(Clone, Debug)]
pub struct Summary {
    pub scan: ScanId,
    pub roots: Vec<PathBuf>,
    pub entries: u64,
    pub elapsed: Duration,
    /// The scan was cancelled, so the results are partial.
    pub cancelled: bool,
    /// How many folders couldn't be read.
    pub unreadable: usize,
    /// Every section, in [`Section::ALL`]'s order, even the empty ones.
    pub sections: Vec<SectionSummary>,
    /// What's been removed since the scan.
    pub removed: Size,
}

#[derive(Clone, Debug)]
pub struct SectionSummary {
    pub section: Section,
    pub items: usize,
    pub size: Size,
    /// How many a cleanup selects by default.
    pub preselected: usize,
    /// The rules with items in this section, largest first.
    pub rules: Vec<RuleSummary>,
}

#[derive(Clone, Debug)]
pub struct RuleSummary {
    pub rule: RuleId,
    pub items: usize,
    pub size: Size,
}

/// Part of a longer list.
#[derive(Clone, Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// Where in the list `items` starts.
    pub offset: usize,
    /// How long the whole list is.
    pub total: usize,
}

/// A folder in a scan's size tree, or an item a rule recognized in its place.
#[derive(Clone, Debug)]
pub struct FolderEntry {
    pub node: NodeId,
    /// A root's whole path, or the name inside its folder.
    pub name: OsString,
    pub path: PathBuf,
    pub kind: NodeKind,
    pub usage: Usage,
    pub files: u64,
    /// How much at or inside it can be removed.
    pub removable: u64,
    /// Part of it couldn't be read, so it's bigger than it shows.
    pub incomplete: bool,
    /// The candidate an item is, unless it's been removed since the scan.
    pub candidate: Option<CandidateId>,
    /// It has folders inside it to show.
    pub has_children: bool,
}

/// One folder of a scan's size tree, or its roots.
#[derive(Clone, Debug)]
pub struct FolderView {
    /// The folders from a root down to this one. Empty for the roots.
    pub trail: Vec<FolderEntry>,
    /// Its largest folders, or the roots, up to the limit asked for.
    pub entries: Vec<FolderEntry>,
    /// How many `entries` leaves out, and how much they take.
    pub hidden: usize,
    pub hidden_on_disk: u64,
    /// The files directly in it, which have no entries of their own.
    pub own_files: u64,
    pub own_usage: Usage,
    pub own_removable: u64,
}

/// A candidate, with the rule that matched it and where it's listed.
#[derive(Clone, Debug)]
pub struct Explained {
    pub candidate: Candidate,
    pub rule: Rule,
    pub section: Section,
}

/// What a cleanup did, item by item, in the order the items were given.
#[derive(Debug)]
pub struct Cleaned {
    /// The journal file recording the run, unless there was nothing to remove.
    pub journal: Option<PathBuf>,
    pub outcomes: Vec<(CandidateId, Outcome)>,
}

/// What restoring or purging did, entry by entry, in the order the IDs were given.
#[derive(Debug, Default)]
pub struct HandledEntries {
    /// The journal file recording the run, unless there was nothing to do.
    pub journal: Option<PathBuf>,
    pub outcomes: Vec<(QuarantineEntry, EntryOutcome)>,
}

impl HandledEntries {
    fn new(entries: Vec<QuarantineEntry>, handled: Handled) -> Self {
        Self {
            journal: Some(handled.journal),
            outcomes: entries.into_iter().zip(handled.outcomes).collect(),
        }
    }
}

/// What finishing the removals that didn't finish did, leftover by leftover.
#[derive(Debug)]
pub struct Finished {
    /// The journal file recording the run, unless nothing was left.
    pub journal: Option<PathBuf>,
    pub outcomes: Vec<(Leftover, Outcome)>,
}

/// Why a session couldn't do what it was asked. Nothing was changed.
#[derive(Debug)]
pub enum SessionError {
    /// A scan or a change is already running, and only one may at a time.
    Busy,
    /// The results asked about are from a scan that's been replaced, or there's been no scan.
    OldScan,
    UnknownCandidate(CandidateId),
    /// The candidate is Protected, or couldn't be checked, so it can't be removed.
    NotRemovable(CandidateId),
    UnknownFolder(u32),
    /// Nothing in quarantine has this ID.
    UnknownEntry(String),
    Io(io::Error),
    Apply(ApplyError),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str("Another scan or cleanup is running. Wait for it to finish."),
            Self::OldScan => f.write_str(
                "These results are from an earlier scan. Scan again to see what's there now.",
            ),
            Self::UnknownCandidate(id) => write!(f, "The scan has no item {}", id.0),
            Self::NotRemovable(id) => write!(f, "Item {} is kept, so it can't be removed", id.0),
            Self::UnknownFolder(index) => write!(f, "The scan has no folder {index}"),
            Self::UnknownEntry(id) => write!(f, "Nothing in quarantine has the ID {id}"),
            Self::Io(error) => error.fmt(f),
            Self::Apply(error) => error.fmt(f),
        }
    }
}

impl Error for SessionError {}

impl From<io::Error> for SessionError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<ApplyError> for SessionError {
    fn from(error: ApplyError) -> Self {
        Self::Apply(error)
    }
}

/// Marks the session busy until it's dropped.
struct Busy<'s>(&'s AtomicBool);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Session {
    pub fn new(engine: Engine) -> Self {
        Self {
            engine,
            current: Mutex::new(None),
            busy: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            last_scan: AtomicU64::new(0),
        }
    }

    /// For what doesn't depend on a scan: the rules, the settings, and the system.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Scans `roots`, or the home folder if there are none, and measures every folder under
    /// them. The results replace the last scan's, and IDs from it stop working.
    pub fn scan(
        &self,
        roots: &[PathBuf],
        progress: &(dyn Fn(Progress) + Sync),
    ) -> Result<ScanId, SessionError> {
        let _busy = self.start()?;
        let roots = if roots.is_empty() {
            self.engine.default_roots()
        } else {
            roots.to_vec()
        };
        let analysis = self.engine.analyze(&roots, progress, &self.cancel)?;
        let id = ScanId(self.last_scan.fetch_add(1, Ordering::Relaxed) + 1);
        *self.lock() = Some(Current {
            id,
            analysis,
            removed: Size::default(),
        });
        Ok(id)
    }

    /// Asks the scan or cleanup running now to stop. A scan keeps what it found so far; a
    /// cleanup skips the items it hasn't reached.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn summary(&self, scan: ScanId) -> Result<Summary, SessionError> {
        self.read(scan, |current| {
            let rules = self.engine.rules();
            let results = &current.analysis.scan;
            let mut sections: BTreeMap<Section, (SectionSummary, BTreeMap<&RuleId, RuleSummary>)> =
                Section::ALL
                    .into_iter()
                    .map(|section| {
                        let summary = SectionSummary {
                            section,
                            items: 0,
                            size: Size::default(),
                            preselected: 0,
                            rules: Vec::new(),
                        };
                        (section, (summary, BTreeMap::new()))
                    })
                    .collect();
            for candidate in &results.candidates {
                let (summary, by_rule) = sections
                    .get_mut(&Section::of(rules, candidate))
                    .expect("every section is listed");
                summary.items += 1;
                summary.size += candidate.found.size;
                summary.preselected += usize::from(candidate.preselected());
                let rule = by_rule
                    .entry(&candidate.found.rule)
                    .or_insert_with(|| RuleSummary {
                        rule: candidate.found.rule.clone(),
                        items: 0,
                        size: Size::default(),
                    });
                rule.items += 1;
                rule.size += candidate.found.size;
            }
            let sections = sections
                .into_values()
                .map(|(mut summary, by_rule)| {
                    summary.rules = by_rule.into_values().collect();
                    summary.rules.sort_by(|a, b| {
                        b.size
                            .on_disk
                            .cmp(&a.size.on_disk)
                            .then_with(|| a.rule.cmp(&b.rule))
                    });
                    summary
                })
                .collect();
            Ok(Summary {
                scan: current.id,
                roots: results.roots.clone(),
                entries: results.entries,
                elapsed: results.elapsed,
                cancelled: results.cancelled,
                unreadable: results.unreadable.len(),
                sections,
                removed: current.removed,
            })
        })
    }

    /// The candidates in `section`, and of `rule` if given, largest first.
    pub fn candidates(
        &self,
        scan: ScanId,
        section: Section,
        rule: Option<&RuleId>,
        offset: usize,
        limit: usize,
    ) -> Result<Page<Candidate>, SessionError> {
        self.read(scan, |current| {
            let rules = self.engine.rules();
            let mut listed: Vec<&Candidate> = current
                .analysis
                .scan
                .candidates
                .iter()
                .filter(|candidate| {
                    Section::of(rules, candidate) == section
                        && rule.is_none_or(|rule| &candidate.found.rule == rule)
                })
                .collect();
            listed.sort_by(|a, b| {
                b.found
                    .size
                    .on_disk
                    .cmp(&a.found.size.on_disk)
                    .then_with(|| a.found.path.cmp(&b.found.path))
            });
            Ok(Page {
                total: listed.len(),
                offset,
                items: listed
                    .into_iter()
                    .skip(offset)
                    .take(limit)
                    .cloned()
                    .collect(),
            })
        })
    }

    pub fn explain(&self, scan: ScanId, id: CandidateId) -> Result<Explained, SessionError> {
        self.read(scan, |current| {
            let candidate = find(current, id)?;
            let rule = self
                .engine
                .rules()
                .get(&candidate.found.rule)
                .ok_or(SessionError::UnknownCandidate(id))?;
            Ok(Explained {
                candidate: candidate.clone(),
                rule: rule.clone(),
                section: Section::of(self.engine.rules(), candidate),
            })
        })
    }

    /// The folder `node` names, from [`NodeId::index`], with its largest folders up to
    /// `limit`, or the scan's roots if there's no `node`. Only items in `categories` count as
    /// removable, or every item if there are none, as `--category` does.
    pub fn folder(
        &self,
        scan: ScanId,
        node: Option<u32>,
        limit: usize,
        categories: &[Category],
    ) -> Result<FolderView, SessionError> {
        self.read(scan, |current| {
            let tree = &current.analysis.tree;
            let counted = |candidate: &&Candidate| {
                categories.is_empty() || categories.contains(&candidate.found.category)
            };
            let entry = |id: NodeId| folder_entry(current, id, counted);
            let Some(index) = node else {
                let roots = tree.roots();
                let shown = roots.len().min(limit);
                return Ok(FolderView {
                    trail: Vec::new(),
                    entries: roots[..shown].iter().map(|&id| entry(id)).collect(),
                    hidden: roots.len() - shown,
                    hidden_on_disk: roots[shown..]
                        .iter()
                        .map(|&id| tree.node(id).usage.on_disk)
                        .sum(),
                    own_files: 0,
                    own_usage: Usage::default(),
                    own_removable: 0,
                });
            };
            let id = tree
                .node_id(index)
                .ok_or(SessionError::UnknownFolder(index))?;
            let mut trail = Vec::new();
            let mut at = Some(id);
            while let Some(id) = at {
                trail.push(entry(id));
                at = tree.node(id).parent;
            }
            trail.reverse();
            let children = tree.children(id);
            let shown = children.len().min(limit);
            let node = tree.node(id);
            let path = tree.path(id);
            Ok(FolderView {
                trail,
                entries: children[..shown]
                    .iter()
                    .map(|&child| entry(child))
                    .collect(),
                hidden: children.len() - shown,
                hidden_on_disk: children[shown..]
                    .iter()
                    .map(|&child| tree.node(child).usage.on_disk)
                    .sum(),
                own_files: node.own_files,
                own_usage: node.own_usage,
                own_removable: removable_under(&current.analysis.scan.candidates, &path)
                    .filter(counted)
                    .filter(|candidate| {
                        candidate.found.kind == EntryKind::File
                            && candidate.found.path.parent() == Some(path.as_path())
                    })
                    .map(|candidate| candidate.found.size.on_disk)
                    .sum(),
            })
        })
    }

    /// Removes the candidates `ids` names from scan `scan`, each the way its tier says: deleted,
    /// or quarantined if it's a Review item. Each is checked again first, as `cleanse clean`
    /// does. What's removed leaves the results.
    pub fn clean(
        &self,
        scan: ScanId,
        ids: &[CandidateId],
        progress: &dyn Fn(usize),
    ) -> Result<Cleaned, SessionError> {
        let _busy = self.start()?;
        let (plan, order) = self.read(scan, |current| {
            let mut seen = HashSet::new();
            let mut items = Vec::new();
            let mut order = Vec::new();
            for &id in ids {
                if !seen.insert(id) {
                    continue;
                }
                let item =
                    PlanItem::approve(find(current, id)?).ok_or(SessionError::NotRemovable(id))?;
                items.push(item);
                order.push(id);
            }
            let plan = Plan {
                roots: current.analysis.scan.roots.clone(),
                items,
            };
            Ok((plan, order))
        })?;
        if plan.items.is_empty() {
            return Ok(Cleaned {
                journal: None,
                outcomes: Vec::new(),
            });
        }

        let applied = self.engine.apply(&plan, progress, &self.cancel)?;
        let outcomes: Vec<(CandidateId, Outcome)> =
            order.into_iter().zip(applied.outcomes).collect();
        // Nothing else could replace the scan while this ran, since the session was busy.
        if let Some(current) = self.lock().as_mut() {
            let gone: HashSet<CandidateId> = outcomes
                .iter()
                .filter(|(_, outcome)| {
                    matches!(
                        outcome,
                        Outcome::Deleted { .. } | Outcome::Quarantined { .. }
                    )
                })
                .map(|(id, _)| *id)
                .collect();
            let removed = &mut current.removed;
            current.analysis.scan.candidates.retain(|candidate| {
                let keep = !gone.contains(&candidate.id);
                if !keep {
                    *removed += candidate.found.size;
                }
                keep
            });
        }
        Ok(Cleaned {
            journal: Some(applied.journal),
            outcomes,
        })
    }

    pub fn quarantined(&self) -> io::Result<Listing> {
        self.engine.quarantined()
    }

    /// Puts back the quarantined items `ids` names, unless something has taken their place.
    pub fn restore(&self, ids: &[String]) -> Result<HandledEntries, SessionError> {
        let _busy = self.start()?;
        let entries = self.entries(ids)?;
        if entries.is_empty() {
            return Ok(HandledEntries::default());
        }
        let handled = self.engine.restore(&entries)?;
        Ok(HandledEntries::new(entries, handled))
    }

    /// Deletes the quarantined items `ids` names for good.
    pub fn purge(&self, ids: &[String]) -> Result<HandledEntries, SessionError> {
        let _busy = self.start()?;
        let entries = self.entries(ids)?;
        if entries.is_empty() {
            return Ok(HandledEntries::default());
        }
        let handled = self.engine.purge(&entries)?;
        Ok(HandledEntries::new(entries, handled))
    }

    pub fn history(&self) -> io::Result<History> {
        self.engine.history()
    }

    /// Removes what's left of every removal that didn't finish, if it's still what was
    /// approved, as `cleanse history --finish` does.
    pub fn finish(&self) -> Result<Finished, SessionError> {
        let _busy = self.start()?;
        let leftovers = self.engine.history()?.leftovers;
        if leftovers.is_empty() {
            return Ok(Finished {
                journal: None,
                outcomes: Vec::new(),
            });
        }
        let applied = self.engine.finish(&leftovers)?;
        Ok(Finished {
            journal: Some(applied.journal),
            outcomes: leftovers.into_iter().zip(applied.outcomes).collect(),
        })
    }

    /// Marks the session busy for one scan or change, refusing if another is running.
    fn start(&self) -> Result<Busy<'_>, SessionError> {
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| SessionError::Busy)?;
        self.cancel.store(false, Ordering::Relaxed);
        Ok(Busy(&self.busy))
    }

    fn lock(&self) -> MutexGuard<'_, Option<Current>> {
        self.current.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `read` on the results of scan `scan`, if they're still the current ones.
    fn read<T>(
        &self,
        scan: ScanId,
        read: impl FnOnce(&Current) -> Result<T, SessionError>,
    ) -> Result<T, SessionError> {
        match self.lock().as_ref() {
            Some(current) if current.id == scan => read(current),
            _ => Err(SessionError::OldScan),
        }
    }

    /// The quarantine entries `ids` names, each once. An ID that isn't in quarantine is an
    /// error, so a mistake never quietly does less than asked.
    fn entries(&self, ids: &[String]) -> Result<Vec<QuarantineEntry>, SessionError> {
        let listing = self.engine.quarantined()?;
        let mut seen = HashSet::new();
        ids.iter()
            .filter(|id| seen.insert(*id))
            .map(|id| {
                listing
                    .entries
                    .iter()
                    .find(|entry| &entry.id == id)
                    .cloned()
                    .ok_or_else(|| SessionError::UnknownEntry(id.clone()))
            })
            .collect()
    }
}

fn find(current: &Current, id: CandidateId) -> Result<&Candidate, SessionError> {
    current
        .analysis
        .scan
        .candidates
        .iter()
        .find(|candidate| candidate.id == id)
        .ok_or(SessionError::UnknownCandidate(id))
}

/// The entry for folder `id`, counting as removable the candidates `counted` keeps. An item is
/// labeled with its candidate whatever it counts.
fn folder_entry(
    current: &Current,
    id: NodeId,
    counted: impl Fn(&&Candidate) -> bool,
) -> FolderEntry {
    let tree = &current.analysis.tree;
    let candidates = &current.analysis.scan.candidates;
    let node = tree.node(id);
    let path = tree.path(id);
    FolderEntry {
        node: id,
        name: node.name.to_os_string(),
        removable: removable_under(candidates, &path)
            .filter(counted)
            .map(|candidate| candidate.found.size.on_disk)
            .sum(),
        candidate: (node.kind == NodeKind::Item)
            .then(|| candidate_at(candidates, &path))
            .flatten(),
        kind: node.kind,
        usage: node.usage,
        files: node.files,
        incomplete: node.incomplete,
        has_children: !tree.children(id).is_empty(),
        path,
    }
}

/// The candidate at `path`. A scan's candidates are sorted by path.
fn candidate_at(candidates: &[Candidate], path: &Path) -> Option<CandidateId> {
    candidates
        .binary_search_by(|candidate| candidate.found.path.as_path().cmp(path))
        .ok()
        .map(|at| candidates[at].id)
}
