//!
//! A session runs a filesystem implementation while it is being mounted
//! to a specific mount point. A session begins by mounting the filesystem
//! and ends by unmounting it. While the filesystem is mounted, the session
//! loop receives, dispatches and replies to kernel requests for filesystem
//! operations under its mount point.
//!

use std::libc::{EAGAIN, EINTR, ENODEV, ENOENT, EBADF};
use native;
use channel;
use channel::Channel;
use Filesystem;
use request::Request;

/// The session data structure
pub struct Session<FS> {
	filesystem: FS,
	mountpoint: Path,
	ch: Channel,
	proto_major: uint,
	proto_minor: uint,
	initialized: bool,
	destroyed: bool,
}

impl<FS: Filesystem+Send> Session<FS> {
	/// Create a new session by mounting the given filesystem to the given mountpoint
	pub fn new (filesystem: FS, mountpoint: &Path, options: &[&[u8]]) -> Session<FS> {
		info!("Mounting {}", mountpoint.display());
		let ch = match Channel::new(mountpoint, options) {
			Ok(ch) => ch,
			Err(err) => fail!("Unable to mount filesystem. Error {:i}", err),
		};
		Session {
			filesystem: filesystem,
			mountpoint: mountpoint.clone(),
			ch: ch,
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
		let mut req = Request::new();
		loop {
			match req.read(self) {
				Err(ENOENT) => continue,		// Operation interrupted. Accordingly to FUSE, this is safe to retry
				Err(EINTR) => continue,			// Interrupted system call, retry
				Err(EAGAIN) => continue,		// Explicitly try again
				Err(ENODEV) | Err(EBADF) => break,			// Filesystem was unmounted, quit the loop
				Err(err) => fail!("Lost connection to FUSE device. Error {:i}", err),
				Ok(..) => req.dispatch(self),
			}
		}
	}

	/// Run the session loop in a background task
	pub fn spawn (self) -> BackgroundSession {
		BackgroundSession::new(self)
	}
}

#[unsafe_destructor]
impl<FS: Filesystem+Send> Drop for Session<FS> {
	fn drop (&mut self) {
		info!("Unmounting {}", self.mountpoint.display());
		self.ch.close();
		channel::unmount(&self.mountpoint);
	}
}

/// The background session data structure
pub struct BackgroundSession {
	mountpoint: Path,
	ch: Channel
}

impl BackgroundSession {
	/// Create a new background session for the given session by running its
	/// session loop in a background task. If the returned handle is dropped,
	/// the filesystem is unmounted and the given session ends.
	pub fn new<FS: Filesystem+Send> (se: Session<FS>) -> BackgroundSession {
		let mountpoint = se.mountpoint.clone();
		let ch = se.ch.clone();
		// The background task is started using a a new native thread
		// since native I/O in the session loop can block
		native::task::spawn(proc() {
			let mut se = se;
			se.run();
		});
		BackgroundSession { mountpoint: mountpoint, ch: ch }
	}
}

impl Drop for BackgroundSession {
	fn drop (&mut self) {
		info!("Unmounting {}", self.mountpoint.display());
		// Closing the fd and unmounting will eventually end the session loop,
		// drop the session and hence end the background task.
		self.ch.close();
		channel::unmount(&self.mountpoint);
	}
}
