use std::collections::HashMap;
use std::path::{Component, Path, PathBuf, Prefix};

use cleanse_model::FileIdentity;
use cleanse_platform::{KnownFolders, Probe};

/// How far a protected location's protection reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// The location itself and anything containing it. Things inside it are fine, as with a
    /// home folder.
    Exact,
    /// The location, anything containing it, and everything inside it.
    Subtree,
}

#[derive(Clone, Debug)]
pub struct Protected {
    pub path: PathBuf,
    pub reach: Reach,
    /// What it is, for explanations: "SSH keys", "installed programs".
    pub what: String,
    /// Folders inside a subtree that aren't protected by it, with everything inside them.
    pub except: Vec<PathBuf>,
}

/// Folders under the home folder whose contents are never cleaned. AI agents' folders hold
/// conversations, memory, and credentials; their disposable parts get explicit rules later.
const HOME_SUBTREES: &[(&str, &str)] = &[
    (".ssh", "SSH keys"),
    (".gnupg", "GPG keys"),
    (".aws", "cloud credentials"),
    (".azure", "cloud credentials"),
    (".config/gcloud", "cloud credentials"),
    (".kube", "Kubernetes credentials"),
    (".docker", "Docker's configuration"),
    (".vscode/extensions", "editor extensions"),
    (".vscode-insiders/extensions", "editor extensions"),
    (".vscode-server", "editor extensions"),
    (".cursor", "Cursor's data and extensions"),
    (".windsurf", "Windsurf's data and extensions"),
    (".claude", "Claude Code's data"),
    (".codex", "Codex's data"),
    (".gemini", "Gemini CLI's data"),
];

/// Folders inside the [`HOME_SUBTREES`] that the owner's own documentation says can be deleted.
/// Built-in rules match exactly these, and everything else inside stays protected. They're kept
/// here, with the safety checks, so a rule can never unprotect anything on its own.
const DISPOSABLE: &[(&str, &[&str])] = &[(
    // https://code.claude.com/docs/en/claude-directory, under "Clear local data"
    ".claude",
    &[
        "backups",
        "cache",
        "debug",
        "file-history",
        "image-cache",
        "logs",
        "paste-cache",
        "plans",
        "plugins/.trash",
        "session-env",
        "shell-snapshots",
        "skills/.trash",
        "statsig",
        "todos",
        "usage-data",
    ],
)];

/// Never cleaned inside: a bundle is one unit, and its contents aren't build output.
const BUNDLE_EXTENSIONS: &[&str] = &["app", "photoslibrary", "xcarchive"];

/// Windows system folders that can appear at the root of any drive, including NTFS drives
/// mounted on other systems.
const DRIVE_SYSTEM_FOLDERS: &[&str] = &["$recycle.bin", "system volume information"];

/// Electron apps keep their code in `resources/app` (`Contents/Resources/app` on macOS), with a
/// `package.json` and `node_modules` that look like a project's. Apps can be installed or
/// unpacked anywhere, especially on Linux, so the layout is recognized wherever it appears.
const ELECTRON_APP: [&str; 2] = ["resources", "app"];

/// Locations no rule can override (the safety model's first gate).
#[derive(Debug)]
pub struct ProtectedPolicy {
    entries: Vec<(Vec<String>, Protected)>,
    /// Each entry's `except` folders, compared the same way.
    except_keys: Vec<Vec<Vec<String>>>,
    /// Identities of subtree-protected locations, so a path inside one is recognized however
    /// it's spelled, such as through an 8.3 short name.
    subtree_ids: HashMap<FileIdentity, Vec<usize>>,
    /// Identities of every protected location and all of its ancestors, so a path that is or
    /// contains one is recognized however it's spelled.
    containing_ids: HashMap<FileIdentity, usize>,
}

impl ProtectedPolicy {
    /// The protected locations for this system and user, plus the folders the user excluded.
    pub fn for_system(folders: &KnownFolders, probe: &dyn Probe, excluded: &[PathBuf]) -> Self {
        let mut locations = system_locations(folders);
        locations.extend(excluded.iter().map(|path| Protected {
            path: path.clone(),
            reach: Reach::Subtree,
            what: "a folder excluded in your settings".to_owned(),
            except: Vec::new(),
        }));
        Self::new(locations, probe)
    }

