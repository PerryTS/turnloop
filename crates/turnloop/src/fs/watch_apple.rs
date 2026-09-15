//! FSEvents supplies directory child-content changes and native recursive scope.
use super::{CHANGE, OVERFLOW, RENAME, State};
use crate::*;
use std::{
    ffi::{CStr, CString, c_char, c_void},
    os::unix::ffi::OsStrExt,
    sync::Arc,
};
type Ref = *const c_void;
#[repr(C)]
struct Context {
    version: isize,
    info: *mut c_void,
    retain: Option<unsafe extern "C" fn(Ref) -> Ref>,
    release: Option<unsafe extern "C" fn(Ref)>,
    description: Option<unsafe extern "C" fn(Ref) -> Ref>,
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(allocator: Ref, bytes: *const c_char, encoding: u32) -> Ref;
    fn CFArrayCreate(allocator: Ref, values: *const Ref, count: isize, callbacks: Ref) -> Ref;
    fn CFRelease(value: Ref);
}
#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn FSEventStreamCreate(
        allocator: Ref,
        callback: unsafe extern "C" fn(
            Ref,
            *mut c_void,
            usize,
            *mut c_void,
            *const u32,
            *const u64,
        ),
        context: *const Context,
        paths: Ref,
        since: u64,
        latency: f64,
        flags: u32,
    ) -> Ref;
    fn FSEventStreamSetDispatchQueue(stream: Ref, queue: Ref);
    fn FSEventStreamStart(stream: Ref) -> u8;
    fn FSEventStreamStop(stream: Ref);
    fn FSEventStreamInvalidate(stream: Ref);
    fn FSEventStreamRelease(stream: Ref);
}
unsafe extern "C" {
    fn dispatch_queue_create(label: *const c_char, attr: Ref) -> Ref;
    fn dispatch_sync_f(queue: Ref, context: *mut c_void, work: unsafe extern "C" fn(*mut c_void));
    fn dispatch_release(queue: Ref);
}
struct Callback {
    state: Arc<State>,
    root: CString,
    recursive: bool,
}
unsafe extern "C" fn receive(
    _: Ref,
    info: *mut c_void,
    count: usize,
    paths: *mut c_void,
    flags: *const u32,
    _: *const u64,
) {
    // SAFETY: FSEvents retains our boxed callback context until stop/invalidate
    // and the serial dispatch queue barrier; native arrays contain count entries.
    let (callback, paths, flags) = unsafe {
        (
            &*info.cast::<Callback>(),
            std::slice::from_raw_parts(paths.cast::<*const c_char>(), count),
            std::slice::from_raw_parts(flags, count),
        )
    };
    let root = callback.root.as_bytes();
    let mut result = 0;
    for (p, f) in paths.iter().zip(flags) {
        if f & 0x7 != 0 {
            result |= OVERFLOW;
        }
        // SAFETY: FSEvents default eventPaths representation is an array of C strings.
        let path = unsafe { CStr::from_ptr(*p) }.to_bytes();
        let Some(relative) = path.strip_prefix(root) else {
            continue;
        };
        if !relative.is_empty() && !relative.starts_with(b"/") {
            continue;
        }
        let relative = relative.strip_prefix(b"/").unwrap_or(relative);
        if !callback.recursive && relative.contains(&b'/') {
            continue;
        }
        if f & (0x20 | 0x100 | 0x200 | 0x800) != 0 {
            result |= RENAME;
        }
        if f & (0x400 | 0x1000 | 0x2000 | 0x4000 | 0x8000) != 0 {
            result |= CHANGE;
        }
    }
    callback.state.event(result);
}
unsafe extern "C" fn barrier(_: *mut c_void) {}
pub(super) struct Watch {
    stream: Ref,
    queue: Ref,
    paths: Ref,
    string: Ref,
    callback: Box<Callback>,
    stopped: bool,
}
impl Watch {
    pub fn new(path: &FsPath, recursive: bool, state: Arc<State>) -> Result<Self> {
        if path.preopen.is_some() {
            return Err(Error::new(ErrorKind::Unsupported));
        }
        let root = std::fs::canonicalize(path.as_path()).map_err(Error::from)?;
        let root = CString::new(root.as_os_str().as_bytes())
            .map_err(|_| Error::new(ErrorKind::InvalidInput))?;
        let callback = Box::new(Callback {
            state,
            root,
            recursive,
        });
        let context = Context {
            version: 0,
            info: (&*callback as *const Callback).cast_mut().cast(),
            retain: None,
            release: None,
            description: None,
        };
        let mut watch = Self {
            stream: std::ptr::null(),
            queue: std::ptr::null(),
            paths: std::ptr::null(),
            string: std::ptr::null(),
            callback,
            stopped: true,
        };
        // SAFETY: prepared UTF-8 C string; CF objects are checked and uniquely released.
        watch.string = unsafe {
            CFStringCreateWithCString(std::ptr::null(), watch.callback.root.as_ptr(), 0x08000100)
        };
        if watch.string.is_null() {
            return Err(Error::new(ErrorKind::InvalidInput));
        }
        // SAFETY: one live CFString, retained by Watch through the stream lifetime.
        watch.paths =
            unsafe { CFArrayCreate(std::ptr::null(), &watch.string, 1, std::ptr::null()) };
        // SAFETY: static label and default serial queue attributes.
        watch.queue = unsafe {
            dispatch_queue_create(c"turnloop-filesystem-watch".as_ptr(), std::ptr::null())
        };
        if watch.paths.is_null() || watch.queue.is_null() {
            return Err(Error::new(ErrorKind::ResourceLimit));
        }
        // SAFETY: valid callback/context/paths. FileEvents + WatchRoot; zero latency
        // requests native delivery without introducing any driver polling interval.
        watch.stream = unsafe {
            FSEventStreamCreate(
                std::ptr::null(),
                receive,
                &context,
                watch.paths,
                u64::MAX,
                0.0,
                0x10 | 0x4,
            )
        };
        if watch.stream.is_null() {
            return Err(Error::new(ErrorKind::Other));
        }
        // SAFETY: valid stream and serial queue, not yet started.
        unsafe { FSEventStreamSetDispatchQueue(watch.stream, watch.queue) };
        // SAFETY: initialized stream; callback context remains pinned in its Box.
        if unsafe { FSEventStreamStart(watch.stream) } == 0 {
            return Err(Error::new(ErrorKind::Other));
        }
        watch.stopped = false;
        Ok(watch)
    }
    pub fn cancel(&mut self) -> Result<()> {
        if !self.stopped {
            // SAFETY: owner never runs on the private callback queue. Stop delivery,
            // invalidate scheduling and wait for earlier callbacks before terminal publication.
            unsafe {
                FSEventStreamStop(self.stream);
                FSEventStreamInvalidate(self.stream);
                dispatch_sync_f(self.queue, std::ptr::null_mut(), barrier);
            }
            self.stopped = true;
            self.callback.state.stopped();
        }
        Ok(())
    }
}
impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.cancel();
        // SAFETY: callbacks quiesced above; release each uniquely owned native object once.
        unsafe {
            if !self.stream.is_null() {
                FSEventStreamRelease(self.stream);
            }
            if !self.queue.is_null() {
                dispatch_release(self.queue);
            }
            if !self.paths.is_null() {
                CFRelease(self.paths);
            }
            if !self.string.is_null() {
                CFRelease(self.string);
            }
        }
    }
}
