//! Starting a program as an administrator through the UAC prompt, as the desktop app starts its
//! elevated helper (ADR 0010).

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_CANCELLED, STILL_ACTIVE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessId, WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

use super::byte_size;

/// A process [`run_as_administrator`] started. Its process ID stays its own while this is held.
#[derive(Debug)]
pub struct Process {
    handle: OwnedHandle,
    id: u32,
}

impl Process {
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Its exit code, or `None` while it runs.
    pub fn exit_code(&self) -> io::Result<Option<u32>> {
        // SAFETY: `handle` is an open process handle. A wait of 0 only checks.
        if unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) } != WAIT_OBJECT_0 {
            return Ok(None);
        }
        let mut code = STILL_ACTIVE.cast_unsigned();
        // SAFETY: `handle` is an open process handle, and `code` is writable.
        if unsafe { GetExitCodeProcess(self.handle.as_raw_handle(), &raw mut code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Some(code))
    }
}

/// Starts `program` with `parameters` as an administrator, which shows the UAC prompt unless
/// this process is an administrator already. The prompt belongs to the window whose `HWND` is
/// `owner`, if given, which brings it to the front. Declining it fails with
/// [`io::ErrorKind::PermissionDenied`].
pub fn run_as_administrator(
    program: &Path,
    parameters: &str,
    owner: Option<usize>,
) -> io::Result<Process> {
    let verb = wide("runas".as_ref());
    let file = wide(program.as_os_str());
    let parameters = wide(parameters.as_ref());
    let mut info = SHELLEXECUTEINFOW {
        cbSize: byte_size::<SHELLEXECUTEINFOW>(),
        // Hand back the process, start it before returning, and leave errors to the caller to
        // show.
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        // A window handle is a number Windows gave out, not memory.
        hwnd: owner.map_or(ptr::null_mut(), ptr::without_provenance_mut),
        lpVerb: verb.as_ptr(),
        lpFile: file.as_ptr(),
        lpParameters: parameters.as_ptr(),
        nShow: SW_HIDE,
        ..SHELLEXECUTEINFOW::default()
    };
    // SAFETY: `info` is a `SHELLEXECUTEINFOW` of the size it says, and its strings are
    // NUL-terminated and outlive the call.
    if unsafe { ShellExecuteExW(&raw mut info) } == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_CANCELLED.cast_signed()) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the request to run as an administrator was declined",
            ));
        }
        return Err(error);
    }
    if info.hProcess.is_null() {
        return Err(io::Error::other(
            "Windows didn't say which process it started",
        ));
    }
    // SAFETY: with `SEE_MASK_NOCLOSEPROCESS`, `hProcess` is an open handle the caller owns.
    let handle = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    // SAFETY: `handle` is an open process handle.
    let id = unsafe { GetProcessId(handle.as_raw_handle()) };
    if id == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Process { handle, id })
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain([0]).collect()
}
