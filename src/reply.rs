//!
//! A reply is passed to filesystem operation implementations and must
//! be used to send back the result of an operation.
//! The reply can optionally be sent to another thread to asynchronously
//! work on an operation and provide the result later. Also it allows
//! replying with a block of data without cloning the data.
//! A reply *must always* be used (by calling either ok() or error()
//! exactly once).
//!

use std::{mem, ptr, slice};
use std::ffi::AsOsStr;
use std::marker::PhantomData;
use std::thunk::Invoke;
use std::os::unix::ffi::OsStrExt;
use libc::{c_int, S_IFIFO, S_IFCHR, S_IFBLK, S_IFDIR, S_IFREG, S_IFLNK};
use libc::consts::os::posix88::EIO;
use time::Timespec;
use fuse::{fuse_attr, fuse_kstatfs, fuse_file_lock, fuse_entry_out, fuse_attr_out};
use fuse::{fuse_open_out, fuse_write_out, fuse_statfs_out, fuse_lk_out, fuse_bmap_out};
#[cfg(target_os = "macos")]
use fuse::fuse_getxtimes_out;
use fuse::{fuse_out_header, fuse_dirent};
use {FileType, FileAttr};

/// Generic reply trait
pub trait Reply {
    /// Create a new reply for the given request
    fn new<F: FnOnce(&[&[u8]])+Send> (unique: u64, sender: F) -> Self;
}

/// Serialize an arbitrary type to bytes (memory copy, useful for fuse_*_out types)
fn as_bytes<T, U, F: FnOnce(&[&[u8]]) -> U> (data: &T, f: F) -> U {
    let len = mem::size_of::<T>();
    match len {
        0 => f(&[]),
        len => {
            let p = data as *const T as *const u8;
            let bytes = unsafe { slice::from_raw_parts(p, len) };
            f(&[bytes])
        },
    }
}

// Some platforms like Linux x86_64 have mode_t = u32, and lint warns of an unused_typecast.
// But others like MacOS x86_64 have mode_t = u16, requiring a typecast.  So, just silence lint.
/// Returns the mode for a given file kind and permission
fn mode_from_kind_and_perm (kind: FileType, perm: u16) -> u32 {
    (match kind {
        FileType::NamedPipe => S_IFIFO,
        FileType::CharDevice => S_IFCHR,
        FileType::BlockDevice => S_IFBLK,
        FileType::Directory => S_IFDIR,
        FileType::RegularFile => S_IFREG,
        FileType::Symlink => S_IFLNK,
    }) as u32 | perm as u32
}

/// Returns a fuse_attr from FileAttr
#[cfg(target_os = "macos")]
fn fuse_attr_from_attr (attr: &FileAttr) -> fuse_attr {
    fuse_attr {
        ino: attr.ino,
        size: attr.size,
        blocks: attr.blocks,
        atime: attr.atime.sec,
        mtime: attr.mtime.sec,
        ctime: attr.ctime.sec,
        crtime: attr.crtime.sec,
        atimensec: attr.atime.nsec,
        mtimensec: attr.mtime.nsec,
        ctimensec: attr.ctime.nsec,
        crtimensec: attr.crtime.nsec,
        mode: mode_from_kind_and_perm(attr.kind, attr.perm),
        nlink: attr.nlink,
        uid: attr.uid,
        gid: attr.gid,
        rdev: attr.rdev,
        flags: attr.flags,
    }
}

/// Returns a fuse_attr from FileAttr
#[cfg(not(target_os = "macos"))]
fn fuse_attr_from_attr (attr: &FileAttr) -> fuse_attr {
    fuse_attr {
        ino: attr.ino,
        size: attr.size,
        blocks: attr.blocks,
        atime: attr.atime.sec,
        mtime: attr.mtime.sec,
        ctime: attr.ctime.sec,
        atimensec: attr.atime.nsec,
        mtimensec: attr.mtime.nsec,
        ctimensec: attr.ctime.nsec,
        mode: mode_from_kind_and_perm(attr.kind, attr.perm),
        nlink: attr.nlink,
        uid: attr.uid,
        gid: attr.gid,
        rdev: attr.rdev,
    }
}

