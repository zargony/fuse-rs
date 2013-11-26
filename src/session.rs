/*!
 * A session is established with the kernel driver while a userspace
 * filesystem is mounted. The session connects to the kernel driver and
 * runs a loop that receives, dispatches and replies kernel requests.
 */

use std::{mem, task, vec};
use std::libc::{dev_t, c_int, mode_t, off_t, size_t};
use std::libc::{EIO, ENOSYS, EPROTO, ERANGE, EAGAIN, EINTR, ENODEV, ENOENT};
use native::*;
use native::consts::*;
use sendable::{Sendable,DirBuffer};
use channel;
use channel::{FusePort,FuseChan};
use Filesystem;
use argument::ArgumentIterator;
use super::logstr;
use request::{Request, new_request, new_request_with_handler, send_reply};

#[cfg(target_os = "macos")]
/// We support async reads and our filesystems are usually case-insensitive
/// TODO: should case sensitivity be an option passable by the implementing FS?
static INIT_FLAGS: u32 = FUSE_ASYNC_READ | FUSE_CASE_INSENSITIVE;

#[cfg(not(target_os = "macos"))]
/// We support async reads
static INIT_FLAGS: u32 = FUSE_ASYNC_READ;

/// The session data structure
pub struct Session<FS> {
	filesystem: FS,
	mountpoint: Path,
	port: FusePort,
	chan: FuseChan,
	proto_major: uint,
	proto_minor: uint,
	initialized: bool,
	destroyed: bool,
}

impl<FS: Filesystem+Send> Session<FS> {
	/// Mount the given filesystem to the given mountpoint
	pub fn mount (filesystem: FS, mountpoint: &Path, options: &[&[u8]]) -> Session<FS> {
		info!("Mounting {}", mountpoint.display());
		let (fport, fchan) = channel::mount(mountpoint, options).expect("unable to mount filesystem");
		Session {
			filesystem: filesystem,
			mountpoint: mountpoint.clone(),
			chan: fchan,
			port: fport,
			proto_major: 0,
			proto_minor: 0,
			initialized: false,
			destroyed: false,
		}
	}

	/// Run the session loop that receives, dispatches and replies to kernel requests.
	/// Make sure to run it on a new single threaded scheduler since the I/O in the
	/// session loop can block.
	pub fn run (&mut self) {
		let mut data:~[u8] = vec::with_capacity(channel::BUFFER_SIZE);
		loop {
			match self.port.read(&mut data) {
				Err(ENOENT) => continue,		// Operation interrupted. Accordingly to FUSE, this is safe to retry
				Err(EINTR) => continue,			// Interrupted system call, retry
				Err(EAGAIN) => continue,		// Explicitly try again
				Err(ENODEV) => break,			// Filesystem was unmounted, quit the loop
				Err(err) => fail!("Lost connection to FUSE device. Error {:i}", err),
				Ok(_) => self.dispatch(data),
			}
		}
	}

	/// Start the session loop in a background task
	pub fn start (self) -> BackgroundSession {
		BackgroundSession::start(self)
	}

