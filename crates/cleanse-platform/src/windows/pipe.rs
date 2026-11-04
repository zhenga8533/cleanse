//! The pipes between the desktop app's window and its elevated helper on Windows (ADR 0010).
//! Windows' UAC prompt can't hand the helper pipes from the window, as pkexec does on Linux, so
//! the helper makes named pipes of its own, which only the window's logon session can open, and
//! each side checks that the other is the process it started or was started by. There are two
//! pipes, one each way: a pipe open for blocking reads and writes can't do both at once.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr;
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_GROUPS,
    TOKEN_QUERY, TokenLogonSid,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_WRITE_DATA,
    PIPE_ACCESS_INBOUND, PIPE_ACCESS_OUTBOUND, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
    SYNCHRONIZE,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, GetNamedPipeServerProcessId,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};

use super::byte_size;

/// The longest name a pair of pipes may have.
const MAX_NAME: usize = 64;
/// What each pipe holds before a write waits for the other side to read.
const BUFFER: u32 = 64 * 1024;
/// How often the window looks for the helper's pipes while it starts.
const RETRY: Duration = Duration::from_millis(50);

/// The window writes requests on one pipe, and reads answers on the other.
pub(crate) const REQUESTS: &str = "requests";
const ANSWERS: &str = "answers";

/// The two pipes between the window and the helper. The window writes to `requests` and reads
/// `answers`; the helper reads `requests` and writes `answers`.
#[derive(Debug)]
pub struct Connection {
    pub requests: File,
    pub answers: File,
}

/// The process that started the helper, checked to be the desktop app in this session.
#[derive(Debug)]
pub struct Parent {
    pid: u32,
    /// Its logon session's SID, as SDDL writes it, which is all the helper's pipes let in.
    logon: String,
    /// Held open, so that its process ID can't go to another process while the helper waits.
    _process: OwnedHandle,
}

impl Parent {
    /// Checks that process `pid` runs `program` in this process's session, and reads its logon
    /// session.
    pub fn check(pid: u32, program: &Path) -> io::Result<Self> {
        let process = open_process(pid)?;
        let image = image_path(&process)?;
        // Windows paths ignore case, and both are the full paths Windows gives.
        if !image.as_os_str().eq_ignore_ascii_case(program.as_os_str()) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "process {pid} runs {}, not {}",
                    image.display(),
                    program.display()
                ),
            ));
        }
        if session_of(pid)? != session_of(std::process::id())? {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("process {pid} is in another session"),
            ));
        }
        Ok(Self {
            pid,
            logon: logon_sid(&process)?,
            _process: process,
        })
    }
}

/// Makes the helper's pipes, named after `name`, which only `parent`'s logon session can open,
/// and waits for `parent` to connect to both. A pipe of that name that's there already, which
/// someone else made, makes this fail.
pub fn serve(name: &str, parent: &Parent) -> io::Result<Connection> {
    let requests = create(name, REQUESTS, PIPE_ACCESS_INBOUND, FILE_WRITE_DATA, parent)?;
    let answers = create(name, ANSWERS, PIPE_ACCESS_OUTBOUND, FILE_READ_DATA, parent)?;
    accept(&requests, parent.pid)?;
    accept(&answers, parent.pid)?;
    Ok(Connection { requests, answers })
}

/// Connects to the helper's pipes, named after `name`, which process `server` must have made,
/// waiting up to `wait` for them while the helper starts.
pub fn connect(name: &str, server: u32, wait: Duration) -> io::Result<Connection> {
    let deadline = Instant::now() + wait;
    let open = |way: &str, right: u32| -> io::Result<File> {
        let path = pipe_name(name, way)?;
        loop {
            // Identification only, so the helper can tell who connected but not act as them.
            let opened = OpenOptions::new()
                .access_mode(SYNCHRONIZE | FILE_READ_ATTRIBUTES | right)
                .custom_flags(SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION)
                .open(&path);
            match opened {
                Err(error)
                    if error.kind() == io::ErrorKind::NotFound && Instant::now() < deadline =>
                {
                    thread::sleep(RETRY);
                }
                opened => return opened,
            }
        }
    };
    let connection = Connection {
        requests: open(REQUESTS, FILE_WRITE_DATA)?,
        answers: open(ANSWERS, FILE_READ_DATA)?,
    };
    for pipe in [&connection.requests, &connection.answers] {
        let mut made_by = 0;
        // SAFETY: `pipe` is an open pipe handle, and `made_by` is writable.
        if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &raw mut made_by) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if made_by != server {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("process {made_by} made the helper's pipes, not process {server}"),
            ));
        }
    }
    Ok(connection)
}

