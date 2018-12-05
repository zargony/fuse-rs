//! Filesystem session
//!
//! A session runs a filesystem implementation while it is being mounted to a specific mount
//! point. A session begins by mounting the filesystem and ends by unmounting it. While the
//! filesystem is mounted, the session loop receives, dispatches and replies to kernel requests
//! for filesystem operations under its mount point.

use std::io;
use std::ffi::OsStr;
use std::path::{PathBuf, Path};
use crossbeam::thread::{Scope, ScopedJoinHandle};
use libc::{EAGAIN, EINTR, ENODEV, ENOENT};

use channel::{self, Channel};
use Filesystem;
use request;

/// The max size of write requests from the kernel. The absolute minimum is 4k,
/// FUSE recommends at least 128k, max 16M. The FUSE default is 16M on macOS
/// and 128k on other systems.
pub const MAX_WRITE_SIZE: usize = 16 * 1024 * 1024;

/// Size of the buffer for reading a request from the kernel. Since the kernel may send
/// up to MAX_WRITE_SIZE bytes in a write request, we use that value plus some extra space.
const BUFFER_SIZE: usize = MAX_WRITE_SIZE + 4096;

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
}

impl<FS: Filesystem> Session<FS> {
    /// Create a new session by mounting the given filesystem to the given mountpoint
    pub fn new(filesystem: FS, mountpoint: &Path, options: &[&OsStr]) -> io::Result<Session<FS>> {
        info!("Mounting {}", mountpoint.display());
        Channel::new(mountpoint, options).map(|ch| {
            Session {
                filesystem: filesystem,
                ch: ch,
                proto_major: 0,
                proto_minor: 0,
                initialized: false,
                destroyed: false,
            }
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
        // Buffer for receiving requests from the kernel. Only one is allocated and
        // it is reused immediately after dispatching to conserve memory and allocations.
        let mut buffer: Vec<u8> = Vec::with_capacity(BUFFER_SIZE);
        loop {
            // Read the next request from the given channel to kernel driver
            // The kernel driver makes sure that we get exactly one request per read
            match self.ch.receive(&mut buffer) {
                Ok(()) => match request::request(self.ch.sender(), &buffer) {
                    // Dispatch request
                    Some(req) => request::dispatch(&req, self),
                    // Quit loop on illegal request
                    None => break,
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

impl<'a, 'b, FS: Filesystem + Send + 'a> Session<FS> {
    /// Run the session loop in a background thread
    pub fn spawn(self, scope: &'b Scope<'a>) -> io::Result<BackgroundSession<'b>> {
        BackgroundSession::new(self, scope)
    }
}

impl<FS: Filesystem> Drop for Session<FS> {
    fn drop(&mut self) {
        info!("Unmounted {}", self.mountpoint().display());
    }
}

/// The background session data structure
#[derive(Debug)]
pub struct BackgroundSession<'b> {
    /// Path of the mounted filesystem
    mountpoint: PathBuf,
    /// Handle to the background thread
    /// Use an Option so we can take() it in the Drop impl
    handle: Option<ScopedJoinHandle<'b, io::Result<()>>>,
}

impl<'b> BackgroundSession<'b> {
    /// Create a new background session for the given session by running its
    /// session loop in a background thread. If the returned handle is dropped,
    /// the filesystem is unmounted and the given session ends.
    pub fn new<'a, FS: Filesystem + Send + 'a>(se: Session<FS>, scope: &'b Scope<'a>) -> io::Result<BackgroundSession<'b>> {
        let mountpoint = se.mountpoint().to_path_buf();
        let handle = scope.spawn(move |_| {
            let mut se = se;
            se.run()
        });
        Ok(BackgroundSession { mountpoint: mountpoint, handle: Some(handle) })
    }
}

impl<'a> Drop for BackgroundSession<'a> {
    fn drop(&mut self) {
        info!("Unmounting {}", self.mountpoint.display());
        // Unmounting the filesystem will eventually end the session loop,
        // drop the session and hence end the background thread.
        match channel::unmount(&self.mountpoint) {
            Ok(()) => { 
                // Only join the thread if we expect it to terminate,
                // i.e if unmount was successful.
                let thread_handle = self.handle.take().unwrap();
                let join_result = thread_handle.join();
                let session_run_result = join_result.unwrap();
                session_run_result.unwrap();
            },
            Err(err) => error!("Failed to unmount {}: {}", self.mountpoint.display(), err),
        }
    }
}
