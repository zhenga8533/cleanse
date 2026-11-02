//! The settings file, `config.toml`: folders to leave alone, rules to turn off, and how large a
//! large file is.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use cleanse_model::RuleId;
use cleanse_platform::KnownFolders;
use serde::Deserialize;

const MIB: u64 = 1 << 20;

/// What counts as a large file unless the settings say otherwise.
pub const DEFAULT_LARGE_FILE_SIZE: u64 = 1024 * MIB;

/// The smallest `large_file_size` accepted. Much smaller, and ordinary photos and documents
/// would all count as large.
pub const MIN_LARGE_FILE_SIZE: u64 = 64 * MIB;

/// What the settings file says. A missing file, or a missing setting, means the default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Folders Cleanse leaves alone: they, everything inside them, and anything containing them
    /// are protected.
    pub exclude: Vec<PathBuf>,
    /// Rules that never match anything.
    pub disabled_rules: Vec<RuleId>,
    /// Files taking at least this many bytes on disk are large files.
    pub large_file_size: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            exclude: Vec::new(),
            disabled_rules: Vec::new(),
            large_file_size: DEFAULT_LARGE_FILE_SIZE,
        }
    }
}

/// A settings file that couldn't be read or doesn't make sense.
#[derive(Debug)]
pub struct ConfigError {
    pub path: PathBuf,
    pub message: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

impl Error for ConfigError {}

impl Config {
    /// Where the settings file is: `cleanse/config.toml` in the folder for app settings.
    pub fn path(folders: &KnownFolders) -> Option<PathBuf> {
        folders
            .config
            .as_ref()
            .map(|config| config.join("cleanse").join("config.toml"))
    }

    /// Reads the settings file at `path`. `~` at the start of a path means `home`.
    pub fn load(path: &Path, home: Option<&Path>) -> Result<Self, ConfigError> {
        let fail = |message: String| ConfigError {
            path: path.to_path_buf(),
            message,
        };
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(&text, home).map_err(fail),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(fail(error.to_string())),
        }
    }

    /// Parses the contents of a settings file. `~` at the start of a path means `home`.
    pub fn parse(text: &str, home: Option<&Path>) -> Result<Self, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct File {
            #[serde(default)]
            exclude: Vec<String>,
            #[serde(default)]
            disabled_rules: Vec<RuleId>,
            #[serde(default)]
            large_file_size: Option<String>,
        }

        let file: File = toml::from_str(text).map_err(|error| error.to_string())?;
        Ok(Self {
            exclude: file
                .exclude
                .iter()
                .map(|path| expand(path, home))
                .collect::<Result<_, _>>()?,
            disabled_rules: file.disabled_rules,
            large_file_size: match &file.large_file_size {
                Some(size) => parse_size(size)?,
                None => DEFAULT_LARGE_FILE_SIZE,
            },
        })
    }
}

/// A whole number of MiB, GiB, or TiB, like `1 GiB`, of at least [`MIN_LARGE_FILE_SIZE`].
fn parse_size(raw: &str) -> Result<u64, String> {
    let invalid = || {
        format!(
            "`{raw}` in `large_file_size` isn't a size. Give a whole number of MiB, GiB, or TiB, \
             like \"1 GiB\"."
        )
    };
    let raw = raw.trim();
    let digits = raw.find(|c: char| !c.is_ascii_digit()).unwrap_or(raw.len());
    let (number, unit) = raw.split_at(digits);
    let number: u64 = number.parse().map_err(|_| invalid())?;
    let unit = match unit.trim() {
        unit if unit.eq_ignore_ascii_case("MiB") => MIB,
        unit if unit.eq_ignore_ascii_case("GiB") => 1024 * MIB,
        unit if unit.eq_ignore_ascii_case("TiB") => 1024 * 1024 * MIB,
        _ => return Err(invalid()),
    };
    let size = number.checked_mul(unit).ok_or_else(invalid)?;
    if size < MIN_LARGE_FILE_SIZE {
        return Err(format!(
            "`large_file_size` must be at least {} MiB, or too many ordinary files would count",
            MIN_LARGE_FILE_SIZE / MIB
        ));
    }
    Ok(size)
}

/// A relative path would depend on where Cleanse happens to be started, so only absolute paths
/// and paths starting with `~` are accepted.
fn expand(raw: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    let relative = if raw == "~" {
        Some("")
    } else {
        raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\"))
    };
    let path = match relative {
        Some(relative) => home
            .ok_or_else(|| format!("`{raw}` starts with ~, but there's no home folder"))?
            .join(relative),
        None => PathBuf::from(raw),
    };
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(format!(
            "`{raw}` in `exclude` isn't a full path. Start it with ~ for your home folder, or \
             give the whole path."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from(if cfg!(windows) {
            r"C:\home"
        } else {
            "/home/me"
        })
    }

    #[test]
    fn reads_exclusions_and_disabled_rules() {
        let absolute = if cfg!(windows) {
            r"D:\\archive"
        } else {
            "/srv/archive"
        };
        let text = format!(
            "exclude = [\"~/work/keep\", \"{absolute}\"]\ndisabled_rules = [\"unity.library\"]\n"
        );

        let config = Config::parse(&text, Some(&home())).unwrap();

        assert_eq!(
            config.exclude,
            [
                home().join("work/keep"),
                PathBuf::from(absolute.replace(r"\\", r"\"))
            ]
        );
        assert_eq!(config.disabled_rules, [RuleId("unity.library".into())]);
    }

    #[test]
    fn an_empty_file_has_no_settings() {
        assert_eq!(Config::parse("", None).unwrap(), Config::default());
    }

    #[test]
    fn rejects_relative_paths_and_unknown_settings() {
        let relative = Config::parse("exclude = [\"work\"]", Some(&home())).unwrap_err();
        let unknown = Config::parse("excludes = []", Some(&home())).unwrap_err();

        assert!(relative.contains("isn't a full path"), "{relative}");
        assert!(unknown.contains("excludes"), "{unknown}");
    }

    #[test]
    fn reads_the_large_file_size_in_binary_units() {
        let size = |text: &str| Config::parse(text, None).map(|config| config.large_file_size);

        assert_eq!(size(""), Ok(DEFAULT_LARGE_FILE_SIZE));
        assert_eq!(size("large_file_size = \"1 GiB\""), Ok(1 << 30));
        assert_eq!(size("large_file_size = \"512mib\""), Ok(512 << 20));
        assert_eq!(size("large_file_size = \" 2 TiB \""), Ok(2 << 40));
        for wrong in ["1.5 GiB", "1 GB", "GiB", "-1 GiB", "99999999999 TiB"] {
            let error = size(&format!("large_file_size = \"{wrong}\"")).unwrap_err();
            assert!(error.contains("isn't a size"), "{wrong}: {error}");
        }
        let small = size("large_file_size = \"10 MiB\"").unwrap_err();
        assert!(small.contains("at least 64 MiB"), "{small}");
    }

    #[test]
    fn a_missing_file_has_no_settings() {
        let dir = tempfile::tempdir().unwrap();

        let config = Config::load(&dir.path().join("config.toml"), None).unwrap();

        assert_eq!(config, Config::default());
    }
}
