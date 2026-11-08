//! Folders opened by handle on Windows. What's inside one is listed, opened, and removed through
//! that handle, by name, never by a path from the top, as std's `remove_dir_all` does. A link,
//! junction, or mount point is always opened as itself and never followed, so nothing swapped in
//! while a folder is being emptied can send a removal outside it.

use std::ffi::{OsStr, OsString};
use std::fs::OpenOptions;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr;
use std::slice;
use std::thread;
use std::time::Duration;

use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_DIRECTORY_FILE, FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT, NtOpenFile,
};
use windows_sys::Win32::Foundation::{
    ERROR_DIR_NOT_EMPTY, ERROR_DIRECTORY, ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER,
    ERROR_NO_MORE_FILES, ERROR_NOT_SUPPORTED, HANDLE, RtlNtStatusToDosError, UNICODE_STRING,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_READONLY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_BASIC_INFO,
    FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FULL_DIR_INFO,
    FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES, FileAttributeTagInfo, FileBasicInfo,
    FileDispositionInfo, FileDispositionInfoEx, FileFullDirectoryInfo,
    GetFileInformationByHandleEx, SYNCHRONIZE, SetFileInformationByHandle,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

use super::NAME_SURROGATE_BIT;
use crate::privileged::Child;

/// Other programs may go on reading, writing, and removing what's inside while it's open.
const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;

/// What listing a folder and opening what's inside it takes.
const LIST: u32 = FILE_LIST_DIRECTORY | SYNCHRONIZE | FILE_READ_ATTRIBUTES;

/// What removing an entry takes. Writing its attributes clears the read-only one where Windows
/// can't be told to ignore it.
const REMOVE: u32 = DELETE | SYNCHRONIZE | FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES;

/// How long removing a folder retries while names removed from inside it linger.
const LINGER: Duration = Duration::from_millis(500);

pub(crate) struct Folder(OwnedHandle);

impl Folder {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .access_mode(LIST)
            .share_mode(SHARE_ALL)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let folder = Self(OwnedHandle::from(file));
        if folder.is_real_folder()? {
            Ok(folder)
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("{} isn't a folder", path.display()),
            ))
        }
    }

    pub(crate) fn children(&self) -> io::Result<Vec<Child>> {
        let mut children = Vec::new();
        // Each entry starts on an 8-byte boundary, as FILE_FULL_DIR_INFO has to.
        let mut buffer = vec![0_u64; 8 * 1024];
        let size = u32::try_from(buffer.len() * size_of::<u64>()).unwrap_or(u32::MAX);
        loop {
            // SAFETY: the buffer is writable for `size` bytes, and the handle was opened to list
            // the folder. Each call goes on from where the last one stopped.
            let listed = unsafe {
                GetFileInformationByHandleEx(
                    self.handle(),
                    FileFullDirectoryInfo,
                    buffer.as_mut_ptr().cast(),
                    size,
                )
            };
            if listed == 0 {
                let error = io::Error::last_os_error();
                return if error.raw_os_error() == Some(win32(ERROR_NO_MORE_FILES)) {
                    Ok(children)
                } else {
                    Err(error)
                };
            }
            let mut offset = 0;
            loop {
                // SAFETY: Windows filled the buffer with entries one after another, each
                // `NextEntryOffset` bytes after the last, aligned, and whole, its name included,
                // so the entry and its `FileNameLength` bytes of name are inside the buffer.
                let (next, attributes, tag, name) = unsafe {
                    let entry = buffer
                        .as_ptr()
                        .byte_add(offset)
                        .cast::<FILE_FULL_DIR_INFO>();
                    let name = slice::from_raw_parts(
                        (&raw const (*entry).FileName).cast::<u16>(),
                        (*entry).FileNameLength as usize / size_of::<u16>(),
                    );
                    (
                        (*entry).NextEntryOffset as usize,
                        (*entry).FileAttributes,
                        // For a reparse point, this field holds its tag instead.
                        (*entry).EaSize,
                        OsString::from_wide(name),
                    )
                };
                if name != "." && name != ".." {
                    children.push(Child {
                        name,
                        is_folder: is_real_folder(attributes, tag),
                    });
                }
                if next == 0 {
                    break;
                }
                offset += next;
            }
        }
    }

    pub(crate) fn open_folder(&self, name: &OsStr) -> io::Result<Option<Self>> {
        match self.open_inside(name, LIST, FILE_DIRECTORY_FILE) {
            Ok(handle) => {
                let folder = Self(handle);
                Ok(folder.is_real_folder()?.then_some(folder))
            }
            Err(error) if error.raw_os_error() == Some(win32(ERROR_DIRECTORY)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn remove_file(&self, name: &OsStr) -> io::Result<()> {
        remove(&self.open_inside(name, REMOVE, 0)?)
    }

    pub(crate) fn remove_folder(&self, name: &OsStr) -> io::Result<()> {
        // Where Windows can't remove names at once, what was removed inside lingers until every
        // program that had it open, such as an antivirus scanner, closes it.
        let mut wait = Duration::from_millis(1);
        loop {
            match remove(&self.open_inside(name, REMOVE, FILE_DIRECTORY_FILE)?) {
                Err(error)
                    if error.raw_os_error() == Some(win32(ERROR_DIR_NOT_EMPTY))
                        && wait < LINGER =>
                {
                    thread::sleep(wait);
                    wait *= 2;
                }
                result => return result,
            }
        }
    }

    fn handle(&self) -> HANDLE {
        self.0.as_raw_handle()
    }

    /// Whether this is a folder, rather than a link or junction to one.
    fn is_real_folder(&self) -> io::Result<bool> {
        let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
        // SAFETY: `info` is writable, and the structure this class fills.
        let read = unsafe {
            GetFileInformationByHandleEx(
                self.handle(),
                FileAttributeTagInfo,
                (&raw mut info).cast(),
                size_of_u32::<FILE_ATTRIBUTE_TAG_INFO>(),
            )
        };
        if read == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(is_real_folder(info.FileAttributes, info.ReparseTag))
    }

    /// Opens `name`, directly inside this folder, with `access`. A link, junction, or mount
    /// point opens as itself.
    fn open_inside(&self, name: &OsStr, access: u32, options: u32) -> io::Result<OwnedHandle> {
        let mut wide: Vec<u16> = name.encode_wide().collect();
        if wide.contains(&u16::from(b'\\')) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a name inside a folder can't hold a backslash",
            ));
        }
        let length = u16::try_from(wide.len() * size_of::<u16>())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "the name is too long"))?;
        let object_name = UNICODE_STRING {
            Length: length,
            MaximumLength: length,
            Buffer: wide.as_mut_ptr(),
        };
        let attributes = OBJECT_ATTRIBUTES {
            Length: size_of_u32::<OBJECT_ATTRIBUTES>(),
            RootDirectory: self.handle(),
            ObjectName: &raw const object_name,
            // Case-sensitive, so in a folder that tells names apart by case, it's that entry.
            Attributes: 0,
            SecurityDescriptor: ptr::null(),
            SecurityQualityOfService: ptr::null(),
        };
        let mut handle: HANDLE = ptr::null_mut();
        let mut status_block = IO_STATUS_BLOCK::default();
        // SAFETY: every pointer is to a local that outlives the call, and the name is one path
        // component, opened relative to this folder's handle.
        let status = unsafe {
            NtOpenFile(
                &raw mut handle,
                access,
                &raw const attributes,
                &raw mut status_block,
                SHARE_ALL,
                options | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            )
        };
        if status < 0 {
            // SAFETY: this only translates a status code.
            let error = unsafe { RtlNtStatusToDosError(status) };
            return Err(io::Error::from_raw_os_error(win32(error)));
        }
        // SAFETY: NtOpenFile succeeded, so `handle` is open, and nothing else owns it.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }
}

