#![allow(
    unsafe_code,
    reason = "file identity, ownership, permissions, sync root, token, folder, and long name queries \
              need Win32 calls that std doesn't wrap"
)]

use std::env;
use std::ffi::OsString;
use std::fs::{File, Metadata, OpenOptions};
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Component, Path, PathBuf};
use std::ptr;

use cleanse_model::{EntryKind, FileIdentity};
use windows_sys::Win32::Foundation::{
    ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT, ERROR_INVALID_FUNCTION, ERROR_SUCCESS, GENERIC_ALL,
    GENERIC_WRITE, HANDLE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation, CopySid,
    DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation, GetLengthSid,
    GetSecurityDescriptorControl, GetSidIdentifierAuthority, GetSidSubAuthority,
    GetSidSubAuthorityCount, GetTokenInformation, INHERIT_ONLY_ACE, IsWellKnownSid,
    OBJECT_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SE_DACL_PROTECTED, SECURITY_NT_AUTHORITY, TOKEN_ELEVATION, TOKEN_ELEVATION_TYPE,
    TOKEN_INFORMATION_CLASS, TOKEN_QUERY, TOKEN_USER, TokenElevation, TokenElevationType,
    TokenElevationTypeDefault, TokenUser, WELL_KNOWN_SID_TYPE, WinBuiltinAdministratorsSid,
    WinLocalServiceSid, WinLocalSystemSid, WinNetworkServiceSid,
};
use windows_sys::Win32::Storage::CloudFilters::{
    CF_SYNC_ROOT_BASIC_INFO, CF_SYNC_ROOT_INFO_BASIC, CfGetSyncRootInfoByHandle,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_APPEND_DATA, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_OFFLINE,
    FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, FILE_ATTRIBUTE_RECALL_ON_OPEN,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_DELETE_CHILD,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO,
    FILE_INFO_BY_HANDLE_CLASS, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_STANDARD_INFO, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA,
    FileAttributeTagInfo, FileIdInfo, FileStandardInfo, GetFileInformationByHandleEx,
    GetLongPathNameW, GetVolumeInformationByHandleW, READ_CONTROL, WRITE_DAC, WRITE_OWNER,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW,
};
use windows_sys::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;
use windows_sys::Win32::System::SystemServices::{
    ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE, FILE_PERSISTENT_ACLS, MAXIMUM_ALLOWED,
    SECURITY_NT_NON_UNIQUE, SECURITY_SERVICE_ID_BASE_RID,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::UI::Shell::{FOLDERID_UserProfiles, KF_FLAG_DEFAULT, SHGetKnownFolderPath};

use crate::{Elevation, FileFacts, FileOwner, ListedFacts, SystemFolders};

#[cfg(any(test, feature = "privileged"))]
pub mod launch;
#[cfg(any(test, feature = "privileged"))]
pub mod pipe;

/// Reparse tags with this bit stand in for another name: symbolic links, junctions, and mount
/// points. They're the only reparse points treated as links.
const NAME_SURROGATE_BIT: u32 = 0x2000_0000;
/// Any of these means some of the data lives elsewhere and reading would fetch it. They stay
/// visible even when the cloud filter disguises a placeholder (see [`in_sync_root`]).
const OFFLINE_ATTRIBUTES: u32 =
    FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS | FILE_ATTRIBUTE_RECALL_ON_OPEN | FILE_ATTRIBUTE_OFFLINE;

pub(crate) fn inspect(path: &Path, user: &User) -> io::Result<FileFacts> {
    let (file, owner_readable) = match open(path, FILE_READ_ATTRIBUTES | READ_CONTROL) {
        Ok(file) => (file, true),
        // The folder's permissions let anyone who can list it read its entries' attributes, but
        // reading an entry's owner takes permission on the entry itself, which another account's
        // entries may not give.
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            (open(path, FILE_READ_ATTRIBUTES)?, false)
        }
        Err(error) => return Err(error),
    };
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
        owner: if owner_readable {
            owner(&file, user)?
        } else {
            None
        },
    })
}