    /// Reads the identities of the locations, and their ancestors, that exist.
    pub fn new(locations: Vec<Protected>, probe: &dyn Probe) -> Self {
        let mut identities: HashMap<PathBuf, Option<FileIdentity>> = HashMap::new();
        let mut identity_of = |path: &Path| {
            *identities
                .entry(path.to_path_buf())
                .or_insert_with(|| probe.inspect(path).ok().map(|facts| facts.identity))
        };
        let mut subtree_ids = HashMap::new();
        let mut containing_ids = HashMap::new();
        for (index, location) in locations.iter().enumerate() {
            if location.reach == Reach::Subtree
                && let Some(id) = identity_of(&location.path)
            {
                subtree_ids.entry(id).or_insert_with(Vec::new).push(index);
            }
            for ancestor in location.path.ancestors() {
                if let Some(id) = identity_of(ancestor) {
                    containing_ids.entry(id).or_insert(index);
                }
            }
        }
        Self {
            except_keys: locations
                .iter()
                .map(|location| location.except.iter().map(|path| key(path)).collect())
                .collect(),
            entries: locations
                .into_iter()
                .map(|location| (key(&location.path), location))
                .collect(),
            subtree_ids,
            containing_ids,
        }
    }

    /// Why `path` is protected, if it is. `own` is the path's identity and `ancestors` those of
    /// its parent folders, as far as they could be read.
    pub fn check(
        &self,
        path: &Path,
        own: Option<FileIdentity>,
        ancestors: &[FileIdentity],
    ) -> Option<&str> {
        if path.parent().is_none() {
            return Some("a drive's root folder");
        }
        let path_key = key(path);
        if path_key.iter().any(|part| {
            DRIVE_SYSTEM_FOLDERS
                .iter()
                .any(|folder| part.eq_ignore_ascii_case(folder))
        }) {
            return Some("Windows system files");
        }
        if path_key.windows(2).any(|pair| {
            pair[0].eq_ignore_ascii_case(ELECTRON_APP[0])
                && pair[1].eq_ignore_ascii_case(ELECTRON_APP[1])
        }) {
            return Some("an installed app");
        }
        if path_key.iter().any(|part| is_bundle(part)) {
            return Some("the inside of an app bundle");
        }
        if path_key.iter().any(|part| part == ".git") {
            return Some("a Git repository's internal data");
        }
        for (index, (location_key, location)) in self.entries.iter().enumerate() {
            let contains = location_key.starts_with(&path_key);
            let inside = location.reach == Reach::Subtree
                && path_key.starts_with(location_key)
                && !self.excepted(index, &path_key);
            if contains || inside {
                return Some(&location.what);
            }
        }
        let by_identity = own
            .and_then(|id| self.containing_ids.get(&id).copied())
            .or_else(|| {
                ancestors.iter().find_map(|id| {
                    self.subtree_ids
                        .get(id)?
                        .iter()
                        .copied()
                        .find(|&index| !self.excepted(index, &path_key))
                })
            });
        by_identity.map(|index| self.entries[index].1.what.as_str())
    }

    /// Whether the path is inside one of entry `index`'s exceptions, spelled the usual way. A
    /// path reaching it another way, such as through a short name, stays protected.
    fn excepted(&self, index: usize, path_key: &[String]) -> bool {
        self.except_keys[index]
            .iter()
            .any(|except| path_key.starts_with(except))
    }
}

/// A path's components for comparison: case-folded where the OS ignores case, with `\\?\C:`
/// and `C:` treated alike.
pub(super) fn key(path: &Path) -> Vec<String> {
    path.components()
        .map(|component| match component {
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => {
                    format!("{}:", char::from(drive).to_ascii_lowercase())
                }
                _ => fold_case(&prefix.as_os_str().to_string_lossy()),
            },
            Component::RootDir => "/".to_owned(),
            Component::CurDir => ".".to_owned(),
            Component::ParentDir => "..".to_owned(),
            Component::Normal(name) => fold_case(&name.to_string_lossy()),
        })
        .collect()
}

fn fold_case(name: &str) -> String {
    if cfg!(any(windows, target_os = "macos")) {
        name.to_lowercase()
    } else {
        name.to_owned()
    }
}

fn is_bundle(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            BUNDLE_EXTENSIONS
                .iter()
                .any(|bundle| extension.eq_ignore_ascii_case(bundle))
        })
}

fn system_locations(folders: &KnownFolders) -> Vec<Protected> {
    let mut locations = Locations::default();
    user_locations(&mut locations, folders);
    os_locations(&mut locations, folders);
    locations.0
}

