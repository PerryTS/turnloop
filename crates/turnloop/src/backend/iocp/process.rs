use super::{Detached, Kind, Native, bool_result, invalid, os_error, port::owned, unsupported};
use crate::{ExitStatus, Notifier, ProcessSpec, ProcessStdio, Result, Signal};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString, c_void},
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, OwnedHandle},
    },
    path::PathBuf,
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use windows_sys::Win32::{
    Foundation::*,
    Storage::FileSystem::*,
    System::{Console::*, JobObjects::*, Pipes::*, Threading::*},
};

fn wide(value: &OsStr) -> Result<Vec<u16>> {
    let mut value: Vec<u16> = value.encode_wide().collect();
    if value.contains(&0) {
        return Err(invalid());
    }
    value.push(0);
    Ok(value)
}
fn quote(value: &OsStr, out: &mut Vec<u16>) -> Result<()> {
    out.push(b'"' as u16);
    let mut slashes = 0;
    for unit in value.encode_wide() {
        if unit == 0 {
            return Err(invalid());
        }
        if unit == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        if unit == b'"' as u16 {
            out.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2 + 1));
        } else {
            out.extend(std::iter::repeat_n(b'\\' as u16, slashes));
        }
        slashes = 0;
        out.push(unit);
    }
    out.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    out.push(b'"' as u16);
    Ok(())
}
fn program(spec: &ProcessSpec) -> Result<PathBuf> {
    let path = PathBuf::from(&spec.program);
    if path.is_absolute() || path.components().count() > 1 {
        return Ok(path);
    }
    let paths = spec
        .env
        .iter()
        .rev()
        .find(|(key, _)| key.eq_ignore_ascii_case("PATH"))
        .map(|(_, value)| value.clone())
        .or_else(|| {
            if spec.env_clear {
                None
            } else {
                std::env::var_os("PATH")
            }
        });
    if let Some(paths) = paths {
        for directory in std::env::split_paths(&paths) {
            let candidate = directory.join(&path);
            if candidate.is_file() {
                return Ok(candidate);
            }
            if candidate.extension().is_none() {
                let executable = candidate.with_extension("exe");
                if executable.is_file() {
                    return Ok(executable);
                }
            }
        }
    }
    Ok(path)
}
pub(super) fn duplicate(handle: HANDLE, inherit: bool) -> Result<OwnedHandle> {
    let mut out = ptr::null_mut();
    // SAFETY: source belongs to this process; returned handle has independent ownership.
    bool_result(unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            handle,
            GetCurrentProcess(),
            &mut out,
            0,
            i32::from(inherit),
            DUPLICATE_SAME_ACCESS,
        )
    })?;
    // SAFETY: successful DuplicateHandle transferred unique ownership.
    unsafe { owned(out) }.map_err(Into::into)
}
pub(super) fn null(input: bool) -> Result<OwnedHandle> {
    let name: Vec<u16> = "NUL\0".encode_utf16().collect();
    // SAFETY: valid device path; exclusive ownership of newly created handle.
    unsafe {
        owned(CreateFileW(
            name.as_ptr(),
            if input { GENERIC_READ } else { GENERIC_WRITE },
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        ))
    }
    .map_err(Into::into)
}
pub(super) fn stdio(index: usize, inherit: bool) -> Result<OwnedHandle> {
    // SAFETY: query borrows the host's original standard handle, never closes it.
    let handle =
        unsafe { GetStdHandle([STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE][index]) };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        let handle = null(index == 0)?;
        return duplicate(handle.as_raw_handle(), inherit);
    }
    duplicate(handle, inherit)
}
fn pipe(index: usize) -> Result<(Detached, OwnedHandle)> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = format!(
        r"\\.\pipe\turnloop-stdio-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let name = wide(OsStr::new(&name))?;
    // SAFETY: unique local-only pipe, parent uses overlapped I/O.
    let parent = unsafe {
        owned(CreateNamedPipeW(
            name.as_ptr(),
            FILE_FLAG_OVERLAPPED
                | FILE_FLAG_FIRST_PIPE_INSTANCE
                | if index == 0 {
                    PIPE_ACCESS_OUTBOUND
                } else {
                    PIPE_ACCESS_INBOUND
                },
            PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            65536,
            65536,
            0,
            ptr::null(),
        ))
    }?;
    // SAFETY: child end is synchronous so ordinary child stdio APIs work.
    let child = unsafe {
        owned(CreateFileW(
            name.as_ptr(),
            if index == 0 {
                GENERIC_READ
            } else {
                GENERIC_WRITE
            },
            0,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        ))
    }?;
    // SAFETY: initialized inactive operation; the successful client open has already
    // connected this instance, so ERROR_PIPE_CONNECTED has no outstanding I/O.
    let mut overlapped = unsafe { std::mem::zeroed() };
    // SAFETY: live parent and stack output retained for this already-connected call.
    let ok = unsafe { ConnectNamedPipe(parent.as_raw_handle(), &mut overlapped) };
    if ok == 0 && os_error().os != Some(ERROR_PIPE_CONNECTED as i32) {
        return Err(os_error());
    }
    Ok((
        Detached::new(Native::Handle(parent), Kind::Pipe, false),
        child,
    ))
}
struct Attributes(Vec<usize>);
impl Attributes {
    fn new(handles: &[HANDLE]) -> Result<Self> {
        let mut bytes = 0;
        // SAFETY: documented sizing query, no output list yet.
        unsafe {
            InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut bytes);
        }
        if bytes == 0 {
            return Err(os_error());
        }
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        // SAFETY: correctly sized and aligned opaque list storage.
        bool_result(unsafe {
            InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), 1, 0, &mut bytes)
        })?;
        let mut list = Self(storage);
        // SAFETY: inherited handles remain live and slice remains fixed through spawn.
        bool_result(unsafe {
            UpdateProcThreadAttribute(
                list.0.as_mut_ptr().cast(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                std::mem::size_of_val(handles),
                ptr::null_mut(),
                ptr::null(),
            )
        })?;
        Ok(list)
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: successful initialization, uniquely owned allocation still live.
        unsafe {
            DeleteProcThreadAttributeList(self.0.as_mut_ptr().cast());
        }
    }
}
struct Context {
    ready: AtomicBool,
    notifier: Notifier,
}
unsafe extern "system" fn exited(context: *mut c_void, _: bool) {
    // SAFETY: Arc allocation remains owned until the registered wait is joined.
    let context = unsafe { &*context.cast::<Context>() };
    context.ready.store(true, Ordering::Release);
    let _ = context.notifier.notify(); // NotFound is permitted during loop destruction.
}
pub(super) struct Child {
    process: OwnedHandle,
    thread: OwnedHandle,
    job: Option<OwnedHandle>,
    context: Arc<Context>,
    wait: HANDLE,
    status: Option<ExitStatus>,
    pub pid: u32,
}
impl Child {
    pub(super) fn ready(&self) -> bool {
        self.context.ready.load(Ordering::Acquire) || self.status.is_some()
    }
    pub(super) fn status(&mut self) -> Result<Option<ExitStatus>> {
        if !self.ready() {
            return Ok(None);
        }
        if let Some(status) = self.status {
            return Ok(Some(status));
        }
        let mut code = 0;
        // SAFETY: registered wait reported actual process termination.
        bool_result(unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) })?;
        self.join()?;
        let status = ExitStatus {
            code: Some(code as i32),
            signal: None,
        };
        self.status = Some(status);
        Ok(Some(status))
    }
    fn join(&mut self) -> Result<()> {
        if !self.wait.is_null() {
            // SAFETY: called outside callback, joining before freeing callback state.
            bool_result(unsafe { UnregisterWaitEx(self.wait, INVALID_HANDLE_VALUE) })?;
            self.wait = ptr::null_mut();
        }
        Ok(())
    }
    pub(super) fn resume(&self) -> Result<()> {
        // SAFETY: owned suspended main thread; all resources/job/wait already installed.
        if unsafe { ResumeThread(self.thread.as_raw_handle()) } == u32::MAX {
            Err(os_error())
        } else {
            Ok(())
        }
    }
    pub(super) fn kill(&mut self, signal: Signal, group: bool) -> Result<()> {
        if self.status.is_some() {
            return Err(crate::Error::new(crate::ErrorKind::NotFound));
        }
        if signal != Signal::Kill {
            return Err(unsupported());
        }
        if group {
            let job = self.job.as_ref().ok_or_else(invalid)?;
            // SAFETY: exclusively owned process tree; no PID/handle reuse is possible.
            bool_result(unsafe { TerminateJobObject(job.as_raw_handle(), 1) })
        } else {
            // SAFETY: process handle pins the identity even if its PID is later reused.
            bool_result(unsafe { TerminateProcess(self.process.as_raw_handle(), 1) })
        }
    }
    pub(super) fn close(&mut self) -> Result<()> {
        if self.status.is_some() {
            return Ok(());
        }
        if self.ready() {
            self.status()?;
            return Ok(());
        }
        self.kill(Signal::Kill, self.job.is_some())
    }
}
impl Drop for Child {
    fn drop(&mut self) {
        if self.status.is_none() {
            // SAFETY: teardown owns the process and optional job; termination then
            // blocking wait prevents a live child or outstanding callback escaping.
            unsafe {
                if let Some(job) = &self.job {
                    TerminateJobObject(job.as_raw_handle(), 1);
                }
                TerminateProcess(self.process.as_raw_handle(), 1);
                WaitForSingleObject(self.process.as_raw_handle(), INFINITE);
            }
        }
        if self.join().is_err() {
            std::process::abort();
        }
    }
}
pub(super) fn spawn(
    spec: &ProcessSpec,
    existing: [Option<HANDLE>; 3],
    notifier: Notifier,
) -> Result<(Child, [Option<Detached>; 3])> {
    if spec.uid.is_some() || spec.gid.is_some() || spec.program.is_empty() {
        return Err(unsupported());
    }
    let mut parents = [None, None, None];
    let mut child_ends = Vec::with_capacity(3);
    for (i, option) in spec.stdio.iter().enumerate() {
        let handle = match option {
            ProcessStdio::Inherit => stdio(i, true)?,
            ProcessStdio::Null => {
                let handle = null(i == 0)?;
                duplicate(handle.as_raw_handle(), true)?
            }
            ProcessStdio::Handle(_) => duplicate(existing[i].ok_or_else(invalid)?, true)?,
            ProcessStdio::Pipe => {
                let (parent, child) = pipe(i)?;
                parents[i] = Some(parent);
                duplicate(child.as_raw_handle(), true)?
            }
        };
        child_ends.push(handle);
    }
    let handles = std::array::from_fn::<_, 3, _>(|i| child_ends[i].as_raw_handle());
    let mut attributes = Attributes::new(&handles)?;
    let application = wide(program(spec)?.as_os_str())?;
    let mut command = Vec::new();
    quote(&spec.program, &mut command)?;
    for arg in &spec.args {
        command.push(b' ' as u16);
        quote(arg, &mut command)?;
    }
    command.push(0);
    if command.len() > 32767 {
        return Err(invalid());
    }
    let mut vars: BTreeMap<OsString, (OsString, OsString)> = BTreeMap::new();
    if !spec.env_clear {
        for (key, value) in std::env::vars_os() {
            vars.insert(key.to_ascii_uppercase(), (key, value));
        }
    }
    for (key, value) in &spec.env {
        if key.is_empty()
            || key
                .encode_wide()
                .any(|unit| unit == 0 || unit == b'=' as u16)
        {
            return Err(invalid());
        }
        vars.insert(key.to_ascii_uppercase(), (key.clone(), value.clone()));
    }
    let mut environment = Vec::new();
    for (_, (key, value)) in vars {
        let mut entry = key;
        entry.push("=");
        entry.push(value);
        environment.extend(wide(&entry)?);
    }
    if environment.is_empty() {
        environment.push(0);
    }
    environment.push(0);
    let cwd = spec
        .cwd
        .as_ref()
        .map(|path| wide(path.as_os_str()))
        .transpose()?;
    // SAFETY: initialized C startup and result structures with correct cb.
    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = handles[0];
    startup.StartupInfo.hStdOutput = handles[1];
    startup.StartupInfo.hStdError = handles[2];
    startup.lpAttributeList = attributes.0.as_mut_ptr().cast();
    let job = if spec.new_process_group {
        // SAFETY: newly created unnamed job, unique ownership.
        let job = unsafe { owned(CreateJobObjectW(ptr::null(), ptr::null())) }?;
        // SAFETY: initialized C structure for exactly the requested information class.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: valid job, information pointer and exact byte size.
        bool_result(unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                ptr::from_ref(&limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        })?;
        Some(job)
    } else {
        None
    };
    // SAFETY: plain writable process output structure.
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: explicit application and quoted writable command line; environment,
    // directory, attribute list and exactly the listed inherited handles stay live.
    bool_result(unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            CREATE_SUSPENDED
                | CREATE_UNICODE_ENVIRONMENT
                | EXTENDED_STARTUPINFO_PRESENT
                | CREATE_NO_WINDOW,
            environment.as_ptr().cast(),
            cwd.as_ref().map_or(ptr::null(), |v| v.as_ptr()),
            &startup.StartupInfo,
            &mut info,
        )
    })?;
    // SAFETY: successful creation transfers these two independent handle owners.
    let mut child = unsafe {
        Child {
            process: owned(info.hProcess)?,
            thread: owned(info.hThread)?,
            job,
            context: Arc::new(Context {
                ready: AtomicBool::new(false),
                notifier,
            }),
            wait: ptr::null_mut(),
            status: None,
            pid: info.dwProcessId,
        }
    };
    if let Some(job) = &child.job {
        // SAFETY: child still suspended, so no descendant can escape job assignment.
        bool_result(unsafe {
            AssignProcessToJobObject(job.as_raw_handle(), child.process.as_raw_handle())
        })?;
    }
    // SAFETY: initialized callback context and owned child precede registration;
    // immediate exits are safe and Drop joins the one-shot callback.
    bool_result(unsafe {
        RegisterWaitForSingleObject(
            &mut child.wait,
            child.process.as_raw_handle(),
            Some(exited),
            Arc::as_ptr(&child.context).cast(),
            INFINITE,
            WT_EXECUTEONLYONCE,
        )
    })?;
    Ok((child, parents))
}