/// Opens `path` itself, even if it's a link, for `access`.
fn open(path: &Path, access: u32) -> io::Result<File> {
    // Reading attributes and the security descriptor never downloads a cloud placeholder's
    // data, and opening the reparse point itself describes a link instead of following it.
    OpenOptions::new()
        .access_mode(access)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

/// The account this process runs as.
#[derive(Debug)]
pub(crate) struct User {
    /// Its SID, held in `u32`s so it's aligned as Windows expects a `SID` to be.
    sid: Vec<u32>,
}

impl User {
    fn sid(&self) -> PSID {
        self.sid.as_ptr().cast_mut().cast()
    }
}

pub(crate) fn current_user() -> io::Result<User> {
    let token = process_token()?;
    let mut needed = 0;
    // SAFETY: `token` is open with `TOKEN_QUERY` access. With no buffer, the call only writes the
    // size it needs to `needed`, which is writable.
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            ptr::null_mut(),
            0,
            &raw mut needed,
        )
    };
    if needed == 0 {
        return Err(io::Error::last_os_error());
    }
    // `TOKEN_USER` points to a SID stored after it in the same buffer, so the buffer is aligned
    // for pointers, which suits both.
    let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    // SAFETY: `token` is open with `TOKEN_QUERY` access, `buffer` is writable for at least the
    // `needed` bytes passed and aligned for a `TOKEN_USER`, and `needed` is writable.
    let ok = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &raw mut needed,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call succeeded, so `buffer` starts with a `TOKEN_USER` whose SID is valid for
    // as long as `buffer` is.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    // SAFETY: `sid` is a valid SID.
    let length = unsafe { GetLengthSid(sid) };
    let mut copy = vec![0u32; (length as usize).div_ceil(size_of::<u32>())];
    // SAFETY: `copy` is writable for at least the `length` bytes passed, and `sid` is a valid SID
    // of that length.
    if unsafe { CopySid(length, copy.as_mut_ptr().cast(), sid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(User { sid: copy })
}

/// Who owns `file`, which must be open with `READ_CONTROL` access, or `None` if no owner is
/// set.
fn owner(file: &File, user: &User) -> io::Result<Option<FileOwner>> {
    if !records_owners(file)? {
        return Ok(Some(FileOwner::Unrecorded));
    }
    let security = Security::read(file, OWNER_SECURITY_INFORMATION)?;
    Ok((!security.owner.is_null()).then(|| classify(security.owner, user)))
}

/// Parts of a file's security descriptor, which Windows allocated and this frees when dropped.
struct Security {
    descriptor: PSECURITY_DESCRIPTOR,
    /// Null if it wasn't asked for, or no owner is set.
    owner: PSID,
    /// Null if it wasn't asked for, or the file has no access list, which lets anyone in.
    dacl: *mut ACL,
}

impl Security {
    /// Reads `what` of `file`'s security descriptor. `file` must be open with `READ_CONTROL`
    /// access.
    fn read(file: &File, what: OBJECT_SECURITY_INFORMATION) -> io::Result<Self> {
        let mut security = Self {
            descriptor: ptr::null_mut(),
            owner: ptr::null_mut(),
            dacl: ptr::null_mut(),
        };
        // SAFETY: `file` is open with `READ_CONTROL` access, and the outputs are writable. The
        // group and audit list aren't asked for, so their outputs may be null.
        let error = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                what,
                &raw mut security.owner,
                ptr::null_mut(),
                &raw mut security.dacl,
                ptr::null_mut(),
                &raw mut security.descriptor,
            )
        };
        if error != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(error.cast_signed()));
        }
        Ok(security)
    }
}

impl Drop for Security {
    fn drop(&mut self) {
        // SAFETY: `GetSecurityInfo` allocated `descriptor` for the caller to free with
        // `LocalFree`, and `owner` and `dacl` point into it, so nothing uses them after this.
        unsafe { LocalFree(self.descriptor) };
    }
}