#[derive(Default)]
struct Locations(Vec<Protected>);

impl Locations {
    /// Skips folders the system doesn't define.
    fn add(&mut self, path: Option<PathBuf>, reach: Reach, what: &str) {
        if let Some(path) = path {
            self.0.push(Protected {
                path,
                reach,
                what: what.to_owned(),
                except: Vec::new(),
            });
        }
    }
}

fn user_locations(locations: &mut Locations, folders: &KnownFolders) {
    for (folder, what) in [
        (&folders.home, "your home folder"),
        (&folders.desktop, "your Desktop"),
        (&folders.documents, "your Documents"),
        (&folders.downloads, "your Downloads"),
        (&folders.pictures, "your Pictures"),
        (&folders.music, "your Music"),
        (&folders.videos, "your Videos"),
        (&folders.cache, "your cache folder"),
        (&folders.config, "your app settings folder"),
        (&folders.data_local, "your app data folder"),
    ] {
        locations.add(folder.clone(), Reach::Exact, what);
    }
    locations.add(Some(folders.temp.clone()), Reach::Exact, "your temp folder");
    locations.add(
        folders.cleanse_data(),
        Reach::Subtree,
        "Cleanse's journal of what it removed",
    );
    if let Some(home) = &folders.home {
        let under = |folder: &Path, relative: &str| {
            let mut path = folder.to_path_buf();
            path.extend(relative.split('/'));
            path
        };
        for (relative, what) in HOME_SUBTREES {
            let path = under(home, relative);
            let except = DISPOSABLE
                .iter()
                .filter(|(folder, _)| folder == relative)
                .flat_map(|(_, inside)| inside.iter().map(|inner| under(&path, inner)))
                .collect();
            locations.0.push(Protected {
                path,
                reach: Reach::Subtree,
                what: (*what).to_owned(),
                except,
            });
        }
    }
}

#[cfg(windows)]
fn os_locations(locations: &mut Locations, folders: &KnownFolders) {
    let env_path = |name: &str| std::env::var_os(name).map(PathBuf::from);
    locations.add(env_path("SystemRoot"), Reach::Subtree, "Windows");
    for variable in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        locations.add(env_path(variable), Reach::Subtree, "installed programs");
    }
    locations.add(env_path("ProgramData"), Reach::Subtree, "shared app data");
    let microsoft = "Windows' and Microsoft apps' data";
    if let Some(local) = &folders.data_local {
        locations.add(
            Some(local.join("Programs")),
            Reach::Subtree,
            "installed programs",
        );
        locations.add(
            Some(local.join("Packages")),
            Reach::Subtree,
            "Microsoft Store apps' data",
        );
        locations.add(Some(local.join("Microsoft")), Reach::Subtree, microsoft);
    }
    if let Some(roaming) = &folders.config {
        locations.add(Some(roaming.join("Microsoft")), Reach::Subtree, microsoft);
    }
}

#[cfg(target_os = "macos")]
fn os_locations(locations: &mut Locations, folders: &KnownFolders) {
    for (path, what) in [
        ("/System", "macOS"),
        ("/Library", "macOS system files"),
        ("/Applications", "installed apps"),
        ("/usr", "macOS system files"),
        ("/bin", "macOS system files"),
        ("/sbin", "macOS system files"),
        ("/etc", "macOS system files"),
        ("/opt", "installed programs"),
    ] {
        locations.add(Some(PathBuf::from(path)), Reach::Subtree, what);
    }
    if let Some(home) = &folders.home {
        locations.add(
            Some(home.join("Library")),
            Reach::Exact,
            "your Library folder",
        );
        locations.add(
            Some(home.join("Library/Keychains")),
            Reach::Subtree,
            "keychains",
        );
        locations.add(
            Some(home.join("Applications")),
            Reach::Subtree,
            "installed apps",
        );
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn os_locations(locations: &mut Locations, folders: &KnownFolders) {
    for path in [
        "/usr", "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/etc", "/boot", "/opt", "/var",
        "/snap", "/proc", "/sys", "/dev", "/run", "/root",
    ] {
        locations.add(Some(PathBuf::from(path)), Reach::Subtree, "system files");
    }
    if let Some(home) = &folders.home {
        locations.add(
            Some(home.join(".local/share/keyrings")),
            Reach::Subtree,
            "keyrings",
        );
    }
}
