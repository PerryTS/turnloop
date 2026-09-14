#[cfg(target_os = "linux")]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::{io, time::Instant};
pub enum Counter {
    #[cfg(target_os = "macos")]
    Mac,
    #[cfg(target_os = "linux")]
    Perf(OwnedFd),
    Portable(Instant),
}
impl Counter {
    pub fn new() -> Self {
        #[cfg(target_os = "macos")]
        {
            let c = Self::Mac;
            if c.read().is_ok_and(|n| n > 0) {
                return c;
            }
            eprintln!("ri_instructions unavailable; reporting elapsed nanoseconds instead");
        }
        #[cfg(target_os = "linux")]
        {
            match perf() {
                Ok(fd) => return Self::Perf(fd),
                Err(e) => eprintln!(
                    "perf_event_open unavailable ({e}); reporting elapsed nanoseconds instead"
                ),
            }
        }
        Self::Portable(Instant::now())
    }
    pub fn unit(&self) -> &'static str {
        match self {
            Self::Portable(_) => "nanoseconds",
            #[cfg(target_os = "macos")]
            Self::Mac => "instructions",
            #[cfg(target_os = "linux")]
            Self::Perf(_) => "instructions:u",
        }
    }
    pub fn read(&self) -> io::Result<u64> {
        match self {
            Self::Portable(start) => Ok(start.elapsed().as_nanos().min(u64::MAX as u128) as u64),
            #[cfg(target_os = "macos")]
            Self::Mac => {
                // SAFETY: rusage_info_v4 is integer/byte storage with a valid zero representation.
                let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
                // SAFETY: the Darwin API's typedef is misleading: buffer points to
                // the rusage struct itself, not a separately allocated pointer. V4
                // matches the size/alignment of the initialized output object.
                let n = unsafe {
                    libc::proc_pid_rusage(
                        libc::getpid(),
                        libc::RUSAGE_INFO_V4,
                        (&mut info as *mut libc::rusage_info_v4).cast(),
                    )
                };
                if n != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(info.ri_instructions)
            }
            #[cfg(target_os = "linux")]
            Self::Perf(fd) => {
                let mut count = 0u64;
                // SAFETY: a live perf descriptor writes one u64 with read_format=0.
                let n = unsafe { libc::read(fd.as_raw_fd(), (&mut count as *mut u64).cast(), 8) };
                if n != 8 {
                    return Err(io::Error::last_os_error());
                }
                Ok(count)
            }
        }
    }
}
#[cfg(target_os = "linux")]
fn perf() -> io::Result<OwnedFd> {
    // PERF_ATTR_SIZE_VER0, from linux/perf_event.h. Only these initial ABI fields
    // are used; no version-dependent tail, samples, or mmap ring is needed.
    #[repr(C)]
    struct Attr {
        kind: u32,
        size: u32,
        config: u64,
        sample_period: u64,
        sample_type: u64,
        read_format: u64,
        flags: u64,
        wakeup_events: u32,
        bp_type: u32,
        config1: u64,
    }
    let attr = Attr {
        kind: 0,
        size: std::mem::size_of::<Attr>() as u32,
        config: 1,
        sample_period: 0,
        sample_type: 0,
        read_format: 0,
        flags: (1 << 5) | (1 << 6),
        wakeup_events: 0,
        bp_type: 0,
        config1: 0,
    };
    assert_eq!(attr.size, 64);
    // SAFETY: valid version-0 perf_event_attr; pid=0 selects this thread, cpu=-1
    // follows it, group=-1 creates a group, flag 8 requests close-on-exec.
    let fd = unsafe { libc::syscall(libc::SYS_perf_event_open, &attr, 0i32, -1i32, -1i32, 8usize) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: perf_event_open returned a newly owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}
