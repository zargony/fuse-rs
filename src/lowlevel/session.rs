//! Low-level filesystem session.
//!
//! A session runs a filesystem implementation while it is being mounted to a specific mountpoint.
//! A session begins by mounting the filesystem and ends by unmounting it. While the session is
//! running, it receives filesystem operation requests from the FUSE kernel driver, dispatches them
//! to the filesystem implementation and provides the resulting replies back to the kernel driver.

use log::{debug, info, warn};
use std::convert::TryFrom;
use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use super::channel::Channel;
use super::filesystem::Filesystem;
use super::request::{Operation, Request};


/// Builder for configuring a low-level filesystem `Session`.
///
/// This builder can be used to configure the session prior to mounting and running the filesystem.
/// See the various methods of this type for configuration options of a session. Eventually call
/// `mount` to mount or `run` to mount and run the filesystem.
#[derive(Debug)]
pub struct SessionBuilder<FS: Filesystem> {
    filesystem: FS,
    mountpoint: PathBuf,
    max_write_size: usize,
}

impl<FS: Filesystem> SessionBuilder<FS> {
    /// Returns mount options as slice of OsStr references.
    fn mount_options(&self) -> &[&OsStr] {
        &[]
    }
}

impl<FS: Filesystem> SessionBuilder<FS> {
    /// Create a new session builder.
    ///
    /// Use the various methods of the returned builder to configure options of the session.
    /// Eventually call `mount` to mount or `run` to mount and run the filesystem.
    pub fn new<P: AsRef<Path>>(filesystem: FS, mountpoint: P) -> Self {
        Self {
            filesystem,
            mountpoint: mountpoint.as_ref().to_owned(),
            max_write_size: 16 * 1024 * 1024,
        }
    }

    /// Set max size of write requests.
    ///
    /// This determines the maximum size of write requests the kernel will send us. Larger write
    /// requests may result in higher performance (depending on your implementation), but also
    /// incur higher memory usage since a buffer of at least this size must be provided for
    /// processing requests.
    ///
    /// FUSE documents that 4k is the absolute minimum, 128k is recommended and 16M is the maximum.
    /// We're using 16M by default (which is also OSXFUSE's default, while libfuse uses 128k). It
    /// is most efficient to choose a multiple of the page size here (which is usually 4k).
    pub fn max_write_size(mut self, max_write_size: usize) -> Self {
        self.max_write_size = max_write_size;
        self
    }

    /// Mount filesystem.
    ///
    /// Use the configured builder to mount the filesystem and create a session. The returned
    /// session needs to be run by calling `Session::run` to have a functioning filesystem.
    /// See also `run` which mounts and runs in a single step.
    pub fn mount(self) -> io::Result<Session<FS>> {
        Session::try_from(self)
    }

    /// Mount and run filesystem.
    ///
    /// Use the configured builder to mount the filesystem, create a session and run it.
    ///
    /// Calling this method is the same as calling `mount` and calling `Session::run` on the
    /// returned session.
    ///
    /// This function doesn't return until the filesystem is unmounted.
    pub fn run(self) -> io::Result<()> {
        self.mount()?.run()
    }
}


/// Low-level filesystem session.
///
/// A session mounts the filesystem to a mointpoint and runs it. While the session is running, it
/// receives filesystem operation requests from the FUSE kernel driver and dispatches them to the
/// filesystem implementation. The session is done running eventually when the filesystem is
/// unmounted.
#[derive(Debug)]
pub struct Session<FS: Filesystem> {
    channel: Channel,
    filesystem: FS,
    max_write_size: usize,
}

impl<FS: Filesystem> TryFrom<SessionBuilder<FS>> for Session<FS> {
    type Error = io::Error;

    /// Converting from `SessionBuilder` to `Session` mounts the filesystem by creating a `Channel`.
    fn try_from(builder: SessionBuilder<FS>) -> Result<Self, Self::Error> {
        info!("Mounting {}", builder.mountpoint.display());
        Ok(Self {
            channel: Channel::mount(&builder.mountpoint, builder.mount_options())?,
            filesystem: builder.filesystem,
            max_write_size: builder.max_write_size,
        })
    }
}

