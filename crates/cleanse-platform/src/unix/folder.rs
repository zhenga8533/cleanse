//! Folders opened by file descriptor on Unix. What's inside one is listed, opened, and removed
//! through that descriptor, by name, never by a path from the top, as std's `remove_dir_all`
//! does. A symbolic link is always removed as itself and never followed, so nothing swapped in
//! while a folder is being emptied can send a removal outside it.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStringExt;
use std::path::Path;

use rustix::fs::{AtFlags, CWD, Dir, FileType, Mode, OFlags, openat, statat, unlinkat};
use rustix::io::Errno;

use crate::privileged::Child;

/// Opens a folder, and fails on a symbolic link rather than following it.
const OPEN_FOLDER: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

pub(crate) struct Folder(OwnedFd);

impl Folder {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        Ok(Self(openat(CWD, path, OPEN_FOLDER, Mode::empty())?))
    }

    pub(crate) fn children(&self) -> io::Result<Vec<Child>> {
        let mut children = Vec::new();
        for entry in Dir::read_from(&self.0)? {
            let entry = entry?;
            let name = entry.file_name();
            if name == c"." || name == c".." {
                continue;
            }
            let is_folder = match entry.file_type() {
                FileType::Directory => true,
                // Some filesystems don't say in their listings.
                FileType::Unknown => {
                    let stat = statat(&self.0, name, AtFlags::SYMLINK_NOFOLLOW)?;
                    FileType::from_raw_mode(stat.st_mode) == FileType::Directory
                }
                _ => false,
            };
            children.push(Child {
                name: OsString::from_vec(name.to_bytes().to_vec()),
                is_folder,
            });
        }
        Ok(children)
    }

    pub(crate) fn open_folder(&self, name: &OsStr) -> io::Result<Option<Self>> {
        match openat(&self.0, name, OPEN_FOLDER, Mode::empty()) {
            Ok(folder) => Ok(Some(Self(folder))),
            // A symbolic link, or something other than a folder put in its place.
            Err(Errno::LOOP | Errno::NOTDIR) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn remove_file(&self, name: &OsStr) -> io::Result<()> {
        Ok(unlinkat(&self.0, name, AtFlags::empty())?)
    }

    pub(crate) fn remove_folder(&self, name: &OsStr) -> io::Result<()> {
        Ok(unlinkat(&self.0, name, AtFlags::REMOVEDIR)?)
    }
}
