//! The settings file, `config.toml`: folders to leave alone and rules to turn off.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use cleanse_model::RuleId;
use cleanse_platform::KnownFolders;
use serde::Deserialize;

/// What the settings file says. A missing file, or a missing setting, means none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    /// Folders Cleanse leaves alone: they, everything inside them, and anything containing them
    /// are protected.
    pub exclude: Vec<PathBuf>,
    /// Rules that never match anything.
    pub disabled_rules: Vec<RuleId>,
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
        }

        let file: File = toml::from_str(text).map_err(|error| error.to_string())?;
        Ok(Self {
            exclude: file
                .exclude
                .iter()
                .map(|path| expand(path, home))
                .collect::<Result<_, _>>()?,
            disabled_rules: file.disabled_rules,
        })
    }
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
    fn a_missing_file_has_no_settings() {
        let dir = tempfile::tempdir().unwrap();

        let config = Config::load(&dir.path().join("config.toml"), None).unwrap();

        assert_eq!(config, Config::default());
    }
}
