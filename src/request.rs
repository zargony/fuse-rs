//! Filesystem operation request
//!
//! A request represents information about a filesystem operation the kernel driver wants us to
//! perform.

use sys;
use std::mem;
use libc::{EIO, ENOSYS, EPROTO};
use time::Timespec;
use argument::ArgumentIterator;
use channel::ChannelSender;
use Filesystem;
use kernel::*;
use kernel::consts::*;
use kernel::fuse_opcode::*;
use reply::{Reply, ReplyRaw, ReplyEmpty, ReplyDirectory, ReplyDirectoryPlus, ReplyRead};
use session::{MAX_WRITE_SIZE, Session};
use std::os::unix::io::RawFd;
use std::slice;

/// We generally support async reads
#[cfg(not(target_os = "macos"))]
const INIT_FLAGS: u32 = FUSE_ASYNC_READ | FUSE_BIG_WRITES | FUSE_ATOMIC_O_TRUNC | FUSE_POSIX_ACL | FUSE_WRITEBACK_CACHE | FUSE_DONT_MASK;

/// On macOS, we additionally support case insensitiveness, volume renames and xtimes
/// TODO: we should eventually let the filesystem implementation decide which flags to set
#[cfg(target_os = "macos")]
const INIT_FLAGS: u32 = FUSE_ASYNC_READ | FUSE_EXPORT_SUPPORT | FUSE_BIG_WRITES | FUSE_CASE_INSENSITIVE | FUSE_VOL_RENAME | FUSE_XTIMES;

const PAGE_SIZE: usize = 4096;

/// Create a new request from the given buffer
pub fn request<'a>(ch: ChannelSender, buffer: &'a [u8]) -> Option<Request<'a>> {
    Request::new(ch, buffer)
}

pub fn request_splice<'a>(ch: ChannelSender, buffer: &'a mut Vec<u8>, pipe_fd: RawFd, size: usize) -> Option<Request<'a>> {
    Request::new_splice_write(ch, buffer, pipe_fd, size)
}

// replace by const, when const fn is stable
#[inline(always)]
pub fn request_write_header_size() -> usize {
    return mem::size_of::<fuse_in_header>() + mem::size_of::<fuse_write_in>();
}

// replace by const, when const fn is stable
#[inline(always)]
pub fn read_limit() -> usize {
    return request_write_header_size() + PAGE_SIZE;
}

/// Dispatch request to the given filesystem
pub fn dispatch<FS: Filesystem>(req: &Request, se: &mut Session<FS>, read_pipe_fd: RawFd, write_pipe_fd: RawFd) {
    req.dispatch(se, read_pipe_fd, write_pipe_fd);
}

/// Request data structure
#[derive(Debug)]
pub struct Request<'a> {
    /// Channel sender for sending the reply
    ch: ChannelSender,
    /// Header of the FUSE request
    header: &'a fuse_in_header,
    /// Operation-specific data payload
    data: &'a [u8],
    /// Pipe Fd in case of a write request
    source_fd: Option<RawFd>
}

/// A file timestamp.
#[derive(Clone, Copy, Debug, Hash, PartialEq)]
pub enum UtimeSpec {
#[cfg(target_os = "linux")]
    /// File timestamp is set to the current time.
    Now,
    /// The corresponding file timestamp is left unchanged.
    Omit,
    /// File timestamp is set to value
    Time(Timespec)
}

#[cfg(target_os = "linux")]
fn atime_to_timespec(arg: &fuse_setattr_in) -> UtimeSpec {
    if arg.valid & FATTR_ATIME_NOW != 0 {
        UtimeSpec::Now
    } else if arg.valid & FATTR_ATIME != 0 {
        UtimeSpec::Time(Timespec::new(arg.atime, arg.atimensec))
    } else {
        UtimeSpec::Omit
    }
}

#[cfg(target_os = "linux")]
fn mtime_to_timespec(arg: &fuse_setattr_in) -> UtimeSpec {
    if arg.valid & FATTR_MTIME_NOW != 0 {
        UtimeSpec::Now
    } else if arg.valid & FATTR_MTIME != 0 {
        UtimeSpec::Time(Timespec::new(arg.mtime, arg.mtimensec))
    } else {
        UtimeSpec::Omit
    }
}