///
/// Raw reply
///
pub struct ReplyRaw<T> {
    /// Unique id of the request to reply to
    unique: u64,
    /// Closure to call for sending the reply
    sender: Option<Box<for<'a> Invoke<&'a [&'a [u8]]> + Send>>,
    /// Marker for being able to have T on this struct (which enforces
    /// reply types to send the correct type of data)
    marker: PhantomData<T>,
}

impl<T> Reply for ReplyRaw<T> {
    fn new<F: FnOnce(&[&[u8]])+Send+'static> (unique: u64, sender: F) -> ReplyRaw<T> {
        let sender: Box<for<'a> Invoke<&'a [&'a [u8]]> + Send> = Box::new(sender);
        ReplyRaw { unique: unique, sender: Some(sender), marker: PhantomData }
    }
}

impl<T> ReplyRaw<T> {
    /// Reply to a request with the given error code and data. Must be called
    /// only once (the `ok` and `error` methods ensure this by consuming `self`)
    fn send (&mut self, err: c_int, bytes: &[&[u8]]) {
        assert!(self.sender.is_some());
        let len = bytes.iter().fold(0, |l, b| { l +  b.len()});
        let header = fuse_out_header {
            len: (mem::size_of::<fuse_out_header>() + len) as u32,
            error: -err,
            unique: self.unique,
        };
        as_bytes(&header, |headerbytes| {
            let sender = self.sender.take().unwrap();
            let mut sendbytes = headerbytes.to_vec();
            sendbytes.push_all(bytes);
            sender.invoke(&sendbytes);
        });
    }

    /// Reply to a request with the given type
    pub fn ok (mut self, data: &T) {
        as_bytes(data, |bytes| {
            self.send(0, bytes);
        })
    }

    /// Reply to a request with the given error code
    pub fn error (mut self, err: c_int) {
        self.send(err, &[]);
    }
}

#[unsafe_destructor]
impl<T> Drop for ReplyRaw<T> {
    fn drop (&mut self) {
        if self.sender.is_some() {
            warn!("Reply not sent for operation {}, replying with I/O error", self.unique);
            self.send(EIO, &[]);
        }
    }
}

///
/// Empty reply
///
pub struct ReplyEmpty {
    reply: ReplyRaw<()>,
}

impl Reply for ReplyEmpty {
    fn new<F: FnOnce(&[&[u8]])+Send> (unique: u64, sender: F) -> ReplyEmpty {
        ReplyEmpty { reply: Reply::new(unique, sender) }
    }
}

impl ReplyEmpty {
    /// Reply to a request with nothing
    pub fn ok (mut self) {
        self.reply.send(0, &[]);
    }

    /// Reply to a request with the given error code
    pub fn error (self, err: c_int) {
        self.reply.error(err);
    }
}

///
/// Data reply
///
pub struct ReplyData {
    reply: ReplyRaw<()>,
}

impl Reply for ReplyData {
    fn new<F: FnOnce(&[&[u8]])+Send> (unique: u64, sender: F) -> ReplyData {
        ReplyData { reply: Reply::new(unique, sender) }
    }
}

impl ReplyData {
    /// Reply to a request with the given data
    pub fn data (mut self, data: &[u8]) {
        self.reply.send(0, &[data]);
    }

    /// Reply to a request with the given error code
    pub fn error (self, err: c_int) {
        self.reply.error(err);
    }
}

///
/// Entry reply
///
pub struct ReplyEntry {
    reply: ReplyRaw<fuse_entry_out>,
}

impl Reply for ReplyEntry {
    fn new<F: FnOnce(&[&[u8]])+Send> (unique: u64, sender: F) -> ReplyEntry {
        ReplyEntry { reply: Reply::new(unique, sender) }
    }
}

impl ReplyEntry {
    /// Reply to a request with the given entry
    pub fn entry (self, ttl: &Timespec, attr: &FileAttr, generation: u64) {
        self.reply.ok(&fuse_entry_out {
            nodeid: attr.ino,
            generation: generation,
            entry_valid: ttl.sec,
            attr_valid: ttl.sec,
            entry_valid_nsec: ttl.nsec,
            attr_valid_nsec: ttl.nsec,
            attr: fuse_attr_from_attr(attr),
        });
    }

