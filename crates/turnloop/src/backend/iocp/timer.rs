use super::port::{Port, TIMER, bool_result, owned};
use std::{
    ffi::{CStr, c_void},
    io,
    os::windows::io::{AsRawHandle, OwnedHandle},
    ptr,
    sync::Arc,
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{HANDLE, HMODULE},
    System::{
        LibraryLoader::{GetModuleHandleW, GetProcAddress},
        Threading::{
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CancelWaitableTimer, CreateWaitableTimerExW,
            SetWaitableTimer, TIMER_ALL_ACCESS,
        },
    },
};

pub fn relative_due(delay: Duration) -> i64 {
    -(delay.as_nanos().div_ceil(100).clamp(1, i64::MAX as u128) as i64)
}

fn high_resolution() -> io::Result<OwnedHandle> {
    // SAFETY: unnamed, noninheritable timer; ownership transferred once.
    // Windows 10 1803+: do not silently downgrade precision if unsupported.
    // https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-createwaitabletimerexw
    unsafe {
        owned(CreateWaitableTimerExW(
            ptr::null(),
            ptr::null(),
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
            TIMER_ALL_ACCESS,
        ))
    }
}

type Create = unsafe extern "system" fn(*mut HANDLE, u32, *const c_void) -> i32;
type Associate = unsafe extern "system" fn(
    HANDLE,
    HANDLE,
    HANDLE,
    *const c_void,
    *const c_void,
    i32,
    usize,
    *mut u8,
) -> i32;
type Cancel = unsafe extern "system" fn(HANDLE, u8) -> i32;

fn symbol(module: HMODULE, name: &CStr) -> io::Result<unsafe extern "system" fn() -> isize> {
    // SAFETY: ntdll is process-resident; name is nul-terminated. No FreeLibrary
    // on a borrowed GetModuleHandle result.
    // https://learn.microsoft.com/en-us/windows/win32/api/libloaderapi/nf-libloaderapi-getprocaddress
    unsafe { GetProcAddress(module, name.as_ptr().cast()) }.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            format!("{} is unavailable", name.to_string_lossy()),
        )
    })
}

fn nt_result(status: i32) -> io::Result<()> {
    if status < 0 {
        Err(io::Error::other(format!("NTSTATUS {status:#010x}")))
    } else {
        Ok(())
    }
}

pub struct PacketTimer {
    packet: OwnedHandle,
    timer: OwnedHandle,
    port: Arc<Port>,
    associate: Associate,
    cancel: Cancel,
    active: bool,
    generation: usize,
}

impl PacketTimer {
    pub fn new(port: Arc<Port>) -> io::Result<Self> {
        let name: Vec<u16> = "ntdll.dll\0".encode_utf16().collect();
        // SAFETY: valid nul-terminated name; ntdll cannot unload during the process.
        // https://learn.microsoft.com/en-us/windows/win32/api/libloaderapi/nf-libloaderapi-getmodulehandlew
        let module = unsafe { GetModuleHandleW(name.as_ptr()) };
        if module.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: each named export has exactly the documented system ABI below.
        // No SDK headers/import library; feature-detect all exports at runtime.
        // https://learn.microsoft.com/en-us/windows/win32/devnotes/ntcreatewaitcompletionpacket
        let create = unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, Create>(symbol(
                module,
                c"NtCreateWaitCompletionPacket",
            )?)
        };
        // SAFETY: exact ABI from the Microsoft devnote.
        // https://learn.microsoft.com/en-us/windows/win32/devnotes/ntassociatewaitcompletionpacket
        let associate = unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, Associate>(symbol(
                module,
                c"NtAssociateWaitCompletionPacket",
            )?)
        };
        // SAFETY: exact ABI from the Microsoft devnote (BOOLEAN is u8).
        // https://learn.microsoft.com/en-us/windows/win32/devnotes/ntcancelwaitcompletionpacket
        let cancel = unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, Cancel>(symbol(
                module,
                c"NtCancelWaitCompletionPacket",
            )?)
        };
        let mut handle = ptr::null_mut();
        // SAFETY: valid output pointer; MAXIMUM_ALLOWED avoids private access-mask constants.
        nt_result(unsafe { create(&mut handle, 0x02000000, ptr::null()) })?;
        // SAFETY: successful NtCreate returned unique ownership.
        let packet = unsafe { owned(handle) }?;
        Ok(Self {
            packet,
            timer: high_resolution()?,
            port,
            associate,
            cancel,
            active: false,
            generation: 0,
        })
    }

    pub fn arm(&mut self, delay: Duration) -> io::Result<()> {
        if self.active {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "dequeue previous timer packet before reuse",
            ));
        }
        let due = relative_due(delay);
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| io::Error::other("timer generation exhausted"))?;
        // SAFETY: live timer; no callback. Arm first to reset any previous signal.
        bool_result(unsafe {
            SetWaitableTimer(self.timer.as_raw_handle(), &due, 0, None, ptr::null(), 0)
        })?;
        let mut already = 0;
        // SAFETY: inactive packet and three live owned handles; contexts are opaque
        // integers and null. AlreadySignaled STILL queues a packet; do not synthesize one.
        // https://learn.microsoft.com/en-us/windows/win32/devnotes/ntassociatewaitcompletionpacket
        nt_result(unsafe {
            (self.associate)(
                self.packet.as_raw_handle(),
                self.port.raw(),
                self.timer.as_raw_handle(),
                TIMER as *const c_void,
                self.generation as *const c_void,
                0,
                0,
                &mut already,
            )
        })?;
        self.active = true;
        Ok(())
    }

    /// # Safety
    /// Call only after dequeuing this timer's TIMER packet on its port.
    pub unsafe fn dequeued(&mut self, generation: usize) {
        if generation == self.generation {
            self.active = false;
        }
    }

    /// Cancel without promising immediate packet reuse. STATUS_PENDING requires
    /// draining; callers should drop this timer or wait for the original packet.
    pub fn cancel(&mut self) -> io::Result<i32> {
        // SAFETY: live timer, no APC.
        bool_result(unsafe { CancelWaitableTimer(self.timer.as_raw_handle()) })?;
        // SAFETY: live packet; remove a signaled packet if it is still queued.
        let status = unsafe { (self.cancel)(self.packet.as_raw_handle(), 1) };
        if status == 0 || status == 0xc0000120u32 as i32 {
            self.active = false;
        }
        if status == 0xc0000120u32 as i32 {
            return Ok(status);
        }
        nt_result(status)?;
        Ok(status)
    }
}
impl Drop for PacketTimer {
    fn drop(&mut self) {
        let _ = self.cancel();
    }
}