	/// Dispatch request to the given filesystem.
	/// This parses a previously read request, calls the appropriate
	/// filesystem operation method and sends back the returned reply
	/// to the kernel
	fn dispatch(&mut self, data_vec:&mut [u8]) {
		// Every request begins with a fuse_in_header struct followed by arbitrary
		// data depending on which opcode it contains
		assert!(data_vec.len() >= mem::size_of::<fuse_in_header>());
		let mut data = ArgumentIterator::new(data_vec);
		let header: &fuse_in_header = data.fetch();
		let opcode: fuse_opcode = match FromPrimitive::from_u32(header.opcode) {
			Some(op) => op,
			None => {
				warn!("Ignoring unknown FUSE operation {:u}", header.opcode)
				self.send_reply_error(header.unique, ENOSYS);
				return;
			},
		};
		match opcode {
			// Filesystem initialization
			FUSE_INIT => {
				let arg: &fuse_init_in = data.fetch();
				debug!("INIT({:u})   kernel: ABI {:u}.{:u}, flags {:#x}, max readahead {:u}", header.unique, arg.major, arg.minor, arg.flags, arg.max_readahead);
				// We don't support ABI versions before 7.6
				if arg.major < 7 || (arg.major < 7 && arg.minor < 6) {
					error!("Unsupported FUSE ABI version {:u}.{:u}", arg.major, arg.minor);
					self.send_reply_error(header.unique, EPROTO);
					return;
				}
				// Remember ABI version supported by kernel
				self.proto_major = arg.major as uint;
				self.proto_minor = arg.minor as uint;
				// Call filesystem init method and give it a chance to return an error
				let res = self.filesystem.init();
				if res.is_err() {
					self.send_reply_error(header.unique, res.unwrap_err());
					return;
				}
				// Reply with our desired version and settings. If the kernel supports a
				// larger major version, it'll re-send a matching init message. If it
				// supports only lower major versions, we replied with an error above.
				let reply = fuse_init_out {
					major: FUSE_KERNEL_VERSION,
					minor: FUSE_KERNEL_MINOR_VERSION,
					max_readahead: arg.max_readahead,
					flags: INIT_FLAGS,
					unused: 0,
					max_write: channel::MAX_WRITE_SIZE,
				};
				debug!("INIT({:u}) response: ABI {:u}.{:u}, flags {:#x}, max readahead {:u}, max write {:u}", header.unique, reply.major, reply.minor, reply.flags, reply.max_readahead, reply.max_write);
				self.initialized = true;
				self.send_reply(header.unique, Ok(reply));
			},
			// Any operation is invalid before initialization
			_ if !self.initialized => {
				warn!("Ignoring FUSE operation {:u} before init", header.opcode);
				self.send_reply_error(header.unique, EIO);
			},
			// Filesystem destroyed
			FUSE_DESTROY => {
				debug!("DESTROY({:u})", header.unique);
				self.filesystem.destroy();
				self.destroyed = true;
				self.send_reply(header.unique, Ok(()));
			},
			// Any operation is invalid after destroy
			_ if self.destroyed => {
				warn!("Ignoring FUSE operation {:u} after destroy", header.opcode);
				self.send_reply_error(header.unique, EIO);
			}

			FUSE_INTERRUPT => {
				let arg: &fuse_interrupt_in = data.fetch();
				debug!("INTERRUPT({:u}) unique {:u}", header.unique, arg.unique);
				self.filesystem.interrupt(header.unique);
			},

			FUSE_LOOKUP => {
				let name = data.fetch_str();
				debug!("LOOKUP({:u}) parent {:#018x}, name {:s}", header.unique, header.nodeid, logstr(name));
				let req = self.new_request(header);
				self.filesystem.lookup(req, header.nodeid, name);
			},
			FUSE_FORGET => {
				let arg: &fuse_forget_in = data.fetch();
				debug!("FORGET({:u}) ino {:#018x}, nlookup {:u}", header.unique, header.nodeid, arg.nlookup);
				self.filesystem.forget(header.nodeid, arg.nlookup as uint);	// no reply
			},
			FUSE_GETATTR => {
				debug!("GETATTR({:u}) ino {:#018x}", header.unique, header.nodeid);
				let req = self.new_request(header);
				self.filesystem.getattr(req, header.nodeid);
			},
			FUSE_SETATTR => {
				let arg: &fuse_setattr_in = data.fetch();
				debug!("SETATTR({:u}) ino {:#018x}, valid {:#x}", header.unique, header.nodeid, arg.valid);
				let req = self.new_request(header);
				self.filesystem.setattr(req, header.nodeid, arg);
			},
			FUSE_READLINK => {
				debug!("READLINK({:u}) ino {:#018x}", header.unique, header.nodeid);
				let req = self.new_request(header);
				self.filesystem.readlink(req, header.nodeid);
			},
			FUSE_MKNOD => {
				let arg: &fuse_mknod_in = data.fetch();
				let name = data.fetch_str();
				debug!("MKNOD({:u}) parent {:#018x}, name {:s}, mode {:#05o}, rdev {:u}", header.unique, header.nodeid, logstr(name), arg.mode, arg.rdev);
				let req = self.new_request(header);
				self.filesystem.mknod(req, header.nodeid, name, arg.mode as mode_t, arg.rdev as dev_t);
			},
			FUSE_MKDIR => {
				let arg: &fuse_mkdir_in = data.fetch();
				let name = data.fetch_str();
				debug!("MKDIR({:u}) parent {:#018x}, name {:s}, mode {:#05o}", header.unique, header.nodeid, logstr(name), arg.mode);
				let req = self.new_request(header);
				self.filesystem.mkdir(req, header.nodeid, name, arg.mode as mode_t);
			},
			FUSE_UNLINK => {
				let name = data.fetch_str();
				debug!("UNLINK({:u}) parent {:#018x}, name {:s}", header.unique, header.nodeid, logstr(name));
				let req = self.new_request(header);
				self.filesystem.unlink(req, header.nodeid, name);
			},
			FUSE_RMDIR => {
				let name = data.fetch_str();
				debug!("RMDIR({:u}) parent {:#018x}, name {:s}", header.unique, header.nodeid, logstr(name));
				let req = self.new_request(header);
				self.filesystem.rmdir(req, header.nodeid, name);
			},
			FUSE_SYMLINK => {
				let name = data.fetch_str();
				let link = data.fetch_str();
				debug!("SYMLINK({:u}) parent {:#018x}, name {:s}, link {:s}", header.unique, header.nodeid, logstr(name), logstr(link));
				let req = self.new_request(header);
				self.filesystem.symlink(req, header.nodeid, name, link);
			},
			FUSE_RENAME => {
				let arg: &fuse_rename_in = data.fetch();
				let name = data.fetch_str();
				let newname = data.fetch_str();
				debug!("RENAME({:u}) parent {:#018x}, name {:s}, newparent {:#018x}, newname {:s}", header.unique, header.nodeid, logstr(name), arg.newdir, logstr(newname));
				let req = self.new_request(header);
				self.filesystem.rename(req, header.nodeid, name, arg.newdir, newname);
			},
			FUSE_LINK => {
				let arg: &fuse_link_in = data.fetch();
				let newname = data.fetch_str();
				debug!("LINK({:u}) ino {:#018x}, newparent {:#018x}, newname {:s}", header.unique, arg.oldnodeid, header.nodeid, logstr(newname));
				let req = self.new_request(header);
				self.filesystem.link(req, arg.oldnodeid, header.nodeid, newname);
			},
			FUSE_OPEN => {
				let arg: &fuse_open_in = data.fetch();
				debug!("OPEN({:u}) ino {:#018x}, flags {:#x}", header.unique, header.nodeid, arg.flags);
				let req = self.new_request(header);
				self.filesystem.open(req, header.nodeid, arg.flags as uint);
			},
			FUSE_READ => {
				let arg: &fuse_read_in = data.fetch();
				debug!("READ({:u}) ino {:#018x}, fh {:u}, offset {:u}, size {:u}", header.unique, header.nodeid, arg.fh, arg.offset, arg.size);
				let req = self.new_request(header);
				self.filesystem.read(req, header.nodeid, arg.fh, arg.offset as off_t, arg.size as size_t);
			},
			FUSE_WRITE => {
				let arg: &fuse_write_in = data.fetch();
				let data = data.fetch_data();
				assert!(data.len() == arg.size as uint);
				debug!("WRITE({:u}) ino {:#018x}, fh {:u}, offset {:u}, size {:u}, flags {:#x}", header.unique, header.nodeid, arg.fh, arg.offset, arg.size, arg.write_flags);
				let req = do new_request_with_handler::<size_t>(self.chan.clone(), header.unique) |chan, unique, result| {
					send_reply(chan, unique, result.map(|written| ~fuse_write_out { size: written as u32, padding: 0 }))
				};
				self.filesystem.write(req, header.nodeid, arg.fh, arg.offset as off_t, data, arg.write_flags as uint);
			},
			FUSE_FLUSH => {
				let arg: &fuse_flush_in = data.fetch();
				debug!("FLUSH({:u}) ino {:#018x}, fh {:u}, lock owner {:u}", header.unique, header.nodeid, arg.fh, arg.lock_owner);
				let req = self.new_request(header);
				self.filesystem.flush(req, header.nodeid, arg.fh, arg.lock_owner);
			},
			FUSE_RELEASE => {
				let arg: &fuse_release_in = data.fetch();
				let flush = match arg.release_flags & FUSE_RELEASE_FLUSH { 0 => false, _ => true };
				debug!("RELEASE({:u}) ino {:#018x}, fh {:u}, flags {:#x}, release flags {:#x}, lock owner {:u}", header.unique, header.nodeid, arg.fh, arg.flags, arg.release_flags, arg.lock_owner);
				let req = self.new_request(header);
				self.filesystem.release(req, header.nodeid, arg.fh, arg.flags as uint, arg.lock_owner, flush);
			},
			FUSE_FSYNC => {
				let arg: &fuse_fsync_in = data.fetch();
				let datasync = match arg.fsync_flags & 1 { 0 => false, _ => true };
				debug!("FSYNC({:u}) ino {:#018x}, fh {:u}, flags {:#x}", header.unique, header.nodeid, arg.fh, arg.fsync_flags);
				let req = self.new_request(header);
				self.filesystem.fsync(req, header.nodeid, arg.fh, datasync);
			},
			FUSE_OPENDIR => {
				let arg: &fuse_open_in = data.fetch();
				debug!("OPENDIR({:u}) ino {:#018x}, flags {:#x}", header.unique, header.nodeid, arg.flags);
				let req = self.new_request(header);
				self.filesystem.opendir(req, header.nodeid, arg.flags as uint);
			},
			FUSE_READDIR => {
				let arg: &fuse_read_in = data.fetch();
				debug!("READDIR({:u}) ino {:#018x}, fh {:u}, offset {:u}, size {:u}", header.unique, header.nodeid, arg.fh, arg.offset, arg.size);
				let req = self.new_request(header);
				self.filesystem.readdir(req, header.nodeid, arg.fh, arg.offset as off_t, DirBuffer::new(arg.size as uint));
			},
			FUSE_RELEASEDIR => {
				let arg: &fuse_release_in = data.fetch();
				debug!("RELEASEDIR({:u}) ino {:#018x}, fh {:u}, flags {:#x}, release flags {:#x}, lock owner {:u}", header.unique, header.nodeid, arg.fh, arg.flags, arg.release_flags, arg.lock_owner);
				let req = self.new_request(header);
				self.filesystem.releasedir(req, header.nodeid, arg.fh, arg.flags as uint);
			},
			FUSE_FSYNCDIR => {
				let arg: &fuse_fsync_in = data.fetch();
				let datasync = match arg.fsync_flags & 1 { 0 => false, _ => true };
				debug!("FSYNCDIR({:u}) ino {:#018x}, fh {:u}, flags {:#x}", header.unique, header.nodeid, arg.fh, arg.fsync_flags);
				let req = self.new_request(header);
				self.filesystem.fsyncdir(req, header.nodeid, arg.fh, datasync);
			},
			FUSE_STATFS => {
				debug!("STATFS({:u}) ino {:#018x}", header.unique, header.nodeid);
				let req = self.new_request(header);
				self.filesystem.statfs(req, header.nodeid);
			},
			FUSE_SETXATTR => {
				let arg: &fuse_setxattr_in = data.fetch();
				let name = data.fetch_str();
				let value = data.fetch_data();
				assert!(value.len() == arg.size as uint);
				debug!("SETXATTR({:u}) ino {:#018x}, name {:s}, size {:u}, flags {:#x}", header.unique, header.nodeid, logstr(name), arg.size, arg.flags);
				#[cfg(target_os = "macos")]
				fn get_position(arg: &fuse_setxattr_in) -> off_t { arg.position as off_t }
				#[cfg(not(target_os = "macos"))]
				fn get_position(_arg: &fuse_setxattr_in) -> off_t { 0 }
				let req = self.new_request(header);
				self.filesystem.setxattr(req, header.nodeid, name, value, arg.flags as uint, get_position(arg));
			},
			FUSE_GETXATTR => {
				let arg: &fuse_getxattr_in = data.fetch();
				let name = data.fetch_str();
				debug!("GETXATTR({:u}) ino {:#018x}, name {:s}, size {:u}", header.unique, header.nodeid, logstr(name), arg.size);
				let size = arg.size as uint;
				let req = do new_request_with_handler::<~[u8]>(self.chan.clone(), header.unique) |chan, unique, result| {
					if size == 0 {
						send_reply(chan, unique, result.map(|value| fuse_getxattr_out { size: value.len() as u32, padding: 0 }));
					} else {
						let reply_result = do result.and_then |value| { 
							if value.len() > size {
								Err(ERANGE)
							} else { Ok(value) }
						};
						send_reply(chan, unique, reply_result);
					}
				};
				self.filesystem.getxattr(req, header.nodeid, name);
			},
			FUSE_LISTXATTR => {
				let arg: &fuse_getxattr_in = data.fetch();
				debug!("LISTXATTR({:u}) ino {:#018x}, size {:u}", header.unique, header.nodeid, arg.size);
				let req = do new_request_with_handler::<~[&[u8]]>(self.chan.clone(), header.unique) |chan, unique, _list| {
					// TODO: If arg.size is zero, the size of the attribute list should be sent with fuse_getxattr_out
					// TODO: If arg.size is non-zero, send the attribute list if it fits, or ERANGE otherwise
					send_reply(chan, unique, Err::<(),c_int>(ENOSYS))
				};
				self.filesystem.listxattr(req, header.nodeid);
			},
			FUSE_REMOVEXATTR => {
				let name = data.fetch_str();
				debug!("REMOVEXATTR({:u}) ino {:#018x}, name {:s}", header.unique, header.nodeid, logstr(name));
				let req = self.new_request(header);
				self.filesystem.removexattr(req, header.nodeid, name);
			},
			FUSE_ACCESS => {
				let arg: &fuse_access_in = data.fetch();
				debug!("ACCESS({:u}) ino {:#018x}, mask {:#05o}", header.unique, header.nodeid, arg.mask);
				let req = self.new_request(header);
				self.filesystem.access(req, header.nodeid, arg.mask as uint);
			},
			FUSE_CREATE => {
				let arg: &fuse_open_in = data.fetch();
				let name = data.fetch_str();
				debug!("CREATE({:u}) parent {:#018x}, name {:s}, mode {:#05o}, flags {:#x}", header.unique, header.nodeid, logstr(name), arg.mode, arg.flags);
				let req = self.new_request(header);
				self.filesystem.create(req, header.nodeid, name, arg.mode as mode_t, arg.flags as uint);
			},
			FUSE_GETLK => {
				let arg: &fuse_lk_in = data.fetch();
				debug!("GETLK({:u}) ino {:#018x}, fh {:u}, lock owner {:u}", header.unique, header.nodeid, arg.fh, arg.owner);
				let req = self.new_request(header);
				self.filesystem.getlk(req, header.nodeid, arg.fh, arg.owner, &arg.lk);
			},
			FUSE_SETLK | FUSE_SETLKW => {
				let arg: &fuse_lk_in = data.fetch();
				let sleep = match opcode { FUSE_SETLKW => true, _ => false };
				debug!("SETLK({:u}) ino {:#018x}, fh {:u}, lock owner {:u}", header.unique, header.nodeid, arg.fh, arg.owner);
				let req = self.new_request(header);
				self.filesystem.setlk(req, header.nodeid, arg.fh, arg.owner, &arg.lk, sleep);
			},
			FUSE_BMAP => {
				let arg: &fuse_bmap_in = data.fetch();
				debug!("BMAP({:u}) ino {:#018x}, blocksize {:u}, ids {:u}", header.unique, header.nodeid, arg.blocksize, arg.block);
				let req = self.new_request(header);
				self.filesystem.bmap(req, header.nodeid, arg.blocksize as size_t, arg.block);
			},
            // OS X only
            FUSE_SETVOLNAME | FUSE_EXCHANGE | FUSE_GETXTIMES => self.dispatch_macos_only(opcode, header, &mut data),
        }
    }

