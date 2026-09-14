use crate::{
    operation::{Endpoint, Operation, drain},
    pipe,
    port::{Entry, Port, Wait, bool_result, owned},
};
use std::{
    ffi::c_void,
    io,
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, OwnedHandle},
    },
    path::Path,
    ptr,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        GENERIC_READ, GENERIC_WRITE, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
        SetHandleInformation, WAIT_OBJECT_0,
    },
    Storage::FileSystem::{PIPE_ACCESS_INBOUND, PIPE_ACCESS_OUTBOUND},
    System::{JobObjects::*, Threading::*},
};

struct Attributes(Vec<usize>);
impl Attributes {
    fn new(handles: &[HANDLE]) -> io::Result<Self> {
        let mut bytes = 0;
        // SAFETY: documented sizing call with null output list; expected to fail.
        // https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-initializeprocthreadattributelist
        unsafe {
            InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut bytes);
        }
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        // SAFETY: aligned allocation of requested length; initializes opaque list.
        bool_result(unsafe {
            InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), 1, 0, &mut bytes)
        })?;
        let mut result = Self(storage);
        // SAFETY: handles remain live until CreateProcess; list lifetime is scoped
        // inside their borrow. Explicit handle list prevents unrelated inheritance.
        // https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute
        bool_result(unsafe {
            UpdateProcThreadAttribute(
                result.0.as_mut_ptr().cast(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                std::mem::size_of_val(handles),
                ptr::null_mut(),
                ptr::null(),
            )
        })?;
        Ok(result)
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: list successfully initialized, allocation remains live during deletion.
        unsafe {
            DeleteProcThreadAttributeList(self.0.as_mut_ptr().cast());
        }
    }
}

struct Child {
    process: OwnedHandle,
    thread: OwnedHandle,
    job: OwnedHandle,
}
impl Drop for Child {
    fn drop(&mut self) {
        // SAFETY: guard owns live process/job. Also kills descendants; no callback memory
        // is released by this operation. Termination is asynchronous, so wait for parent.
        unsafe {
            TerminateJobObject(self.job.as_raw_handle(), 99);
            TerminateProcess(self.process.as_raw_handle(), 99); // also covers assignment failure
            WaitForSingleObject(self.process.as_raw_handle(), INFINITE);
        }
    }
}

fn spawn(executable: &Path, handles: &[HANDLE; 3], mode: &str) -> io::Result<Child> {
    let application: Vec<u16> = executable
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    if application[..application.len() - 1].contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "nul in executable",
        ));
    }
    let mut command: Vec<u16> = format!("\"turnloop-child\" {mode}\0")
        .encode_utf16()
        .collect();
    for handle in handles {
        // SAFETY: owned child pipe endpoint. Parent server ends remain noninheritable.
        bool_result(unsafe {
            SetHandleInformation(*handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT)
        })?;
    }
    let mut attributes = Attributes::new(handles)?;
    // SAFETY: zero-initialized Win32 POD outputs, with cb set before use.
    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = handles[0];
    startup.StartupInfo.hStdOutput = handles[1];
    startup.StartupInfo.hStdError = handles[2];
    startup.lpAttributeList = attributes.0.as_mut_ptr().cast();
    // SAFETY: unnamed noninheritable job, unique ownership.
    let job = unsafe { owned(CreateJobObjectW(ptr::null(), ptr::null())) }?;
    // SAFETY: valid zero-initialized information struct.
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: exact class/struct/size match; live owned job.
    bool_result(unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of_val(&limits) as u32,
        )
    })?;
    // SAFETY: all-zero PROCESS_INFORMATION is valid output storage.
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: application is explicit (no ambiguous search), command writable;
    // inherited handles/list remain live. Suspended creation closes the escape
    // race: assign the job before the child can run or create descendants.
    // https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessw
    bool_result(unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            CREATE_SUSPENDED | CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
            ptr::null(),
            ptr::null(),
            &startup.StartupInfo,
            &mut info,
        )
    })?;
    // SAFETY: successful CreateProcess returned two uniquely owned handles.
    let child = unsafe {
        Child {
            process: owned(info.hProcess)?,
            thread: owned(info.hThread)?,
            job,
        }
    };
    // SAFETY: child suspended; job configured and process handle grants assignment.
    // https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-assignprocesstojobobject
    bool_result(unsafe {
        AssignProcessToJobObject(child.job.as_raw_handle(), child.process.as_raw_handle())
    })?;
    Ok(child)
}

