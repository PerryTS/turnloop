use super::{FileState, file, output, record};
use crate::*;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    os::windows::io::{AsRawHandle, FromRawHandle},
};
use windows_sys::Win32::{Foundation::*, Storage::FileSystem::*};
fn error() -> Error {
    std::io::Error::last_os_error().into()
}
fn status(ok: i32) -> Result<()> {
    if ok == 0 { Err(error()) } else { Ok(()) }
}
fn path(p: &FsPath) -> Result<*const u16> {
    if p.preopen.is_some() {
        return Err(Error::new(ErrorKind::Unsupported));
    }
    Ok(p.native.as_ptr())
}
fn create(p: &FsPath, access: u32, creation: u32, flags: u32) -> Result<File> {
    // SAFETY: prepared wide path; default security/template and owned return handle.
    let h = unsafe {
        CreateFileW(
            path(p)?,
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            creation,
            flags,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(error());
    }
    // SAFETY: CreateFileW returned a fresh owned handle.
    Ok(unsafe { File::from_raw_handle(h) })
}
pub(super) fn open(p: &FsPath, o: FileOptions) -> Result<File> {
    if (!o.read && !o.write) || ((o.create || o.exclusive || o.truncate || o.append) && !o.write) {
        return Err(Error::new(ErrorKind::InvalidInput));
    }
    let access = if o.read { GENERIC_READ } else { 0 }
        | if o.write {
            if o.append {
                FILE_APPEND_DATA
            } else {
                GENERIC_WRITE
            }
        } else {
            0
        };
    let creation = if o.exclusive {
        CREATE_NEW
    } else if o.create && o.truncate {
        CREATE_ALWAYS
    } else if o.create {
        OPEN_ALWAYS
    } else if o.truncate {
        TRUNCATE_EXISTING
    } else {
        OPEN_EXISTING
    };
    let flags = FILE_ATTRIBUTE_NORMAL
        | if o.follow_symlinks {
            0
        } else {
            FILE_FLAG_OPEN_REPARSE_POINT
        };
    let f = create(p, access, creation, flags)?;
    let m = metadata(&f)?;
    if m.kind != FileType::File {
        return Err(Error::new(if m.kind == FileType::Directory {
            ErrorKind::IsADirectory
        } else {
            ErrorKind::Unsupported
        }));
    }
    Ok(f)
}
pub(super) fn execute(r: FsRequest, state: Option<&mut FileState>) -> Result<FsResult> {
    match r {
        FsRequest::Read { buffer, offset, .. } => {
            let s = file(state)?;
            let f = s.file.as_mut().expect("open file");
            f.seek(SeekFrom::Start(offset.unwrap_or(s.cursor)))
                .map_err(Error::from)?;
            let n = f.read(output(&mut buffer)).map_err(Error::from)?;
            if offset.is_none() {
                s.cursor += n as u64;
            }
            Ok(FsResult::Read(n))
        }
        FsRequest::Write { buffer, offset, .. } => {
            let s = file(state)?;
            if s.append && offset.is_some() {
                return Err(Error::new(ErrorKind::InvalidInput));
            }
            let f = s.file.as_mut().expect("open file");
            if !s.append {
                f.seek(SeekFrom::Start(offset.unwrap_or(s.cursor)))
                    .map_err(Error::from)?;
            }
            let n = f.write(buffer.as_slice()).map_err(Error::from)?;
            if offset.is_none() {
                s.cursor += n as u64;
            }
            Ok(FsResult::Wrote(n))
        }
        FsRequest::Fstat(_) => {
            metadata(file(state)?.file.as_ref().expect("open file")).map(FsResult::Metadata)
        }
        FsRequest::Stat {
            path: p,
            follow_symlinks,
        } => {
            let f = create(
                &p,
                FILE_READ_ATTRIBUTES,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS
                    | if follow_symlinks {
                        0
                    } else {
                        FILE_FLAG_OPEN_REPARSE_POINT
                    },
            )?;
            metadata(&f).map(FsResult::Metadata)
        }
        FsRequest::Sync { data_only, .. } => {
            let f = file(state)?.file.as_ref().expect("open file");
            (if data_only {
                f.sync_data()
            } else {
                f.sync_all()
            })
            .map_err(Error::from)?;
            Ok(FsResult::Done)
        }
        FsRequest::Truncate { size, .. } => {
            file(state)?
                .file
                .as_ref()
                .expect("open file")
                .set_len(size)
                .map_err(Error::from)?;
            Ok(FsResult::Done)
        }
        FsRequest::Mkdir { path: p, .. } => {
            // SAFETY: prepared wide path and default security attributes.
            status(unsafe { CreateDirectoryW(path(&p)?, std::ptr::null()) })?;
            Ok(FsResult::Done)
        }
        FsRequest::Rename { from, to } => {
            // SAFETY: both wide paths live through this synchronous operation.
            status(unsafe { MoveFileExW(path(&from)?, path(&to)?, MOVEFILE_REPLACE_EXISTING) })?;
            Ok(FsResult::Done)
        }
        FsRequest::Unlink(p) => {
            // SAFETY: prepared wide pathname.
            status(unsafe { DeleteFileW(path(&p)?) })?;
            Ok(FsResult::Done)
        }
        FsRequest::Rmdir(p) => {
            // SAFETY: prepared wide pathname.
            status(unsafe { RemoveDirectoryW(path(&p)?) })?;
            Ok(FsResult::Done)
        }
        FsRequest::ReadDir {
            path: p,
            mut buffer,
            cookie,
        } => directory(&p, output(&mut buffer), cookie),
    }
}
fn time(t: FILETIME) -> FileTime {
    let ticks = (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    FileTime {
        seconds: (ticks / 10_000_000) as i64 - 11_644_473_600,
        nanoseconds: ((ticks % 10_000_000) * 100) as u32,
    }
}
fn kind(attributes: u32) -> FileType {
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        FileType::Symlink
    } else if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        FileType::Directory
    } else {
        FileType::File
    }
}
fn metadata(f: &File) -> Result<FileMetadata> {
    let mut info = std::mem::MaybeUninit::uninit();
    // SAFETY: owned file handle and correctly sized native output.
    status(unsafe { GetFileInformationByHandle(f.as_raw_handle(), info.as_mut_ptr()) })?;
    // SAFETY: successful API initialized every field.
    let i = unsafe { info.assume_init() };
    Ok(FileMetadata {
        kind: kind(i.dwFileAttributes),
        size: (u64::from(i.nFileSizeHigh) << 32) | u64::from(i.nFileSizeLow),
        inode: Some((u64::from(i.nFileIndexHigh) << 32) | u64::from(i.nFileIndexLow)),
        device: Some(u64::from(i.dwVolumeSerialNumber)),
        mode: None,
        uid: None,
        gid: None,
        links: Some(u64::from(i.nNumberOfLinks)),
        accessed: Some(time(i.ftLastAccessTime)),
        modified: Some(time(i.ftLastWriteTime)),
        changed: None,
        created: Some(time(i.ftCreationTime)),
    })
}
struct Search(HANDLE);
impl Drop for Search {
    fn drop(&mut self) {
        // SAFETY: unique live search handle, no asynchronous operation.
        unsafe { FindClose(self.0) };
    }
}
fn directory(p: &FsPath, buffer: &mut [u8], cookie: u64) -> Result<FsResult> {
    path(p)?;
    let mut data = std::mem::MaybeUninit::<WIN32_FIND_DATAW>::uninit();
    // SAFETY: prepared wildcard path and correctly sized output.
    let h = unsafe { FindFirstFileW(p.search.as_ptr(), data.as_mut_ptr()) };
    if h == INVALID_HANDLE_VALUE {
        return Err(error());
    }
    let search = Search(h);
    let mut count = 0;
    let mut used = 0;
    loop {
        // SAFETY: FindFirst/Next succeeded before reaching this iteration.
        let d = unsafe { data.assume_init_ref() };
        let len = d
            .cFileName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(d.cFileName.len());
        let wide = &d.cFileName[..len];
        if wide != [46] && wide != [46, 46] {
            if count >= cookie {
                let mut utf8 = [0u8; 1040];
                let mut n = 0;
                for c in char::decode_utf16(wide.iter().copied()) {
                    let c = c.map_err(|_| Error::new(ErrorKind::InvalidInput))?;
                    n += c.encode_utf8(&mut utf8[n..]).len();
                }
                let typ = match kind(d.dwFileAttributes) {
                    FileType::File => 1,
                    FileType::Directory => 2,
                    FileType::Symlink => 3,
                    _ => 0,
                };
                if !record(buffer, &mut used, &utf8[..n], typ) {
                    if used == 0 {
                        return Err(Error::new(ErrorKind::ResourceLimit));
                    }
                    return Ok(FsResult::Directory {
                        bytes: used,
                        cookie: count,
                        eof: false,
                    });
                }
            }
            count += 1;
        }
        // SAFETY: live search handle and output storage, previous entry no longer borrowed.
        if unsafe { FindNextFileW(search.0, data.as_mut_ptr()) } == 0 {
            let e = error();
            if e.os != Some(ERROR_NO_MORE_FILES as i32) {
                return Err(e);
            }
            return Ok(FsResult::Directory {
                bytes: used,
                cookie: count,
                eof: true,
            });
        }
    }
}
