use std::collections::BTreeMap;

use cleanse_model::{Category, EntryKind, Risk, RuleId};
use cleanse_platform::process_name;
use serde::Deserialize;

/// One cleanup rule, as written in a rule file.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: RuleId,
    /// A short name for headings, like "Node.js dependencies".
    pub name: String,
    pub category: Category,
    pub risk: Risk,
    /// Why a matched item is safe to remove, in a sentence or two.
    pub explain: String,
    /// How the item comes back, when that takes a deliberate step.
    #[serde(default)]
    pub regenerate: Option<String>,
    #[serde(rename = "match")]
    pub target: Target,
    /// The app that writes what this rule matches. While it's running, matches are kept.
    #[serde(default)]
    pub owner: Option<Owner>,
    /// Keep each match until nothing in it has changed for this many days, and not since the
    /// computer last started. For places programs leave files they may still need later, like
    /// the temp folder.
    #[serde(default)]
    pub idle_days: Option<u32>,
    /// The command that cleans what this rule matches the way its own tool intends, for the
    /// person to run. Cleanse only shows it (ADR 0003). Only `protected` rules have one, so
    /// Cleanse never removes their matches itself.
    #[serde(default)]
    pub advise: Option<String>,
    /// Small directory trees, and exactly what this rule must match in each.
    #[serde(default, rename = "test")]
    pub tests: Vec<RuleTest>,
}

/// An app, and how to tell that it's running.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    /// Its name, like "Claude Code".
    pub app: String,
    /// The names of its programs, lowercase and without `.exe`, like `claude`.
    pub processes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    /// A directory inside a project, recognized by its name plus evidence about the project.
    ProjectArtifact {
        /// Directory names. `*` matches any run of characters.
        names: Vec<String>,
        /// Files next to the directory, at least one of which must exist. A pattern containing
        /// `*` matches a single file name; anything else is a path relative to the parent.
        #[serde(default)]
        markers: Vec<String>,
        /// Files inside the directory, all of which must exist.
        #[serde(default)]
        contains: Vec<String>,
        /// Match only where the project's Git repository ignores the directory. For names too
        /// common to trust on their own, like `build` or `dist`.
        #[serde(default)]
        require_git_ignored: bool,
    },
    /// A fixed location under one of the user's standard folders.
    KnownLocation {
        locations: Vec<Location>,
        /// What the locations name. Anything else at a location doesn't match.
        #[serde(default)]
        entry: Entry,
    },
    /// Any file taking at least `large_file_size` from the settings on disk, outside the folders
    /// apps keep their data in. It's checked after every other rule, so a file another rule
    /// knows stays under that rule.
    LargeFile {},
}

/// What a known location names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Entry {
    #[default]
    Dir,
    File,
    /// Files and folders alike.
    Any,
}