    /// Handle OS X operation
    #[cfg(target_os = "macos")]
    fn dispatch_macos_only(&mut self, opcode: fuse_opcode, header: &fuse_in_header, data: &mut ArgumentIterator) {
        match opcode {
			FUSE_SETVOLNAME => {
				let name = data.fetch_str();
				debug!("SETVOLNAME({:u}) name {:s}", header.unique, logstr(name));
				let req = self.new_request(header);
				self.filesystem.setvolname(req, name);
			},
			FUSE_EXCHANGE => {
				let arg: &fuse_exchange_in = data.fetch();
				let oldname = data.fetch_str();
				let newname = data.fetch_str();
				debug!("EXCHANGE({:u}) parent {:#018x}, name {:s}, newparent {:#018x}, newname {:s}, options {:#x}", header.unique, arg.olddir, logstr(oldname), arg.newdir, logstr(newname), arg.options);
				let req = self.new_request(header);
				self.filesystem.exchange(req, arg.olddir, oldname, arg.newdir, newname, arg.options as uint);
			},
			FUSE_GETXTIMES => {
				debug!("GETXTIMES({:u}) ino {:#018x}", header.unique, header.nodeid);
				let req = self.new_request(header);
				self.filesystem.getxtimes(req, header.nodeid);
			},
            _ => unreachable!(),
		}
	}

