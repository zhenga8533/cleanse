use std::cell::OnceCell;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::{self, Metadata};
use std::io;
use std::path::{self, Component, Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use cleanse_model::{EntryKind, Evidence};
use cleanse_platform::{KnownFolders, SystemFolders, listed_facts};

use super::{Base, Entry, Location, Os, Rule, Scope, Target};
use crate::TOMBSTONE_PREFIX;
use crate::git::{GitInfo, GitStatus};
use crate::scan::{Descend, WalkReport, walk};

const CACHEDIR_TAG: &str = "CACHEDIR.TAG";

/// What a folder listing says about an entry: all a rule looks at besides its path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Listed {
    pub kind: EntryKind,
    /// Bytes on disk, as [`listed_facts`] gives them, for files. Cloud files whose data isn't on
    /// this device take none.
    pub on_disk: u64,
}

impl Listed {
    /// Reads what a walk, `explain`, and the check before removal all go by, so they agree.
    pub fn of(metadata: &Metadata) -> Self {
        let kind = EntryKind::from(metadata.file_type());
        Self {
            kind,
            on_disk: if kind == EntryKind::File {
                listed_facts(metadata).on_disk
            } else {
                0
            },
        }
    }
}

/// A file or directory that a rule recognized.
#[derive(Clone, Debug)]
pub struct Detection<'r> {
    pub rule: &'r Rule,
    pub path: PathBuf,
    pub kind: EntryKind,
    pub evidence: Vec<Evidence>,
    /// What Git says about the item, which the safety checks use to rule out tracked files.
    pub git: GitStatus,
}

/// Matches files and directories against a set of rules, with known locations resolved for one
/// user and OS, or for system cleanup.
#[derive(Debug)]
pub struct Matcher<'r> {
    artifacts: Vec<&'r Rule>,
    locations: HashMap<PathBuf, &'r Rule>,
    patterns: Vec<Pattern<'r>>,
    /// The depth, in path components, and the last name, compared like [`fold_name`], of every
    /// location that names files. A walk sees far more files than folders, so a file at a depth
    /// or with a name none of them has is ruled out before its whole path is compared.
    file_locations: Vec<(usize, String)>,
    large_files: Option<LargeFiles<'r>>,
    /// `None` in system cleanup, which doesn't read Git.
    git: Option<GitInfo>,
}

/// A known location with a `*` in its path, so it's compared name by name below its base.
#[derive(Debug)]
struct Pattern<'r> {
    base: PathBuf,
    names: Vec<String>,
    rule: &'r Rule,
}

/// The `large_file` rule, and where it doesn't look.
#[derive(Debug)]
struct LargeFiles<'r> {
    rule: &'r Rule,
    /// Files taking fewer bytes on disk aren't large. Nothing is, until
    /// [`Matcher::with_large_files`] says.
    min: u64,
    /// The folders apps keep their data in, compared like [`location_key`]. What's large in them
    /// is the apps' own business, and rules for those folders say what's safe there.
    app_data: Vec<PathBuf>,
    home: Option<PathBuf>,
}

impl<'r> Matcher<'r> {
    /// Matches user rules, with the user's folders. System rules are left out, as are locations
    /// whose base folder isn't defined on this system.
    pub fn new(rules: impl IntoIterator<Item = &'r Rule>, folders: &KnownFolders, os: Os) -> Self {
        let mut matcher = Self::empty(Some(GitInfo::new()));
        for rule in rules.into_iter().filter(|rule| rule.scope == Scope::User) {
            match &rule.target {
                Target::ProjectArtifact { .. } => matcher.artifacts.push(rule),
                Target::LargeFile {} => {
                    matcher.large_files = Some(LargeFiles {
                        rule,
                        min: u64::MAX,
                        app_data: app_data_folders(folders, os),
                        home: folders.home.as_deref().map(location_key),
                    });
                }
                Target::KnownLocation { locations, entry } => {
                    matcher.add_locations(rule, locations, *entry, os, |base| {
                        base_folder(folders, base)
                    });
                }
            }
        }
        matcher
    }

