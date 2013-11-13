/*!
 * Communication channel to the FUSE kernel driver.
 */

use std::{mem, os, vec};
use std::libc::{c_int, c_void, size_t};
use std::libc::{EIO};
use native::{fuse_args, fuse_mount_compat25, fuse_unmount_compat22, fuse_out_header, fuse_in_header};
use sendable::Sendable;
use extra::sync::Mutex;
use extra::arc::Arc;

/// Maximum write size. FUSE recommends at least 128k, max 16M. Default on OS X is 16M.
pub static MAX_WRITE_SIZE: u32 = 16*1024*1024;

/// Size of the buffer for reading a request from the kernel. Since the kernel may send
/// up to MAX_WRITE_SIZE bytes in a write request, we use that value plus some extra space.
pub static BUFFER_SIZE: uint = MAX_WRITE_SIZE as uint + 4096;

// This structure owns the file descriptor which is the channel through which this process talks to
// the kernel.  When it is destroyed, the underlying file descriptor is closed, and the filesystem
// unmounted.
struct Channel {
	mountpoint: Path,
	priv fd: c_int,
}

/// Allows reading from the fuse channel (i.e. the file descriptor through which we communicate with
/// the kernel).  Its name is by analogy to rust's standard `Port`, even though it isn't one and has
/// a different API.
#[deriving(Clone)]
pub struct FusePort { 
	priv mutex: Mutex,
	priv channel: Arc<Channel>
}

/// Allows writing to the fuse channel (i.e. the file descriptor through which we communicate with
/// the kernel.)  Its name is by analogy to rust's standard `Chan`, even though it isn't one and has
// a different API.
#[deriving(Clone)]
pub struct FuseChan {
	priv mutex: Mutex,
	priv channel: Arc<Channel>
}

/// Helper function to provide options as a fuse_args struct
/// (which contains an argc count and an argv pointer)
fn with_fuse_args<T> (options: &[&[u8]], f: &fn(&fuse_args) -> T) -> T {
	do "rust-fuse".with_c_str |progname| {
		let args = options.map(|arg| arg.to_c_str());
		let argptrs = [progname] + args.map(|arg| arg.with_ref(|s| s));
		do argptrs.as_imm_buf |argv, argc| {
			f(&fuse_args { argc: argc as i32, argv: argv, allocated: 0 })
		}
	}
}

// Libc provides iovec based I/O using readv and writev functions
mod libc {
	use std::libc::{c_int, c_void, size_t, ssize_t};

	/// Iovec data structure for readv and writev calls.
	pub struct iovec {
		iov_base: *c_void,
		iov_len: size_t,
	}

	extern "system" {
		/// Read data from fd into multiple buffers
		pub fn readv (fd: c_int, iov: *mut iovec, iovcnt: c_int) -> ssize_t;
		/// Write data from multiple buffers to fd
		pub fn writev (fd: c_int, iov: *iovec, iovcnt: c_int) -> ssize_t;
	}
}

/// Creates a new communication channel to the kernel driver by mounting the given mountpoint.
/// Return a cloneable `FusePort` and `FuseChan` that may be used to communicate with it, or an
/// "errno" value for errors.
pub fn mount (mountpoint: &Path, options: &[&[u8]]) -> Result<(FusePort, FuseChan), c_int> {
	do mountpoint.with_c_str |mnt| {
		do with_fuse_args(options) |args| {
			let fd = unsafe { fuse_mount_compat25(mnt, args) };
			if fd < 0 { 
				Err(os::errno() as c_int)
			} else { 
				let channel = Arc::new(Channel { fd: fd, mountpoint: mountpoint.clone() });
				Ok((FusePort {mutex: Mutex::new(), channel: channel.clone()},
					FuseChan {mutex: Mutex::new(), channel: channel.clone()}))
			}
		}
	}
}

/// Unmount a given mountpoint
pub fn unmount (mountpoint: &Path) {
	do mountpoint.with_c_str |mnt| {
		unsafe { fuse_unmount_compat22(mnt); }
	}
}

