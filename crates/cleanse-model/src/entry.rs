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