    /// Matches system rules, with system cleanup's folders, and leaves every other rule out.
    /// Git isn't read: system cleanup reads nothing the user could have written, and anyone can
    /// create a repository above a system folder.
    pub fn system(
        rules: impl IntoIterator<Item = &'r Rule>,
        folders: &SystemFolders,
        os: Os,
    ) -> Self {
        let mut matcher = Self::empty(None);
        for rule in rules.into_iter().filter(|rule| rule.scope == Scope::System) {
            if let Target::KnownLocation { locations, entry } = &rule.target {
                matcher.add_locations(rule, locations, *entry, os, |base| {
                    system_base_folder(folders, base)
                });
            }
        }
        matcher
    }

    fn empty(git: Option<GitInfo>) -> Self {
        Self {
            artifacts: Vec::new(),
            locations: HashMap::new(),
            patterns: Vec::new(),
            file_locations: Vec::new(),
            large_files: None,
            git,
        }
    }

    /// Adds `rule`'s locations on `os` whose base folder `base_folder` defines.
    fn add_locations<'f>(
        &mut self,
        rule: &'r Rule,
        locations: &[Location],
        entry: Entry,
        os: Os,
        base_folder: impl Fn(Base) -> Option<&'f Path>,
    ) {
        for spec in locations.iter().filter(|spec| spec.os == os) {
            let Some(base) = base_folder(spec.base) else {
                continue;
            };
            if entry.allows(EntryKind::File) {
                let depth = join_relative(base, &spec.path).components().count();
                let name = spec.path.rsplit('/').next().unwrap_or_default();
                self.file_locations.push((depth, fold_name(name)));
            }
            if spec.path.contains('*') {
                self.patterns.push(Pattern {
                    base: location_key(base),
                    names: spec.path.split('/').map(fold_name).collect(),
                    rule,
                });
            } else {
                self.locations
                    .insert(location_key(&join_relative(base, &spec.path)), rule);
            }
        }
    }

    fn git_status(&self, path: &Path, kind: EntryKind) -> GitStatus {
        self.git
            .as_ref()
            .map_or(GitStatus::NotChecked, |git| git.status(path, kind))
    }

    /// Makes the `large_file` rule, if there is one, match files taking at least `min` bytes on
    /// disk. Without this, it matches nothing.
    #[must_use]
    pub fn with_large_files(mut self, min: u64) -> Self {
        if let Some(large_files) = &mut self.large_files {
            large_files.min = min;
        }
        self
    }

    /// Checks one file or directory, as found by a walk. Known locations only match absolute
    /// paths, and project artifacts are only directories. A file no other rule recognizes can
    /// still be a large file. Links and Cleanse's own tombstones never match.
    ///
    /// Evidence that can't be read counts as missing, so a project that can't be inspected isn't
    /// matched. The walk reports the unreadable directories itself.
    pub fn match_entry(&self, path: &Path, listed: Listed) -> Option<Detection<'r>> {
        let kind = listed.kind;
        let large = self
            .large_files
            .as_ref()
            .filter(|large| kind == EntryKind::File && listed.on_disk >= large.min);
        match kind {
            EntryKind::Dir => {}
            EntryKind::File if large.is_some() || self.may_be_file_location(path) => {}
            EntryKind::File | EntryKind::Link | EntryKind::Other => return None,
        }
        if path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.starts_with(TOMBSTONE_PREFIX))
        {
            return None;
        }
        let key = location_key(path);
        let location = self
            .locations
            .get(&key)
            .copied()
            .filter(|rule| location_entry(rule).allows(kind))
            .or_else(|| self.match_pattern(&key, kind))
            .or_else(|| {
                large
                    .filter(|large| !large.in_app_data(&key))
                    .map(|large| large.rule)
            });
        if let Some(rule) = location {
            return Some(Detection {
                rule,
                path: path.to_path_buf(),
                kind,
                evidence: Vec::new(),
                git: self.git_status(path, kind),
            });
        }
        if kind != EntryKind::Dir {
            return None;
        }

        // Rules are written in ASCII, so a name that isn't valid Unicode can't match one.
        let name = path.file_name()?.to_str()?;
        let parent = path.parent()?;
        // Looked up only once a rule's name and evidence match, and at most once per directory.
        let git = OnceCell::new();
        self.artifacts.iter().find_map(|rule| {
            let Target::ProjectArtifact {
                names,
                markers,
                contains,
                require_git_ignored,
            } = &rule.target
            else {
                return None;
            };
            if !names.iter().any(|pattern| wildcard_match(pattern, name)) {
                return None;
            }
            let mut evidence = artifact_evidence(path, parent, markers, contains)?;
            let git = git.get_or_init(|| self.git_status(path, kind));
            let ignored = matches!(git, GitStatus::Known(facts) if facts.ignored);
            if *require_git_ignored && !ignored {
                return None;
            }
            if ignored {
                evidence.push(Evidence::IgnoredByGit);
            }
            Some(Detection {
                rule,
                path: path.to_path_buf(),
                kind,
                evidence,
                git: git.clone(),
            })
        })
    }

    /// Whether the file at `path` is at the depth, and has the name, of a location that names
    /// files.
    fn may_be_file_location(&self, path: &Path) -> bool {
        if self.file_locations.is_empty() {
            return false;
        }
        let depth = path.components().count();
        if !self.file_locations.iter().any(|(at, _)| *at == depth) {
            return false;
        }
        path.file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| {
                let name = fold_name(name);
                self.file_locations
                    .iter()
                    .any(|(at, wanted)| *at == depth && wildcard_match(wanted, &name))
            })
    }

    /// The rule whose location, written with a `*`, is the `kind` of item with this key.
    fn match_pattern(&self, key: &Path, kind: EntryKind) -> Option<&'r Rule> {
        self.patterns.iter().find_map(|pattern| {
            if !location_entry(pattern.rule).allows(kind) {
                return None;
            }
            let mut names = key.strip_prefix(&pattern.base).ok()?.iter();
            let all_match = pattern.names.iter().all(|wanted| {
                names
                    .next()
                    .and_then(OsStr::to_str)
                    .is_some_and(|name| wildcard_match(wanted, name))
            });
            (all_match && names.next().is_none()).then_some(pattern.rule)
        })
    }

    /// Walks everything below `root`, sorted by path, without entering the directories it
    /// matches: anything inside one is part of that match.
    pub fn find(&self, root: &Path) -> io::Result<(Vec<Detection<'r>>, WalkReport)> {
        let root = path::absolute(root)?;
        let found = Mutex::new(Vec::new());
        let report = walk(&root, |entry| {
            match self.match_entry(entry.path, Listed::of(entry.metadata)) {
                Some(detection) => {
                    found
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(detection);
                    Descend::No
                }
                None => Descend::Yes,
            }
        })?;
        let mut found = found.into_inner().unwrap_or_else(PoisonError::into_inner);
        found.sort_by(|a, b| a.path.cmp(&b.path));
        Ok((found, report))
    }
}

