use std::cell::OnceCell;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{self, Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use cleanse_model::{EntryKind, Evidence};
use cleanse_platform::KnownFolders;

use super::{Base, Entry, Os, Rule, Target};
use crate::TOMBSTONE_PREFIX;
use crate::git::{GitInfo, GitStatus};
use crate::scan::{Descend, WalkReport, walk};

const CACHEDIR_TAG: &str = "CACHEDIR.TAG";

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
/// user and OS.
#[derive(Debug)]
pub struct Matcher<'r> {
    artifacts: Vec<&'r Rule>,
    locations: HashMap<PathBuf, &'r Rule>,
    patterns: Vec<Pattern<'r>>,
    /// The depth, in path components, and the last name, compared like [`fold_name`], of every
    /// location that names files. A walk sees far more files than folders, so a file at a depth
    /// or with a name none of them has is ruled out before its whole path is compared.
    file_locations: Vec<(usize, String)>,
    git: GitInfo,
}

/// A known location with a `*` in its path, so it's compared name by name below its base.
#[derive(Debug)]
struct Pattern<'r> {
    base: PathBuf,
    names: Vec<String>,
    rule: &'r Rule,
}

impl<'r> Matcher<'r> {
    /// Locations whose base folder isn't defined on this system are left out.
    pub fn new(rules: impl IntoIterator<Item = &'r Rule>, folders: &KnownFolders, os: Os) -> Self {
        let mut artifacts = Vec::new();
        let mut locations = HashMap::new();
        let mut patterns = Vec::new();
        let mut file_locations = Vec::new();
        for rule in rules {
            match &rule.target {
                Target::ProjectArtifact { .. } => artifacts.push(rule),
                Target::KnownLocation {
                    locations: specs,
                    entry,
                } => {
                    for spec in specs.iter().filter(|spec| spec.os == os) {
                        let Some(base) = base_folder(folders, spec.base) else {
                            continue;
                        };
                        if entry.allows(EntryKind::File) {
                            let depth = join_relative(base, &spec.path).components().count();
                            let name = spec.path.rsplit('/').next().unwrap_or_default();
                            file_locations.push((depth, fold_name(name)));
                        }
                        if spec.path.contains('*') {
                            patterns.push(Pattern {
                                base: location_key(base),
                                names: spec.path.split('/').map(fold_name).collect(),
                                rule,
                            });
                        } else {
                            locations.insert(location_key(&join_relative(base, &spec.path)), rule);
                        }
                    }
                }
            }
        }
        Self {
            artifacts,
            locations,
            patterns,
            file_locations,
            git: GitInfo::new(),
        }
    }

    /// Checks one file or directory of `kind`, as found by a walk. Known locations only match
    /// absolute paths, and project artifacts are only directories. Links and Cleanse's own
    /// tombstones never match.
    ///
    /// Evidence that can't be read counts as missing, so a project that can't be inspected isn't
    /// matched. The walk reports the unreadable directories itself.
    pub fn match_entry(&self, path: &Path, kind: EntryKind) -> Option<Detection<'r>> {
        match kind {
            EntryKind::Dir => {}
            EntryKind::File if self.may_be_file_location(path) => {}
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
            .or_else(|| self.match_pattern(&key, kind));
        if let Some(rule) = location {
            return Some(Detection {
                rule,
                path: path.to_path_buf(),
                kind,
                evidence: Vec::new(),
                git: self.git.status(path),
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
            let git = git.get_or_init(|| self.git.status(path));
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
            match self.match_entry(entry.path, entry.kind) {
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

/// What a known location rule's locations name.
fn location_entry(rule: &Rule) -> Entry {
    match &rule.target {
        Target::KnownLocation { entry, .. } => *entry,
        Target::ProjectArtifact { .. } => Entry::Dir,
    }
}

pub(crate) fn base_folder(folders: &KnownFolders, base: Base) -> Option<&Path> {
    match base {
        Base::Home => folders.home.as_deref(),
        Base::Cache => folders.cache.as_deref(),
        Base::Config => folders.config.as_deref(),
        Base::DataLocal => folders.data_local.as_deref(),
        Base::Temp => Some(&folders.temp),
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
