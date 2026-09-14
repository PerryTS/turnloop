//! Synchronous inherited pipe: one explicitly requested reader thread.
use crate::port::{Entry, Port, Wait, bool_result, owned};
use std::{
    io,
    os::windows::io::AsRawHandle,
    ptr,
    sync::{Arc, Mutex},
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::ERROR_BROKEN_PIPE,
    Storage::FileSystem::{ReadFile, WriteFile},
    System::Pipes::CreatePipe,
};

pub fn probe() -> io::Result<usize> {
    let port = Arc::new(Port::new()?);
    let mut read_handle = ptr::null_mut();
    let mut write_handle = ptr::null_mut();
    // SAFETY: two valid outputs; CreatePipe deliberately creates synchronous handles.
    // https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-createpipe
    bool_result(unsafe { CreatePipe(&mut read_handle, &mut write_handle, ptr::null(), 4096) })?;
    // SAFETY: successful CreatePipe returns two unique owning handles.
    let (reader, writer) = unsafe { (owned(read_handle)?, owned(write_handle)?) };
    let storage = Arc::new(Mutex::new(([0u8; 64], 0usize)));
    let thread_port = Arc::clone(&port);
    let thread_storage = Arc::clone(&storage);
    let worker = std::thread::spawn(move || -> io::Result<()> {
        let mut data = [0u8; 64];
        let mut total = 0;
        loop {
            let mut n = 0;
            // SAFETY: reader uniquely owned on worker; synchronous call cannot
            // outlive stack buffer. Posting only after releasing the queue lock.
            let ok = unsafe {
                ReadFile(
                    reader.as_raw_handle(),
                    data.as_mut_ptr(),
                    data.len() as u32,
                    &mut n,
                    ptr::null_mut(),
                )
            };
            if ok == 0 {
                let e = io::Error::last_os_error();
                if e.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
                    break;
                }
                return Err(e);
            }
            if n == 0 {
                break;
            }
            let mut output = thread_storage
                .lock()
                .map_err(|_| io::Error::other("reader queue poisoned"))?;
            let end = total + n as usize;
            if end > output.0.len() {
                return Err(io::Error::other("reader capacity exceeded"));
            }
            output.0[total..end].copy_from_slice(&data[..n as usize]);
            total = end;
            output.1 = total;
            drop(output);
            thread_port.post(20, n)?;
        }
        thread_port.post(21, total as u32)
    });
    let data = b"synchronous-stdio";
    let mut n = 0;
    // SAFETY: synchronous write; data lives until call returns.
    let write_result = bool_result(unsafe {
        WriteFile(
            writer.as_raw_handle(),
            data.as_ptr(),
            data.len() as u32,
            &mut n,
            ptr::null_mut(),
        )
    });
    drop(writer); // EOF unblocks the reader, including all subsequent error paths.
    let thread_result = worker
        .join()
        .map_err(|_| io::Error::other("reader panicked"))?;
    write_result?;
    thread_result?;
    assert_eq!(n as usize, data.len());
    let mut received = 0;
    let mut eof = false;
    let mut entries = [Entry::default(); 8];
    while !eof {
        let Wait::Entries(n) = port.wait(Some(Duration::from_secs(2)), false, &mut entries)? else {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "stdio completion missing",
            ));
        };
        for entry in &entries[..n] {
            match entry.key {
                20 => received += entry.bytes as usize,
                21 => {
                    assert_eq!(entry.bytes as usize, data.len());
                    eof = true;
                }
                _ => return Err(io::Error::other("unexpected stdio packet")),
            }
        }
    }
    let output = storage
        .lock()
        .map_err(|_| io::Error::other("reader queue poisoned"))?;
    assert_eq!(&output.0[..output.1], data);
    assert_eq!(received, data.len());
    Ok(received)
}