struct ExitContext {
    port: Arc<Port>,
    calls: AtomicU32,
    error: AtomicU32,
}
unsafe extern "system" fn exited(context: *mut c_void, timed_out: bool) {
    // SAFETY: pinned Box lives until UnregisterWaitEx(INVALID_HANDLE_VALUE) joins.
    let context = unsafe { &*context.cast::<ExitContext>() };
    context.calls.fetch_add(1, Ordering::Relaxed);
    if timed_out {
        context.error.store(1, Ordering::Release);
    }
    if let Err(error) = context.port.post(40, 0) {
        context
            .error
            .store(error.raw_os_error().unwrap_or(1) as u32, Ordering::Release);
    }
}
struct ExitWait {
    wait: HANDLE,
    context: Option<Box<ExitContext>>,
}
impl ExitWait {
    fn new(child: &Child, port: Arc<Port>) -> io::Result<Self> {
        let context = Box::new(ExitContext {
            port,
            calls: AtomicU32::new(0),
            error: AtomicU32::new(0),
        });
        let mut wait = ptr::null_mut();
        // SAFETY: child outlives registration; callback may run immediately, so
        // context is fully initialized first. One-shot avoids repeated exit callbacks.
        // https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-registerwaitforsingleobject
        bool_result(unsafe {
            RegisterWaitForSingleObject(
                &mut wait,
                child.process.as_raw_handle(),
                Some(exited),
                (&*context as *const ExitContext).cast(),
                INFINITE,
                WT_EXECUTEONLYONCE,
            )
        })?;
        Ok(Self {
            wait,
            context: Some(context),
        })
    }
    fn finish(&mut self) -> io::Result<u32> {
        if !self.wait.is_null() {
            // SAFETY: called outside callback on live wait handle. INVALID_HANDLE_VALUE
            // blocks until all callbacks finish; a wait registration is NOT CloseHandle-able.
            // https://learn.microsoft.com/en-us/windows/win32/api/threadpoollegacyapiset/nf-threadpoollegacyapiset-unregisterwaitex
            bool_result(unsafe { UnregisterWaitEx(self.wait, INVALID_HANDLE_VALUE) })?;
            self.wait = ptr::null_mut();
        }
        let context = self
            .context
            .as_ref()
            .ok_or_else(|| io::Error::other("missing exit context"))?;
        let error = context.error.load(Ordering::Acquire);
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        Ok(context.calls.load(Ordering::Acquire))
    }
}
impl Drop for ExitWait {
    fn drop(&mut self) {
        if self.finish().is_err() && !self.wait.is_null() {
            // Unexpected unregister failure: retain callback storage, never free live context.
            if let Some(context) = self.context.take() {
                let _ = Box::leak(context);
            }
        }
    }
}

pub fn probe(executable: &Path, register_after_exit: bool) -> io::Result<u32> {
    let port = Arc::new(Port::new()?);
    let mut endpoints = Vec::with_capacity(3);
    let mut child_ends = Vec::with_capacity(3);
    for index in 0..3 {
        let name = pipe::pipe_name();
        let endpoint = Rc::new(Endpoint::Pipe(pipe::server(
            &name,
            if index == 0 {
                PIPE_ACCESS_OUTBOUND
            } else {
                PIPE_ACCESS_INBOUND
            },
        )?));
        pipe::associate(&port, &endpoint, 41 + index)?;
        let mut connecting = Operation::new(Rc::clone(&endpoint))?;
        pipe::connect(&mut connecting)?;
        // Child ends must be synchronous for normal stdio consumers. Only the
        // parent's ends are overlapped; the two ends are separate file objects.
        child_ends.push(pipe::client(
            &name,
            if index == 0 {
                GENERIC_READ
            } else {
                GENERIC_WRITE
            },
            false,
        )?);
        drain(&port, &mut [&mut connecting])?;
        endpoints.push(endpoint);
    }
    let handles = [
        child_ends[0].as_raw_handle(),
        child_ends[1].as_raw_handle(),
        child_ends[2].as_raw_handle(),
    ];
    let child = spawn(executable, &handles, "")?;
    drop(child_ends); // ensure parent copies cannot keep stdout/stderr open
    let mut input = Operation::new(Rc::clone(&endpoints[0]))?;
    let mut output = Operation::new(Rc::clone(&endpoints[1]))?;
    let mut error = Operation::new(Rc::clone(&endpoints[2]))?;
    pipe::read(&mut output)?;
    pipe::read(&mut error)?;
    pipe::write(&mut input, b"turnloop")?;
    let mut wait = if register_after_exit {
        None
    } else {
        Some(ExitWait::new(&child, Arc::clone(&port))?)
    };
    // SAFETY: suspended primary thread, job already assigned, stdio reads armed.
    if unsafe { ResumeThread(child.thread.as_raw_handle()) } == u32::MAX {
        return Err(io::Error::last_os_error());
    }
    if register_after_exit {
        // SAFETY: process handle live; child writes only a bounded tiny payload.
        assert_eq!(
            // SAFETY: owned process handle remains live throughout the bounded wait.
            unsafe { WaitForSingleObject(child.process.as_raw_handle(), 3000) },
            WAIT_OBJECT_0
        );
        wait = Some(ExitWait::new(&child, Arc::clone(&port))?);
    }
    let mut exits = 0;
    let mut batch = [Entry::default(); 16];
    let deadline = Instant::now() + Duration::from_secs(3);
    while exits == 0 || input.pending || output.pending || error.pending {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "child completion missing",
            ));
        }
        if let Wait::Entries(n) = port.wait(
            Some(deadline.saturating_duration_since(Instant::now())),
            false,
            &mut batch,
        )? {
            for entry in &batch[..n] {
                if entry.key == 40 {
                    exits += 1;
                    continue;
                }
                let mut ops = [&mut input, &mut output, &mut error];
                let op = ops
                    .iter_mut()
                    .find(|op| op.identity() == entry.overlapped)
                    .ok_or_else(|| io::Error::other("unknown child I/O packet"))?;
                op.complete(*entry);
            }
        }
    }
    assert_eq!(exits, 1);
    assert_eq!(input.result, Some((0, 8)));
    assert_eq!(output.result, Some((0, 19)));
    assert_eq!(error.result, Some((0, 9)));
    assert_eq!(&output.data()[..19], b"child-out:turnloop\n");
    assert_eq!(&error.data()[..9], b"child-err");
    let mut exit_code = 0;
    // SAFETY: process exit notification received, valid output for exit code.
    bool_result(unsafe { GetExitCodeProcess(child.process.as_raw_handle(), &mut exit_code) })?;
    assert_eq!(exit_code, 23);
    assert_eq!(
        wait.as_mut()
            .ok_or_else(|| io::Error::other("missing wait"))?
            .finish()?,
        1
    );
    Ok(exit_code)
}

