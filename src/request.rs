//! Filesystem operation request
//!
//! A request represents information about a filesystem operation the kernel driver wants us to
//! perform.
//!
//! TODO: This module is meant to go away soon in favor of `lowlevel::Request`.

use std::convert::TryFrom;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use libc::{EIO, ENOSYS, EPROTO};
use fuse_abi::*;
use fuse_abi::consts::*;
use log::{debug, error, warn};

use crate::channel::ChannelSender;
use crate::lowlevel;
use crate::reply::{Reply, ReplyEmpty, ReplyInit, ReplyDirectory};
use crate::session::{MAX_WRITE_SIZE, Session};
use crate::Filesystem;

/// We generally support async reads
#[cfg(not(target_os = "macos"))]
const INIT_FLAGS: u32 = FUSE_ASYNC_READ;
// TODO: Add FUSE_EXPORT_SUPPORT and FUSE_BIG_WRITES (requires ABI 7.10)

/// On macOS, we additionally support case insensitiveness, volume renames and xtimes
/// TODO: we should eventually let the filesystem implementation decide which flags to set
#[cfg(target_os = "macos")]
const INIT_FLAGS: u32 = FUSE_ASYNC_READ | FUSE_CASE_INSENSITIVE | FUSE_VOL_RENAME | FUSE_XTIMES;
// TODO: Add FUSE_EXPORT_SUPPORT and FUSE_BIG_WRITES (requires ABI 7.10)

/// Request data structure
#[derive(Debug)]
pub struct Request<'a> {
    /// Channel sender for sending the reply
    ch: ChannelSender,
    /// Request raw data
    data: &'a [u8],
    /// Parsed request
    request: lowlevel::Request<'a>,
}