impl<FS: Filesystem> Drop for Session<FS> {
    /// Dropping a `Session` unmounts the filesystem because the `Channel` is dropped as well.
    fn drop(&mut self) {
        info!("Unmounting {}", self.channel.mountpoint().display());
    }
}

impl<FS: Filesystem> Session<FS> {
    /// Read next packet from the kernel driver
    fn next_packet<'a>(&mut self, buffer: &'a mut [u8]) -> io::Result<Option<&'a [u8]>> {
        loop {
            match self.channel.read(buffer) {
                // Received packet from the kernel driver, return it
                Ok(len) => return Ok(Some(&buffer[..len])),
                // Error while reading from the kernel driver
                Err(err) => match err.raw_os_error() {
                    // Operation interrupted. Accordingly to FUSE, this is safe to retry
                    Some(libc::ENOENT) => continue,
                    // Interrupted system call, retry
                    Some(libc::EINTR) => continue,
                    // Explicitly try again
                    Some(libc::EAGAIN) => continue,
                    // Filesystem was unmounted, quit the loop
                    Some(libc::ENODEV) => break,
                    // Some other error occured, return it
                    _ => return Err(err),
                },
            }
        }
        Ok(None)
    }

    // Dispatch request to the filesystem implementation
    fn dispatch_request(&mut self, request: Request<'_>) {
        debug!("{}", request);
    }
}

impl<FS: Filesystem> Session<FS> {
    /// (Prepare to) mount a filesystem and create a new session.
    ///
    /// Returns a `SessionBuilder` that can be used to configure and eventually run the session.
    pub fn builder<P: AsRef<Path>>(filesystem: FS, mountpoint: P) -> SessionBuilder<FS> {
        SessionBuilder::new(filesystem, mountpoint)
    }

    /// Run the session.
    ///
    /// Runs the session loop of a mounted filesystem. The session loop receives filesystem
    /// operation requests from the FUSE kernel driver and dispatches them to method calls into the
    /// filesystem implementation until the filesystem gets unmounted.
    ///
    /// This function doesn't return until the filesystem is unmounted.
    pub fn run(mut self) -> io::Result<()> {
        // Size of a buffer for reading one request from the kernel. Since the kernel may send up
        // to `max_write_size` bytes in a write request, we use that value plus some extra space.
        // FIXME: This should depend on the actual page size the kernel uses
        let buffer_size = self.max_write_size + 4096;

        // Buffer for receiving requests from the kernel. Only one is allocated for now and it's
        // reused immediately after dispatching to conserve memory and allocations.
        // TODO: Implement multiple buffers and concurrent dispatch of async operations
        // TODO: Add a configurable pool of preallocated/dynamic buffers
        let mut buffer = vec![0; buffer_size];

        // Read and dispatch requests from the kernel driver
        while let Some(packet) = self.next_packet(&mut buffer)? {
            match Request::try_from(packet) {
                // Request parsed successfully, dispatch it
                Ok(request) => {
                    self.dispatch_request(request);
                }
                // Error parsing the request, log a warning and try next
                Err(err) => warn!("{}", err),
            }
        }
        Ok(())
    }
}


/// Mount and run filesystem.
///
/// Mounts the given filesystem to the given mountpoint and runs it. This is a convenient shortcut
/// for `Session::builder(filesystem, mountpoint).mount().run()` in case you want to mount and run
/// the filesystem with the default configuration. Please refer to `Session::builder` and
/// `SessionBuilder` for customizing behavior.
///
/// This function doesn't return until the filesystem is unmounted.
pub fn mount<FS: Filesystem, P: AsRef<Path>>(filesystem: FS, mountpoint: P) -> io::Result<()> {
    Session::builder(filesystem, mountpoint).run()
}