    /// Reply to a request with the given error code
    pub fn error (self, err: c_int) {
        self.reply.error(err);
    }
}

///
/// Attribute Reply
///
pub struct ReplyAttr {
    reply: ReplyRaw<fuse_attr_out>,
}

impl Reply for ReplyAttr {
    fn new<F: FnOnce(&[&[u8]])+Send> (unique: u64, sender: F) -> ReplyAttr {
        ReplyAttr { reply: Reply::new(unique, sender) }
    }
}

impl ReplyAttr {
    /// Reply to a request with the given attribute
    pub fn attr (self, ttl: &Timespec, attr: &FileAttr) {
        self.reply.ok(&fuse_attr_out {
            attr_valid: ttl.sec,
            attr_valid_nsec: ttl.nsec,
            dummy: 0,
            attr: fuse_attr_from_attr(attr),
        });
    }

    /// Reply to a request with the given error code
    pub fn error (self, err: c_int) {
        self.reply.error(err);
    }
}

///
/// XTimes Reply
///
#[cfg(target_os = "macos")]
pub struct ReplyXTimes {
    reply: ReplyRaw<fuse_getxtimes_out>,
}

#[cfg(target_os = "macos")]
impl Reply for ReplyXTimes {
    fn new<F: FnOnce(&[&[u8]])+Send> (unique: u64, sender: F) -> ReplyXTimes {
        ReplyXTimes { reply: Reply::new(unique, sender) }
    }
}

#[cfg(target_os = "macos")]
impl ReplyXTimes {
    /// Reply to a request with the given xtimes
    pub fn xtimes (self, bkuptime: Timespec, crtime: Timespec) {
        self.reply.ok(&fuse_getxtimes_out {
            bkuptime: bkuptime.sec,
            crtime: crtime.sec,
            bkuptimensec: bkuptime.nsec,
            crtimensec: crtime.nsec,
        });
    }

    /// Reply to a request with the given error code
    pub fn error (self, err: c_int) {
        self.reply.error(err);
    }
}

///
/// Open Reply
///
pub struct ReplyOpen {
    reply: ReplyRaw<fuse_open_out>,
}

impl Reply for ReplyOpen {
    fn new<F: FnOnce(&[&[u8]])+Send> (unique: u64, sender: F) -> ReplyOpen {
        ReplyOpen { reply: Reply::new(unique, sender) }
    }
}

impl ReplyOpen {
    /// Reply to a request with the given open result
    pub fn opened (self, fh: u64, flags: u32) {
        self.reply.ok(&fuse_open_out {
            fh: fh,
            open_flags: flags,
            padding: 0,
        });
    }

    /// Reply to a request with the given error code
    pub fn error (self, err: c_int) {
        self.reply.error(err);
    }
}

///
/// Write Reply
///
pub struct ReplyWrite {
    reply: ReplyRaw<fuse_write_out>,
}

impl Reply for ReplyWrite {
    fn new<F: FnOnce(&[&[u8]])+Send> (unique: u64, sender: F) -> ReplyWrite {
        ReplyWrite { reply: Reply::new(unique, sender) }
    }
}

impl ReplyWrite {
    /// Reply to a request with the given open result
    pub fn written (self, size: u32) {
        self.reply.ok(&fuse_write_out {
            size: size,
            padding: 0,
        });
    }

