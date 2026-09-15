//! Typed filesystem requests. Prepare paths once; accepted requests retain buffers
//! through their terminal completion. File operations are FIFO per handle.
use crate::{Error, ErrorKind, Handle, IoBufMut, Result, WriteBuf};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Debug)]
/// Prepared, reusable pathname. Cloning does not allocate. On WASI paths are
/// resolved relative to an explicit preopened directory (see `FsPath::preopened`).
pub struct FsPath {
    pub(crate) path: Arc<PathBuf>,
    #[cfg(unix)]
    pub(crate) native: Arc<std::ffi::CString>,
    #[cfg(windows)]
    pub(crate) native: Arc<[u16]>,
    #[cfg(windows)]
    pub(crate) search: Arc<[u16]>,
    pub(crate) preopen: Option<usize>,
}
impl FsPath {
    /// Prepare a native pathname. Embedded NULs are rejected before submission.
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        #[cfg(unix)]
        let native = {
            use std::os::unix::ffi::OsStrExt;
            Arc::new(
                std::ffi::CString::new(path.as_os_str().as_bytes())
                    .map_err(|_| Error::new(ErrorKind::InvalidInput))?,
            )
        };
        #[cfg(windows)]
        let native = {
            use std::os::windows::ffi::OsStrExt;
            let mut wide: Vec<_> = path.as_os_str().encode_wide().collect();
            if wide.contains(&0) {
                return Err(Error::new(ErrorKind::InvalidInput));
            }
            wide.push(0);
            Arc::from(wide)
        };
        #[cfg(target_arch = "wasm32")]
        if path.to_str().is_none_or(|s| s.contains('\0')) {
            return Err(Error::new(ErrorKind::InvalidInput));
        }
        #[cfg(windows)]
        let search = {
            use std::os::windows::ffi::OsStrExt;
            let mut wide: Vec<_> = path.join("*").as_os_str().encode_wide().collect();
            wide.push(0);
            Arc::from(wide)
        };
        Ok(Self {
            path: Arc::new(path.to_path_buf()),
            #[cfg(any(unix, windows))]
            native,
            #[cfg(windows)]
            search,
            preopen: None,
        })
    }
    /// Capability-relative WASI path; `directory` indexes wasi:filesystem/preopens.
    /// Absolute paths and parent components are rejected, and the host enforces
    /// symlink confinement. No ambient native path fallback is performed.
    pub fn preopened(directory: usize, path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if path.is_absolute()
            || path.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir | std::path::Component::Prefix(_)
                )
            })
        {
            return Err(Error::new(ErrorKind::InvalidInput));
        }
        let mut result = Self::new(path)?;
        result.preopen = Some(directory);
        Ok(result)
    }
    /// Original pathname, for host diagnostics.
    pub fn as_path(&self) -> &Path {
        &self.path
    }
}

