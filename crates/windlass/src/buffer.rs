//! Stable host memory and explicitly released pool leases.
use std::{cell::RefCell, ptr::NonNull, rc::Rc};

/// A stable, initialized, immutable byte region. Does not own the memory.
#[derive(Debug)]
pub struct IoBuf {
    ptr: NonNull<u8>,
    len: usize,
}
impl IoBuf {
    /// # Safety
    /// `ptr` must be non-null and readable for `len` bytes. Keep the region alive,
    /// unmoved and unmodified until the terminal completion is delivered, or until
    /// the driver is dropped (which must quiesce all native I/O before returning).
    pub unsafe fn from_raw_parts(ptr: *const u8, len: usize) -> Self {
        Self {
            ptr: NonNull::new(ptr.cast_mut()).expect("non-null buffer pointer"),
            len,
        }
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr.as_ptr()
    }
}
/// A stable, exclusively writable region. Does not own the memory.
#[derive(Debug)]
pub struct IoBufMut {
    ptr: NonNull<u8>,
    len: usize,
}
impl IoBufMut {
    /// # Safety
    /// `ptr` must be non-null and writable for `len` bytes. The region must remain
    /// alive and unmoved, and no other access is permitted, until delivery of the
    /// terminal completion or return from driver destruction.
    pub unsafe fn from_raw_parts(ptr: *mut u8, len: usize) -> Self {
        Self {
            ptr: NonNull::new(ptr).expect("non-null buffer pointer"),
            len,
        }
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn as_mut_ptr(&self) -> *mut u8 {
        self.ptr.as_ptr()
    }
}
#[derive(Debug)]
pub enum ReadBuf {
    Provided(IoBufMut),
    Pooled,
}
#[derive(Debug)]
pub enum WriteBuf {
    Provided(IoBuf),
    Owned(Vec<u8>),
}
impl WriteBuf {
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Self::Provided(b) => {
                // SAFETY: construction promises a readable region for the entire op lifetime.
                unsafe { std::slice::from_raw_parts(b.as_ptr(), b.len()) }
            }
            Self::Owned(b) => b,
        }
    }
}

/// Fixed upper bound keeps writev descriptors inline in the operation table.
pub const MAX_IOV: usize = 8;
#[derive(Debug)]
pub struct WriteVectored {
    pub bufs: [Option<WriteBuf>; MAX_IOV],
}
impl WriteVectored {
    pub fn new(bufs: impl IntoIterator<Item = WriteBuf>) -> crate::Result<Self> {
        let mut out = Self {
            bufs: std::array::from_fn(|_| None),
        };
        for (i, b) in bufs.into_iter().enumerate() {
            if i == MAX_IOV {
                return Err(crate::Error::new(crate::ErrorKind::InvalidInput));
            }
            out.bufs[i] = Some(b);
        }
        Ok(out)
    }
}

#[derive(Clone, Debug)]
pub struct BufferPool {
    inner: Rc<RefCell<Vec<Vec<u8>>>>,
}
impl BufferPool {
    pub fn new(count: usize, size: usize) -> Self {
        Self {
            inner: Rc::new(RefCell::new((0..count).map(|_| vec![0; size]).collect())),
        }
    }
    /// Exhaustion applies backpressure: the backend keeps the read pending.
    pub fn acquire(&self) -> Option<BufLease> {
        self.inner.borrow_mut().pop().map(|data| BufLease {
            data: Some(data),
            len: 0,
            pool: self.inner.clone(),
        })
    }
}
/// Owns the buffer until explicitly dropped/released. May safely survive turns.
/// Keeping all leases applies backpressure; it never allocates a new pool buffer.
#[derive(Debug)]
pub struct BufLease {
    data: Option<Vec<u8>>,
    len: usize,
    pool: Rc<RefCell<Vec<Vec<u8>>>>,
}
impl BufLease {
    pub fn as_slice(&self) -> &[u8] {
        &self.data.as_ref().expect("live lease")[..self.len]
    }
    pub fn writable(&mut self) -> &mut [u8] {
        self.data.as_mut().expect("live lease")
    }
    pub fn set_len(&mut self, len: usize) {
        assert!(len <= self.data.as_ref().expect("live lease").len());
        self.len = len;
    }
    pub fn release(self) {}
}
impl Drop for BufLease {
    fn drop(&mut self) {
        if let Some(data) = self.data.take() {
            self.pool.borrow_mut().push(data);
        }
    }
}