/// Whether the folder at `path` is one only Administrators and SYSTEM can change: a real folder,
/// not a link, that one of them owns, whose access list inherits nothing and lets no one else in.
pub(crate) fn is_private_dir(path: &Path) -> io::Result<bool> {
    let file = open(path, FILE_READ_ATTRIBUTES | READ_CONTROL)?;
    let tag: FILE_ATTRIBUTE_TAG_INFO = query(&file)?;
    if tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || tag.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
    {
        return Ok(false);
    }
    let security = Security::read(
        &file,
        OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
    )?;
    let mut control = 0;
    let mut revision = 0;
    // SAFETY: `descriptor` is a valid security descriptor, and the outputs are writable.
    let ok = unsafe {
        GetSecurityDescriptorControl(security.descriptor, &raw mut control, &raw mut revision)
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    if control & SE_DACL_PROTECTED == 0
        || security.owner.is_null()
        || !is_administrators_or_system(security.owner)
        || security.dacl.is_null()
    {
        return Ok(false);
    }

    for entry in entries(&security)? {
        match entry.kind {
            ACCESS_DENIED_ACE_TYPE => {}
            ACCESS_ALLOWED_ACE_TYPE if entry.who == Who::AdministratorsOrSystem => {}
            // Anyone else, and object, callback, and other entries, which could let someone in
            // on conditions.
            _ => return Ok(false),
        }
    }
    Ok(true)
}

/// Whether `path` is a real file or folder, not a link, that only SYSTEM, Administrators, and
/// services such as `NT SERVICE\TrustedInstaller` can change: one of them owns it, and its access
/// list lets no one else change it, what's in it, or who may. Others may read and run it.
pub(crate) fn only_admins_can_change(path: &Path) -> io::Result<bool> {
    const CHANGE: u32 = FILE_WRITE_DATA
        | FILE_APPEND_DATA
        | FILE_WRITE_EA
        | FILE_DELETE_CHILD
        | FILE_WRITE_ATTRIBUTES
        | DELETE
        | WRITE_DAC
        | WRITE_OWNER
        | GENERIC_WRITE
        | GENERIC_ALL
        | MAXIMUM_ALLOWED;

    let file = open(path, FILE_READ_ATTRIBUTES | READ_CONTROL)?;
    let tag: FILE_ATTRIBUTE_TAG_INFO = query(&file)?;
    if tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Ok(false);
    }
    let security = Security::read(
        &file,
        OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
    )?;
    // Whoever owns it can change its access list.
    if security.owner.is_null() || who(security.owner) == Who::Other || security.dacl.is_null() {
        return Ok(false);
    }
    for entry in entries(&security)? {
        match entry.kind {
            ACCESS_DENIED_ACE_TYPE => {}
            // Entries only for what's made inside it don't apply to it.
            _ if entry.flags & INHERIT_ONLY_ACE != 0 => {}
            ACCESS_ALLOWED_ACE_TYPE if entry.who != Who::Other || entry.mask & CHANGE == 0 => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

/// An entry in an access list.
struct Entry {
    kind: u32,
    flags: u32,
    mask: u32,
    /// Who an allowed or denied entry is for. Other kinds keep that elsewhere, so it's `Other`.
    who: Who,
}

/// Who an access list's entry or an owner is, as far as trusting them with what runs as an
/// administrator goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Who {
    AdministratorsOrSystem,
    /// A service's own account, such as `NT SERVICE\TrustedInstaller`, which Windows makes for
    /// services administrators install.
    Service,
    Other,
}

/// `sid` must be a valid SID.
fn who(sid: PSID) -> Who {
    if is_administrators_or_system(sid) {
        Who::AdministratorsOrSystem
    } else if is_service(sid) {
        Who::Service
    } else {
        Who::Other
    }
}

/// The entries of `security`'s access list, which must have been read and not be null.
fn entries(security: &Security) -> io::Result<Vec<Entry>> {
    let mut size = ACL_SIZE_INFORMATION::default();
    // SAFETY: `dacl` is a valid access list, and `size` is a writable `ACL_SIZE_INFORMATION`
    // (what `AclSizeInformation` returns) of exactly the size passed.
    let ok = unsafe {
        GetAclInformation(
            security.dacl,
            (&raw mut size).cast(),
            byte_size::<ACL_SIZE_INFORMATION>(),
            AclSizeInformation,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut entries = Vec::new();
    for index in 0..size.AceCount {
        let mut ace = ptr::null_mut();
        // SAFETY: `dacl` is a valid access list with more than `index` entries, and `ace` is
        // writable.
        if unsafe { GetAce(security.dacl, index, &raw mut ace) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `GetAce` gave a pointer to an entry in the access list, which starts with a
        // header.
        let header = unsafe { *ace.cast::<ACE_HEADER>() };
        let kind = u32::from(header.AceType);
        let (mask, who) = if kind == ACCESS_ALLOWED_ACE_TYPE || kind == ACCESS_DENIED_ACE_TYPE {
            let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
            // SAFETY: allowed and denied entries are both laid out as an `ACCESS_ALLOWED_ACE`,
            // whose SID starts at `SidStart`.
            unsafe { ((*ace).Mask, who((&raw mut (*ace).SidStart).cast())) }
        } else {
            (0, Who::Other)
        };
        entries.push(Entry {
            kind,
            flags: u32::from(header.AceFlags),
            mask,
            who,
        });
    }
    Ok(entries)
}

/// `sid` must be a valid SID.
fn is_administrators_or_system(sid: PSID) -> bool {
    is_sid(sid, WinBuiltinAdministratorsSid) || is_sid(sid, WinLocalSystemSid)
}

/// Whether `sid`, a valid SID, is the well-known one of `kind`.
fn is_sid(sid: PSID, kind: WELL_KNOWN_SID_TYPE) -> bool {
    // SAFETY: `sid` is a valid SID.
    unsafe { IsWellKnownSid(sid, kind) != 0 }
}

/// Has Windows load the DLLs this process asks for only from System32 from now on.
#[cfg(any(test, feature = "privileged"))]
pub(crate) fn load_dlls_from_system32_only() -> io::Result<()> {
    use windows_sys::Win32::System::LibraryLoader::{
        LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories,
    };

    // SAFETY: setting where DLLs are looked for has no preconditions.
    if unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Loads and unloads the DLL named `name`, as Windows looks for it, for tests. It must be a copy
/// of one of Windows' own.
#[cfg(test)]
pub(crate) fn load_dll(name: &str) -> io::Result<()> {
    use windows_sys::Win32::Foundation::FreeLibrary;
    use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;

    let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
    // SAFETY: `name` is NUL-terminated, and the caller passes a DLL that's safe to load.
    let module = unsafe { LoadLibraryW(name.as_ptr()) };
    if module.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `module` was just loaded, and nothing else uses it.
    unsafe { FreeLibrary(module) };
    Ok(())
}

/// Creates the folder at `path`, which mustn't exist, owned by Administrators, with an access list
/// that inherits nothing and lets only Administrators and SYSTEM in from the moment it exists.
#[cfg(any(test, feature = "privileged"))]
pub(crate) fn create_private_dir(path: &Path) -> io::Result<()> {
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;

    // Owned by Administrators (S-1-5-32-544), protected from inheriting, and full control for
    // SYSTEM (S-1-5-18) and Administrators, which folders and files inside inherit.
    const SDDL: &str = "O:S-1-5-32-544D:P(A;OICI;FA;;;S-1-5-18)(A;OICI;FA;;;S-1-5-32-544)";

    let sddl: Vec<u16> = SDDL.encode_utf16().chain([0]).collect();
    let path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: `sddl` is NUL-terminated, `descriptor` is writable, and the size output may be
    // null.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &raw mut descriptor,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: byte_size::<SECURITY_ATTRIBUTES>(),
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    // SAFETY: `path` is NUL-terminated, and `attributes` holds the valid security descriptor
    // made above.
    let created = unsafe { CreateDirectoryW(path.as_ptr(), &raw const attributes) } != 0;
    let error = io::Error::last_os_error();
    // SAFETY: the conversion allocated `descriptor` for the caller to free with `LocalFree`, and
    // it isn't used after this.
    unsafe { LocalFree(descriptor) };
    if created { Ok(()) } else { Err(error) }
}

/// Whether the volume `file` is on records owners. FAT and exFAT drives don't, and Windows
/// reports a made-up owner for their files.
fn records_owners(file: &File) -> io::Result<bool> {
    let mut flags = 0;
    // SAFETY: `file` is an open handle and `flags` is writable. The other outputs are null with
    // no size, so nothing else is written.
    let ok = unsafe {
        GetVolumeInformationByHandleW(
            file.as_raw_handle(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            &raw mut flags,
            ptr::null_mut(),
            0,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(flags & FILE_PERSISTENT_ACLS != 0)
    }
}

/// `sid` must be a valid SID.
fn classify(sid: PSID, user: &User) -> FileOwner {
    let is = |kind| is_sid(sid, kind);
    let is_user = || {
        // SAFETY: `sid` and the user's SID are valid SIDs.
        unsafe { EqualSid(sid, user.sid()) != 0 }
    };
    if is(WinLocalSystemSid)
        || is(WinLocalServiceSid)
        || is(WinNetworkServiceSid)
        || is_service(sid)
    {
        FileOwner::System
    } else if is(WinBuiltinAdministratorsSid) {
        FileOwner::Administrators
    } else if is_user() {
        FileOwner::CurrentUser
    } else {
        FileOwner::Other
    }
}

/// Whether `sid`, a valid SID, is a service's own account, S-1-5-80-…, such as
/// `NT SERVICE\TrustedInstaller`, which owns most of the Windows folder.
fn is_service(sid: PSID) -> bool {
    // SAFETY: `sid` is a valid SID, so its authority and count of subauthorities can be read.
    let (authority, count) = unsafe {
        (
            (*GetSidIdentifierAuthority(sid)).Value,
            *GetSidSubAuthorityCount(sid),
        )
    };
    authority == SECURITY_NT_AUTHORITY.Value
        && count > 0
        // SAFETY: `sid` is a valid SID with at least one subauthority.
        && unsafe { *GetSidSubAuthority(sid, 0) } == SECURITY_SERVICE_ID_BASE_RID.cast_unsigned()
}

/// `%TEMP%` can name folders by their 8.3 short names, like `C:\Users\RUNNER~1\...`, but folder
/// listings give long names, which known locations are compared with. So the temp folder is
/// given by its long names, or as it is if they can't be read.
pub(crate) fn temp_dir() -> PathBuf {
    let temp = env::temp_dir();
    long_path(&temp).unwrap_or(temp)
}

pub(crate) fn system_folders() -> io::Result<SystemFolders> {
    let system_root = windows_folder()?;
    let program_data = program_data(&system_root)?;
    Ok(SystemFolders {
        system_root: Some(system_root),
        program_data: Some(program_data),
        var: None,
    })
}

/// The Windows folder as the system recorded it at startup, which `%SystemRoot%` and `%windir%`
/// don't change.
fn windows_folder() -> io::Result<PathBuf> {
    const MAX_PATH: usize = 260;

    let mut buffer = vec![0; MAX_PATH];
    loop {
        let size = u32::try_from(buffer.len()).expect("the buffer holds a path's length");
        // SAFETY: `buffer` is writable for the `size` characters passed.
        let length = unsafe { GetSystemWindowsDirectoryW(buffer.as_mut_ptr(), size) };
        let length = usize::try_from(length).expect("a path's length fits in memory");
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        // A length that doesn't fit is the size needed, counting the NUL. Otherwise it doesn't
        // count the NUL.
        if length < buffer.len() {
            buffer.truncate(length);
            return Ok(PathBuf::from(OsString::from_wide(&buffer)));
        }
        buffer.resize(length, 0);
    }
}

/// The `ProgramData` folder as the registry records it, usually `%SystemDrive%\ProgramData`.
fn program_data(windows: &Path) -> io::Result<PathBuf> {
    recorded_folder(
        windows,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList",
        "ProgramData",
    )
}

/// Where Windows installs programs for every user, usually `C:\Program Files`, as the registry
/// records it.
pub(crate) fn program_files() -> io::Result<PathBuf> {
    recorded_folder(
        &windows_folder()?,
        r"SOFTWARE\Microsoft\Windows\CurrentVersion",
        "ProgramFilesDir",
    )
}

/// A folder as the registry records it in `value` under `key`. The shell's known-folder lookup
/// expands `%SystemDrive%` in it from the environment, so here that's the drive of `windows`, the
/// Windows folder, which is what it means. Any other variable is refused.
fn recorded_folder(windows: &Path, key: &str, value: &str) -> io::Result<PathBuf> {
    const SYSTEM_DRIVE: &str = "%SystemDrive%";

    let recorded = registry_text(key, value)?;
    let Some(Component::Prefix(drive)) = windows.components().next() else {
        return Err(io::Error::other(format!(
            "the Windows folder, {}, isn't on a drive",
            windows.display()
        )));
    };
    let mut folder = OsString::new();
    let rest = match recorded.get(..SYSTEM_DRIVE.len()) {
        Some(start) if start.eq_ignore_ascii_case(SYSTEM_DRIVE) => {
            folder.push(drive.as_os_str());
            &recorded[SYSTEM_DRIVE.len()..]
        }
        _ => &recorded,
    };
    folder.push(rest);
    let folder = PathBuf::from(folder);
    if rest.contains('%') || !folder.is_absolute() {
        return Err(io::Error::other(format!(
            "the registry records {value} as {recorded}, which isn't a full path or names an \
             environment variable other than {SYSTEM_DRIVE}"
        )));
    }
    Ok(folder)
}

/// A text value under `HKEY_LOCAL_MACHINE`, which only administrators can change, as it's stored:
/// any environment variables in it aren't expanded.
fn registry_text(key: &str, value: &str) -> io::Result<String> {
    const FLAGS: u32 = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;

    let key: Vec<u16> = key.encode_utf16().chain([0]).collect();
    let value: Vec<u16> = value.encode_utf16().chain([0]).collect();
    let mut size = 0;
    // SAFETY: `key` and `value` are NUL-terminated. Without a buffer, the call only writes the
    // size it needs, in bytes, to `size`, which is writable.
    let error = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            FLAGS,
            ptr::null_mut(),
            ptr::null_mut(),
            &raw mut size,
        )
    };
    if error != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(error.cast_signed()));
    }
    let mut text = vec![0u16; (size as usize).div_ceil(size_of::<u16>())];
    // SAFETY: `key` and `value` are NUL-terminated, `text` is writable for at least the `size`
    // bytes passed, and `size` is writable.
    let error = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            FLAGS,
            ptr::null_mut(),
            text.as_mut_ptr().cast(),
            &raw mut size,
        )
    };
    if error != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(error.cast_signed()));
    }
    let length = text
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(text.len());
    String::from_utf16(&text[..length]).map_err(io::Error::other)
}