/// `\\.\pipe\<name>-<way>`. Names are letters, digits, and dashes.
fn pipe_name(name: &str, way: &str) -> io::Result<String> {
    let valid = !name.is_empty()
        && name.len() <= MAX_NAME
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    if !valid {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name:?} isn't a name for the helper's pipes"),
        ));
    }
    Ok(format!(r"\\.\pipe\{name}-{way}"))
}

/// Makes one of the helper's pipes, flowing `direction`, which lets `parent`'s logon session in
/// for `right`, waiting, and reading attributes, which checking the helper's process ID takes.
pub(crate) fn create(
    name: &str,
    way: &str,
    direction: u32,
    right: u32,
    parent: &Parent,
) -> io::Result<File> {
    let path: Vec<u16> = pipe_name(name, way)?.encode_utf16().chain([0]).collect();
    let sddl: Vec<u16> = format!(
        "D:P(A;;0x{:x};;;{})",
        SYNCHRONIZE | FILE_READ_ATTRIBUTES | right,
        parent.logon
    )
    .encode_utf16()
    .chain([0])
    .collect();
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
    // SAFETY: `path` is NUL-terminated, and `attributes` holds the valid security descriptor made
    // above. One instance only, so no one else can make another of the same name.
    let pipe = unsafe {
        CreateNamedPipeW(
            path.as_ptr(),
            direction | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            BUFFER,
            BUFFER,
            0,
            &raw const attributes,
        )
    };
    let error = io::Error::last_os_error();
    // SAFETY: the conversion allocated `descriptor` for the caller to free with `LocalFree`, and
    // it isn't used after this.
    unsafe { LocalFree(descriptor) };
    if pipe == INVALID_HANDLE_VALUE {
        return Err(error);
    }
    // SAFETY: `CreateNamedPipeW` succeeded, so `pipe` is an open handle nothing else owns.
    Ok(File::from(unsafe { OwnedHandle::from_raw_handle(pipe) }))
}

/// Waits for a client to connect to `pipe`, and checks that it's process `parent`.
fn accept(pipe: &File, parent: u32) -> io::Result<()> {
    // SAFETY: `pipe` is an open pipe handle, made for blocking calls, so no `OVERLAPPED` is
    // needed.
    let connected = unsafe { ConnectNamedPipe(pipe.as_raw_handle(), ptr::null_mut()) } != 0;
    // A client that connected before the call is connected too.
    if !connected {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_PIPE_CONNECTED.cast_signed()) {
            return Err(error);
        }
    }
    let mut client = 0;
    // SAFETY: `pipe` is an open pipe handle, and `client` is writable.
    if unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle(), &raw mut client) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if client != parent {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("process {client} connected, not process {parent}"),
        ));
    }
    Ok(())
}

fn open_process(pid: u32) -> io::Result<OwnedHandle> {
    // SAFETY: opening a process by ID has no preconditions.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `OpenProcess` succeeded, so `process` is an open handle nothing else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(process) })
}

