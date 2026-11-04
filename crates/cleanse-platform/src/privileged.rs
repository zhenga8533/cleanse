//! The changes only system cleanup makes (ADR 0010), behind the `privileged` feature, which only
//! `cleanse-exec` turns on. `clippy.toml` keeps other code from calling them.

use std::io;
use std::path::Path;

use crate::os;

/// Creates the folder at `path`, which mustn't exist, so that only Administrators and SYSTEM, or
/// root, can open it from the moment it exists. Files and folders made inside it inherit that.
///
/// [`Probe::is_private_dir`](crate::Probe::is_private_dir) checks it afterwards, since a folder
/// someone else made first makes this fail.
pub fn create_private_dir(path: &Path) -> io::Result<()> {
    os::create_private_dir(path)
}
