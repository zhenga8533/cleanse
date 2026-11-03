//! What commands return, shaped for the web view. `ts-rs` writes a TypeScript type for each to
//! `apps/desktop/src/bindings/` when the tests run, and CI fails if those are out of date.
//!
//! Paths are here to be shown, never sent back: the web view names items by their IDs.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::path::Path;

use cleanse_engine::{self as engine, Explained, NodeKind, Progress, RuleSet, Summary, advice};
use cleanse_model::{self as model, Candidate};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct AppInfo {
    pub version: String,
    /// Cleanse runs as an administrator or root, so it won't change anything.
    pub elevated: bool,
}

/// The folders a scan covers.
#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct Folders {
    pub folders: Vec<String>,
    /// None were chosen, so `folders` is the home folder.
    pub home: bool,
}

/// How far a scan has got.
#[derive(Clone, Copy, Debug, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) enum ScanProgress {
    /// Looking through folders for things rules recognize.
    Walking {
        #[ts(type = "number")]
        entries: u64,
    },
    /// Measuring and checking what the rules found.
    Assessing { done: usize, total: usize },
}

impl From<Progress> for ScanProgress {
    fn from(progress: Progress) -> Self {
        match progress {
            Progress::Walking { entries } => Self::Walking { entries },
            Progress::Assessing { done, total } => Self::Assessing { done, total },
        }
    }
}

/// How much space something takes, and how much removing it would free, in bytes.
#[derive(Clone, Copy, Debug, Default, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct Size {
    #[ts(type = "number")]
    pub logical: u64,
    #[ts(type = "number")]
    pub on_disk: u64,
    /// The part of `onDisk` that removing it is certain to free. Data shared with files
    /// elsewhere isn't counted.
    #[ts(type = "number")]
    pub guaranteed: u64,
}

impl From<model::Size> for Size {
    fn from(size: model::Size) -> Self {
        Self {
            logical: size.logical,
            on_disk: size.on_disk,
            guaranteed: size.guaranteed,
        }
    }
}

/// Where an item is listed in the results.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) enum Section {
    /// Safe and Regenerable items.
    Removable,
    /// Review items, which only a person can decide about.
    Review,
    /// Kept, because their own tool cleans them better.
    Advice,
    /// Kept by a safety check or their rule.
    Kept,
}

impl From<engine::Section> for Section {
    fn from(section: engine::Section) -> Self {
        match section {
            engine::Section::Removable => Self::Removable,
            engine::Section::Review => Self::Review,
            engine::Section::Advice => Self::Advice,
            engine::Section::Kept => Self::Kept,
        }
    }
}

impl From<Section> for engine::Section {
    fn from(section: Section) -> Self {
        match section {
            Section::Removable => Self::Removable,
            Section::Review => Self::Review,
            Section::Advice => Self::Advice,
            Section::Kept => Self::Kept,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) enum Risk {
    Safe,
    Regenerable,
    Review,
    Protected,
}

impl From<model::Risk> for Risk {
    fn from(risk: model::Risk) -> Self {
        match risk {
            model::Risk::Safe => Self::Safe,
            model::Risk::Regenerable => Self::Regenerable,
            model::Risk::Review => Self::Review,
            model::Risk::Protected => Self::Protected,
        }
    }
}

/// How a cleanup removes an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) enum Disposal {
    Delete,
    Quarantine,
    Trash,
}

impl From<model::Disposal> for Disposal {
    fn from(disposal: model::Disposal) -> Self {
        match disposal {
            model::Disposal::Delete => Self::Delete,
            model::Disposal::Quarantine => Self::Quarantine,
            model::Disposal::Trash => Self::Trash,
        }
    }
}

/// A reason to be careful about removing an item.
#[derive(Clone, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct Concern {
    pub message: String,
    /// It keeps the item. Otherwise it's a warning, and the item isn't selected to begin with.
    pub blocks: bool,
}

/// What a scan found, section by section.
#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct Results {
    #[ts(type = "number")]
    pub scan: u64,
    pub roots: Vec<String>,
    /// Files, folders, and links looked at.
    #[ts(type = "number")]
    pub entries: u64,
    pub seconds: f64,
    /// The scan was cancelled, so the results are partial.
    pub cancelled: bool,
    /// How many folders couldn't be read.
    pub unreadable: usize,
    /// Every section, in the order they're shown, even the empty ones.
    pub sections: Vec<SectionResults>,
    /// What's been removed since the scan.
    pub removed: Size,
    /// What a cleanup selects to begin with, as `cleanse clean` does: Safe and Regenerable items
    /// with nothing to warn about.
    pub preselected: Vec<Choice>,
    /// What can be removed in each category, largest first.
    pub categories: Vec<CategoryTotal>,
}