#[cfg(not(target_os = "linux"))]
fn atime_to_timespec(arg: &fuse_setattr_in) -> UtimeSpec {
    if arg.valid & FATTR_ATIME != 0 {
        UtimeSpec::Time(Timespec::new(arg.atime, arg.atimensec))
    } else {
        UtimeSpec::Omit
    }
}

#[cfg(not(target_os = "linux"))]
fn mtime_to_timespec(arg: &fuse_setattr_in) -> UtimeSpec {
    if arg.valid & FATTR_MTIME != 0 {
        UtimeSpec::Time(Timespec::new(arg.mtime, arg.mtimensec))
    } else {
        UtimeSpec::Omit
    }
}

fn read_request<'a>(ch: ChannelSender, fd: RawFd, buffer: &'a mut [u8], offset: usize, request_size: usize) -> Option<Request<'a>> {
    assert!(buffer.len() > request_size);
    match sys::read(fd, &mut buffer[offset..]) {
        Ok(_size) => {
            let mut data = ArgumentIterator::new(&buffer[..request_size]);
            let req = Request {
                ch: ch,
                header: data.fetch(),
                data: data.fetch_data(),
                source_fd: None,
            };
            if request_size != req.header.len as usize {
                error!("Fuse request size does not match header length: expected: {}, got: {}", request_size, req.header.len);
                return None;
            }
            return Some(req);
        }
        Err(errno) => {
            error!("Error reading from FUSE pipe ({})", errno);
            return None;
        }
    }
}

