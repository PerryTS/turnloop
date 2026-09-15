use super::{Detached, Kind, Native, invalid, port::owned};
use crate::{PipeName, Result};
use std::{os::windows::ffi::OsStrExt, ptr};
use windows_sys::Win32::{Foundation::*, Storage::FileSystem::*, System::Pipes::*};

pub(super) fn name(name: &PipeName) -> Result<Vec<u16>> {
    let mut value: Vec<u16> = name.0.as_os_str().encode_wide().collect();
    let prefix: Vec<u16> = r"\\.\pipe\".encode_utf16().collect();
    if !value.starts_with(&prefix) || value.len() == prefix.len() || value.contains(&0) {
        return Err(invalid());
    }
    value.push(0);
    Ok(value)
}
pub(super) fn instance(name: &[u16], first: bool) -> Result<Detached> {
    // SAFETY: validated terminated local name; exclusively owned overlapped byte
    // pipe. FIRST_PIPE_INSTANCE rejects accidental binding to another server.
    let handle = unsafe {
        owned(CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX
                | FILE_FLAG_OVERLAPPED
                | if first {
                    FILE_FLAG_FIRST_PIPE_INSTANCE
                } else {
                    0
                },
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            65536,
            65536,
            0,
            ptr::null(),
        ))
    }?;
    Ok(Detached::new(
        Native::Handle(handle),
        Kind::PipeListener,
        false,
    ))
}
pub(super) fn connect(name: &[u16]) -> Result<Detached> {
    // SAFETY: validated terminated name; noninherited, overlapped client endpoint.
    let handle = unsafe {
        owned(CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
            ptr::null_mut(),
        ))
    }?;
    Ok(Detached::new(Native::Handle(handle), Kind::Pipe, false))
}
