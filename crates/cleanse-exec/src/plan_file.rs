//! Plan files: saved by `cleanse scan --save`, and read back by `cleanse apply`.

#![allow(
    clippy::disallowed_methods,
    reason = "cleanse-exec is the one crate that changes the filesystem (ADR 0002)"
)]

use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Write};
use std::path::{Path, PathBuf};

use cleanse_model::SavedPlan;
use serde::Deserialize;

/// Why a plan couldn't be saved or read.
#[derive(Debug)]
pub enum PlanFileError {
    Io(io::Error),
    /// The file isn't a plan Cleanse saved, or it's damaged.
    NotAPlan(String),
    /// The plan is in a format this version of Cleanse doesn't read.
    UnknownFormat(u32),
    /// A path in the plan isn't a full path, so what it names would depend on the folder Cleanse
    /// runs in.
    RelativePath(PathBuf),
    /// Saving would replace something that isn't a plan.
    WouldReplace,
}

impl fmt::Display for PlanFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::NotAPlan(detail) => write!(f, "it isn't a plan Cleanse saved ({detail})"),
            Self::UnknownFormat(format) => write!(
                f,
                "it's in plan format {format}, and this version of Cleanse reads format {}",
                SavedPlan::FORMAT
            ),
            Self::RelativePath(path) => {
                write!(
                    f,
                    "it has a path that isn't a full path: {}",
                    path.display()
                )
            }
            Self::WouldReplace => f.write_str(
                "something that isn't a plan Cleanse saved is already there, so it was left \
                 alone. Choose another name.",
            ),
        }
    }
}

impl Error for PlanFileError {}

impl From<io::Error> for PlanFileError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Just enough of a plan to tell whether a file is one, and which format it's in.
#[derive(Deserialize)]
struct Header {
    cleanse_plan: Option<u32>,
}

/// Saves `plan` to `path`. A file already there is replaced only if it's a plan Cleanse saved,
/// so a mistyped name can't overwrite something else.
pub fn save_plan(path: &Path, plan: &SavedPlan) -> Result<(), PlanFileError> {
    let mut json = serde_json::to_vec_pretty(plan).map_err(io::Error::other)?;
    json.push(b'\n');

    let mut options = OpenOptions::new();
    options.write(true);
    match File::open(path) {
        Ok(existing) => {
            let header = serde_json::from_reader::<_, Header>(BufReader::new(existing));
            if !matches!(
                header,
                Ok(Header {
                    cleanse_plan: Some(_)
                })
            ) {
                return Err(PlanFileError::WouldReplace);
            }
            options.truncate(true);
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            options.create_new(true);
        }
        Err(error) => return Err(error.into()),
    }
    let mut file = options.open(path)?;
    file.write_all(&json)?;
    file.sync_all()?;
    Ok(())
}

/// Reads a plan saved by [`save_plan`], which may have been edited since.
pub fn load_plan(path: &Path) -> Result<SavedPlan, PlanFileError> {
    let text = fs::read_to_string(path)?;
    // Some Windows editors start UTF-8 files with a byte order mark.
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);

    let header: Header =
        serde_json::from_str(text).map_err(|error| PlanFileError::NotAPlan(error.to_string()))?;
    match header.cleanse_plan {
        Some(SavedPlan::FORMAT) => {}
        Some(format) => return Err(PlanFileError::UnknownFormat(format)),
        None => return Err(PlanFileError::NotAPlan("no cleanse_plan field".into())),
    }
    let plan: SavedPlan =
        serde_json::from_str(text).map_err(|error| PlanFileError::NotAPlan(error.to_string()))?;

    let paths = plan
        .roots
        .iter()
        .chain(plan.items.iter().map(|saved| &saved.item.path));
    for path in paths {
        if !path.is_absolute() {
            return Err(PlanFileError::RelativePath(path.clone()));
        }
    }
    Ok(plan)
}
