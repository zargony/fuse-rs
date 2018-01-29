//! Filesystem session
//!
//! A session runs a filesystem implementation while it is being mounted to a specific mount
//! point. A session begins by mounting the filesystem and ends by unmounting it. While the
//! filesystem is mounted, the session loop receives, dispatches and replies to kernel requests
//! for filesystem operations under its mount point.

use std::io;
use std::ffi::OsStr;
use std::fmt;
use std::os::unix::io::RawFd;
use std::path::{PathBuf, Path};
use thread_scoped::{scoped, JoinGuard};
use libc::{EAGAIN, EINTR, ENODEV, ENOENT};
use channel::{self, Channel};
use Filesystem;
use request;

/// The max size of write requests from the kernel. The absolute minimum is 4k,
/// FUSE recommends at least 128k, max 16M. The FUSE default is 16M on macOS
/// and 128k on other systems.
pub const MAX_WRITE_SIZE: usize = 128 * 1024;

const PAGE_SIZE: usize = 4096;

/// Size of the buffer for reading a request from the kernel. Since the kernel may send
/// up to MAX_WRITE_SIZE bytes in a write request, we use that value plus some extra space.
const BUFFER_SIZE: usize = MAX_WRITE_SIZE + PAGE_SIZE;


/// The session data structure
#[derive(Debug)]
pub struct Session<FS: Filesystem> {
    /// Filesystem operation implementations
    pub filesystem: FS,
    /// Communication channel to the kernel driver
    ch: Channel,
    /// FUSE protocol major version
    pub proto_major: u32,
    /// FUSE protocol minor version
    pub proto_minor: u32,
    /// True if the filesystem is initialized (init operation done)
    pub initialized: bool,
    /// True if the filesystem was destroyed (destroy operation done)
    pub destroyed: bool,
    /// True, if splice() syscall should be used for /dev/fuse
    pub splice_write: bool,
    /// Number of queued requests in the kernel
    pub max_background: u16,
    /// Threshold when waiting fuse users are put into sleep state instead of busy loop
    pub congestion_threshold: u16
}

impl<FS: Filesystem> Session<FS> {
    /// Create a new session by mounting the given filesystem to the given mountpoint
    #[cfg(feature = "libfuse")]
    pub fn new(filesystem: FS, mountpoint: &Path, options: &[&OsStr], splice_write: bool, max_background: u16, congestion_threshold: u16) -> io::Result<Session<FS>> {
        info!("Mounting {}", mountpoint.display());
        Channel::new(mountpoint, options, BUFFER_SIZE).map(|ch| {
            Session {
                filesystem: filesystem,
                ch: ch,
                proto_major: 0,
                proto_minor: 0,
                initialized: false,
                destroyed: false,
                splice_write: splice_write,
                max_background: max_background,
                congestion_threshold: congestion_threshold
            }
        })
    }

    /// Create a new session by using a file descriptor "/dev/fuse"
    pub fn new_from_fd(filesystem: FS, fd: RawFd, mountpoint: &Path, splice_write: bool, max_background: u16, congestion_threshold: u16) -> io::Result<Session<FS>> {
        Ok(Session {
            filesystem: filesystem,
            ch: try!(Channel::new_from_fd(fd, mountpoint, BUFFER_SIZE)),
            proto_major: 0,
            proto_minor: 0,
            // This hacky in general, but ok for CntrFS,
            // we need this in CntrFs to support multi-threading.
            initialized: true,
            destroyed: false,
            splice_write: splice_write,
            max_background: max_background,
            congestion_threshold: congestion_threshold
        })
    }

    /// Return path of the mounted filesystem
    pub fn mountpoint(&self) -> &Path {
        &self.ch.mountpoint()
    }

    /// Run the session loop that receives kernel requests and dispatches them to method
    /// calls into the filesystem. This read-dispatch-loop is non-concurrent to prevent
    /// having multiple buffers (which take up much memory), but the filesystem methods
    /// may run concurrent by spawning threads.
    pub fn run(&mut self) -> io::Result<()> {
        if self.splice_write {
            self.run_splice_write()
        } else {
            self.run_no_splice_write()
        }
    }