fn long_path(path: &Path) -> Option<PathBuf> {
    let short: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: `short` is NUL-terminated. Without a buffer, the call only returns the length
    // needed, counting the NUL.
    let needed = unsafe { GetLongPathNameW(short.as_ptr(), ptr::null_mut(), 0) };
    if needed == 0 {
        return None;
    }
    let mut long = vec![0; usize::try_from(needed).ok()?];
    // SAFETY: `short` is NUL-terminated, and `long` is writable for the `needed` characters
    // passed as its size.
    let written = unsafe { GetLongPathNameW(short.as_ptr(), long.as_mut_ptr(), needed) };
    // The length written doesn't count the NUL. Needing more than fits means the path changed
    // between the calls.
    if written == 0 || written >= needed {
        return None;
    }
    long.truncate(usize::try_from(written).ok()?);
    Some(PathBuf::from(OsString::from_wide(&long)))
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

/// The process's access token, open for queries.
fn process_token() -> io::Result<OwnedHandle> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle that needs no closing, and `token` is
    // writable for the duration of the call.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `OpenProcessToken` succeeded, so `token` is an open handle that nothing else
    // owns. `OwnedHandle` closes it when dropped.
    Ok(unsafe { OwnedHandle::from_raw_handle(token) })
}

pub(crate) fn elevation(user: &User) -> io::Result<Elevation> {
    let token = process_token()?;
    // SAFETY: `TokenElevation` returns a `TOKEN_ELEVATION`, a `u32`, and `TokenElevationType`
    // a `TOKEN_ELEVATION_TYPE`, an `i32`.
    let (elevation, kind) = unsafe {
        (
            token_information::<TOKEN_ELEVATION>(&token, TokenElevation)?,
            token_information::<TOKEN_ELEVATION_TYPE>(&token, TokenElevationType)?,
        )
    };
    Ok(elevation_of(
        elevation.TokenIsElevated != 0,
        kind,
        is_person(user.sid()),
    ))
}