impl LargeFiles<'_> {
    /// Whether the file with this key is somewhere apps keep their data: in one of `app_data`,
    /// or with a name starting with `.`, as tools name their files and folders, in its path.
    /// Names above the home folder don't count, so a home folder anywhere works.
    fn in_app_data(&self, key: &Path) -> bool {
        if self.app_data.iter().any(|folder| key.starts_with(folder)) {
            return true;
        }
        let below_home = self
            .home
            .as_deref()
            .and_then(|home| key.strip_prefix(home).ok())
            .unwrap_or(key);
        below_home.components().any(|part| {
            matches!(part, Component::Normal(name) if name.as_encoded_bytes().starts_with(b"."))
        })
    }
}

/// The folders apps keep their data in, compared like [`location_key`].
fn app_data_folders(folders: &KnownFolders, os: Os) -> Vec<PathBuf> {
    let home = |relative: &str| {
        folders
            .home
            .as_deref()
            .map(|home| join_relative(home, relative))
    };
    let own = match os {
        // `Local`, `LocalLow`, and `Roaming`.
        Os::Windows => home("AppData"),
        Os::Macos => home("Library"),
        // Snap packages keep each app's data here.
        Os::Linux => home("snap"),
    };
    [
        folders.cache.clone(),
        folders.config.clone(),
        folders.data_local.clone(),
        Some(folders.temp.clone()),
        own,
    ]
    .into_iter()
    .flatten()
    // An undefined folder can be left empty, which would contain every path.
    .filter(|folder| folder.is_absolute())
    .map(|folder| location_key(&folder))
    .collect()
}