    fn run_no_splice_write(&mut self) -> io::Result<()> {
        let mut buffer: Vec<u8> = vec![0; BUFFER_SIZE];

        loop {
            // Read the next request from the given channel to kernel driver
            // The kernel driver makes sure that we get exactly one request per read
            match self.ch.receive(&mut buffer) {
                Ok(()) => {
                    match request::request(self.ch.sender(), &buffer) {
                        // Dispatch request
                        Some(req) => {
                            let read_pipe_fd = self.ch.read_pipe_fd;
                            let write_pipe_fd = self.ch.write_pipe_fd;

                            request::dispatch(&req, self, read_pipe_fd, write_pipe_fd);
                        },
                        // Quit loop on illegal request
                        None => break,
                    }
                },
                Err(err) => match err.raw_os_error() {
                    // Operation interrupted. Accordingly to FUSE, this is safe to retry
                    Some(ENOENT) => continue,
                    // Interrupted system call, retry
                    Some(EINTR) => continue,
                    // Explicitly try again
                    Some(EAGAIN) => continue,
                    // Filesystem was unmounted, quit the loop
                    Some(ENODEV) => break,
                    // Unhandled error
                    _ => return Err(err),
                }
            }
        }
        Ok(())
    }

    fn run_splice_write(&mut self) -> io::Result<()> {
        // Buffer for receiving requests from the kernel. Only one is allocated and
        // it is reused immediately after dispatching to conserve memory and allocations.
        // For small requests we copy the whole requests to this buffer
        let mut buffer: Vec<u8> = vec![0; MAX_WRITE_SIZE];

        loop {
            // Read the next request from the given channel to kernel driver
            // The kernel driver makes sure that we get exactly one request per read
            match self.ch.receive_splice(MAX_WRITE_SIZE) {
                Ok(size) => {
                    match request::request_splice(self.ch.sender(), &mut buffer, self.ch.read_pipe_fd, size) {
                        // Dispatch request
                        Some(req) => {
                            let read_pipe_fd = self.ch.read_pipe_fd;
                            let write_pipe_fd = self.ch.write_pipe_fd;

                            request::dispatch(&req, self, read_pipe_fd, write_pipe_fd);
                        },
                        // Quit loop on illegal request
                        None => break,
                    }
                },
                Err(err) => match err.raw_os_error() {
                    // Operation interrupted. Accordingly to FUSE, this is safe to retry
                    Some(ENOENT) => continue,
                    // Interrupted system call, retry
                    Some(EINTR) => continue,
                    // Explicitly try again
                    Some(EAGAIN) => continue,
                    // Filesystem was unmounted, quit the loop
                    Some(ENODEV) => break,
                    // Unhandled error
                    _ => return Err(err),
                }
            }
        }
        Ok(())
    }
}

impl<'a, FS: Filesystem + Send + 'a> Session<FS> {
    /// Run the session loop in a background thread
    pub unsafe fn spawn(self) -> io::Result<BackgroundSession<'a>> {
        BackgroundSession::new(self)
    }
}

impl<FS: Filesystem> Drop for Session<FS> {
    fn drop(&mut self) {
        info!("Unmounted {}", self.mountpoint().display());
    }
}

/// The background session data structure
pub struct BackgroundSession<'a> {
    /// Path of the mounted filesystem
    pub mountpoint: PathBuf,
    /// Thread guard of the background session
    pub guard: JoinGuard<'a, io::Result<()>>,
}

impl<'a> BackgroundSession<'a> {
    /// Create a new background session for the given session by running its
    /// session loop in a background thread. If the returned handle is dropped,
    /// the filesystem is unmounted and the given session ends.
    pub unsafe fn new<FS: Filesystem + Send + 'a>(se: Session<FS>) -> io::Result<BackgroundSession<'a>> {
        let mountpoint = se.mountpoint().to_path_buf();
        let guard = scoped(move || {
            let mut se = se;
            se.run()
        });
        Ok(BackgroundSession { mountpoint: mountpoint, guard: guard })
    }
}

impl<'a> Drop for BackgroundSession<'a> {
    fn drop(&mut self) {
        info!("Unmounting {}", self.mountpoint.display());
        // Unmounting the filesystem will eventually end the session loop,
        // drop the session and hence end the background thread.
        #[cfg(feature = "libfuse")]
        match channel::unmount(&self.mountpoint) {
            Ok(()) => (),
            Err(err) => error!("Failed to unmount {}: {}", self.mountpoint.display(), err),
        }
    }
}

// replace with #[derive(Debug)] if Debug ever gets implemented for
// thread_scoped::JoinGuard
impl<'a> fmt::Debug for BackgroundSession<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "BackgroundSession {{ mountpoint: {:?}, guard: JoinGuard<()> }}", self.mountpoint)
    }
}
