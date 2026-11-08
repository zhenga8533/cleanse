//! What changes the filesystem, or only system cleanup does (ADR 0010), behind the `privileged`
//! feature, which only `cleanse-exec` and the elevated helper turn on: removing what's inside a
//! folder through its handle, creating system cleanup's data folder, and on Windows, starting the
//! desktop app's helper through the UAC prompt, the pipes between the app's window and the
//! helper, and where the helper loads DLLs from. `clippy.toml` keeps other code from removing
//! anything, starting the helper, making or opening the pipes, or making the folder.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;

#[cfg(windows)]
pub use crate::windows::launch::{Process, run_as_administrator};
#[cfg(windows)]
pub use crate::windows::pipe;

use crate::os;

/// A folder opened without following links. What's inside it is listed, opened, and removed
/// through it, by name, never by a path from the top, so nothing swapped in for part of that path
/// while it's being emptied, such as a link, can send a removal elsewhere. Links, junctions, and
/// mount points are only ever removed as themselves.
///
/// Its methods can be called from many threads at once.
pub struct Folder(os::folder::Folder);

/// An entry directly inside a [`Folder`].
#[derive(Debug)]
pub struct Child {
    pub name: OsString,
    /// A folder, rather than a file, a link, or anything else, as the listing said.
    pub is_folder: bool,
}

impl Folder {
    /// Opens the folder at `path`. Fails if it's a link or junction, rather than a folder.
    pub fn open(path: &Path) -> io::Result<Self> {
        os::folder::Folder::open(path).map(Self)
    }

    /// Everything directly inside it.
    pub fn children(&self) -> io::Result<Vec<Child>> {
        self.0.children()
    }

    /// Opens the folder `name` inside it, or `None` if `name` isn't a folder, such as a link put
    /// in its place since it was listed.
    pub fn open_folder(&self, name: &OsStr) -> io::Result<Option<Self>> {
        Ok(self.0.open_folder(name)?.map(Self))
    }

    /// Removes `name`, a file or a link, from inside it. A link goes, never what it points to.
    pub fn remove_file(&self, name: &OsStr) -> io::Result<()> {
        self.0.remove_file(name)
    }

    /// Removes the empty folder `name` from inside it.
    pub fn remove_folder(&self, name: &OsStr) -> io::Result<()> {
        self.0.remove_folder(name)
    }
}

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