/// An elevated token has an unelevated twin for the same sign-in only when its type is
/// `TokenElevationTypeFull`. `TokenElevationTypeDefault` means Windows gave the account this
/// token alone, so everything it runs is elevated: UAC is off, or the account is one UAC
/// doesn't filter, such as the built-in Administrator.
pub(crate) fn elevation_of(elevated: bool, kind: TOKEN_ELEVATION_TYPE, person: bool) -> Elevation {
    if !elevated {
        Elevation::None
    } else if kind == TokenElevationTypeDefault && person {
        Elevation::AlwaysElevated
    } else {
        Elevation::Elevated
    }
}

/// The token information of `class`, which is a `T`.
///
/// # Safety
///
/// `T` must be the fixed-size type `class` returns, valid whatever bytes it holds.
unsafe fn token_information<T: Default>(
    token: &OwnedHandle,
    class: TOKEN_INFORMATION_CLASS,
) -> io::Result<T> {
    let mut information = T::default();
    let mut written = 0;
    // SAFETY: `token` is open with `TOKEN_QUERY` access, `information` is a writable `T`, the
    // type `class` returns, of exactly the size passed, and `written` is writable.
    let ok = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            class,
            (&raw mut information).cast(),
            byte_size::<T>(),
            &raw mut written,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(information)
}

