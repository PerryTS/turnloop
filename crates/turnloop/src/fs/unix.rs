use super::{FileState, file, output, record};
use crate::*;
use std::{
    ffi::CStr,
    fs::File,
    os::fd::{AsRawFd, FromRawFd},
};
fn error() -> Error {
    std::io::Error::last_os_error().into()
}
fn status(n: i32) -> Result<()> {
    if n < 0 { Err(error()) } else { Ok(()) }
}
fn path(p: &FsPath) -> Result<*const libc::c_char> {
    if p.preopen.is_some() {
        return Err(Error::new(ErrorKind::Unsupported));
    }
    Ok(p.native.as_ptr())
}
pub(super) fn open(p: &FsPath, o: FileOptions) -> Result<File> {
    if (!o.read && !o.write) || ((o.create || o.exclusive || o.truncate || o.append) && !o.write) {
        return Err(Error::new(ErrorKind::InvalidInput));
    }
    let mut flags = if o.read && o.write {
        libc::O_RDWR
    } else if o.write {
        libc::O_WRONLY
    } else {
        libc::O_RDONLY
    };
    flags |= libc::O_CLOEXEC | libc::O_NONBLOCK;
    if o.create || o.exclusive {
        flags |= libc::O_CREAT;
    }
    if o.exclusive {
        flags |= libc::O_EXCL;
    }
    if o.truncate {
        flags |= libc::O_TRUNC;
    }
    if o.append {
        flags |= libc::O_APPEND;
    }
    if !o.follow_symlinks {
        flags |= libc::O_NOFOLLOW;
    }
    // SAFETY: prepared NUL-terminated path and scalar creation flags/mode.
    let fd = unsafe { libc::open(path(p)?, flags, o.mode as libc::c_uint) };
    if fd < 0 {
        return Err(error());
    }
    // SAFETY: open returned a fresh owned descriptor.
    let file = unsafe { File::from_raw_fd(fd) };
    let meta = file.metadata().map_err(Error::from)?;
    if !meta.is_file() {
        return Err(Error::new(if meta.is_dir() {
            ErrorKind::IsADirectory
        } else {
            ErrorKind::Unsupported
        }));
    }
    Ok(file)
}
fn offset(value: u64) -> Result<libc::off_t> {
    value
        .try_into()
        .map_err(|_| Error::new(ErrorKind::InvalidInput))
}
pub(super) fn execute(request: FsRequest, state: Option<&mut FileState>) -> Result<FsResult> {
    match request {
        FsRequest::Read {
            buffer, offset: at, ..
        } => {
            let fd = file(state)?.file.as_ref().expect("open file").as_raw_fd();
            let at = at.map(offset).transpose()?;
            loop {
                // SAFETY: this worker exclusively retains the output region and live fd.
                let n = unsafe {
                    match at {
                        Some(at) => libc::pread(fd, buffer.as_mut_ptr().cast(), buffer.len(), at),
                        None => libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()),
                    }
                };
                if n >= 0 {
                    return Ok(FsResult::Read(n as usize));
                }
                let e = error();
                if e.os != Some(libc::EINTR) {
                    return Err(e);
                }
            }
        }
        FsRequest::Write {
            buffer, offset: at, ..
        } => {
            let state = file(state)?;
            if state.append && at.is_some() {
                return Err(Error::new(ErrorKind::InvalidInput));
            }
            let fd = state.file.as_ref().expect("open file").as_raw_fd();
            let bytes = buffer.as_slice();
            let at = at.map(offset).transpose()?;
            loop {
                // SAFETY: stable initialized input remains owned until the syscall ends.
                let n = unsafe {
                    match at {
                        Some(at) => libc::pwrite(fd, bytes.as_ptr().cast(), bytes.len(), at),
                        None => libc::write(fd, bytes.as_ptr().cast(), bytes.len()),
                    }
                };
                if n >= 0 {
                    return Ok(FsResult::Wrote(n as usize));
                }
                let e = error();
                if e.os != Some(libc::EINTR) {
                    return Err(e);
                }
            }
        }
        FsRequest::Stat {
            path: p,
            follow_symlinks,
        } => {
            let mut stat = std::mem::MaybeUninit::uninit();
            // SAFETY: valid path and writable native stat output. Read only after success.
            status(unsafe {
                if follow_symlinks {
                    libc::stat(path(&p)?, stat.as_mut_ptr())
                } else {
                    libc::lstat(path(&p)?, stat.as_mut_ptr())
                }
            })?;
            // SAFETY: successful stat initialized the structure.
            Ok(FsResult::Metadata(metadata(unsafe { stat.assume_init() })))
        }
        FsRequest::Fstat(_) => {
            let fd = file(state)?.file.as_ref().expect("open file").as_raw_fd();
            let mut stat = std::mem::MaybeUninit::uninit();
            // SAFETY: live descriptor and native output initialized on success.
            status(unsafe { libc::fstat(fd, stat.as_mut_ptr()) })?;
            // SAFETY: successful fstat initialized the structure.
            Ok(FsResult::Metadata(metadata(unsafe { stat.assume_init() })))
        }
        FsRequest::Sync { data_only, .. } => {
            let fd = file(state)?.file.as_ref().expect("open file").as_raw_fd();
            // SAFETY: owned open file descriptor; no pointers escape.
            #[cfg(target_vendor = "apple")]
            let result = unsafe { libc::fsync(fd) };
            #[cfg(target_vendor = "apple")]
            let _ = data_only;
            #[cfg(not(target_vendor = "apple"))]
            // SAFETY: live owned descriptor, no pointers retained.
            let result = unsafe {
                if data_only {
                    libc::fdatasync(fd)
                } else {
                    libc::fsync(fd)
                }
            };
            status(result)?;
            Ok(FsResult::Done)
        }
        FsRequest::Truncate { size, .. } => {
            let fd = file(state)?.file.as_ref().expect("open file").as_raw_fd();
            // SAFETY: owned file and representable byte length.
            status(unsafe { libc::ftruncate(fd, offset(size)?) })?;
            Ok(FsResult::Done)
        }
        FsRequest::Mkdir { path: p, mode } => {
            // SAFETY: prepared pathname and native mode.
            status(unsafe { libc::mkdir(path(&p)?, mode as libc::mode_t) })?;
            Ok(FsResult::Done)
        }
        FsRequest::Rename { from, to } => {
            // SAFETY: both prepared paths remain alive for this synchronous call.
            status(unsafe { libc::rename(path(&from)?, path(&to)?) })?;
            Ok(FsResult::Done)
        }
        FsRequest::Unlink(p) => {
            // SAFETY: prepared live pathname.
            status(unsafe { libc::unlink(path(&p)?) })?;
            Ok(FsResult::Done)
        }
        FsRequest::Rmdir(p) => {
            // SAFETY: prepared live pathname.
            status(unsafe { libc::rmdir(path(&p)?) })?;
            Ok(FsResult::Done)
        }
        FsRequest::ReadDir {
            path: p,
            mut buffer,
            cookie,
        } => directory(&p, output(&mut buffer), cookie),
    }
}
fn metadata(s: libc::stat) -> FileMetadata {
    let kind = match s.st_mode & libc::S_IFMT {
        libc::S_IFREG => FileType::File,
        libc::S_IFDIR => FileType::Directory,
        libc::S_IFLNK => FileType::Symlink,
        _ => FileType::Other,
    };
    FileMetadata {
        kind,
        size: s.st_size as u64,
        inode: Some(s.st_ino as _),
        device: Some(s.st_dev as _),
        mode: Some(s.st_mode as _),
        uid: Some(s.st_uid),
        gid: Some(s.st_gid),
        links: Some(s.st_nlink as _),
        accessed: Some(FileTime {
            seconds: s.st_atime as _,
            nanoseconds: s.st_atime_nsec as _,
        }),
        modified: Some(FileTime {
            seconds: s.st_mtime as _,
            nanoseconds: s.st_mtime_nsec as _,
        }),
        changed: Some(FileTime {
            seconds: s.st_ctime as _,
            nanoseconds: s.st_ctime_nsec as _,
        }),
        #[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
        created: Some(FileTime {
            seconds: s.st_birthtime as _,
            nanoseconds: s.st_birthtime_nsec as _,
        }),
        #[cfg(not(any(target_vendor = "apple", target_os = "freebsd")))]
        created: None,
    }
}
struct Directory(*mut libc::DIR);
impl Drop for Directory {
    fn drop(&mut self) {
        // SAFETY: this worker owns the directory stream and has no active borrowed entry.
        unsafe { libc::closedir(self.0) };
    }
}
fn directory(p: &FsPath, buffer: &mut [u8], cookie: u64) -> Result<FsResult> {
    // SAFETY: prepared pathname; returned directory is exclusively owned below.
    let raw = unsafe { libc::opendir(path(p)?) };
    if raw.is_null() {
        return Err(error());
    }
    let dir = Directory(raw);
    let mut count = 0;
    let mut used = 0;
    loop {
        // SAFETY: errno location belongs to this worker thread.
        unsafe {
            #[cfg(any(target_os = "linux", target_os = "android"))]
            {
                *libc::__errno_location() = 0;
            }
            #[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
            {
                *libc::__error() = 0;
            }
        }
        // SAFETY: exclusive valid directory stream; entry inspected before next readdir.
        let entry = unsafe { libc::readdir(dir.0) };
        if entry.is_null() {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() != Some(0) {
                return Err(e.into());
            }
            return Ok(FsResult::Directory {
                bytes: used,
                cookie: count,
                eof: true,
            });
        }
        // SAFETY: readdir supplied a live entry with a NUL-terminated name.
        let (name, typ) = unsafe {
            (
                CStr::from_ptr((*entry).d_name.as_ptr()).to_bytes(),
                (*entry).d_type,
            )
        };
        if name == b"." || name == b".." {
            continue;
        }
        if count < cookie {
            count += 1;
            continue;
        }
        let kind = match typ {
            libc::DT_REG => 1,
            libc::DT_DIR => 2,
            libc::DT_LNK => 3,
            _ => 0,
        };
        if !record(buffer, &mut used, name, kind) {
            if used == 0 {
                return Err(Error::new(ErrorKind::ResourceLimit));
            }
            return Ok(FsResult::Directory {
                bytes: used,
                cookie: count,
                eof: false,
            });
        }
        count += 1;
    }
}