impl Results {
    pub(crate) fn new(
        summary: Summary,
        rules: &RuleSet,
        preselected: Vec<Choice>,
        categories: Vec<CategoryTotal>,
    ) -> Self {
        Self {
            scan: summary.scan.0,
            roots: summary.roots.iter().map(|root| lossy(root)).collect(),
            entries: summary.entries,
            seconds: summary.elapsed.as_secs_f64(),
            cancelled: summary.cancelled,
            unreadable: summary.unreadable,
            sections: summary
                .sections
                .into_iter()
                .map(|section| SectionResults {
                    section: section.section.into(),
                    items: section.items,
                    size: section.size.into(),
                    rules: section
                        .rules
                        .into_iter()
                        .map(|rule| RuleTotal {
                            name: rule_name(rules, &rule.rule),
                            rule: rule.rule.0,
                            items: rule.items,
                            size: rule.size.into(),
                        })
                        .collect(),
                })
                .collect(),
            removed: summary.removed.into(),
            preselected,
            categories,
        }
    }
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct SectionResults {
    pub section: Section,
    pub items: usize,
    pub size: Size,
    /// The rules with items here, largest first.
    pub rules: Vec<RuleTotal>,
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct RuleTotal {
    pub rule: String,
    pub name: String,
    pub items: usize,
    pub size: Size,
}

/// What can be removed in one category.
#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct CategoryTotal {
    /// The name `--category` takes, like `build_output`.
    pub category: String,
    pub name: String,
    pub items: usize,
    pub size: Size,
}

impl CategoryTotal {
    /// The totals of `removable`, by category, largest first.
    pub(crate) fn of<'c>(removable: impl IntoIterator<Item = &'c Candidate>) -> Vec<Self> {
        let mut by_category: BTreeMap<model::Category, (usize, model::Size)> = BTreeMap::new();
        for candidate in removable {
            let total = by_category.entry(candidate.found.category).or_default();
            total.0 += 1;
            total.1 += candidate.found.size;
        }
        let mut totals: Vec<Self> = by_category
            .into_iter()
            .map(|(category, (items, size))| Self {
                category: category.id().to_owned(),
                name: category.to_string(),
                items,
                size: size.into(),
            })
            .collect();
        totals.sort_by_key(|total| Reverse(total.size.on_disk));
        totals
    }
}

/// An item a cleanup can remove, and how.
#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct Choice {
    #[ts(type = "number")]
    pub id: u64,
    pub size: Size,
    pub disposal: Disposal,
}

impl Choice {
    /// Protected items can't be chosen, so they have none.
    pub(crate) fn of(candidate: &Candidate) -> Option<Self> {
        Some(Self {
            id: candidate.id.0,
            size: candidate.found.size.into(),
            disposal: candidate.default_disposal()?.into(),
        })
    }
}

/// Something a rule recognized.
#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct Item {
    #[ts(type = "number")]
    pub id: u64,
    pub path: String,
    pub rule: String,
    pub risk: Risk,
    pub size: Size,
    /// The number of files, counting the item itself if it's a file.
    #[ts(type = "number")]
    pub files: u64,
    /// When its most recently modified file was modified, in seconds since the Unix epoch.
    #[ts(type = "number | null")]
    pub modified: Option<u64>,
    pub concerns: Vec<Concern>,
    /// How a cleanup removes it, unless it's kept.
    pub disposal: Option<Disposal>,
    /// The command that cleans it better than removing it would, if its own tool does.
    pub command: Option<String>,
    pub preselected: bool,
}

impl Item {
    pub(crate) fn new(candidate: &Candidate, rules: &RuleSet) -> Self {
        Self {
            id: candidate.id.0,
            path: lossy(&candidate.found.path),
            rule: candidate.found.rule.0.clone(),
            risk: candidate.risk().into(),
            size: candidate.found.size.into(),
            files: candidate.found.files,
            modified: candidate.found.modified,
            concerns: candidate
                .concerns()
                .iter()
                .map(|concern| Concern {
                    message: concern.to_string(),
                    blocks: concern.blocks(),
                })
                .collect(),
            disposal: candidate.default_disposal().map(Disposal::from),
            command: advice(rules, candidate).map(str::to_owned),
            preselected: candidate.preselected(),
        }
    }
}

/// Part of a section's items.
#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct ItemPage {
    pub items: Vec<Item>,
    /// Where in the list `items` starts.
    pub offset: usize,
    /// How long the whole list is.
    pub total: usize,
}

/// An item, the rule that recognized it, and why it can be removed or is kept.
#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct Details {
    pub item: Item,
    pub section: Section,
    pub rule_name: String,
    /// Why what the rule recognizes is safe to remove.
    pub explain: String,
    /// How it comes back, when that takes a deliberate step.
    pub regenerate: Option<String>,
    /// What the rule found that supports removing it.
    pub evidence: Vec<String>,
}