    /// Reply to a request with the given error code
    pub fn error (self, err: c_int) {
        self.reply.error(err);
    }
}

///
/// Statfs Reply
///
pub struct ReplyStatfs {
    reply: ReplyRaw<fuse_statfs_out>,
}

impl Reply for ReplyStatfs {
    fn new<F: FnOnce(&[&[u8]])+Send> (unique: u64, sender: F) -> ReplyStatfs {
        ReplyStatfs { reply: Reply::new(unique, sender) }
    }
}

impl ReplyStatfs {
    /// Reply to a request with the given open result
    pub fn statfs (self, blocks: u64, bfree: u64, bavail: u64, files: u64, ffree: u64, bsize: u32, namelen: u32, frsize: u32) {
        self.reply.ok(&fuse_statfs_out {
            st: fuse_kstatfs {
                blocks: blocks,
                bfree: bfree,
                bavail: bavail,
                files: files,
                ffree: ffree,
                bsize: bsize,
                namelen: namelen,
                frsize: frsize,
                padding: 0,
                spare: [0; 6],
            },
        });
    }

    /// Reply to a request with the given error code
    pub fn error (self, err: c_int) {
        self.reply.error(err);
    }
}

///
/// Create reply
///
pub struct ReplyCreate {
    reply: ReplyRaw<(fuse_entry_out, fuse_open_out)>,
}

impl Reply for ReplyCreate {
    fn new<F: FnOnce(&[&[u8]])+Send> (unique: u64, sender: F) -> ReplyCreate {
        ReplyCreate { reply: Reply::new(unique, sender) }
    }
}

impl ReplyCreate {
    /// Reply to a request with the given entry
    pub fn created (self, ttl: &Timespec, attr: &FileAttr, generation: u64, fh: u64, flags: u32) {
        self.reply.ok(&(fuse_entry_out {
            nodeid: attr.ino,
            generation: generation,
            entry_valid: ttl.sec,
            attr_valid: ttl.sec,
            entry_valid_nsec: ttl.nsec,
            attr_valid_nsec: ttl.nsec,
            attr: fuse_attr_from_attr(attr),
        }, fuse_open_out {
            fh: fh,
            open_flags: flags,
            padding: 0,
        }));
    }

    /// Reply to a request with the given error code
    pub fn error (self, err: c_int) {
        self.reply.error(err);
    }
}

///
/// Lock Reply
///
pub struct ReplyLock {
    reply: ReplyRaw<fuse_lk_out>,
}

impl Reply for ReplyLock {
    fn new<F: FnOnce(&[&[u8]])+Send> (unique: u64, sender: F) -> ReplyLock {
        ReplyLock { reply: Reply::new(unique, sender) }
    }
}

impl ReplyLock {
    /// Reply to a request with the given open result
    pub fn locked (self, start: u64, end: u64, typ: u32, pid: u32) {
        self.reply.ok(&fuse_lk_out {
            lk: fuse_file_lock {
                start: start,
                end: end,
                typ: typ,
                pid: pid,
            },
        });
    }

    /// Reply to a request with the given error code
    pub fn error (self, err: c_int) {
        self.reply.error(err);
    }
}

///
/// Bmap Reply
///
pub struct ReplyBmap {
    reply: ReplyRaw<fuse_bmap_out>,
}

impl Reply for ReplyBmap {
    fn new<F: FnOnce(&[&[u8]])+Send> (unique: u64, sender: F) -> ReplyBmap {
        ReplyBmap { reply: Reply::new(unique, sender) }
    }
}

impl ReplyBmap {
    /// Reply to a request with the given open result
    pub fn bmap (self, block: u64) {
        self.reply.ok(&fuse_bmap_out {
            block: block,
        });
    }

    /// Reply to a request with the given error code
    pub fn error (self, err: c_int) {
        self.reply.error(err);
    }
}

///
/// Directory reply
///
pub struct ReplyDirectory {
    reply: ReplyRaw<()>,
    size: usize,
    data: Vec<u8>,
}

impl Reply for ReplyDirectory {
    fn new<F: FnOnce(&[&[u8]])+Send> (unique: u64, sender: F) -> ReplyDirectory {
        ReplyDirectory { reply: Reply::new(unique, sender), size: 0, data: Vec::with_capacity(4096) }
    }
}

impl ReplyDirectory {
    /// Changes the max size of the directory buffer
    pub fn sized (mut self, size: usize) -> ReplyDirectory {
        self.size = size;
        self.data.reserve(size);
        self
    }

