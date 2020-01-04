//! Filesystem operation reply
//!
//! A reply is passed to filesystem operation implementations and must be used to send back the
//! result of an operation. The reply can optionally be sent to another thread to asynchronously
//! work on an operation and provide the result later. Also it allows replying with a block of
//! data without cloning the data. A reply *must always* be used (by calling either ok() or
//! error() exactly once).
//!
//! TODO: This module is meant to go away soon in favor of `lowlevel::reply`.

use std::{mem, ptr};
use std::convert::AsRef;
use std::ffi::OsStr;
use std::fmt;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::time::Duration;
#[cfg(target_os = "macos")]
use std::time::SystemTime;
use fuse_abi::fuse_dirent;
use libc::{c_int, S_IFIFO, S_IFCHR, S_IFBLK, S_IFDIR, S_IFREG, S_IFLNK, S_IFSOCK};

use crate::{FileType, FileAttr};
use crate::lowlevel;

/// Generic reply callback to send data
pub trait ReplySender: Write + Send + fmt::Debug + 'static {}

impl<T: Write + Send + fmt::Debug + 'static> ReplySender for T {}

/// Generic reply trait
pub trait Reply {
    /// Create a new reply for the given request
    fn new<S: ReplySender>(unique: u64, sender: S) -> Self;
}

// Some platforms like Linux x86_64 have mode_t = u32, and lint warns of a trivial_numeric_casts.
// But others like macOS x86_64 have mode_t = u16, requiring a typecast.  So, just silence lint.
#[allow(trivial_numeric_casts)]
/// Returns the mode for a given file type and permission
fn mode_from_type_and_perm(file_type: FileType, perm: u16) -> u32 {
    (match file_type {
        FileType::NamedPipe => S_IFIFO,
        FileType::CharDevice => S_IFCHR,
        FileType::BlockDevice => S_IFBLK,
        FileType::Directory => S_IFDIR,
        FileType::RegularFile => S_IFREG,
        FileType::Symlink => S_IFLNK,
        FileType::Socket => S_IFSOCK,
    }) as u32 | perm as u32
}

///
/// Empty reply
///
#[derive(Debug)]
pub struct ReplyEmpty {
    unique: u64,
    sender: Box<dyn ReplySender>,
}

impl Reply for ReplyEmpty {
    fn new<S: ReplySender>(unique: u64, sender: S) -> ReplyEmpty {
        Self { unique, sender: Box::new(sender) }
    }
}

impl ReplyEmpty {
    /// Reply to a request with nothing
    pub fn ok(mut self) {
        let payload = lowlevel::reply::Data::from(&[][..]);
        let reply = lowlevel::reply::Reply::new(self.unique, Ok(payload));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }

    /// Reply to a request with the given error code
    pub fn error(mut self, err: c_int) {
        let reply = lowlevel::reply::Reply::<lowlevel::reply::Data<'_>>::new(self.unique, Err(err));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }
}

///
/// Data reply
///
#[derive(Debug)]
pub struct ReplyData {
    unique: u64,
    sender: Box<dyn ReplySender>,
}

impl Reply for ReplyData {
    fn new<S: ReplySender>(unique: u64, sender: S) -> ReplyData {
        Self { unique, sender: Box::new(sender) }
    }
}

impl ReplyData {
    /// Reply to a request with the given data
    pub fn data(mut self, data: &[u8]) {
        let payload = lowlevel::reply::Data::from(data);
        let reply = lowlevel::reply::Reply::new(self.unique, Ok(payload));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }

    /// Reply to a request with the given error code
    pub fn error(mut self, err: c_int) {
        let reply = lowlevel::reply::Reply::<lowlevel::reply::Data<'_>>::new(self.unique, Err(err));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }
}

///
/// Init reply
///
#[derive(Debug)]
pub struct ReplyInit {
    unique: u64,
    sender: Box<dyn ReplySender>,
}

impl Reply for ReplyInit {
    fn new<S: ReplySender>(unique: u64, sender: S) -> ReplyInit {
        Self { unique, sender: Box::new(sender) }
    }
}

impl ReplyInit {
    /// Reply to a request with the given entry
    pub fn init(mut self, major: u32, minor: u32, max_readahead: u32, flags: u32, max_write: u32) {
        let payload = lowlevel::reply::Init::new(major, minor, max_readahead, flags, max_write);
        let reply = lowlevel::reply::Reply::new(self.unique, Ok(payload));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }

    /// Reply to a request with the given error code
    pub fn error(mut self, err: c_int) {
        let reply = lowlevel::reply::Reply::<lowlevel::reply::Init>::new(self.unique, Err(err));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }
}

///
/// Entry reply
///
#[derive(Debug)]
pub struct ReplyEntry {
    unique: u64,
    sender: Box<dyn ReplySender>,
}

impl Reply for ReplyEntry {
    fn new<S: ReplySender>(unique: u64, sender: S) -> ReplyEntry {
        Self { unique, sender: Box::new(sender) }
    }
}

impl ReplyEntry {
    /// Reply to a request with the given entry
    pub fn entry(mut self, ttl: &Duration, attr: &FileAttr, generation: u64) {
        let payload = lowlevel::reply::Entry::new(ttl, attr, generation);
        let reply = lowlevel::reply::Reply::new(self.unique, Ok(payload));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }

    /// Reply to a request with the given error code
    pub fn error(mut self, err: c_int) {
        let reply = lowlevel::reply::Reply::<lowlevel::reply::Entry>::new(self.unique, Err(err));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }
}

///
/// Attribute Reply
///
#[derive(Debug)]
pub struct ReplyAttr {
    unique: u64,
    sender: Box<dyn ReplySender>,
}

impl Reply for ReplyAttr {
    fn new<S: ReplySender>(unique: u64, sender: S) -> ReplyAttr {
        Self { unique, sender: Box::new(sender) }
    }
}

impl ReplyAttr {
    /// Reply to a request with the given attribute
    pub fn attr(mut self, ttl: &Duration, attr: &FileAttr) {
        let payload = lowlevel::reply::Attr::new(ttl, attr);
        let reply = lowlevel::reply::Reply::new(self.unique, Ok(payload));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }

    /// Reply to a request with the given error code
    pub fn error(mut self, err: c_int) {
        let reply = lowlevel::reply::Reply::<lowlevel::reply::Attr>::new(self.unique, Err(err));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }
}

///
/// XTimes Reply
///
#[cfg(target_os = "macos")]
#[derive(Debug)]
pub struct ReplyXTimes {
    unique: u64,
    sender: Box<dyn ReplySender>,
}

#[cfg(target_os = "macos")]
impl Reply for ReplyXTimes {
    fn new<S: ReplySender>(unique: u64, sender: S) -> ReplyXTimes {
        Self { unique, sender: Box::new(sender) }
    }
}

#[cfg(target_os = "macos")]
impl ReplyXTimes {
    /// Reply to a request with the given xtimes
    pub fn xtimes(mut self, bkuptime: SystemTime, crtime: SystemTime) {
        let payload = lowlevel::reply::XTimes::new(&bkuptime, &crtime);
        let reply = lowlevel::reply::Reply::new(self.unique, Ok(payload));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }

    /// Reply to a request with the given error code
    pub fn error(mut self, err: c_int) {
        let reply = lowlevel::reply::Reply::<lowlevel::reply::XTimes>::new(self.unique, Err(err));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }
}

///
/// Open Reply
///
#[derive(Debug)]
pub struct ReplyOpen {
    unique: u64,
    sender: Box<dyn ReplySender>,
}

impl Reply for ReplyOpen {
    fn new<S: ReplySender>(unique: u64, sender: S) -> ReplyOpen {
        Self { unique, sender: Box::new(sender) }
    }
}

impl ReplyOpen {
    /// Reply to a request with the given open result
    pub fn opened(mut self, fh: u64, flags: u32) {
        let payload = lowlevel::reply::Open::new(fh, flags);
        let reply = lowlevel::reply::Reply::new(self.unique, Ok(payload));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }

    /// Reply to a request with the given error code
    pub fn error(mut self, err: c_int) {
        let reply = lowlevel::reply::Reply::<lowlevel::reply::Open>::new(self.unique, Err(err));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }
}

///
/// Write Reply
///
#[derive(Debug)]
pub struct ReplyWrite {
    unique: u64,
    sender: Box<dyn ReplySender>,
}

impl Reply for ReplyWrite {
    fn new<S: ReplySender>(unique: u64, sender: S) -> ReplyWrite {
        Self { unique, sender: Box::new(sender) }
    }
}

impl ReplyWrite {
    /// Reply to a request with the given open result
    pub fn written(mut self, size: u32) {
        let payload = lowlevel::reply::Write::new(size);
        let reply = lowlevel::reply::Reply::new(self.unique, Ok(payload));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }

    /// Reply to a request with the given error code
    pub fn error(mut self, err: c_int) {
        let reply = lowlevel::reply::Reply::<lowlevel::reply::Write>::new(self.unique, Err(err));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }
}

///
/// Statfs Reply
///
#[derive(Debug)]
pub struct ReplyStatfs {
    unique: u64,
    sender: Box<dyn ReplySender>,
}

impl Reply for ReplyStatfs {
    fn new<S: ReplySender>(unique: u64, sender: S) -> ReplyStatfs {
        Self { unique, sender: Box::new(sender) }
    }
}

impl ReplyStatfs {
    /// Reply to a request with the given open result
    pub fn statfs(mut self, blocks: u64, bfree: u64, bavail: u64, files: u64, ffree: u64, bsize: u32, namelen: u32, frsize: u32) {
        let payload = lowlevel::reply::StatFs::new(blocks, bfree, bavail, files, ffree, bsize, namelen, frsize);
        let reply = lowlevel::reply::Reply::new(self.unique, Ok(payload));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }

    /// Reply to a request with the given error code
    pub fn error(mut self, err: c_int) {
        let reply = lowlevel::reply::Reply::<lowlevel::reply::StatFs>::new(self.unique, Err(err));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }
}

///
/// Create reply
///
#[derive(Debug)]
pub struct ReplyCreate {
    unique: u64,
    sender: Box<dyn ReplySender>,
}

impl Reply for ReplyCreate {
    fn new<S: ReplySender>(unique: u64, sender: S) -> ReplyCreate {
        Self { unique, sender: Box::new(sender) }
    }
}

impl ReplyCreate {
    /// Reply to a request with the given entry
    pub fn created(mut self, ttl: &Duration, attr: &FileAttr, generation: u64, fh: u64, flags: u32) {
        let payload = lowlevel::reply::Create::new(ttl, attr, generation, fh, flags);
        let reply = lowlevel::reply::Reply::new(self.unique, Ok(payload));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }

    /// Reply to a request with the given error code
    pub fn error(mut self, err: c_int) {
        let reply = lowlevel::reply::Reply::<lowlevel::reply::Create>::new(self.unique, Err(err));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }
}

///
/// Lock Reply
///
#[derive(Debug)]
pub struct ReplyLock {
    unique: u64,
    sender: Box<dyn ReplySender>,
}

impl Reply for ReplyLock {
    fn new<S: ReplySender>(unique: u64, sender: S) -> ReplyLock {
        Self { unique, sender: Box::new(sender) }
    }
}

impl ReplyLock {
    /// Reply to a request with the given open result
    pub fn locked(mut self, start: u64, end: u64, typ: u32, pid: u32) {
        let payload = lowlevel::reply::Lock::new(start, end, typ, pid);
        let reply = lowlevel::reply::Reply::new(self.unique, Ok(payload));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }

    /// Reply to a request with the given error code
    pub fn error(mut self, err: c_int) {
        let reply = lowlevel::reply::Reply::<lowlevel::reply::Lock>::new(self.unique, Err(err));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }
}

///
/// Bmap Reply
///
#[derive(Debug)]
pub struct ReplyBmap {
    unique: u64,
    sender: Box<dyn ReplySender>,
}

impl Reply for ReplyBmap {
    fn new<S: ReplySender>(unique: u64, sender: S) -> ReplyBmap {
        Self { unique, sender: Box::new(sender) }
    }
}

impl ReplyBmap {
    /// Reply to a request with the given open result
    pub fn bmap(mut self, block: u64) {
        let payload = lowlevel::reply::Bmap::new(block);
        let reply = lowlevel::reply::Reply::new(self.unique, Ok(payload));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }

    /// Reply to a request with the given error code
    pub fn error(mut self, err: c_int) {
        let reply = lowlevel::reply::Reply::<lowlevel::reply::Bmap>::new(self.unique, Err(err));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }
}

///
/// Directory reply
///
#[derive(Debug)]
pub struct ReplyDirectory {
    unique: u64,
    sender: Box<dyn ReplySender>,
    data: Vec<u8>,
}

impl ReplyDirectory {
    /// Creates a new ReplyDirectory with a specified buffer size.
    pub fn new<S: ReplySender>(unique: u64, sender: S, size: usize) -> ReplyDirectory {
        Self { unique, sender: Box::new(sender), data: Vec::with_capacity(size) }
    }

    /// Add an entry to the directory reply buffer. Returns true if the buffer is full.
    /// A transparent offset value can be provided for each entry. The kernel uses these
    /// value to request the next entries in further readdir calls
    pub fn add<T: AsRef<OsStr>>(&mut self, ino: u64, offset: i64, file_type: FileType, name: T) -> bool {
        let name = name.as_ref().as_bytes();
        let entlen = mem::size_of::<fuse_dirent>() + name.len();
        let entsize = (entlen + mem::size_of::<u64>() - 1) & !(mem::size_of::<u64>() - 1); // 64bit align
        let padlen = entsize - entlen;
        if self.data.len() + entsize > self.data.capacity() { return true; }
        unsafe {
            let p = self.data.as_mut_ptr().offset(self.data.len() as isize);
            let pdirent: *mut fuse_dirent = mem::transmute(p);
            (*pdirent).ino = ino;
            (*pdirent).off = offset as u64;
            (*pdirent).namelen = name.len() as u32;
            (*pdirent).typ = mode_from_type_and_perm(file_type, 0) >> 12;
            let p = p.offset(mem::size_of_val(&*pdirent) as isize);
            ptr::copy_nonoverlapping(name.as_ptr(), p, name.len());
            let p = p.offset(name.len() as isize);
            ptr::write_bytes(p, 0u8, padlen);
            let newlen = self.data.len() + entsize;
            self.data.set_len(newlen);
        }
        false
    }

    /// Reply to a request with the filled directory buffer
    pub fn ok(mut self) {
        let payload = lowlevel::reply::Data::from(self.data);
        let reply = lowlevel::reply::Reply::new(self.unique, Ok(payload));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }

    /// Reply to a request with the given error code
    pub fn error(mut self, err: c_int) {
        let reply = lowlevel::reply::Reply::<lowlevel::reply::Data<'_>>::new(self.unique, Err(err));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }
}

///
/// Xattr reply
///
#[derive(Debug)]
pub struct ReplyXattr {
    unique: u64,
    sender: Box<dyn ReplySender>,
}

impl Reply for ReplyXattr {
    fn new<S: ReplySender>(unique: u64, sender: S) -> ReplyXattr {
        Self { unique, sender: Box::new(sender) }
    }
}

impl ReplyXattr {
    /// Reply to a request with the size of the xattr.
    pub fn size(mut self, size: u32) {
        let payload = lowlevel::reply::XAttrSize::new(size);
        let reply = lowlevel::reply::Reply::new(self.unique, Ok(payload));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }

    /// Reply to a request with the data in the xattr.
    pub fn data(mut self, data: &[u8]) {
        let payload = lowlevel::reply::Data::from(data);
        let reply = lowlevel::reply::Reply::new(self.unique, Ok(payload));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }

    /// Reply to a request with the given error code.
    pub fn error(mut self, err: c_int) {
        let reply = lowlevel::reply::Reply::<lowlevel::reply::XAttrSize>::new(self.unique, Err(err));
        let _ = self.sender.write_vectored(&reply.to_io_slices());
    }
}

#[cfg(test)]
mod test {
    use std::{mem, io, slice, thread};
    use std::sync::mpsc;
    use crate::FileType;
    use super::*;