/// A folder, rather than a link or junction to one, from its attributes and reparse tag.
fn is_real_folder(attributes: u32, tag: u32) -> bool {
    let link = attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 && tag & NAME_SURROGATE_BIT != 0;
    attributes & FILE_ATTRIBUTE_DIRECTORY != 0 && !link
}

/// Removes what `handle` has open once it's closed. POSIX semantics remove the name at once,
/// even while other programs have it open, and read-only files go too.
fn remove(handle: &OwnedHandle) -> io::Result<()> {
    let info = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    // SAFETY: the handle was opened for deleting, and `info` is the structure this class takes.
    let set = unsafe {
        SetFileInformationByHandle(
            handle.as_raw_handle(),
            FileDispositionInfoEx,
            (&raw const info).cast(),
            size_of_u32::<FILE_DISPOSITION_INFO_EX>(),
        )
    };
    if set != 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    // Older Windows and filesystems such as FAT support neither.
    let unsupported = [
        ERROR_INVALID_PARAMETER,
        ERROR_NOT_SUPPORTED,
        ERROR_INVALID_FUNCTION,
    ];
    if !unsupported
        .iter()
        .any(|&code| error.raw_os_error() == Some(win32(code)))
    {
        return Err(error);
    }
    clear_read_only(handle)?;
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: as above.
    let set = unsafe {
        SetFileInformationByHandle(
            handle.as_raw_handle(),
            FileDispositionInfo,
            (&raw const info).cast(),
            size_of_u32::<FILE_DISPOSITION_INFO>(),
        )
    };
    if set == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn clear_read_only(handle: &OwnedHandle) -> io::Result<()> {
    let mut info = FILE_BASIC_INFO::default();
    // SAFETY: `info` is writable, and the structure this class fills.
    let read = unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle(),
            FileBasicInfo,
            (&raw mut info).cast(),
            size_of_u32::<FILE_BASIC_INFO>(),
        )
    };
    if read == 0 {
        return Err(io::Error::last_os_error());
    }
    if info.FileAttributes & FILE_ATTRIBUTE_READONLY == 0 {
        return Ok(());
    }
    // Times of zero, and attributes of zero, leave those as they are, so a file with no other
    // attributes needs the one that says so.
    let attributes = info.FileAttributes & !FILE_ATTRIBUTE_READONLY;
    let info = FILE_BASIC_INFO {
        FileAttributes: if attributes == 0 {
            FILE_ATTRIBUTE_NORMAL
        } else {
            attributes
        },
        ..FILE_BASIC_INFO::default()
    };
    // SAFETY: the handle was opened for writing attributes, and `info` is this class's structure.
    let set = unsafe {
        SetFileInformationByHandle(
            handle.as_raw_handle(),
            FileBasicInfo,
            (&raw const info).cast(),
            size_of_u32::<FILE_BASIC_INFO>(),
        )
    };
    if set == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// A Win32 error code, as `io::Error` holds them.
fn win32(code: u32) -> i32 {
    i32::try_from(code).unwrap_or(i32::MAX)
}

fn size_of_u32<T>() -> u32 {
    u32::try_from(size_of::<T>()).unwrap_or(u32::MAX)
}