/// What a known location rule's locations name.
fn location_entry(rule: &Rule) -> Entry {
    match &rule.target {
        Target::KnownLocation { entry, .. } => *entry,
        Target::ProjectArtifact { .. } => Entry::Dir,
        Target::LargeFile {} => Entry::File,
    }
}

pub(crate) fn base_folder(folders: &KnownFolders, base: Base) -> Option<&Path> {
    match base {
        Base::Home => folders.home.as_deref(),
        Base::Cache => folders.cache.as_deref(),
        Base::Config => folders.config.as_deref(),
        Base::DataLocal => folders.data_local.as_deref(),
        Base::Temp => Some(&folders.temp),
        // On Linux, `xdg-user-dirs-update` points a user folder that doesn't exist at the home
        // folder, which isn't a folder of downloads.
        Base::Downloads => folders
            .downloads
            .as_deref()
            .filter(|downloads| folders.home.as_deref() != Some(*downloads)),
        Base::SystemRoot | Base::ProgramData | Base::Var => None,
    }
}

fn system_base_folder(folders: &SystemFolders, base: Base) -> Option<&Path> {
    match base {
        Base::SystemRoot => folders.system_root.as_deref(),
        Base::ProgramData => folders.program_data.as_deref(),
        Base::Var => folders.var.as_deref(),
        Base::Home
        | Base::Cache
        | Base::Config
        | Base::DataLocal
        | Base::Temp
        | Base::Downloads => None,
    }
}

/// Windows and macOS compare names case-insensitively by default, so a known location matches
/// however its folders happen to be capitalized.
fn location_key(path: &Path) -> PathBuf {
    if cfg!(any(windows, target_os = "macos")) {
        PathBuf::from(path.to_string_lossy().to_lowercase())
    } else {
        path.to_path_buf()
    }
}

/// One name of a location, compared the way [`location_key`] compares paths.
fn fold_name(name: &str) -> String {
    if cfg!(any(windows, target_os = "macos")) {
        name.to_lowercase()
    } else {
        name.to_owned()
    }
}

/// `relative` is `/`-separated, as written in rule files.
fn join_relative(base: &Path, relative: &str) -> PathBuf {
    let mut path = base.to_path_buf();
    path.extend(relative.split('/'));
    path
}

fn artifact_evidence(
    dir: &Path,
    parent: &Path,
    markers: &[String],
    contains: &[String],
) -> Option<Vec<Evidence>> {
    let mut evidence = Vec::new();
    if !markers.is_empty() {
        let file = markers
            .iter()
            .find_map(|marker| find_marker(parent, marker))?;
        evidence.push(Evidence::Marker { file });
    }
    for file in contains {
        if !is_file(&join_relative(dir, file)) {
            return None;
        }
        if file != CACHEDIR_TAG {
            evidence.push(Evidence::Contains { file: file.clone() });
        }
    }
    if is_file(&dir.join(CACHEDIR_TAG)) {
        evidence.push(Evidence::CacheDirTag);
    }
    Some(evidence)
}

/// Returns the name of the file in `parent` that satisfies `marker`, if any.
fn find_marker(parent: &Path, marker: &str) -> Option<String> {
    if !marker.contains('*') {
        return is_file(&join_relative(parent, marker)).then(|| marker.to_owned());
    }
    fs::read_dir(parent)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .find_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            wildcard_match(marker, &name).then_some(name)
        })
}

/// A regular file, not a link to one: evidence has to be what it appears to be.
fn is_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
}

/// Matches `name` against `pattern`, where `*` stands for any run of characters.
pub(crate) fn wildcard_match(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    for part in middle {
        let Some(index) = rest.find(part) else {
            return false;
        };
        rest = &rest[index + part.len()..];
    }
    rest.len() >= last.len() && rest.ends_with(last)
}