impl<'a> Request<'a> {
    fn new_splice_write(ch: ChannelSender, buffer: &'a mut Vec<u8>, pipe_fd: RawFd, size: usize) -> Option<Request<'a>> {
        // Every request always begins with a fuse_in_header struct
        // followed by arbitrary data depending on which opcode it contains
        if size < mem::size_of::<fuse_in_header>() {
            error!("Read of FUSE request shorter then header ({} < {})", buffer.len(), mem::size_of::<fuse_in_header>());
            return None;
        }

        // optimisation: read smaller requests in one go
        if size < read_limit() {
            return read_request(ch, pipe_fd, buffer.as_mut_slice(), 0, size);
        }

        match sys::read(pipe_fd, &mut buffer[..request_write_header_size()]) {
            Ok(size) => {
                if size < request_write_header_size() {
                    error!("Short read of FUSE request: {} < {}", size, request_write_header_size());
                    return None;
                }
            }
            Err(errno) => {
                error!("Error reading from FUSE pipe ({})", errno);
                return None;
            }
        }

        let not_write_request = {
            let mut data = ArgumentIterator::new(buffer);
            let header: &fuse_in_header = data.fetch();
            match fuse_opcode::from_u32(header.opcode) {
                Some(opcode) => opcode != FUSE_WRITE,
                None => {
                    error!("Received invalid fuse opcode {}: discarding request", header.opcode);
                    true
                },
            }
        };

        if not_write_request {
            return read_request(ch, pipe_fd, buffer.as_mut_slice(), request_write_header_size(), size);
        }

        let mut data = ArgumentIterator::new(buffer);
        let req = Request {
            ch: ch,
            header: data.fetch(),
            data: data.fetch_data(),
            source_fd: Some(pipe_fd),
        };

        if size != req.header.len as usize {
            error!("Short read of in FUSE write request expected: {}, got: {}", req.header.len, buffer.len());
        }

        Some(req)
    }

    /// Create a new request from the given buffer
    fn new(ch: ChannelSender, buffer: &'a [u8]) -> Option<Request<'a>> {
        // Every request always begins with a fuse_in_header struct
        // followed by arbitrary data depending on which opcode it contains
        if buffer.len() < mem::size_of::<fuse_in_header>() {
            error!("Short read of FUSE request ({} < {})", buffer.len(), mem::size_of::<fuse_in_header>());
            return None;
        }
        let mut data = ArgumentIterator::new(buffer);
        let req = Request {
            ch: ch,
            header: data.fetch(),
            data: data.fetch_data(),
            source_fd: None,
        };
        if buffer.len() < req.header.len as usize {
            error!("Short read of FUSE request ({} < {})", buffer.len(), req.header.len);
            return None;
        }
        Some(req)
    }

    /// Dispatch request to the given filesystem.
    /// This calls the appropriate filesystem operation method for the
    /// request and sends back the returned reply to the kernel
    fn dispatch<FS: Filesystem>(&self, se: &mut Session<FS>, read_pipe_fd: RawFd, write_pipe_fd: RawFd) {
        let opcode = match fuse_opcode::from_u32(self.header.opcode) {
            Some(op) => op,
            None => {
                warn!("Ignoring unknown FUSE operation {}", self.header.opcode);
                self.reply::<ReplyEmpty>().error(ENOSYS);
                return;
            }
        };
        let mut data = ArgumentIterator::new(self.data);
        match opcode {
            // Filesystem initialization
            FUSE_INIT => {
                let reply: ReplyRaw<fuse_init_out> = self.reply();
                let arg: &fuse_init_in = data.fetch();
                debug!("INIT({})   kernel: ABI {}.{}, flags {:#x}, max readahead {}", self.header.unique, arg.major, arg.minor, arg.flags, arg.max_readahead);
                // We don't support ABI versions before 7.6
                if arg.major < 7 || (arg.major == 7 && arg.minor < 6) {
                    error!("Unsupported FUSE ABI version {}.{}", arg.major, arg.minor);
                    reply.error(EPROTO);
                    return;
                }
                // Remember ABI version supported by kernel
                se.proto_major = arg.major;
                se.proto_minor = arg.minor;
                // Call filesystem init method and give it a chance to return an error
                let res = se.filesystem.init(self);
                if let Err(err) = res {
                    reply.error(err);
                    return;
                }
                // Reply with our desired version and settings. If the kernel supports a
                // larger major version, it'll re-send a matching init message. If it
                // supports only lower major versions, we replied with an error above.
                let init = fuse_init_out {
                    major: FUSE_KERNEL_VERSION,
                    minor: FUSE_KERNEL_MINOR_VERSION,
                    max_readahead: arg.max_readahead,       // accept any readahead size
                    flags: arg.flags & INIT_FLAGS,          // use features given in INIT_FLAGS and reported as capable
                    max_background: 0,
                    congestion_threshold: 0,
                    max_write: MAX_WRITE_SIZE as u32,       // use a max write size that fits into the session's buffer
                    time_gran: 1,
                    reserved: [0; 9]
                };
                debug!("INIT({}) response: ABI {}.{}, flags {:#x}, max readahead {}, max write {}", self.header.unique, init.major, init.minor, init.flags, init.max_readahead, init.max_write);
                se.initialized = true;
                reply.ok(&init);
            }
            // Any operation is invalid before initialization
            _ if !se.initialized => {
                warn!("Ignoring FUSE operation {} before init", self.header.opcode);
                self.reply::<ReplyEmpty>().error(EIO);
            }
            // Filesystem destroyed
            FUSE_DESTROY => {
                debug!("DESTROY({})", self.header.unique);
                se.filesystem.destroy(self);
                se.destroyed = true;
                self.reply::<ReplyEmpty>().ok();
            }
            // Any operation is invalid after destroy
            _ if se.destroyed => {
                warn!("Ignoring FUSE operation {} after destroy", self.header.opcode);
                self.reply::<ReplyEmpty>().error(EIO);
            }

            FUSE_INTERRUPT => {
                let arg: &fuse_interrupt_in = data.fetch();
                debug!("INTERRUPT({}) unique {}", self.header.unique, arg.unique);
                // TODO: handle FUSE_INTERRUPT
                self.reply::<ReplyEmpty>().error(ENOSYS);
            }

            FUSE_LOOKUP => {
                let name = data.fetch_str();
                debug!("LOOKUP({}) parent {:#018x}, name {:?}", self.header.unique, self.header.nodeid, name);
                se.filesystem.lookup(self, self.header.nodeid, &name, self.reply());
            }
            FUSE_FORGET => {
                let arg: &fuse_forget_in = data.fetch();
                debug!("FORGET({}) ino {:#018x}, nlookup {}", self.header.unique, self.header.nodeid, arg.nlookup);
                se.filesystem.forget(self, self.header.nodeid, arg.nlookup); // no reply
            }
            FUSE_GETATTR => {
                debug!("GETATTR({}) ino {:#018x}", self.header.unique, self.header.nodeid);
                se.filesystem.getattr(self, self.header.nodeid, self.reply());
            }
            FUSE_SETATTR => {
                let arg: &fuse_setattr_in = data.fetch();
                debug!("SETATTR({}) ino {:#018x}, valid {:#x}", self.header.unique, self.header.nodeid, arg.valid);
                let mode = match arg.valid & FATTR_MODE {
                    0 => None,
                    _ => Some(arg.mode),
                };
                let uid = match arg.valid & FATTR_UID {
                    0 => None,
                    _ => Some(arg.uid),
                };
                let gid = match arg.valid & FATTR_GID {
                    0 => None,
                    _ => Some(arg.gid),
                };
                let size = match arg.valid & FATTR_SIZE {
                    0 => None,
                    _ => Some(arg.size),
                };
                let atime = atime_to_timespec(arg);
                let mtime = mtime_to_timespec(arg);
                let fh = match arg.valid & FATTR_FH {
                    0 => None,
                    _ => Some(arg.fh),
                };
                #[cfg(target_os = "macos")]
                #[inline]
                fn get_macos_setattr(arg: &fuse_setattr_in) -> (Option<Timespec>, Option<Timespec>, Option<Timespec>, Option<u32>) {
                    let crtime = match arg.valid & FATTR_CRTIME {
                        0 => None,
                        _ => Some(Timespec::new(arg.crtime, arg.crtimensec)),
                    };
                    let chgtime = match arg.valid & FATTR_CHGTIME {
                        0 => None,
                        _ => Some(Timespec::new(arg.chgtime, arg.chgtimensec)),
                    };
                    let bkuptime = match arg.valid & FATTR_BKUPTIME {
                        0 => None,
                        _ => Some(Timespec::new(arg.bkuptime, arg.bkuptimensec)),
                    };
                    let flags = match arg.valid & FATTR_FLAGS {
                        0 => None,
                        _ => Some(arg.flags),
                    };
                    (crtime, chgtime, bkuptime, flags)
                }
                #[cfg(not(target_os = "macos"))]
                #[inline]
                fn get_macos_setattr(_arg: &fuse_setattr_in) -> (Option<Timespec>, Option<Timespec>, Option<Timespec>, Option<u32>) {
                    (None, None, None, None)
                }
                let (crtime, chgtime, bkuptime, flags) = get_macos_setattr(arg);
                se.filesystem.setattr(self, self.header.nodeid, mode, uid, gid, size, atime, mtime, fh, crtime, chgtime, bkuptime, flags, self.reply());
            }
            FUSE_READLINK => {
                debug!("READLINK({}) ino {:#018x}", self.header.unique, self.header.nodeid);
                se.filesystem.readlink(self, self.header.nodeid, self.reply());
            }
            FUSE_MKNOD => {
                let arg: &fuse_mknod_in = data.fetch();
                let name = data.fetch_str();
                debug!("MKNOD({}) parent {:#018x}, name {:?}, mode {:#05o}, umask {:?}, rdev {}", self.header.unique, self.header.nodeid, name, arg.mode, arg.umask, arg.rdev);
                se.filesystem.mknod(self, self.header.nodeid, &name, arg.mode, arg.umask, arg.rdev, self.reply());
            }
            FUSE_MKDIR => {
                let arg: &fuse_mkdir_in = data.fetch();
                let name = data.fetch_str();
                debug!("MKDIR({}) parent {:#018x}, name {:?}, mode {:#05o}, umask {:?}", self.header.unique, self.header.nodeid, name, arg.mode, arg.umask);
                se.filesystem.mkdir(self, self.header.nodeid, &name, arg.mode, arg.umask, self.reply());
            }
            FUSE_UNLINK => {
                let name = data.fetch_str();
                debug!("UNLINK({}) parent {:#018x}, name {:?}", self.header.unique, self.header.nodeid, name);
                se.filesystem.unlink(self, self.header.nodeid, &name, self.reply());
            }
            FUSE_RMDIR => {
                let name = data.fetch_str();
                debug!("RMDIR({}) parent {:#018x}, name {:?}", self.header.unique, self.header.nodeid, name);
                se.filesystem.rmdir(self, self.header.nodeid, &name, self.reply());
            }
            FUSE_SYMLINK => {
                let name = data.fetch_str();
                let link = data.fetch_path();
                debug!("SYMLINK({}) parent {:#018x}, name {:?}, link {:?}", self.header.unique, self.header.nodeid, name, link);
                se.filesystem.symlink(self, self.header.nodeid, &name, &link, self.reply());
            }
            FUSE_RENAME => {
                let arg: &fuse_rename_in = data.fetch();
                let name = data.fetch_str();
                let newname = data.fetch_str();
                debug!("RENAME({}) parent {:#018x}, name {:?}, newparent {:#018x}, newname {:?}", self.header.unique, self.header.nodeid, name, arg.newdir, newname);
                se.filesystem.rename(self, self.header.nodeid, &name, arg.newdir, &newname, self.reply());
            }
            FUSE_RENAME2 => {
                let arg: &fuse_rename2_in = data.fetch();
                let name = data.fetch_str();
                let newname = data.fetch_str();
                debug!("RENAME2({}) parent {:#018x}, name {:?}, newparent {:#018x}, newname {:?}, flags {:?}", self.header.unique, self.header.nodeid, name, arg.newdir, newname, arg.flags);
                se.filesystem.rename2(self, self.header.nodeid, &name, arg.newdir, &newname, arg.flags, self.reply());
            }
            FUSE_LINK => {
                let arg: &fuse_link_in = data.fetch();
                let newname = data.fetch_str();
                debug!("LINK({}) ino {:#018x}, newparent {:#018x}, newname {:?}", self.header.unique, arg.oldnodeid, self.header.nodeid, newname);
                se.filesystem.link(self, arg.oldnodeid, self.header.nodeid, &newname, self.reply());
            }
            FUSE_OPEN => {
                let arg: &fuse_open_in = data.fetch();
                debug!("OPEN({}) ino {:#018x}, flags {:#x}", self.header.unique, self.header.nodeid, arg.flags);
                se.filesystem.open(self, self.header.nodeid, arg.flags, self.reply());
            }
            FUSE_READ => {
                let arg: &fuse_read_in = data.fetch();
                debug!("READ({}) ino {:#018x}, fh {}, offset {}, size {}", self.header.unique, self.header.nodeid, arg.fh, arg.offset, arg.size);
                let reply = ReplyRead::new(self.header.unique, self.ch, read_pipe_fd, write_pipe_fd);
                se.filesystem.read(self, self.header.nodeid, arg.fh, arg.offset, arg.size, reply);
            }
            FUSE_WRITE => {
                let arg: &fuse_write_in = data.fetch();
                let data = if self.source_fd.is_some() {
                    &[]
                } else {
                    data.fetch_data()
                };
                assert!(self.source_fd.is_some() || data.len() == arg.size as usize);

                debug!("WRITE({}) ino {:#018x}, fh {}, offset {}, size {}, flags {:#x}", self.header.unique, self.header.nodeid, arg.fh, arg.offset, arg.size, arg.write_flags);
                se.filesystem.write(self, self.header.nodeid, arg.fh, arg.offset, self.source_fd, data, arg.size, arg.write_flags, self.reply());
            }
            FUSE_FLUSH => {
                let arg: &fuse_flush_in = data.fetch();
                debug!("FLUSH({}) ino {:#018x}, fh {}, lock owner {}", self.header.unique, self.header.nodeid, arg.fh, arg.lock_owner);
                se.filesystem.flush(self, self.header.nodeid, arg.fh, arg.lock_owner, self.reply());
            }
            FUSE_RELEASE => {
                let arg: &fuse_release_in = data.fetch();
                let flush = match arg.release_flags & FUSE_RELEASE_FLUSH {
                    0 => false,
                    _ => true,
                };
                debug!("RELEASE({}) ino {:#018x}, fh {}, flags {:#x}, release flags {:#x}, lock owner {}", self.header.unique, self.header.nodeid, arg.fh, arg.flags, arg.release_flags, arg.lock_owner);
                se.filesystem.release(self, self.header.nodeid, arg.fh, arg.flags, arg.lock_owner, flush, self.reply());
            }
            FUSE_FSYNC => {
                let arg: &fuse_fsync_in = data.fetch();
                let datasync = match arg.fsync_flags & 1 {
                    0 => false,
                    _ => true,
                };
                debug!("FSYNC({}) ino {:#018x}, fh {}, flags {:#x}", self.header.unique, self.header.nodeid, arg.fh, arg.fsync_flags);
                se.filesystem.fsync(self, self.header.nodeid, arg.fh, datasync, self.reply());
            }
            FUSE_OPENDIR => {
                let arg: &fuse_open_in = data.fetch();
                debug!("OPENDIR({}) ino {:#018x}, flags {:#x}", self.header.unique, self.header.nodeid, arg.flags);
                se.filesystem.opendir(self, self.header.nodeid, arg.flags, self.reply());
            }
            FUSE_READDIR => {
                let arg: &fuse_read_in = data.fetch();
                debug!("READDIR({}) ino {:#018x}, fh {}, offset {}, size {}", self.header.unique, self.header.nodeid, arg.fh, arg.offset, arg.size);
                se.filesystem.readdir(self, self.header.nodeid, arg.fh, arg.offset, ReplyDirectory::new(self.header.unique, self.ch, arg.size as usize));
            }
            FUSE_READDIRPLUS => {
                let arg: &fuse_read_in = data.fetch();
                debug!("READDIRPLUS({}) ino {:#018x}, fh {}, offset {}, size {}", self.header.unique, self.header.nodeid, arg.fh, arg.offset, arg.size);
                se.filesystem.readdirplus(self, self.header.nodeid, arg.fh, arg.offset, ReplyDirectoryPlus::new(self.header.unique, self.ch, arg.size as usize));
            }
            FUSE_RELEASEDIR => {
                let arg: &fuse_release_in = data.fetch();
                debug!("RELEASEDIR({}) ino {:#018x}, fh {}, flags {:#x}, release flags {:#x}, lock owner {}", self.header.unique, self.header.nodeid, arg.fh, arg.flags, arg.release_flags, arg.lock_owner);
                se.filesystem.releasedir(self, self.header.nodeid, arg.fh, arg.flags, self.reply());
            }
            FUSE_FSYNCDIR => {
                let arg: &fuse_fsync_in = data.fetch();
                let datasync = match arg.fsync_flags & 1 { 0 => false, _ => true };
                debug!("FSYNCDIR({}) ino {:#018x}, fh {}, flags {:#x}", self.header.unique, self.header.nodeid, arg.fh, arg.fsync_flags);
                se.filesystem.fsyncdir(self, self.header.nodeid, arg.fh, datasync, self.reply());
            }
            FUSE_STATFS => {
                debug!("STATFS({}) ino {:#018x}", self.header.unique, self.header.nodeid);
                se.filesystem.statfs(self, self.header.nodeid, self.reply());
            }
            FUSE_SETXATTR => {
                let arg: &fuse_setxattr_in = data.fetch();
                let name = data.fetch_str();
                let value = data.fetch_data();
                debug!("value.len = {}, arg.size = {}", value.len(), arg.size);
                assert!(value.len() == arg.size as usize);
                debug!("SETXATTR({}) ino {:#018x}, name {:?}, size {}, flags {:#x}", self.header.unique, self.header.nodeid, name, arg.size, arg.flags);
                #[cfg(target_os = "macos")] #[inline]
                fn get_position (arg: &fuse_setxattr_in) -> u32 { arg.position }
                #[cfg(not(target_os = "macos"))] #[inline]
                fn get_position (_arg: &fuse_setxattr_in) -> u32 { 0 }
                se.filesystem.setxattr(self, self.header.nodeid, name, value, arg.flags, get_position(arg), self.reply());
            }
            FUSE_GETXATTR => {
                let arg: &fuse_getxattr_in = data.fetch();
                let name = data.fetch_str();
                debug!("GETXATTR({}) ino {:#018x}, name {:?}, size {}", self.header.unique, self.header.nodeid, name, arg.size);
                se.filesystem.getxattr(self, self.header.nodeid, name, arg.size, self.reply());
            }
            FUSE_LISTXATTR => {
                let arg: &fuse_getxattr_in = data.fetch();
                debug!("LISTXATTR({}) ino {:#018x}, size {}", self.header.unique, self.header.nodeid, arg.size);
                se.filesystem.listxattr(self, self.header.nodeid, arg.size, self.reply());
            }
            FUSE_REMOVEXATTR => {
                let name = data.fetch_str();
                debug!("REMOVEXATTR({}) ino {:#018x}, name {:?}", self.header.unique, self.header.nodeid, name);
                se.filesystem.removexattr(self, self.header.nodeid, name, self.reply());
            }
            FUSE_ACCESS => {
                let arg: &fuse_access_in = data.fetch();
                debug!("ACCESS({}) ino {:#018x}, mask {:#05o}", self.header.unique, self.header.nodeid, arg.mask);
                se.filesystem.access(self, self.header.nodeid, arg.mask, self.reply());
            }
            FUSE_CREATE => {
                let arg: &fuse_create_in = data.fetch();
                let name = data.fetch_str();
                debug!("CREATE({}) parent {:#018x}, name {:?}, mode {:#05o}, umask {:?}, flags {:#x}", self.header.unique, self.header.nodeid, name, arg.mode, arg.umask, arg.flags);
                se.filesystem.create(self, self.header.nodeid, &name, arg.mode, arg.umask, arg.flags, self.reply());
            }
            FUSE_GETLK => {
                let arg: &fuse_lk_in = data.fetch();
                debug!("GETLK({}) ino {:#018x}, fh {}, lock owner {}", self.header.unique, self.header.nodeid, arg.fh, arg.owner);
                se.filesystem.getlk(self, self.header.nodeid, arg.fh, arg.owner, arg.lk.start, arg.lk.end, arg.lk.typ, arg.lk.pid, self.reply());
            }
            FUSE_SETLK | FUSE_SETLKW => {
                let arg: &fuse_lk_in = data.fetch();
                let sleep = match opcode {
                    FUSE_SETLKW => true,
                    _ => false,
                };
                debug!("SETLK({}) ino {:#018x}, fh {}, lock owner {}", self.header.unique, self.header.nodeid, arg.fh, arg.owner);
                se.filesystem.setlk(self, self.header.nodeid, arg.fh, arg.owner, arg.lk.start, arg.lk.end, arg.lk.typ, arg.lk.pid, sleep, self.reply());
            }
            FUSE_BMAP => {
                let arg: &fuse_bmap_in = data.fetch();
                debug!("BMAP({}) ino {:#018x}, blocksize {}, ids {}", self.header.unique, self.header.nodeid, arg.blocksize, arg.block);
                se.filesystem.bmap(self, self.header.nodeid, arg.blocksize, arg.block, self.reply());
            },
            FUSE_IOCTL => {
                let arg: &fuse_ioctl_in = data.fetch();
                debug!("IOCTL({}) ino {:#018x}, fh {}, flags {}, cmd {}, in_size {}, out_size {}", self.header.unique, self.header.nodeid, arg.fh, arg.flags, arg.cmd, arg.in_size, arg.out_size);
                let in_data = if arg.in_size > 0 {
                    Some(data.fetch_data())
                } else {
                    None
                };
                if (arg.flags & FUSE_IOCTL_UNRESTRICTED) > 0 {
                    self.reply::<ReplyEmpty>().error(ENOSYS);
                } else {
                    se.filesystem.ioctl(self, self.header.nodeid, arg.fh, arg.flags, arg.cmd, in_data, arg.out_size, self.reply());
                }
            },
            FUSE_POLL => {
                let _arg: &fuse_poll_in = data.fetch();
                //debug!("IOCTL({}) ino {:#018x}, fh {}, flags {}, in_size {}, out_size {}", self.header.unique, self.header.nodeid, arg.fh, arg.in_size, arg.out_size);
                //se.filesystem.poll(self, self.header.nodeid, arg.fh, arg.flags, arg.in_size, arg.out_size, self.reply());
                self.reply::<ReplyEmpty>().error(ENOSYS);
            },
            FUSE_NOTIFY_REPLY => {
                let _arg: &fuse_notify_retrieve_in = data.fetch();
                self.reply::<ReplyEmpty>().error(ENOSYS);
            },
            FUSE_BATCH_FORGET => {
                let arg: &fuse_batch_forget_in = data.fetch();
                let data = data.fetch_data();
                assert!(data.len() / mem::size_of::<fuse_forget_data>() == arg.count as usize);

                let inodes : &[fuse_forget_data] = unsafe {
                    slice::from_raw_parts(data.as_ptr() as *const fuse_forget_data, arg.count as usize)
                };

                debug!("FUSE_BATCH_FORGET({}) count {}", self.header.unique, arg.count);
                se.filesystem.forget_multi(self, inodes); // no reply
            },
            FUSE_FALLOCATE => {
                let arg: &fuse_fallocate_in = data.fetch();
                debug!("FALLOCATE({}) ino {:#018x}, fh {}, offset {}, length {}, mode {}", self.header.unique, self.header.nodeid, arg.fh, arg.offset, arg.length, arg.mode);
                se.filesystem.fallocate(self, self.header.nodeid, arg.fh, arg.offset, arg.length, arg.mode, self.reply());
            },
            FUSE_LSEEK => {
                let arg: &fuse_lseek_in = data.fetch();
                debug!("LSEEK({}) fh {}, offset {}, whence {}", self.header.unique, arg.fh, arg.offset, arg.whence);
                se.filesystem.lseek(self, self.header.nodeid, arg.fh, arg.offset, arg.whence, self.reply());
            },
            #[cfg(target_os = "macos")]
            FUSE_SETVOLNAME => {
                let name = data.fetch_str();
                debug!("SETVOLNAME({}) name {:?}", self.header.unique, name);
                se.filesystem.setvolname(self, name, self.reply());
            }
            #[cfg(target_os = "macos")]
            FUSE_EXCHANGE => {
                let arg: &fuse_exchange_in = data.fetch();
                let oldname = data.fetch_str();
                let newname = data.fetch_str();
                debug!("EXCHANGE({}) parent {:#018x}, name {:?}, newparent {:#018x}, newname {:?}, options {:#x}", self.header.unique, arg.olddir, oldname, arg.newdir, newname, arg.options);
                se.filesystem.exchange(self, arg.olddir, &oldname, arg.newdir, &newname, arg.options, self.reply());
            }
            #[cfg(target_os = "macos")]
            FUSE_GETXTIMES => {
                debug!("GETXTIMES({}) ino {:#018x}", self.header.unique, self.header.nodeid);
                se.filesystem.getxtimes(self, self.header.nodeid, self.reply());
            },
            CUSE_INIT => {
                let _arg: &cuse_init_in = data.fetch();
                self.reply::<ReplyEmpty>().error(ENOSYS);
            },
        }
    }

    /// Create a reply object for this request that can be passed to the filesystem
    /// implementation and makes sure that a request is replied exactly once
    fn reply<T: Reply>(&self) -> T {
        Reply::new(self.header.unique, self.ch)
    }

    /// Returns the unique identifier of this request
    #[inline]
    #[allow(dead_code)]
    pub fn unique(&self) -> u64 {
        self.header.unique
    }

    /// Returns the uid of this request
    #[inline]
    #[allow(dead_code)]
    pub fn uid(&self) -> u32 {
        self.header.uid
    }

    /// Returns the gid of this request
    #[inline]
    #[allow(dead_code)]
    pub fn gid(&self) -> u32 {
        self.header.gid
    }

    /// Returns the pid of this request
    #[inline]
    #[allow(dead_code)]
    pub fn pid(&self) -> u32 {
        self.header.pid
    }
}