impl Entry {
    /// Whether an item of `kind` is what this names. A link never is.
    pub fn allows(self, kind: EntryKind) -> bool {
        match self {
            Self::Dir => kind == EntryKind::Dir,
            Self::File => kind == EntryKind::File,
            Self::Any => matches!(kind, EntryKind::Dir | EntryKind::File),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub os: Os,
    pub base: Base,
    /// A `/`-separated path relative to `base`. In names after the first, `*` matches any run
    /// of characters, for folders whose names vary, like a browser's profiles. Under `temp` and
    /// `downloads`, the first name can have `*` too.
    pub path: String,
}

/// The standard folder a [`Location`] is relative to. See
/// [`cleanse_platform::KnownFolders`] for what each one is on each OS.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Base {
    Home,
    Cache,
    Config,
    DataLocal,
    Temp,
    Downloads,
}

impl Base {
    /// Folders that hold whatever anyone put there, rather than one app's data, so a location
    /// can match by pattern directly inside them.
    fn is_shared(self) -> bool {
        matches!(self, Self::Temp | Self::Downloads)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Os {
    Windows,
    Macos,
    Linux,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else {
            Self::Linux
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleTest {
    /// Run only on this OS. Tests for a known location need it, since locations differ by OS.
    #[serde(default)]
    pub os: Option<Os>,
    /// Paths to create, `/`-separated and relative to the test's root. A trailing `/` makes a
    /// directory; anything else is an empty file. Known locations live under folders named
    /// after their base: `home/`, `cache/`, `config/`, `data_local/`, `temp/`, and
    /// `downloads/`.
    pub tree: Vec<String>,
    /// Files to create with the given contents, such as a `.gitignore`.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    /// Files to create with this many bytes in them, for rules that look at size. Tests treat
    /// files taking 64 KiB or more on disk as large.
    #[serde(default)]
    pub sizes: BTreeMap<String, u64>,
    /// Make the test's root a Git repository.
    #[serde(default)]
    pub git: bool,
    /// Files to add to the repository's index.
    #[serde(default)]
    pub tracked: Vec<String>,
    /// The paths the rule must match in the tree, and nothing else.
    pub matches: Vec<String>,
}

impl Rule {
    pub(super) fn validate(&self) -> Result<(), String> {
        validate_id(&self.id.0)?;
        if self.name.trim().is_empty() {
            return Err("`name` is empty".into());
        }
        if self.explain.trim().is_empty() {
            return Err("`explain` is empty".into());
        }
        match &self.target {
            Target::ProjectArtifact {
                names,
                markers,
                contains,
                require_git_ignored: _,
            } => {
                if names.is_empty() {
                    return Err("`names` is empty".into());
                }
                for name in names {
                    validate_name(name)?;
                }
                if markers.is_empty() && contains.is_empty() {
                    return Err(
                        "needs `markers` or `contains`: a directory name alone isn't evidence"
                            .into(),
                    );
                }
                for marker in markers {
                    if marker.contains('*') {
                        validate_name(marker)?;
                    } else {
                        validate_relative(marker)?;
                    }
                }
                for file in contains {
                    validate_relative(file)?;
                }
            }
            Target::KnownLocation { locations, .. } => {
                if locations.is_empty() {
                    return Err("`locations` is empty".into());
                }
                for location in locations {
                    validate_location(location)?;
                }
            }
            // Nothing says what a large file is, so only the person can decide about one.
            Target::LargeFile {} => {
                if self.risk != Risk::Review {
                    return Err("a `large_file` rule must be `review`".into());
                }
                if self.owner.is_some() || self.idle_days.is_some() {
                    return Err("a `large_file` rule can't have `owner` or `idle_days`".into());
                }
            }
        }
        if self.idle_days == Some(0) {
            return Err("`idle_days` must be at least 1".into());
        }
        if let Some(command) = &self.advise {
            validate_advise(self.risk, command)?;
        }
        if let Some(owner) = &self.owner {
            if owner.app.trim().is_empty() {
                return Err("`owner.app` is empty".into());
            }
            if owner.processes.is_empty() {
                return Err("`owner.processes` is empty".into());
            }
            for process in &owner.processes {
                if process.is_empty() || process_name(process) != *process {
                    return Err(format!(
                        "`{process}` in `owner.processes` must be a lowercase program name \
                         without `.exe`"
                    ));
                }
            }
        }
        for test in &self.tests {
            let paths = test
                .tree
                .iter()
                .chain(test.files.keys())
                .chain(test.sizes.keys())
                .chain(&test.tracked)
                .chain(&test.matches);
            for path in paths {
                validate_relative(path.strip_suffix('/').unwrap_or(path))?;
            }
            if !test.git && !test.tracked.is_empty() {
                return Err("a test with `tracked` files needs `git = true`".into());
            }
        }
        Ok(())
    }
}

/// A command to show for a rule whose matches Cleanse never removes itself.
fn validate_advise(risk: Risk, command: &str) -> Result<(), String> {
    if risk != Risk::Protected {
        return Err(
            "`advise` is only for `protected` rules, whose matches the command cleans instead"
                .into(),
        );
    }
    if command.trim().is_empty() || command.contains(['\n', '\r']) {
        return Err("`advise` must be a command on one line".into());
    }
    Ok(())
}

/// Lowercase letters, digits, `_`, and `-`, in at least two `.`-separated parts, like
/// `python.venv`.
fn validate_id(id: &str) -> Result<(), String> {
    let parts: Vec<&str> = id.split('.').collect();
    let valid = parts.len() >= 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        });
    if valid {
        Ok(())
    } else {
        Err(format!(
            "`{id}` isn't a valid id: use lowercase letters, digits, `_`, and `-`, in two or more \
             parts separated by `.`, like `python.venv`"
        ))
    }
}

/// A single name, possibly with `*` wildcards.
fn validate_name(name: &str) -> Result<(), String> {
    if name.contains('/') || !is_pattern_segment(name) {
        return Err(format!("`{name}` must be a single file or directory name"));
    }
    Ok(())
}

/// A `/`-separated relative path that can't leave the folder it's relative to.
fn validate_relative(path: &str) -> Result<(), String> {
    if path.is_empty() || !path.split('/').all(is_plain_segment) {
        return Err(format!(
            "`{path}` must be a relative path made of `/`-separated names, without `.` or `..`"
        ));
    }
    Ok(())
}

/// Like [`validate_relative`], but names after the first may contain `*`. The first is literal,
/// so a location always stays inside one app's folder, except under `temp` and `downloads`,
/// which hold whatever anyone put there.
fn validate_location(location: &Location) -> Result<(), String> {
    let path = &location.path;
    let mut names = path.split('/');
    let first_ok = names.next().is_some_and(|first| {
        if location.base.is_shared() {
            is_pattern_segment(first)
        } else {
            is_plain_segment(first)
        }
    });
    if !first_ok || !names.all(is_pattern_segment) {
        return Err(format!(
            "`{path}` must be a relative path made of `/`-separated names, without `.` or `..`, \
             and with `*` only after the first name, except under `temp` and `downloads`"
        ));
    }
    Ok(())
}

/// A name that may contain `*`, but is otherwise plain.
fn is_pattern_segment(segment: &str) -> bool {
    is_plain_segment(&segment.replace('*', "x"))
}

/// Rejects `.` and `..`, which could leave the folder; `\`, so `/` is the only separator;
/// `:`, which Windows reads as a drive or an alternate data stream; and `*`, outside names
/// meant as patterns.
fn is_plain_segment(segment: &str) -> bool {
    !segment.is_empty() && segment != "." && segment != ".." && !segment.contains(['\\', ':', '*'])
}