pub fn probe_kill_tree(executable: &Path) -> io::Result<u32> {
    let port = Arc::new(Port::new()?);
    let mut endpoints = Vec::with_capacity(3);
    let mut child_ends = Vec::with_capacity(3);
    for index in 0..3 {
        let name = pipe::pipe_name();
        let endpoint = Rc::new(Endpoint::Pipe(pipe::server(
            &name,
            if index == 0 {
                PIPE_ACCESS_OUTBOUND
            } else {
                PIPE_ACCESS_INBOUND
            },
        )?));
        pipe::associate(&port, &endpoint, 41 + index)?;
        let mut connecting = Operation::new(Rc::clone(&endpoint))?;
        pipe::connect(&mut connecting)?;
        child_ends.push(pipe::client(
            &name,
            if index == 0 {
                GENERIC_READ
            } else {
                GENERIC_WRITE
            },
            false,
        )?);
        drain(&port, &mut [&mut connecting])?;
        endpoints.push(endpoint);
    }
    let handles = [
        child_ends[0].as_raw_handle(),
        child_ends[1].as_raw_handle(),
        child_ends[2].as_raw_handle(),
    ];
    let child = spawn(executable, &handles, "--tree")?;
    drop(child_ends);
    let mut output = Operation::new(Rc::clone(&endpoints[1]))?;
    pipe::read(&mut output)?;
    let mut wait = ExitWait::new(&child, Arc::clone(&port))?;
    // SAFETY: child suspended, job assigned and read armed.
    if unsafe { ResumeThread(child.thread.as_raw_handle()) } == u32::MAX {
        return Err(io::Error::last_os_error());
    }
    drain(&port, &mut [&mut output])?;
    assert_eq!(output.result, Some((0, 4)));
    let pid = u32::from_le_bytes(
        output.data()[..4]
            .try_into()
            .map_err(|_| io::Error::other("invalid child pid"))?,
    );
    assert_ne!(pid, 0);
    // SAFETY: pid returned by our child, access only for query/wait; handle uniquely owned.
    // https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-openprocess
    let grandchild = unsafe {
        owned(OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        ))
    }?;
    // SAFETY: both handles live, zero timeout verifies both processes are still alive.
    unsafe {
        assert_eq!(
            WaitForSingleObject(child.process.as_raw_handle(), 0),
            windows_sys::Win32::Foundation::WAIT_TIMEOUT
        );
        assert_eq!(
            WaitForSingleObject(grandchild.as_raw_handle(), 0),
            windows_sys::Win32::Foundation::WAIT_TIMEOUT
        );
        bool_result(TerminateJobObject(child.job.as_raw_handle(), 77))?;
        assert_eq!(
            WaitForSingleObject(child.process.as_raw_handle(), 3000),
            WAIT_OBJECT_0
        );
        assert_eq!(
            WaitForSingleObject(grandchild.as_raw_handle(), 3000),
            WAIT_OBJECT_0
        );
    }
    let mut batch = [Entry::default(); 4];
    assert_eq!(
        port.wait(Some(Duration::from_secs(3)), false, &mut batch)?,
        Wait::Entries(1)
    );
    assert_eq!(batch[0].key, 40);
    assert_eq!(wait.finish()?, 1);
    Ok(pid)
}
