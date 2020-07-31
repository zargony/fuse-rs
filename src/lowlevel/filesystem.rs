//! Low-level filesystem interface.
//!
//! Interface for implementing low-level filesystems.

use std::ffi::OsStr;
use std::os::raw::c_int;
use std::path::Path;
use std::time::SystemTime;

use super::reply;
use super::request::Request;


/// Result type of filesystem handler methods.
///
/// On failure, a method can return an `errno` error code as defined in the `libc` crate,
/// e.g. `ENOENT` or `EIO`.
pub type Result<T> = std::result::Result<T, c_int>;


/// Low-level filesystem implementation trait.
///
/// This trait must be implemented to provide a userspace filesystem via FUSE. Reasonable default
/// implementations are provided here to get a mountable filesystem that does nothing.
///
/// These methods correspond to `fuse_lowlevel_ops` in libfuse.
//
// TODO: All methods should be async. Requires Rust support or depending on the `async-trait` crate.
pub trait Filesystem {
    /// Initialize filesystem.
    ///
    /// Called once before any other filesystem method.
    fn init(&mut self, _req: &Request<'_>) -> Result<()> {
        Ok(())
    }

    /// Clean up filesystem.
    ///
    /// Called on filesystem exit.
    //
    // FIXME: This method is non-idiomatic and should be removed. Filesystems can implement `Drop` instead.
    fn destroy(&mut self, _req: &Request<'_>) {}

    /// Look up a directory entry by name and get its attributes.
    fn lookup(&mut self, _req: &Request<'_>, _parent: u64, _name: &OsStr) -> Result<reply::Entry> {
        Err(libc::ENOSYS)
    }

    /// Forget about an inode.
    ///
    /// The `nlookup` parameter indicates the number of lookups previously performed on this inode.
    /// If the filesystem implements inode lifetimes, it is recommended that inodes acquire a
    /// single reference on each lookup, and lose `nlookup` references on each forget. The
    /// filesystem may ignore forget calls, if the inodes don't need to have a limited lifetime.
    ///
    /// On unmount it is not guaranteed, that all referenced inodes will receive a forget message.
    fn forget(&mut self, _req: &Request<'_>, _ino: u64, _nlookup: u64) {}

    /// Get file attributes.
    fn getattr(&mut self, _req: &Request<'_>, _ino: u64) -> Result<reply::Attr> {
        Err(libc::ENOSYS)
    }

    /// Set file attributes.
    #[allow(clippy::too_many_arguments)]
    fn setattr(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        _size: Option<u64>,
        _atime: Option<SystemTime>,
        _mtime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
    ) -> Result<reply::Attr> {
        Err(libc::ENOSYS)
    }

    /// Read the target of a symbolic link.
    //
    // TODO: Return type should be Result<PathBuf> or Result<AsRef<OsStr>> or similar
    fn readlink(&mut self, _req: &Request<'_>, _ino: u64) -> Result<reply::Data<'_>> {
        Err(libc::ENOSYS)
    }

    /// Create a file node.
    ///
    /// Create a regular file, character device, block device, fifo or socket node.
    fn mknod(
        &mut self,
        _req: &Request<'_>,
        _parent: u64,
        _name: &OsStr,
        _mode: u32,
        _rdev: u32,
    ) -> Result<reply::Entry> {
        Err(libc::ENOSYS)
    }

    /// Create a directory.
    fn mkdir(
        &mut self,
        _req: &Request<'_>,
        _parent: u64,
        _name: &OsStr,
        _mode: u32,
    ) -> Result<reply::Entry> {
        Err(libc::ENOSYS)
    }

    /// Remove a file.
    fn unlink(&mut self, _req: &Request<'_>, _parent: u64, _name: &OsStr) -> Result<()> {
        Err(libc::ENOSYS)
    }

    /// Remove a directory.
    fn rmdir(&mut self, _req: &Request<'_>, _parent: u64, _name: &OsStr) -> Result<()> {
        Err(libc::ENOSYS)
    }

    /// Create a symbolic link.
    fn symlink(
        &mut self,
        _req: &Request<'_>,
        _parent: u64,
        _name: &OsStr,
        _link: &Path,
    ) -> Result<reply::Entry> {
        Err(libc::ENOSYS)
    }

    /// Rename a file.
    fn rename(
        &mut self,
        _req: &Request<'_>,
        _parent: u64,
        _name: &OsStr,
        _newparent: u64,
        _newname: &OsStr,
    ) -> Result<()> {
        Err(libc::ENOSYS)
    }

    /// Create a hard link to a file.
    fn link(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _newparent: u64,
        _newname: &OsStr,
    ) -> Result<reply::Entry> {
        Err(libc::ENOSYS)
    }

    /// Open a file.
    ///
    /// Open flags (with the exception of O_CREAT, O_EXCL, O_NOCTTY and O_TRUNC) are available in
    /// `flags`. Filesystems may store an arbitrary file handle (pointer, index, etc) in `fh`, and
    /// use this in other all other file operations (`read`, `write`, `flush`, `release`, `fsync`).
    /// Filesystems may also implement stateless file I/O and not store anything in `fh`. There are
    /// also some flags (direct_io, keep_cache) which the filesystem may set, to change the way the
    /// file is opened. See `fuse_file_info` structure in <fuse_common.h> for more details.
    fn open(&mut self, _req: &Request<'_>, _ino: u64, _flags: u32) -> Result<reply::Open> {
        Ok(reply::Open::new(0, 0))
    }

    /// Read data from an open file.
    ///
    /// `read` should send exactly the number of bytes requested except on EOF or error, otherwise
    /// the rest of the data will be substituted with zeroes. An exception to this is when the file
    /// has been opened in 'direct_io' mode, in which case the return value of the read system call
    /// will reflect the return value of this operation. `fh` will contain the value set by the
    /// open method.
    fn read(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _offset: i64,
        _size: u32,
    ) -> Result<reply::Data<'_>> {
        Err(libc::ENOSYS)
    }

    /// Write data.
    ///
    /// `write` should return exactly the number of bytes requested except on error. An exception
    /// to this is when the file has been opened in 'direct_io' mode, in which case the return
    /// value of the write system call will reflect the return value of this operation. `fh` will
    /// contain the value set by the `open` method.
    fn write(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _offset: i64,
        _data: &[u8],
        _flags: u32,
    ) -> Result<u32> {
        Err(libc::ENOSYS)
    }

    /// Flush method.
    ///
    /// This is called on each close() of the opened file. Since file descriptors can be duplicated
    /// (dup, dup2, fork), for one `open` call there may be many `flush` calls. Filesystems
    /// shouldn't assume that `flush` will always be called after some writes, or that if will be
    /// called at all. `fh` will contain the value set by the `open` method.
    ///
    /// Note: the name of the method is misleading, since (unlike `fsync`) the filesystem is not
    /// forced to flush pending writes. One reason to flush data, is if the filesystem wants to
    /// return write errors. If the filesystem supports file locking operations (`setlk`, `getlk`)
    /// it should remove all locks belonging to `lock_owner`.
    fn flush(&mut self, _req: &Request<'_>, _ino: u64, _fh: u64, _lock_owner: u64) -> Result<()> {
        Err(libc::ENOSYS)
    }

    /// Release an open file.
    ///
    /// `release` is called when there are no more references to an open file: all file descriptors
    /// are closed and all memory mappings are unmapped. For every `open` call there will be
    /// exactly one release call. The filesystem may reply with an error, but error values are not
    /// returned to close() or munmap() which triggered the release. `fh` will contain the value
    /// set by the open method. `flags` will contain the same flags as for `open`.
    fn release(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _flags: u32,
        _lock_owner: u64,
        _flush: bool,
    ) -> Result<()> {
        Ok(())
    }

    /// Synchronize file contents.
    ///
    /// If the datasync parameter is non-zero, then only the user data should be flushed, not the
    /// meta data.
    fn fsync(&mut self, _req: &Request<'_>, _ino: u64, _fh: u64, _datasync: bool) -> Result<()> {
        Err(libc::ENOSYS)
    }

    /// Open a directory.
    ///
    /// Filesystem may store an arbitrary file handle (pointer, index, etc) in `fh`, and use this
    /// in other all other directory stream operations (`readdir`, `releasedir`, `fsyncdir`).
    /// Filesystem may also implement stateless directory I/O and not store anything in `fh`,
    /// though that makes it impossible to implement standard conforming directory stream
    /// operations in case the contents of the directory can change between opendir and releasedir.
    fn opendir(&mut self, _req: &Request<'_>, _ino: u64, _flags: u32) -> Result<reply::Open> {
        Ok(reply::Open::new(0, 0))
    }

    /// Read directory.
    ///
    /// `fh` will contain the value set by the `opendir` method.
    //
    // TODO: Encapsulate directory fill buffer as reply::Directory and use as return type here
    fn readdir(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _offset: i64,
    ) -> Result<reply::Data<'_>> {
        Err(libc::ENOSYS)
    }

    /// Release an open directory.
    ///
    /// For every `opendir` call there will be exactly one releasedir call. `fh` will contain the
    /// value set by the `opendir` method.
    fn releasedir(&mut self, _req: &Request<'_>, _ino: u64, _fh: u64, _flags: u32) -> Result<()> {
        Ok(())
    }

    /// Synchronize directory contents.
    ///
    /// If the `datasync` parameter is set, then only the directory contents should be flushed, not
    /// the meta data. `fh` will contain the value set by the `opendir` method.
    fn fsyncdir(&mut self, _req: &Request<'_>, _ino: u64, _fh: u64, _datasync: bool) -> Result<()> {
        Err(libc::ENOSYS)
    }

    /// Get file system statistics.
    fn statfs(&mut self, _req: &Request<'_>, _ino: u64) -> Result<reply::StatFs> {
        Ok(reply::StatFs::new(0, 0, 0, 0, 0, 512, 255, 0))
    }

    /// Set an extended attribute.
    fn setxattr(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _name: &OsStr,
        _value: &[u8],
        _flags: u32,
        _position: u32,
    ) -> Result<()> {
        Err(libc::ENOSYS)
    }

    /// Get an extended attribute.
    fn getxattr(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _name: &OsStr,
    ) -> Result<reply::Data<'_>> {
        Err(libc::ENOSYS)
    }