/// The full path of the program `process` runs.
fn image_path(process: &OwnedHandle) -> io::Result<PathBuf> {
    // Windows paths are at most 32,767 characters.
    let mut path = vec![0u16; 32_768];
    let mut length = u32::try_from(path.len()).expect("the buffer is small");
    // SAFETY: `process` is open with `PROCESS_QUERY_LIMITED_INFORMATION` access, and `path` is
    // writable for the `length` characters passed.
    let ok = unsafe {
        QueryFullProcessImageNameW(
            process.as_raw_handle(),
            PROCESS_NAME_WIN32,
            path.as_mut_ptr(),
            &raw mut length,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    path.truncate(usize::try_from(length).expect("a path's length fits in memory"));
    Ok(PathBuf::from(OsString::from_wide(&path)))
}

fn session_of(pid: u32) -> io::Result<u32> {
    let mut session = 0;
    // SAFETY: `session` is writable.
    if unsafe { ProcessIdToSessionId(pid, &raw mut session) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(session)
}

/// The SID of `process`'s logon session, as SDDL writes it.
fn logon_sid(process: &OwnedHandle) -> io::Result<String> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: `process` is open with `PROCESS_QUERY_LIMITED_INFORMATION` access, which opening
    // its token takes, and `token` is writable.
    if unsafe { OpenProcessToken(process.as_raw_handle(), TOKEN_QUERY, &raw mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `OpenProcessToken` succeeded, so `token` is an open handle nothing else owns.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut needed = 0;
    // SAFETY: `token` is open with `TOKEN_QUERY` access. With no buffer, the call only writes the
    // size it needs to `needed`.
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenLogonSid,
            ptr::null_mut(),
            0,
            &raw mut needed,
        )
    };
    if needed == 0 {
        return Err(io::Error::last_os_error());
    }
    // `TOKEN_GROUPS` points to SIDs stored after it in the same buffer, so the buffer is aligned
    // for pointers.
    let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    // SAFETY: `token` is open with `TOKEN_QUERY` access, and `buffer` is writable for at least
    // the `needed` bytes passed and aligned for a `TOKEN_GROUPS`.
    let ok = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenLogonSid,
            buffer.as_mut_ptr().cast(),
            needed,
            &raw mut needed,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call succeeded, so `buffer` starts with a `TOKEN_GROUPS`.
    let groups = unsafe { &*buffer.as_ptr().cast::<TOKEN_GROUPS>() };
    if groups.GroupCount == 0 {
        return Err(io::Error::other("the process has no logon session"));
    }
    // SAFETY: the group's SID is valid while `buffer` is.
    unsafe { sid_text(groups.Groups[0].Sid) }
}

/// `sid` as SDDL writes it.
///
/// # Safety
///
/// `sid` must point to a valid SID.
unsafe fn sid_text(sid: PSID) -> io::Result<String> {
    let mut text = ptr::null_mut();
    // SAFETY: the caller passes a valid SID, and `text` is writable.
    if unsafe { ConvertSidToStringSidW(sid, &raw mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut length = 0;
    // SAFETY: the conversion succeeded, so `text` is a NUL-terminated string, read up to its NUL.
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: `text` holds `length` characters before its NUL.
    let sid = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    // SAFETY: the conversion allocated `text` for the caller to free with `LocalFree`, and it
    // isn't used after this.
    unsafe { LocalFree(text.cast()) };
    Ok(sid)
}

/// The SIDs, as SDDL writes them, that `pipe`'s access list lets in, and what each may do.
#[cfg(test)]
pub(crate) fn allowed(pipe: &File) -> io::Result<Vec<(String, u32)>> {
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL_SIZE_INFORMATION, AclSizeInformation, DACL_SECURITY_INFORMATION,
        GetAce, GetAclInformation,
    };

    let security = super::Security::read(pipe, DACL_SECURITY_INFORMATION)?;
    let mut size = ACL_SIZE_INFORMATION::default();
    // SAFETY: `dacl` is a valid access list, and `size` is a writable `ACL_SIZE_INFORMATION` of
    // exactly the size passed.
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
    let mut allowed = Vec::new();
    for index in 0..size.AceCount {
        let mut ace = ptr::null_mut();
        // SAFETY: `dacl` is a valid access list with more than `index` entries.
        if unsafe { GetAce(security.dacl, index, &raw mut ace) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
        // SAFETY: `ace` is a valid entry, whose SID starts at `SidStart`. The pipes' access lists
        // hold only allowed entries.
        let entry = unsafe { (sid_text((&raw mut (*ace).SidStart).cast())?, (*ace).Mask) };
        allowed.push(entry);
    }
    Ok(allowed)
}

/// What SDDL writes for the logon session of the process `pid`, for tests.
#[cfg(test)]
pub(crate) fn logon_of(pid: u32) -> io::Result<String> {
    logon_sid(&open_process(pid)?)
}