impl<'a> Request<'a> {
    /// Create a new request from the given data
    pub fn new(ch: ChannelSender, data: &'a [u8]) -> Option<Request<'a>> {
        let request = match lowlevel::Request::try_from(data) {
            Ok(request) => request,
            Err(err) => {
                // FIXME: Reply with ENOSYS?
                error!("{}", err);
                return None;
            }
        };

        Some(Self { ch, data, request})
    }

    /// Dispatch request to the given filesystem.
    /// This calls the appropriate filesystem operation method for the
    /// request and sends back the returned reply to the kernel
    pub fn dispatch<FS: Filesystem>(&self, se: &mut Session<FS>) {
        debug!("{}", self.request);

        match self.request.operation() {
            // Filesystem initialization
            lowlevel::Operation::Init { arg } => {
                let reply: ReplyInit = self.reply();
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
                let (major, minor) = (FUSE_KERNEL_VERSION, FUSE_KERNEL_MINOR_VERSION);
                let max_readahead = arg.max_readahead;      // accept any readahead size
                let flags = arg.flags & INIT_FLAGS;         // use features given in INIT_FLAGS and reported as capable
                let max_write = MAX_WRITE_SIZE as u32;      // use a max write size that fits into the session's buffer
                debug!("INIT response: ABI {}.{}, flags {:#x}, max readahead {}, max write {}", major, minor, flags, max_readahead, max_write);
                se.initialized = true;
                reply.init(major, minor, max_readahead, flags, max_write);
            }
            // Any operation is invalid before initialization
            _ if !se.initialized => {
                warn!("Ignoring FUSE operation before init: {}", self.request);
                self.reply::<ReplyEmpty>().error(EIO);
            }
            // Filesystem destroyed
            lowlevel::Operation::Destroy => {
                se.filesystem.destroy(self);
                se.destroyed = true;
                self.reply::<ReplyEmpty>().ok();
            }
            // Any operation is invalid after destroy
            _ if se.destroyed => {
                warn!("Ignoring FUSE operation after destroy: {}", self.request);
                self.reply::<ReplyEmpty>().error(EIO);
            }

            lowlevel::Operation::Interrupt { .. } => {
                // TODO: handle FUSE_INTERRUPT
                self.reply::<ReplyEmpty>().error(ENOSYS);
            }

            lowlevel::Operation::Lookup { name } => {
                se.filesystem.lookup(self, self.request.nodeid(), &name, self.reply());
            }
            lowlevel::Operation::Forget { arg } => {
                se.filesystem.forget(self, self.request.nodeid(), arg.nlookup); // no reply
            }
            lowlevel::Operation::GetAttr => {
                se.filesystem.getattr(self, self.request.nodeid(), self.reply());
            }
            lowlevel::Operation::SetAttr { arg } => {
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
                let atime = match arg.valid & FATTR_ATIME {
                    0 => None,
                    _ => Some(UNIX_EPOCH + Duration::new(arg.atime, arg.atimensec)),
                };
                let mtime = match arg.valid & FATTR_MTIME {
                    0 => None,
                    _ => Some(UNIX_EPOCH + Duration::new(arg.mtime, arg.mtimensec)),
                };
                let fh = match arg.valid & FATTR_FH {
                    0 => None,
                    _ => Some(arg.fh),
                };
                #[cfg(target_os = "macos")]
                #[inline]
                fn get_macos_setattr(arg: &fuse_setattr_in) -> (Option<SystemTime>, Option<SystemTime>, Option<SystemTime>, Option<u32>) {
                    let crtime = match arg.valid & FATTR_CRTIME {
                        0 => None,
                        _ => Some(UNIX_EPOCH + Duration::new(arg.crtime, arg.crtimensec)),
                    };
                    let chgtime = match arg.valid & FATTR_CHGTIME {
                        0 => None,
                        _ => Some(UNIX_EPOCH + Duration::new(arg.chgtime, arg.chgtimensec)),
                    };
                    let bkuptime = match arg.valid & FATTR_BKUPTIME {
                        0 => None,
                        _ => Some(UNIX_EPOCH + Duration::new(arg.bkuptime, arg.bkuptimensec)),
                    };
                    let flags = match arg.valid & FATTR_FLAGS {
                        0 => None,
                        _ => Some(arg.flags),
                    };
                    (crtime, chgtime, bkuptime, flags)
                }
                #[cfg(not(target_os = "macos"))]
                #[inline]
                fn get_macos_setattr(_arg: &fuse_setattr_in) -> (Option<SystemTime>, Option<SystemTime>, Option<SystemTime>, Option<u32>) {
                    (None, None, None, None)
                }
                let (crtime, chgtime, bkuptime, flags) = get_macos_setattr(arg);
                se.filesystem.setattr(self, self.request.nodeid(), mode, uid, gid, size, atime, mtime, fh, crtime, chgtime, bkuptime, flags, self.reply());
            }
            lowlevel::Operation::ReadLink => {
                se.filesystem.readlink(self, self.request.nodeid(), self.reply());
            }
            lowlevel::Operation::MkNod { arg, name } => {
                se.filesystem.mknod(self, self.request.nodeid(), &name, arg.mode, arg.rdev, self.reply());
            }
            lowlevel::Operation::MkDir { arg, name } => {
                se.filesystem.mkdir(self, self.request.nodeid(), &name, arg.mode, self.reply());
            }
            lowlevel::Operation::Unlink { name } => {
                se.filesystem.unlink(self, self.request.nodeid(), &name, self.reply());
            }
            lowlevel::Operation::RmDir { name } => {
                se.filesystem.rmdir(self, self.request.nodeid(), &name, self.reply());
            }
            lowlevel::Operation::SymLink { name, link } => {
                se.filesystem.symlink(self, self.request.nodeid(), &name, &Path::new(link), self.reply());
            }
            lowlevel::Operation::Rename { arg, name, newname } => {
                se.filesystem.rename(self, self.request.nodeid(), &name, arg.newdir, &newname, self.reply());
            }
            lowlevel::Operation::Link { arg, name } => {
                se.filesystem.link(self, arg.oldnodeid, self.request.nodeid(), &name, self.reply());
            }
            lowlevel::Operation::Open { arg } => {
                se.filesystem.open(self, self.request.nodeid(), arg.flags, self.reply());
            }
            lowlevel::Operation::Read { arg } => {
                se.filesystem.read(self, self.request.nodeid(), arg.fh, arg.offset as i64, arg.size, self.reply());
            }
            lowlevel::Operation::Write { arg, data } => {
                assert!(data.len() == arg.size as usize);
                se.filesystem.write(self, self.request.nodeid(), arg.fh, arg.offset as i64, data, arg.write_flags, self.reply());
            }
            lowlevel::Operation::Flush { arg } => {
                se.filesystem.flush(self, self.request.nodeid(), arg.fh, arg.lock_owner, self.reply());
            }
            lowlevel::Operation::Release { arg } => {
                let flush = match arg.release_flags & FUSE_RELEASE_FLUSH {
                    0 => false,
                    _ => true,
                };
                se.filesystem.release(self, self.request.nodeid(), arg.fh, arg.flags, arg.lock_owner, flush, self.reply());
            }
            lowlevel::Operation::FSync { arg } => {
                let datasync = match arg.fsync_flags & 1 {
                    0 => false,
                    _ => true,
                };
                se.filesystem.fsync(self, self.request.nodeid(), arg.fh, datasync, self.reply());
            }
            lowlevel::Operation::OpenDir { arg } => {
                se.filesystem.opendir(self, self.request.nodeid(), arg.flags, self.reply());
            }
            lowlevel::Operation::ReadDir { arg } => {
                se.filesystem.readdir(self, self.request.nodeid(), arg.fh, arg.offset as i64, ReplyDirectory::new(self.request.unique(), self.ch, arg.size as usize));
            }
            lowlevel::Operation::ReleaseDir { arg } => {
                se.filesystem.releasedir(self, self.request.nodeid(), arg.fh, arg.flags, self.reply());
            }
            lowlevel::Operation::FSyncDir { arg } => {
                let datasync = match arg.fsync_flags & 1 {
                    0 => false,
                    _ => true,
                };
                se.filesystem.fsyncdir(self, self.request.nodeid(), arg.fh, datasync, self.reply());
            }
            lowlevel::Operation::StatFs => {
                se.filesystem.statfs(self, self.request.nodeid(), self.reply());
            }
            lowlevel::Operation::SetXAttr { arg, name, value } => {
                assert!(value.len() == arg.size as usize);
                #[cfg(target_os = "macos")]
                #[inline]
                fn get_position (arg: &fuse_setxattr_in) -> u32 { arg.position }
                #[cfg(not(target_os = "macos"))]
                #[inline]
                fn get_position (_arg: &fuse_setxattr_in) -> u32 { 0 }
                se.filesystem.setxattr(self, self.request.nodeid(), name, value, arg.flags, get_position(arg), self.reply());
            }
            lowlevel::Operation::GetXAttr { arg, name } => {
                se.filesystem.getxattr(self, self.request.nodeid(), name, arg.size, self.reply());
            }
            lowlevel::Operation::ListXAttr { arg } => {
                se.filesystem.listxattr(self, self.request.nodeid(), arg.size, self.reply());
            }
            lowlevel::Operation::RemoveXAttr { name } => {
                se.filesystem.removexattr(self, self.request.nodeid(), name, self.reply());
            }
            lowlevel::Operation::Access { arg } => {
                se.filesystem.access(self, self.request.nodeid(), arg.mask, self.reply());
            }
            lowlevel::Operation::Create { arg, name } => {
                se.filesystem.create(self, self.request.nodeid(), &name, arg.mode, arg.flags, self.reply());
            }
            lowlevel::Operation::GetLk { arg } => {
                se.filesystem.getlk(self, self.request.nodeid(), arg.fh, arg.owner, arg.lk.start, arg.lk.end, arg.lk.typ, arg.lk.pid, self.reply());
            }
            lowlevel::Operation::SetLk { arg } => {
                se.filesystem.setlk(self, self.request.nodeid(), arg.fh, arg.owner, arg.lk.start, arg.lk.end, arg.lk.typ, arg.lk.pid, false, self.reply());
            }
            lowlevel::Operation::SetLkW { arg } => {
                se.filesystem.setlk(self, self.request.nodeid(), arg.fh, arg.owner, arg.lk.start, arg.lk.end, arg.lk.typ, arg.lk.pid, true, self.reply());
            }
            lowlevel::Operation::BMap { arg } => {
                se.filesystem.bmap(self, self.request.nodeid(), arg.blocksize, arg.block, self.reply());
            }

            #[cfg(target_os = "macos")]
            lowlevel::Operation::SetVolName { name } => {
                se.filesystem.setvolname(self, name, self.reply());
            }
            #[cfg(target_os = "macos")]
            lowlevel::Operation::GetXTimes => {
                se.filesystem.getxtimes(self, self.request.nodeid(), self.reply());
            }
            #[cfg(target_os = "macos")]
            lowlevel::Operation::Exchange { arg, oldname, newname } => {
                se.filesystem.exchange(self, arg.olddir, &oldname, arg.newdir, &newname, arg.options, self.reply());
            }
        }
    }

    /// Create a reply object for this request that can be passed to the filesystem
    /// implementation and makes sure that a request is replied exactly once
    fn reply<T: Reply>(&self) -> T {
        Reply::new(self.request.unique(), self.ch)
    }

    /// Returns the unique identifier of this request
    #[inline]
    #[allow(dead_code)]
    pub fn unique(&self) -> u64 {
        self.request.unique()
    }

    /// Returns the uid of this request
    #[inline]
    #[allow(dead_code)]
    pub fn uid(&self) -> u32 {
        self.request.uid()
    }

    /// Returns the gid of this request
    #[inline]
    #[allow(dead_code)]
    pub fn gid(&self) -> u32 {
        self.request.gid()
    }

    /// Returns the pid of this request
    #[inline]
    #[allow(dead_code)]
    pub fn pid(&self) -> u32 {
        self.request.pid()
    }
}