impl Details {
    pub(crate) fn new(explained: &Explained, rules: &RuleSet) -> Self {
        Self {
            item: Item::new(&explained.candidate, rules),
            section: explained.section.into(),
            rule_name: explained.rule.name.clone(),
            explain: explained.rule.explain.clone(),
            regenerate: explained.rule.regenerate.clone(),
            evidence: explained
                .candidate
                .found
                .evidence
                .iter()
                .map(ToString::to_string)
                .collect(),
        }
    }
}

/// What a cleanup did.
#[derive(Debug, Default, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct CleanReport {
    pub deleted: Amount,
    /// Moved into quarantine, where they take space until they're purged.
    pub quarantined: Amount,
    /// Left alone, because something about them changed since the scan, or the cleanup stopped
    /// first.
    pub skipped: Vec<Problem>,
    /// Attempted, and couldn't be removed.
    pub failed: Vec<Problem>,
    /// The journal file recording the cleanup, unless there was nothing to remove.
    pub journal: Option<String>,
}

#[derive(Debug, Default, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct Amount {
    pub items: usize,
    pub size: Size,
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct Problem {
    pub path: String,
    pub reason: String,
}

/// How much space a folder takes, in bytes.
#[derive(Clone, Copy, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct Usage {
    #[ts(type = "number")]
    pub logical: u64,
    #[ts(type = "number")]
    pub on_disk: u64,
}

impl From<engine::Usage> for Usage {
    fn from(usage: engine::Usage) -> Self {
        Self {
            logical: usage.logical,
            on_disk: usage.on_disk,
        }
    }
}

/// What a folder entry is.
#[derive(Clone, Copy, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) enum EntryKind {
    Folder,
    /// Something a rule recognized, counted whole.
    Item,
    /// A folder on another filesystem, which the scan didn't enter or measure.
    OtherFilesystem,
}

impl From<NodeKind> for EntryKind {
    fn from(kind: NodeKind) -> Self {
        match kind {
            NodeKind::Folder => Self::Folder,
            NodeKind::Item => Self::Item,
            NodeKind::OtherFilesystem => Self::OtherFilesystem,
        }
    }
}

/// One folder of a scan's size tree, or its roots, with its largest entries.
#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct FolderView {
    /// The folders from a root down to this one. Empty for the roots.
    pub trail: Vec<FolderEntry>,
    /// Its largest entries, or the roots, largest first.
    pub entries: Vec<FolderEntry>,
    /// How many `entries` leaves out, and how much they take.
    pub hidden: usize,
    #[ts(type = "number")]
    pub hidden_on_disk: u64,
    /// The files directly in it, which have no entries of their own.
    #[ts(type = "number")]
    pub own_files: u64,
    pub own_usage: Usage,
    #[ts(type = "number")]
    pub own_removable: u64,
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct FolderEntry {
    /// Names the folder for the `folder` command.
    pub node: u32,
    /// A root's whole path, or the name inside its folder.
    pub name: String,
    pub path: String,
    pub kind: EntryKind,
    pub usage: Usage,
    #[ts(type = "number")]
    pub files: u64,
    /// How much at or inside it can be removed, counting only the categories asked for.
    #[ts(type = "number")]
    pub removable: u64,
    /// Part of it couldn't be read, so it takes more than shown.
    pub incomplete: bool,
    /// It has entries inside it to show.
    pub has_children: bool,
    /// The item a rule recognized here, unless it's been removed since the scan.
    pub item: Option<EntryItem>,
}

impl FolderEntry {
    pub(crate) fn new(entry: &engine::FolderEntry, item: Option<EntryItem>) -> Self {
        Self {
            node: entry.node.index(),
            name: entry.name.to_string_lossy().into_owned(),
            path: lossy(&entry.path),
            kind: entry.kind.into(),
            usage: entry.usage.into(),
            files: entry.files,
            removable: entry.removable,
            incomplete: entry.incomplete,
            has_children: entry.has_children,
            item,
        }
    }
}

/// What a rule made of an item in the size tree.
#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct EntryItem {
    #[ts(type = "number")]
    pub id: u64,
    pub rule_name: String,
    pub risk: Risk,
    /// The command that cleans it better, if its own tool does.
    pub command: Option<String>,
}

impl EntryItem {
    pub(crate) fn new(explained: &Explained, rules: &RuleSet) -> Self {
        Self {
            id: explained.candidate.id.0,
            rule_name: explained.rule.name.clone(),
            risk: explained.candidate.risk().into(),
            command: advice(rules, &explained.candidate).map(str::to_owned),
        }
    }
}

/// For display only: names that aren't valid Unicode get replacement characters.
pub(crate) fn lossy(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A rule's name, or its ID if the rules don't have it.
fn rule_name(rules: &RuleSet, id: &model::RuleId) -> String {
    rules
        .get(id)
        .map_or_else(|| id.0.clone(), |rule| rule.name.clone())
}