#[derive(Clone, Copy, Debug)]
/// Portable open options, covering Node r/r+/w/w+/wx/a/a+ variants.
pub struct FileOptions {
    /// Allow reads.
    pub read: bool,
    /// Allow writes.
    pub write: bool,
    /// Create when absent.
    pub create: bool,
    /// Require creation of a new file.
    pub exclusive: bool,
    /// Truncate an existing file at open.
    pub truncate: bool,
    /// Append writes atomically where supported. Positional writes are rejected.
    pub append: bool,
    /// Follow the final symlink.
    pub follow_symlinks: bool,
    /// Unix creation permissions, filtered by umask. WASI uses host permissions.
    pub mode: u32,
}
impl Default for FileOptions {
    fn default() -> Self {
        Self {
            read: true,
            write: false,
            create: false,
            exclusive: false,
            truncate: false,
            append: false,
            follow_symlinks: true,
            mode: 0o666,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Portable file kind; special native objects remain distinguishable.
pub enum FileType {
    /// Regular bytes.
    File,
    /// Directory.
    Directory,
    /// Symbolic link.
    Symlink,
    /// Socket, device, FIFO or another native type.
    Other,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Unix-epoch timestamp, with nanoseconds retained.
pub struct FileTime {
    /// Signed seconds from the Unix epoch.
    pub seconds: i64,
    /// Fractional nanoseconds, less than one billion.
    pub nanoseconds: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Metadata required for Perry Stats and watchFile snapshots. Unavailable fields
/// are None, never fabricated zero values.
pub struct FileMetadata {
    /// Native file kind.
    pub kind: FileType,
    /// Length in bytes.
    pub size: u64,
    /// Native inode/file identity when available.
    pub inode: Option<u64>,
    /// Native device identity when available.
    pub device: Option<u64>,
    /// Unix mode including type bits when available.
    pub mode: Option<u32>,
    /// Owner uid when available.
    pub uid: Option<u32>,
    /// Owner gid when available.
    pub gid: Option<u32>,
    /// Hard link count when available.
    pub links: Option<u64>,
    /// Last access.
    pub accessed: Option<FileTime>,
    /// Last content change.
    pub modified: Option<FileTime>,
    /// Last metadata change.
    pub changed: Option<FileTime>,
    /// Creation time when available.
    pub created: Option<FileTime>,
}
#[derive(Debug)]
/// One typed request. Cursor operations on a handle execute in submission order;
/// independent handles/path operations may complete in any order. Writes may be
/// partial, matching Node fs.write. A cancelled started mutation is not rolled back.
pub enum FsRequest {
    /// Read at an explicit offset, or advance the file cursor with None.
    Read {
        /// File identity.
        file: Handle,
        /// Stable output bytes.
        buffer: IoBufMut,
        /// None selects the streaming cursor.
        offset: Option<u64>,
    },
    /// Write once at an explicit offset or the streaming cursor.
    Write {
        /// File identity.
        file: Handle,
        /// Stable input bytes.
        buffer: WriteBuf,
        /// None selects the streaming cursor.
        offset: Option<u64>,
    },
    /// Metadata by path; false implements lstat.
    Stat {
        /// Prepared path.
        path: FsPath,
        /// Follow the final symbolic link.
        follow_symlinks: bool,
    },
    /// Metadata for an opened file.
    Fstat(Handle),
    /// Flush file data, and optionally metadata.
    Sync {
        /// File identity.
        file: Handle,
        /// True implements fdatasync.
        data_only: bool,
    },
    /// Change file length without changing its cursor.
    Truncate {
        /// File identity.
        file: Handle,
        /// New byte length.
        size: u64,
    },
    /// Create one directory (recursive traversal belongs to the host).
    Mkdir {
        /// Prepared path.
        path: FsPath,
        /// Unix permissions, filtered by umask.
        mode: u32,
    },
    /// Atomically rename where the host filesystem permits it.
    Rename {
        /// Existing path.
        from: FsPath,
        /// Destination path.
        to: FsPath,
    },
    /// Remove a file or symbolic link.
    Unlink(FsPath),
    /// Remove an empty directory.
    Rmdir(FsPath),
    /// Read a page of entries. `cookie` is the number of entries already consumed;
    /// concurrent directory mutation may repeat/omit entries, as with native APIs.
    /// Records: little-endian u16 name length, u8 type (0 other,1 file,2 dir,3 link),
    /// followed by filename bytes (native bytes on Unix, UTF-8 on Windows/WASI).
    ReadDir {
        /// Prepared directory path.
        path: FsPath,
        /// Stable output memory; a name never spans pages.
        buffer: IoBufMut,
        /// Start with zero, then use the completion cookie.
        cookie: u64,
    },
}
impl FsRequest {
    pub(crate) fn handle(&self) -> Option<Handle> {
        match self {
            Self::Read { file, .. }
            | Self::Write { file, .. }
            | Self::Sync { file, .. }
            | Self::Truncate { file, .. }
            | Self::Fstat(file) => Some(*file),
            _ => None,
        }
    }
}
#[derive(Debug)]
/// Successful typed result. Errors use OpResult::Err and cancellation uses the
/// ordinary terminal Cancelled/Stopped results.
pub enum FsResult {
    /// Open succeeded. The reserved handle is usable until explicitly closed.
    Opened,
    /// Bytes read; zero means EOF (or an empty input buffer).
    Read(usize),
    /// Bytes written; partial writes are possible.
    Wrote(usize),
    /// Native metadata.
    Metadata(FileMetadata),
    /// Directory page, with the next continuation cookie.
    Directory {
        /// Initialized output byte length.
        bytes: usize,
        /// Number of entries consumed so far.
        cookie: u64,
        /// Reached the directory's end.
        eof: bool,
    },
    /// Namespace, truncate or sync operation succeeded.
    Done,
}

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::Service;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Coalesced native watch notification for the watched root. Native APIs can
/// omit names; this surface deliberately reports root invalidation. The host
/// rescans to derive filenames. Overflow requires a full recursive rescan.
/// No ordering or one-event-per-write guarantee is possible across native APIs.
pub struct WatchEvent {
    /// Content or metadata may have changed.
    pub changed: bool,
    /// Entries may have been created, removed, moved, or the root replaced.
    pub renamed: bool,
    /// Native events were lost; rescan the complete watched scope.
    pub overflow: bool,
}
#[cfg(not(target_arch = "wasm32"))]
mod watch;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use watch::Watches;
