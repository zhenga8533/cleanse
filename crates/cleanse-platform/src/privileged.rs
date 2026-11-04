//! What only system cleanup does (ADR 0010), behind the `privileged` feature, which only
//! `cleanse-exec` and the elevated helper turn on: creating its data folder, and on Windows, the
//! pipes between the desktop app's window and the helper, and where the helper loads DLLs from.
//! `clippy.toml` keeps other code from making the pipes or the folder.

use std::io;
use std::path::Path;

#[cfg(windows)]
pub use crate::windows::pipe;

use crate::os;

/// Has Windows load the DLLs this process asks for only from System32 from now on, never from
/// beside its program or the current folder, where someone could plant one for an elevated
/// process to run. The DLLs a program imports are loaded before it runs, so only its link flags
/// can do the same for those: see the elevated helper's `build.rs`.
#[cfg(windows)]
pub fn load_dlls_from_system32_only() -> io::Result<()> {
    crate::windows::load_dlls_from_system32_only()
}

/// Creates the folder at `path`, which mustn't exist, so that only Administrators and SYSTEM, or
/// root, can open it from the moment it exists. Files and folders made inside it inherit that.
///
/// [`Probe::is_private_dir`](crate::Probe::is_private_dir) checks it afterwards, since a folder
/// someone else made first makes this fail.
pub fn create_private_dir(path: &Path) -> io::Result<()> {
    os::create_private_dir(path)
}
