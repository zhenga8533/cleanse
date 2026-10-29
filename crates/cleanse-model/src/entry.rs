use std::fs::FileType;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Dir,
    /// A symbolic link, junction, or other name-surrogate reparse point. Never followed.
    Link,
    /// Sockets, FIFOs, devices, and anything else that isn't a file, directory, or link.
    Other,
}

impl From<FileType> for EntryKind {
    fn from(file_type: FileType) -> Self {
        // `is_symlink` comes first: on Windows it also covers junctions and mount points.
        if file_type.is_symlink() {
            Self::Link
        } else if file_type.is_dir() {
            Self::Dir
        } else if file_type.is_file() {
            Self::File
        } else {
            Self::Other
        }
    }
}
