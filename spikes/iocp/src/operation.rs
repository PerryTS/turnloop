//! Reusable probe storage. Allocates at fixture creation, never on re-submission.
use crate::port::{Entry, Port, Wait, bool_result, owned};
use std::{
    cell::UnsafeCell,
    io,
    os::windows::io::{AsRawHandle, AsRawSocket, OwnedHandle, OwnedSocket},
    ptr,
    rc::Rc,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{ERROR_IO_PENDING, ERROR_NOT_FOUND, HANDLE},
    Networking::WinSock::{WSAGetLastError, WSAGetOverlappedResult},
    System::{
        IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
        Threading::{CreateEventW, ResetEvent},
    },
};

pub(crate) enum Endpoint {
    Socket(OwnedSocket),
    Pipe(OwnedHandle),
}
impl Endpoint {
    pub fn raw(&self) -> HANDLE {
        match self {
            Self::Socket(s) => s.as_raw_socket() as HANDLE,
            Self::Pipe(p) => p.as_raw_handle(),
        }
    }
}

#[repr(C)]
struct Storage {
    overlapped: OVERLAPPED,
    buffer: [u8; 512],
}

pub(crate) struct Operation {
    storage: Box<UnsafeCell<Storage>>,
    event: OwnedHandle,
    pub endpoint: Rc<Endpoint>,
    pub pending: bool,
    pub completions: usize,
    pub synchronous: usize,
    pub result: Option<(i32, u32)>,
}

impl Operation {
    pub fn new(endpoint: Rc<Endpoint>) -> io::Result<Self> {
        // SAFETY: manual-reset, initially nonsignaled unnamed event, unique ownership.
        let event = unsafe { owned(CreateEventW(ptr::null(), 1, 0, ptr::null())) }?;
        // SAFETY: all-zero OVERLAPPED and byte array are valid, no I/O yet.
        let storage = Box::new(UnsafeCell::new(unsafe { std::mem::zeroed() }));
        Ok(Self {
            storage,
            event,
            endpoint,
            pending: false,
            completions: 0,
            synchronous: 0,
            result: None,
        })
    }
    pub fn prepare(&mut self) -> io::Result<(*mut OVERLAPPED, *mut u8)> {
        assert!(!self.pending, "cannot reuse a live OVERLAPPED");
        self.result = None;
        // SAFETY: event live, previous completion has been dequeued.
        bool_result(unsafe { ResetEvent(self.event.as_raw_handle()) })?;
        // SAFETY: storage idle and exclusive; no reference to kernel-mutated memory
        // is kept across submission. Box address never moves.
        unsafe {
            (*self.storage.get()).overlapped = std::mem::zeroed();
            (*self.storage.get()).overlapped.hEvent = self.event.as_raw_handle();
            Ok((
                ptr::addr_of_mut!((*self.storage.get()).overlapped),
                ptr::addr_of_mut!((*self.storage.get()).buffer).cast(),
            ))
        }
    }
    pub fn data(&self) -> &[u8] {
        assert!(!self.pending);
        // SAFETY: kernel finished before pending was cleared; storage stays live.
        unsafe { &(*self.storage.get()).buffer }
    }
    pub fn set_data(&mut self, data: &[u8]) {
        assert!(!self.pending);
        // SAFETY: no pending I/O, unique access to initialized storage.
        unsafe { (&mut (*self.storage.get()).buffer)[..data.len()].copy_from_slice(data) };
    }
    pub fn identity(&self) -> usize {
        self.storage.get() as usize
    }
    /// Called immediately after a submit; `error` was captured before another API.
    pub fn submitted(
        &mut self,
        success: bool,
        error: i32,
        bytes: u32,
        skip: bool,
    ) -> io::Result<()> {
        if success && skip {
            self.synchronous += 1;
            self.completions += 1;
            self.result = Some((0, bytes));
        } else if success || error == ERROR_IO_PENDING as i32 {
            self.pending = true;
        } else {
            return Err(io::Error::from_raw_os_error(error));
        }
        Ok(())
    }
    pub fn complete(&mut self, entry: Entry) {
        assert!(self.pending, "duplicate completion");
        assert_eq!(entry.overlapped, self.identity());
        self.pending = false;
        self.completions += 1;
        self.result = Some((entry.status, entry.bytes));
    }
    pub fn cancel(&self) -> io::Result<()> {
        // SAFETY: endpoint/storage live. ERROR_NOT_FOUND means completion won;
        // neither it nor success authorizes freeing/reusing storage.
        // https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-cancelioex
        if unsafe { CancelIoEx(self.endpoint.raw(), self.storage.get().cast()) } == 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(ERROR_NOT_FOUND as i32) {
                return Err(e);
            }
        }
        Ok(())
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        if self.pending {
            let _ = self.cancel();
            let mut bytes = 0;
            let mut flags = 0;
            // SAFETY: keep endpoint, event, OVERLAPPED and bytes alive until actual
            // completion even when a test fails. This wait is teardown, not turn().
            // https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-wsagetoverlappedresult
            // https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-getoverlappedresult
            unsafe {
                match &*self.endpoint {
                    Endpoint::Socket(s) => {
                        WSAGetOverlappedResult(
                            s.as_raw_socket() as usize,
                            self.storage.get().cast(),
                            &mut bytes,
                            1,
                            &mut flags,
                        );
                    }
                    Endpoint::Pipe(p) => {
                        GetOverlappedResult(
                            p.as_raw_handle(),
                            self.storage.get().cast(),
                            &mut bytes,
                            1,
                        );
                    }
                }
            }
            // Port entries retain opaque addresses only; the port never dereferences
            // them. The fixture must stop dispatching after a failed probe.
        }
    }
}

pub(crate) fn wsa_error() -> i32 {
    // SAFETY: thread-local error query has no pointer requirements.
    unsafe { WSAGetLastError() }
}

pub(crate) fn drain(port: &Port, ops: &mut [&mut Operation]) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut entries = [Entry::default(); 16];
    while ops.iter().any(|op| op.pending) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "missing I/O completion",
            ));
        }
        if let Wait::Entries(n) = port.wait(Some(remaining), false, &mut entries)? {
            for entry in &entries[..n] {
                let op = ops
                    .iter_mut()
                    .find(|op| op.identity() == entry.overlapped)
                    .ok_or_else(|| io::Error::other("unexpected completion"))?;
                op.complete(*entry);
            }
        }
    }
    Ok(())
}