    /// List extended attribute names.
    //
    // TODO: Encapsulate xattr list buffer as reply::XAttrList and use as return type here
    fn listxattr(&mut self, _req: &Request<'_>, _ino: u64, _size: u32) -> Result<reply::Data<'_>> {
        Err(libc::ENOSYS)
    }

    /// Remove an extended attribute.
    fn removexattr(&mut self, _req: &Request<'_>, _ino: u64, _name: &OsStr) -> Result<()> {
        Err(libc::ENOSYS)
    }

    /// Check file access permissions.
    ///
    /// This will be called for the access() system call. If the 'default_permissions' mount option
    /// is given, this method is not called.
    ///
    /// This method is not called under Linux kernel versions 2.4.x.
    fn access(&mut self, _req: &Request<'_>, _ino: u64, _mask: u32) -> Result<()> {
        Err(libc::ENOSYS)
    }

    /// Create and open a file.
    ///
    /// If the file does not exist, first create it with the specified mode, and then open it. Open
    /// flags (with the exception of O_NOCTTY) are available in `flags`. Filesystem may store an
    /// arbitrary file handle (pointer, index, etc) in `fh`, and use this in other all other file
    /// operations (`read`, `write`, `flush`, `release`, `fsync`). There are also some flags
    /// (direct_io, keep_cache) which the filesystem may set, to change the way the file is opened.
    /// See `fuse_file_info` structure in <fuse_common.h> for more details. If this method is not
    /// implemented or under Linux kernel versions earlier than 2.6.15, the `mknod` and `open`
    /// methods will be called instead.
    fn create(
        &mut self,
        _req: &Request<'_>,
        _parent: u64,
        _name: &OsStr,
        _mode: u32,
        _flags: u32,
    ) -> Result<reply::Create> {
        Err(libc::ENOSYS)
    }

    /// Test for a POSIX file lock.
    #[allow(clippy::too_many_arguments)]
    fn getlk(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _owner: u64,
        _start: u64,
        _end: u64,
        _typ: u32,
        _pid: u32,
    ) -> Result<reply::Lock> {
        Err(libc::ENOSYS)
    }

    /// Acquire, modify or release a POSIX file lock.
    ///
    /// For POSIX threads (NPTL) there's a 1-1 relation between `pid` and `owner`, but otherwise
    /// this is not always the case. For checking lock ownership, 'owner' must be used.
    ///
    /// Note: if the locking methods are not implemented, the kernel will still allow file locking
    /// to work locally. Hence these are only interesting for network filesystems and similar.
    #[allow(clippy::too_many_arguments)]
    fn setlk(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _owner: u64,
        _start: u64,
        _end: u64,
        _typ: u32,
        _pid: u32,
        _sleep: bool,
    ) -> Result<()> {
        Err(libc::ENOSYS)
    }

    /// Map block index within file to block index within device.
    ///
    /// Note: This makes sense only for block device backed filesystems mounted with the 'blkdev'
    /// option.
    fn bmap(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _blocksize: u32,
        _idx: u64,
    ) -> Result<reply::Bmap> {
        Err(libc::ENOSYS)
    }

    /// macOS only: Rename the volume.
    #[cfg(target_os = "macos")]
    fn setvolname(&mut self, _req: &Request<'_>, _name: &OsStr) -> Result<()> {
        Err(libc::ENOSYS)
    }

    /// macOS only (undocumented)
    #[cfg(target_os = "macos")]
    fn exchange(
        &mut self,
        _req: &Request<'_>,
        _parent: u64,
        _name: &OsStr,
        _newparent: u64,
        _newname: &OsStr,
        _options: u64,
    ) -> Result<()> {
        Err(libc::ENOSYS)
    }

    /// macOS only: Query extended times.
    #[cfg(target_os = "macos")]
    fn getxtimes(&mut self, _req: &Request<'_>, _ino: u64) -> Result<reply::XTimes> {
        Err(libc::ENOSYS)
    }
}