    /// Serialize an arbitrary type to bytes (memory copy, useful for fuse_*_out types)
    fn as_bytes<T, U, F: FnOnce(&[&[u8]]) -> U>(data: &T, f: F) -> U {
        let len = mem::size_of::<T>();
        match len {
            0 => f(&[]),
            len => {
                let p = data as *const T as *const u8;
                let bytes = unsafe { slice::from_raw_parts(p, len) };
                f(&[bytes])
            }
        }
    }

    #[allow(dead_code)]
    #[repr(C)]
    struct Data { a: u8, b: u8, c: u16 }

    #[test]
    fn serialize_empty() {
        let data = ();
        as_bytes(&data, |bytes| {
            assert!(bytes.is_empty());
        });
    }

    #[test]
    fn serialize_slice() {
        let data: [u8; 4] = [0x12, 0x34, 0x56, 0x78];
        as_bytes(&data, |bytes| {
            assert_eq!(bytes, [[0x12, 0x34, 0x56, 0x78]]);
        });
    }

    #[test]
    #[cfg(target_endian = "little")]
    fn serialize_struct() {
        let data = Data { a: 0x12, b: 0x34, c: 0x5678 };
        as_bytes(&data, |bytes| {
            assert_eq!(bytes, [[0x12, 0x34, 0x78, 0x56]]);
        });
    }

    #[test]
    #[cfg(target_endian = "little")]
    fn serialize_tuple() {
        let data = (Data { a: 0x12, b: 0x34, c: 0x5678 }, Data { a: 0x9a, b: 0xbc, c: 0xdef0 });
        as_bytes(&data, |bytes| {
            assert_eq!(bytes, [[0x12, 0x34, 0x78, 0x56, 0x9a, 0xbc, 0xf0, 0xde]]);
        });
    }


    #[derive(Debug)]
    struct AssertSender {
        expected: Vec<u8>,
    }

    impl Write for AssertSender {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            panic!("ReplySender::write is not supposed to be called");
        }

        fn flush(&mut self) -> io::Result<()> {
            panic!("ReplySender::flush is not supposed to be called");
        }

        fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
            let data: Vec<u8> = bufs.iter().map(|buf| buf.iter()).flatten().copied().collect();
            assert_eq!(self.expected, data);
            Ok(data.len())
        }
    }

    #[test]
    #[cfg(target_endian = "little")]
    fn reply_empty() {
        let sender = AssertSender {
            expected: vec![
                0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0xef, 0xbe, 0xad, 0xde, 0x00, 0x00, 0x00, 0x00,
            ]
        };
        let reply: ReplyEmpty = Reply::new(0xdeadbeef, sender);
        reply.ok();
    }

    #[test]
    #[cfg(target_endian = "little")]
    fn reply_directory() {
        let sender = AssertSender {
            expected: vec![
                0x50, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0xef, 0xbe, 0xad, 0xde, 0x00, 0x00, 0x00, 0x00,
                0xbb, 0xaa, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x05, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00,  0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x00 ,0x00, 0x00,
                0xdd, 0xcc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x08, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00,  0x77, 0x6f, 0x72, 0x6c, 0x64, 0x2e, 0x72, 0x73,
            ]
        };
        let mut reply = ReplyDirectory::new(0xdeadbeef, sender, 4096);
        reply.add(0xaabb, 1, FileType::Directory, "hello");
        reply.add(0xccdd, 2, FileType::RegularFile, "world.rs");
        reply.ok();
    }


    #[derive(Debug)]
    struct AsyncSender(mpsc::Sender<()>);

    impl Write for AsyncSender {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            panic!("ReplySender::write is not supposed to be called");
        }

        fn flush(&mut self) -> io::Result<()> {
            panic!("ReplySender::flush is not supposed to be called");
        }

        fn write_vectored(&mut self, _bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
            self.0.send(()).unwrap();
            Ok(0)
        }
    }

    #[test]
    fn async_reply() {
        let (tx, rx) = mpsc::channel();
        let reply: ReplyEmpty = Reply::new(0xdeadbeef, AsyncSender(tx));
        thread::spawn(move || {
            reply.ok();
        });
        rx.recv().unwrap();
    }
}