    /// Add an entry to the directory reply buffer. Returns true if the buffer is full.
    /// A transparent offset value can be provided for each entry. The kernel uses these
    /// value to request the next entries in further readdir calls
    pub fn add<T: AsOsStr> (&mut self, ino: u64, offset: u64, kind: FileType, name: T) -> bool {
        let name = name.as_os_str().as_bytes();
        let entlen = mem::size_of::<fuse_dirent>() + name.len();
        let entsize = (entlen + mem::size_of::<u64>() - 1) & !(mem::size_of::<u64>() - 1);  // 64bit align
        let padlen = entsize - entlen;
        if self.data.len() + entsize > self.data.capacity() { return true; }
        unsafe {
            let p = self.data.as_mut_ptr().offset(self.data.len() as isize);
            let pdirent: *mut fuse_dirent = mem::transmute(p);
            (*pdirent).ino = ino;
            (*pdirent).off = offset;
            (*pdirent).namelen = name.len() as u32;
            (*pdirent).typ = mode_from_kind_and_perm(kind, 0) >> 12;
            let p = p.offset(mem::size_of_val(&*pdirent) as isize);
            ptr::copy_nonoverlapping(p, name.as_ptr(), name.len());
            let p = p.offset(name.len() as isize);
            ptr::write_bytes(p, 0u8, padlen);
            let newlen = self.data.len() + entsize;
            self.data.set_len(newlen);
        }
        false
    }

    /// Reply to a request with the filled directory buffer
    pub fn ok (mut self) {
        self.reply.send(0, &[&self.data]);
    }

    /// Reply to a request with the given error code
    pub fn error (self, err: c_int) {
        self.reply.error(err);
    }
}


#[cfg(test)]
mod test {
    use std::thread;
    use std::sync::mpsc::channel;
    use time::Timespec;
    use super::as_bytes;
    use super::{Reply, ReplyRaw, ReplyEmpty, ReplyData, ReplyEntry, ReplyAttr, ReplyOpen};
    use super::{ReplyWrite, ReplyStatfs, ReplyCreate, ReplyLock, ReplyBmap, ReplyDirectory};
    #[cfg(target_os = "macos")]
    use super::ReplyXTimes;
    use {FileType, FileAttr};

    #[allow(dead_code)]
    struct Data { a: u8, b: u8, c: u16 }

    #[test]
    fn serialize_empty () {
        let data = ();
        as_bytes(&data, |bytes| {
            assert!(bytes.is_empty());
        });
    }

    #[test]
    fn serialize_slice () {
        let data: [u8; 4] = [0x12, 0x34, 0x56, 0x78];
        as_bytes(&data, |bytes| {
            assert_eq!(bytes, [[0x12, 0x34, 0x56, 0x78]]);
        });
    }

    #[test]
    fn serialize_struct () {
        let data = Data { a: 0x12, b: 0x34, c: 0x5678 };
        as_bytes(&data, |bytes| {
            assert_eq!(bytes, [[0x12, 0x34, 0x78, 0x56]]);
        });
    }

    #[test]
    fn serialize_tuple () {
        let data = (Data { a: 0x12, b: 0x34, c: 0x5678 }, Data { a: 0x9a, b: 0xbc, c: 0xdef0 });
        as_bytes(&data, |bytes| {
            assert_eq!(bytes, [[0x12, 0x34, 0x78, 0x56, 0x9a, 0xbc, 0xf0, 0xde]]);
        });
    }

