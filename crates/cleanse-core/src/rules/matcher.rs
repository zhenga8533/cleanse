use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{self, Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use cleanse_model::{EntryKind, Evidence};
use cleanse_platform::KnownFolders;

use super::{Base, Os, Rule, Target};
use crate::scan::{Descend, WalkReport, walk};

const CACHEDIR_TAG: &str = "CACHEDIR.TAG";

/// A directory that a rule recognized.
#[derive(Clone, Debug)]
pub struct Detection<'r> {
    pub rule: &'r Rule,
    pub path: PathBuf,
    pub evidence: Vec<Evidence>,
}

/// Matches directories against a set of rules, with known locations resolved for one user and
/// OS.
#[derive(Debug)]
pub struct Matcher<'r> {
    artifacts: Vec<&'r Rule>,
    locations: HashMap<PathBuf, &'r Rule>,
}

impl<'r> Matcher<'r> {
    /// Locations whose base folder isn't defined on this system are left out.
    pub fn new(rules: impl IntoIterator<Item = &'r Rule>, folders: &KnownFolders, os: Os) -> Self {
        let mut artifacts = Vec::new();
        let mut locations = HashMap::new();
        for rule in rules {
            match &rule.target {
                Target::ProjectArtifact { .. } => artifacts.push(rule),
                Target::KnownLocation { locations: specs } => {
                    for spec in specs.iter().filter(|spec| spec.os == os) {
                        if let Some(base) = base_folder(folders, spec.base) {
                            locations.insert(location_key(&join_relative(base, &spec.path)), rule);
                        }
                    }
                }
            }
        }
        Self {
            artifacts,
            locations,
        }
    }

    /// Checks one directory, as found by a walk. Known locations only match absolute paths.
    ///
    /// Evidence that can't be read counts as missing, so a project that can't be inspected isn't
    /// matched. The walk reports the unreadable directories itself.
    pub fn match_dir(&self, path: &Path) -> Option<Detection<'r>> {
        if let Some(rule) = self.locations.get(&location_key(path)) {
            return Some(Detection {
                rule,
                path: path.to_path_buf(),
                evidence: Vec::new(),
            });
        }

        // Rules are written in ASCII, so a name that isn't valid Unicode can't match one.
        let name = path.file_name()?.to_str()?;
        let parent = path.parent()?;
        self.artifacts.iter().find_map(|rule| {
            let Target::ProjectArtifact {
                names,
                markers,
                contains,
            } = &rule.target
            else {
                return None;
            };
            if !names.iter().any(|pattern| wildcard_match(pattern, name)) {
                return None;
            }
            artifact_evidence(path, parent, markers, contains).map(|evidence| Detection {
                rule,
                path: path.to_path_buf(),
                evidence,
            })
        })
    }

    /// Walks everything below `root`, sorted by path, without entering the directories it
    /// matches: anything inside one is part of that match.
    pub fn find(&self, root: &Path) -> io::Result<(Vec<Detection<'r>>, WalkReport)> {
        let root = path::absolute(root)?;
        let found = Mutex::new(Vec::new());
        let report = walk(&root, |entry| {
            if entry.kind != EntryKind::Dir {
                return Descend::Yes;
            }
            match self.match_dir(entry.path) {
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

fn base_folder(folders: &KnownFolders, base: Base) -> Option<&Path> {
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
pub(super) fn wildcard_match(pattern: &str, name: &str) -> bool {
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