impl Channel {
	/// Closes the communication channel to the kernel driver
	fn close (&mut self) {
		// TODO: send ioctl FUSEDEVIOCSETDAEMONDEAD on OS X before closing the fd
		unsafe { ::std::libc::close(self.fd); }
		self.fd = -1;
	}
}

impl Drop for Channel {
	fn drop(&mut self) {
		self.close();
		// Close channel before unnmount to prevent sync unmount deadlock
		unmount(&self.mountpoint);
	}
}

impl FusePort {
	/// Receives data up to the capacity of the given buffer
	/// Read the next request from the given channel to kernel driver
	pub fn read(&self, data:&mut ~[u8]) -> Result<(), c_int> {
		assert!(data.capacity() >= BUFFER_SIZE);
		// The kernel driver makes sure that we get exactly one request per read
		
		let res = do self.mutex.lock {
			data.clear();
			let capacity = data.capacity();
			let rc = do data.as_mut_buf |ptr, _| {
				// FIXME: This read can block the whole scheduler (and therefore multiple other tasks)
				unsafe { ::std::libc::read(self.channel.get().fd, ptr as *mut c_void, capacity as size_t) }
			};
			if rc >= 0 { unsafe { vec::raw::set_len(data, rc as uint); } }
			if rc < 0 { Err(os::errno() as c_int) } else { Ok(()) }
		};
		if res.is_ok() && data.len() < mem::size_of::<fuse_in_header>() {
			error!("Short read on FUSE device");
			Err(EIO)
		} else {
			res
		}
	}

	pub fn channel<'a>(&'a self) -> &'a Channel {
		self.channel.get()
	}
}

impl FuseChan {
	/// Send all data in the slice of slice of bytes in a single write
	fn send_buffer (&self, buffer: &[&[u8]]) -> Result<(), c_int> {
		let iovecs = do buffer.map |d| {
			do d.as_imm_buf |bufptr, buflen| {
				libc::iovec { iov_base: bufptr as *c_void, iov_len: buflen as size_t }
			}
		};
		let rc = do iovecs.as_imm_buf |iovptr, iovcnt| {
			// FIXME: This write can block the whole scheduler (and therefore multiple other tasks)
			unsafe { libc::writev(self.channel.get().fd, iovptr, iovcnt as c_int) }
		};
		if rc < 0 { Err(os::errno() as c_int) } else { Ok(()) }
	}

	/// Send a piece of typed data along with a header
	pub fn send<T: Sendable>(&self, unique: u64, err: c_int, data: &T) {
		do self.mutex.lock {
			do data.as_bytegroups |databytes| {
				let datalen = databytes.iter().fold(0, |l, b| { l +  b.len()});
				let outheader = fuse_out_header {
					len: mem::size_of::<fuse_out_header>() as u32 + datalen as u32,
					error: err as i32,
					unique: unique,
				};
				do outheader.as_bytegroups |headbytes| {
					self.send_buffer(headbytes + databytes);
				}
			}
		}
	}

	pub fn channel<'a>(&'a self) -> &'a Channel {
		self.channel.get()
	}
}


#[cfg(test)]
mod test {
	use super::with_fuse_args;
	use std::vec;

	#[test]
	fn test_with_fuse_args () {
		do with_fuse_args([bytes!("foo"), bytes!("bar")]) |args| {
			unsafe {
				assert!(args.argc == 3);
				do vec::raw::buf_as_slice(*args.argv.offset(0) as *u8, 10) |bytes| { assert!(bytes == bytes!("rust-fuse\0") ); }
				do vec::raw::buf_as_slice(*args.argv.offset(1) as *u8, 4) |bytes| { assert!(bytes == bytes!("foo\0")); }
				do vec::raw::buf_as_slice(*args.argv.offset(2) as *u8, 4) |bytes| { assert!(bytes == bytes!("bar\0")); }
			}
		}
	}
}
