#![allow(
    unsafe_code,
    reason = "file identity, sync root, and token queries need Win32 calls that std doesn't wrap"
)]

use std::fs::{File, Metadata, OpenOptions};
use std::io;
use std::mem::size_of;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr;

use cleanse_model::{EntryKind, FileIdentity};
use windows_sys::Win32::Foundation::{
    ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT, ERROR_INVALID_FUNCTION, HANDLE,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use windows_sys::Win32::Storage::CloudFilters::{
    CF_SYNC_ROOT_BASIC_INFO, CF_SYNC_ROOT_INFO_BASIC, CfGetSyncRootInfoByHandle,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
    FILE_ATTRIBUTE_RECALL_ON_OPEN, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO,
    FILE_INFO_BY_HANDLE_CLASS, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_STANDARD_INFO, FileAttributeTagInfo, FileIdInfo, FileStandardInfo,
    GetFileInformationByHandleEx,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::{FileFacts, ListedFacts};

/// Reparse tags with this bit stand in for another name: symbolic links, junctions, and mount
/// points. They're the only reparse points treated as links.
const NAME_SURROGATE_BIT: u32 = 0x2000_0000;
/// Any of these means some of the data lives elsewhere and reading would fetch it. They stay
/// visible even when the cloud filter disguises a placeholder (see [`in_sync_root`]).
const OFFLINE_ATTRIBUTES: u32 =
    FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS | FILE_ATTRIBUTE_RECALL_ON_OPEN | FILE_ATTRIBUTE_OFFLINE;

pub(crate) fn inspect(path: &Path) -> io::Result<FileFacts> {
    // Asking only for attribute access never downloads a cloud placeholder's data, and opening
    // the reparse point itself describes a link instead of following it.
    let file = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let id: FILE_ID_INFO = query(&file)?;
    let standard: FILE_STANDARD_INFO = query(&file)?;
    let tag: FILE_ATTRIBUTE_TAG_INFO = query(&file)?;

    let reparse_tag =
        (tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0).then_some(tag.ReparseTag);
    let kind = if reparse_tag.is_some_and(|tag| tag & NAME_SURROGATE_BIT != 0) {
        EntryKind::Link
    } else if tag.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        EntryKind::Dir
    } else {
        EntryKind::File
    };
    Ok(FileFacts {
        identity: FileIdentity {
            volume: id.VolumeSerialNumber,
            file: u128::from_le_bytes(id.FileId.Identifier),
        },
        kind,
        links: u64::from(standard.NumberOfLinks),
        allocated: u64::try_from(standard.AllocationSize).unwrap_or(0),
        cloud_synced: in_sync_root(&file)?,
        offline: tag.FileAttributes & OFFLINE_ATTRIBUTES != 0,
    })
}

pub(crate) fn listed_facts(metadata: &Metadata) -> ListedFacts {
    let offline = metadata.file_attributes() & OFFLINE_ATTRIBUTES != 0;
    ListedFacts {
        on_disk: if offline { 0 } else { metadata.len() },
        links: None,
        offline,
    }
}

/// Asks the Cloud Files API whether the file is inside a registered sync root. Checking for a
/// cloud reparse tag doesn't work: the cloud filter disguises placeholders as plain files to
/// ordinary processes, hiding the tag.
fn in_sync_root(file: &File) -> io::Result<bool> {
    const NOT_UNDER_SYNC_ROOT: i32 = hresult_from_win32(ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT);
    // What a volume without the Cloud Files filter answers, as on GitHub's Windows Server
    // runners. Nothing on such a volume can be in a sync root.
    const NO_CLOUD_FILTER: i32 = hresult_from_win32(ERROR_INVALID_FUNCTION);

    let mut info = CF_SYNC_ROOT_BASIC_INFO::default();
    let mut written = 0;
    // SAFETY: `file` is an open handle, `info` is a writable `CF_SYNC_ROOT_BASIC_INFO` (what
    // `CF_SYNC_ROOT_INFO_BASIC` returns) of exactly the size passed, and `written` is writable.
    let result = unsafe {
        CfGetSyncRootInfoByHandle(
            file.as_raw_handle(),
            CF_SYNC_ROOT_INFO_BASIC,
            (&raw mut info).cast(),
            byte_size::<CF_SYNC_ROOT_BASIC_INFO>(),
            &raw mut written,
        )
    };
    match result {
        0.. => Ok(true),
        NOT_UNDER_SYNC_ROOT | NO_CLOUD_FILTER => Ok(false),
        error => Err(io::Error::from_raw_os_error(error)),
    }
}

/// `HRESULT_FROM_WIN32`: how the Cloud Files API reports Win32 errors.
const fn hresult_from_win32(error: u32) -> i32 {
    (0x8007_0000 | error).cast_signed()
}

/// A struct that `GetFileInformationByHandleEx` fills in, paired with the information class
/// that produces it.
trait HandleInfo: Default {
    const CLASS: FILE_INFO_BY_HANDLE_CLASS;
}

impl HandleInfo for FILE_ID_INFO {
    const CLASS: FILE_INFO_BY_HANDLE_CLASS = FileIdInfo;
}

impl HandleInfo for FILE_STANDARD_INFO {
    const CLASS: FILE_INFO_BY_HANDLE_CLASS = FileStandardInfo;
}

impl HandleInfo for FILE_ATTRIBUTE_TAG_INFO {
    const CLASS: FILE_INFO_BY_HANDLE_CLASS = FileAttributeTagInfo;
}

fn query<T: HandleInfo>(file: &File) -> io::Result<T> {
    let mut info = T::default();
    // SAFETY: `file` is an open handle, `info` is a writable `T` that outlives the call, the
    // size passed is exactly `T`'s, and `T::CLASS` is the class whose output type is `T`.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            T::CLASS,
            (&raw mut info).cast(),
            byte_size::<T>(),
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(info)
    }
}

pub(crate) fn is_elevated() -> io::Result<bool> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle that needs no closing, and `token` is
    // writable for the duration of the call.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `OpenProcessToken` succeeded, so `token` is an open handle that nothing else
    // owns. `OwnedHandle` closes it when dropped.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };

    let mut elevation = TOKEN_ELEVATION::default();
    let mut written = 0;
    // SAFETY: `token` is open with `TOKEN_QUERY` access, `elevation` is a writable
    // `TOKEN_ELEVATION` (what `TokenElevation` returns) of exactly the size passed, and
    // `written` is writable.
    let ok = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenElevation,
            (&raw mut elevation).cast(),
            byte_size::<TOKEN_ELEVATION>(),
            &raw mut written,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(elevation.TokenIsElevated != 0)
}

fn byte_size<T>() -> u32 {
    u32::try_from(size_of::<T>()).expect("Win32 information structs are far smaller than 4 GiB")
}