    #[test]
    fn reply_raw () {
        let data = Data { a: 0x12, b: 0x34, c: 0x5678 };
        let reply: ReplyRaw<Data> = Reply::new(0xdeadbeef, |bytes| {
            assert_eq!(bytes, [
                vec![0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0xef, 0xbe, 0xad, 0xde, 0x00, 0x00, 0x00, 0x00],
                vec![0x12, 0x34, 0x78, 0x56],
            ]);
        });
        reply.ok(&data);
    }

    #[test]
    fn reply_error () {
        let reply: ReplyRaw<Data> = Reply::new(0xdeadbeef, |bytes| {
            assert_eq!(bytes, [
                vec![0x10, 0x00, 0x00, 0x00, 0xbe, 0xff, 0xff, 0xff,  0xef, 0xbe, 0xad, 0xde, 0x00, 0x00, 0x00, 0x00],
            ]);
        });
        reply.error(66);
    }

    #[test]
    fn reply_empty () {
        let reply: ReplyEmpty = Reply::new(0xdeadbeef, |bytes| {
            assert_eq!(bytes, [
                vec![0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0xef, 0xbe, 0xad, 0xde, 0x00, 0x00, 0x00, 0x00],
            ]);
        });
        reply.ok();
    }

    #[test]
    fn reply_data () {
        let reply: ReplyData = Reply::new(0xdeadbeef, |bytes| {
            assert_eq!(bytes, [
                vec![0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0xef, 0xbe, 0xad, 0xde, 0x00, 0x00, 0x00, 0x00],
                vec![0xde, 0xad, 0xbe, 0xef],
            ]);
        });
        reply.data(&[0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn reply_entry () {
        let reply: ReplyEntry = Reply::new(0xdeadbeef, |bytes| {
            if cfg!(target_os = "macos") {
                assert_eq!(bytes, [
                    vec![0x98, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0xef, 0xbe, 0xad, 0xde, 0x00, 0x00, 0x00, 0x00],
                    vec![0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0xaa, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x78, 0x56, 0x00, 0x00, 0x78, 0x56, 0x00, 0x00,  0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x22, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x33, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x78, 0x56, 0x00, 0x00, 0x78, 0x56, 0x00, 0x00,  0x78, 0x56, 0x00, 0x00, 0x78, 0x56, 0x00, 0x00,
                         0xa4, 0x81, 0x00, 0x00, 0x55, 0x00, 0x00, 0x00,  0x66, 0x00, 0x00, 0x00, 0x77, 0x00, 0x00, 0x00,
                         0x88, 0x00, 0x00, 0x00, 0x99, 0x00, 0x00, 0x00],
                ]);
            } else {
                assert_eq!(bytes, [
                    vec![0x88, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0xef, 0xbe, 0xad, 0xde, 0x00, 0x00, 0x00, 0x00],
                    vec![0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0xaa, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x78, 0x56, 0x00, 0x00, 0x78, 0x56, 0x00, 0x00,  0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x22, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x33, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x78, 0x56, 0x00, 0x00, 0x78, 0x56, 0x00, 0x00,
                         0x78, 0x56, 0x00, 0x00, 0xa4, 0x81, 0x00, 0x00,  0x55, 0x00, 0x00, 0x00, 0x66, 0x00, 0x00, 0x00,
                         0x77, 0x00, 0x00, 0x00, 0x88, 0x00, 0x00, 0x00],
                ]);
            }
        });
        let time = Timespec::new(0x1234, 0x5678);
        let attr = FileAttr { ino: 0x11, size: 0x22, blocks: 0x33, atime: time, mtime: time, ctime: time, crtime: time,
            kind: FileType::RegularFile, perm: 0o644, nlink: 0x55, uid: 0x66, gid: 0x77, rdev: 0x88, flags: 0x99 };
        reply.entry(&time, &attr, 0xaa);
    }

    #[test]
    fn reply_attr () {
        let reply: ReplyAttr = Reply::new(0xdeadbeef, |bytes| {
            if cfg!(target_os = "macos") {
                assert_eq!(bytes, [
                    vec![0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0xef, 0xbe, 0xad, 0xde, 0x00, 0x00, 0x00, 0x00],
                    vec![0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x78, 0x56, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x22, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x33, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x78, 0x56, 0x00, 0x00, 0x78, 0x56, 0x00, 0x00,
                         0x78, 0x56, 0x00, 0x00, 0x78, 0x56, 0x00, 0x00,  0xa4, 0x81, 0x00, 0x00, 0x55, 0x00, 0x00, 0x00,
                         0x66, 0x00, 0x00, 0x00, 0x77, 0x00, 0x00, 0x00,  0x88, 0x00, 0x00, 0x00, 0x99, 0x00, 0x00, 0x00],
                ]);
            } else {
                assert_eq!(bytes, [
                    vec![0x70, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0xef, 0xbe, 0xad, 0xde, 0x00, 0x00, 0x00, 0x00],
                    vec![0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x78, 0x56, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x22, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x33, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x78, 0x56, 0x00, 0x00, 0x78, 0x56, 0x00, 0x00,  0x78, 0x56, 0x00, 0x00, 0xa4, 0x81, 0x00, 0x00,
                         0x55, 0x00, 0x00, 0x00, 0x66, 0x00, 0x00, 0x00,  0x77, 0x00, 0x00, 0x00, 0x88, 0x00, 0x00, 0x00],
                ]);
            }
        });
        let time = Timespec::new(0x1234, 0x5678);
        let attr = FileAttr { ino: 0x11, size: 0x22, blocks: 0x33, atime: time, mtime: time, ctime: time, crtime: time,
            kind: FileType::RegularFile, perm: 0o644, nlink: 0x55, uid: 0x66, gid: 0x77, rdev: 0x88, flags: 0x99 };
        reply.attr(&time, &attr);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn reply_xtimes () {
        let reply: ReplyXTimes = Reply::new(0xdeadbeef, |bytes| {
            assert_eq!(bytes, [
                vec![0x28, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0xef, 0xbe, 0xad, 0xde, 0x00, 0x00, 0x00, 0x00],
                vec![0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                     0x78, 0x56, 0x00, 0x00, 0x78, 0x56, 0x00, 0x00],
            ]);
        });
        let time = Timespec::new(0x1234, 0x5678);
        reply.xtimes(time, time);
    }

    #[test]
    fn reply_open () {
        let reply: ReplyOpen = Reply::new(0xdeadbeef, |bytes| {
            assert_eq!(bytes, [
                vec![0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0xef, 0xbe, 0xad, 0xde, 0x00, 0x00, 0x00, 0x00],
                vec![0x22, 0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x33, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            ]);
        });
        reply.opened(0x1122, 0x33);
    }

    #[test]
    fn reply_write () {
        let reply: ReplyWrite = Reply::new(0xdeadbeef, |bytes| {
            assert_eq!(bytes, [
                vec![0x18, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0xef, 0xbe, 0xad, 0xde, 0x00, 0x00, 0x00, 0x00],
                vec![0x22, 0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            ]);
        });
        reply.written(0x1122);
    }

    #[test]
    fn reply_statfs () {
        let reply: ReplyStatfs = Reply::new(0xdeadbeef, |bytes| {
            assert_eq!(bytes, [
                vec![0x60, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0xef, 0xbe, 0xad, 0xde, 0x00, 0x00, 0x00, 0x00],
                vec![0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x22, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                     0x33, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x44, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                     0x55, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x66, 0x00, 0x00, 0x00, 0x77, 0x00, 0x00, 0x00,
                     0x88, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                     0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            ]);
        });
        reply.statfs(0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88);
    }

    #[test]
    fn reply_create () {
        let reply: ReplyCreate = Reply::new(0xdeadbeef, |bytes| {
            if cfg!(target_os = "macos") {
                assert_eq!(bytes, [
                    vec![0xa8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0xef, 0xbe, 0xad, 0xde, 0x00, 0x00, 0x00, 0x00],
                    vec![0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0xaa, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x78, 0x56, 0x00, 0x00, 0x78, 0x56, 0x00, 0x00,  0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x22, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x33, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x78, 0x56, 0x00, 0x00, 0x78, 0x56, 0x00, 0x00,  0x78, 0x56, 0x00, 0x00, 0x78, 0x56, 0x00, 0x00,
                         0xa4, 0x81, 0x00, 0x00, 0x55, 0x00, 0x00, 0x00,  0x66, 0x00, 0x00, 0x00, 0x77, 0x00, 0x00, 0x00,
                         0x88, 0x00, 0x00, 0x00, 0x99, 0x00, 0x00, 0x00,  0xbb, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0xcc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
                ]);
            } else {
                assert_eq!(bytes, [
                    vec![0x98, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0xef, 0xbe, 0xad, 0xde, 0x00, 0x00, 0x00, 0x00],
                    vec![0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0xaa, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x78, 0x56, 0x00, 0x00, 0x78, 0x56, 0x00, 0x00,  0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x22, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x33, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x78, 0x56, 0x00, 0x00, 0x78, 0x56, 0x00, 0x00,
                         0x78, 0x56, 0x00, 0x00, 0xa4, 0x81, 0x00, 0x00,  0x55, 0x00, 0x00, 0x00, 0x66, 0x00, 0x00, 0x00,
                         0x77, 0x00, 0x00, 0x00, 0x88, 0x00, 0x00, 0x00,  0xbb, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                         0xcc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
                ]);
            }
        });
        let time = Timespec::new(0x1234, 0x5678);
        let attr = FileAttr { ino: 0x11, size: 0x22, blocks: 0x33, atime: time, mtime: time, ctime: time, crtime: time,
            kind: FileType::RegularFile, perm: 0o644, nlink: 0x55, uid: 0x66, gid: 0x77, rdev: 0x88, flags: 0x99 };
        reply.created(&time, &attr, 0xaa, 0xbb, 0xcc);
    }

    #[test]
    fn reply_lock () {
        let reply: ReplyLock = Reply::new(0xdeadbeef, |bytes| {
            assert_eq!(bytes, [
                vec![0x28, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0xef, 0xbe, 0xad, 0xde, 0x00, 0x00, 0x00, 0x00],
                vec![0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x22, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                     0x33, 0x00, 0x00, 0x00, 0x44, 0x00, 0x00, 0x00],
            ]);
        });
        reply.locked(0x11, 0x22, 0x33, 0x44);
    }

    #[test]
    fn reply_bmap () {
        let reply: ReplyBmap = Reply::new(0xdeadbeef, |bytes| {
            assert_eq!(bytes, [
                vec![0x18, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0xef, 0xbe, 0xad, 0xde, 0x00, 0x00, 0x00, 0x00],
                vec![0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            ]);
        });
        reply.bmap(0x1234);
    }

    #[test]
    fn reply_directory () {
        let mut reply: ReplyDirectory = Reply::new(0xdeadbeef, |bytes| {
            assert_eq!(bytes, [
                vec![0x50, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0xef, 0xbe, 0xad, 0xde, 0x00, 0x00, 0x00, 0x00],
                vec![0xbb, 0xaa, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                     0x05, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00,  0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x00 ,0x00, 0x00,
                     0xdd, 0xcc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                     0x08, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00,  0x77, 0x6f, 0x72, 0x6c, 0x64, 0x2e, 0x72, 0x73],
            ]);
        });
        reply.add(0xaabb, 1, FileType::Directory, "hello");
        reply.add(0xccdd, 2, FileType::RegularFile, "world.rs");
        reply.ok();
    }

    #[test]
    fn async_reply () {
        let (tx, rx) = channel::<()>();
        let reply: ReplyEmpty = Reply::new(0xdeadbeef, move |_| {
            tx.send(()).unwrap();
        });
        thread::spawn(move || {
            reply.ok();
        });
        rx.recv().unwrap();
    }
}