    /// Warn about unsupported OS X operation on other os
    #[cfg(not(target_os = "macos"))]
    fn dispatch_macos_only(&mut self, _opcode: fuse_opcode, header: &fuse_in_header, _data: &mut ArgumentIterator) {
        warn!("Ignoring unsupported FUSE operation {:u}", header.opcode);
        self.send_reply_error(header.unique, ENOSYS);
    }

	fn new_request<T:Sendable+Send>(&mut self, header: &fuse_in_header) -> Request<T> {
		new_request::<T>(self.chan.clone(), header.unique)
	}

	fn send_reply_error(&self, unique: u64, err:c_int) {
		self.send_reply::<()>(unique, Err(err));
	}

	fn send_reply<T:Sendable>(&self, unique: u64, result: Result<T,c_int>) {
		new_request::<T>(self.chan.clone(), unique).reply(result);
	}
}

/// The background session data structure
pub struct BackgroundSession {
	mountpoint: Path,
}

impl BackgroundSession {
	/// Start the session loop of the given session in a background task
	pub fn start<FS: Filesystem+Send> (se: Session<FS>) -> BackgroundSession {
		let mountpoint = se.mountpoint.clone();
		// The background task is started using a a new single threaded
		// scheduler since I/O in the session loop can block
		do task::spawn_sched(task::SingleThreaded) {
			let mut se = se;
			se.run();
		}
		BackgroundSession { mountpoint: mountpoint }
	}

	/// End the session by unmounting the filesystem (which will
	/// eventually end the session loop)
	pub fn unmount (&self) {
		info!("Unmounting {}", self.mountpoint.display());
		channel::unmount(&self.mountpoint);
	}
}