/// Whether `sid`, a valid SID, is a person's account rather than a service's: a local or domain
/// account, S-1-5-21-…, or a Microsoft Entra ID one, S-1-12-1-….
fn is_person(sid: PSID) -> bool {
    /// Microsoft Entra ID's authority, 12, which `windows-sys` has no name for.
    const ENTRA_ID_AUTHORITY: [u8; 6] = [0, 0, 0, 0, 0, 12];

    // SAFETY: `sid` is a valid SID, so its authority and count of subauthorities can be read.
    let (authority, count) = unsafe {
        (
            (*GetSidIdentifierAuthority(sid)).Value,
            *GetSidSubAuthorityCount(sid),
        )
    };
    if count == 0 {
        return false;
    }
    // SAFETY: `sid` is a valid SID with at least one subauthority.
    let first = unsafe { *GetSidSubAuthority(sid, 0) };
    (authority == SECURITY_NT_AUTHORITY.Value && first == SECURITY_NT_NON_UNIQUE.cast_unsigned())
        || (authority == ENTRA_ID_AUTHORITY && first == 1)
}

/// Whether the account whose SID is `text`, such as `S-1-5-18`, counts as a person's.
#[cfg(test)]
pub(crate) fn is_person_sid(text: &str) -> io::Result<bool> {
    use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;

    let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
    let mut sid = ptr::null_mut();
    // SAFETY: `wide` is NUL-terminated, and `sid` is writable.
    if unsafe { ConvertStringSidToSidW(wide.as_ptr(), &raw mut sid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let person = is_person(sid);
    // SAFETY: `ConvertStringSidToSidW` allocated `sid` for the caller to free with `LocalFree`,
    // and nothing uses it after this.
    unsafe { LocalFree(sid.cast()) };
    Ok(person)
}

/// `FOLDERID_UserProfiles`, where Windows makes every account's home folder.
pub(crate) fn user_profiles() -> Option<PathBuf> {
    let folder_id = FOLDERID_UserProfiles;
    let mut path = ptr::null_mut();
    // SAFETY: `folder_id` is a valid GUID, a null token means this process's, and `path` is
    // writable.
    let result = unsafe {
        SHGetKnownFolderPath(
            &raw const folder_id,
            KF_FLAG_DEFAULT.cast_unsigned(),
            ptr::null_mut(),
            &raw mut path,
        )
    };
    let folder = (result >= 0 && !path.is_null()).then(|| {
        // SAFETY: on success, `path` is a NUL-terminated string that stays valid until it's
        // freed below.
        let wide = unsafe {
            let mut length = 0;
            while *path.add(length) != 0 {
                length += 1;
            }
            std::slice::from_raw_parts(path, length)
        };
        PathBuf::from(OsString::from_wide(wide))
    });
    // SAFETY: Windows allocated `path`, or left it null, and it must be freed whether the call
    // succeeded or not. Nothing uses it after this.
    unsafe { CoTaskMemFree(path.cast()) };
    folder
}

fn byte_size<T>() -> u32 {
    u32::try_from(size_of::<T>()).expect("Win32 information structs are far smaller than 4 GiB")
}
