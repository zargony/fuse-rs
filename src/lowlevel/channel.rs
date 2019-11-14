//! Low-level filesystem kernel driver communication.
//!
//! Raw communication channel to the FUSE kernel driver.

use fuse_sys::{fuse_args, fuse_mount_compat25};
use std::ffi::{CStr, CString, OsStr};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};


macro_rules! try_io {
    ($x:expr) => {
        match $x {
            rc if rc < 0 => return Err(io::Error::last_os_error()),
            rc => rc,
        }
    };
}


/// A raw communication channel to the FUSE kernel driver
#[derive(Debug)]
pub struct Channel {
    fd: RawFd,
    mountpoint: PathBuf,
}

impl AsRawFd for Channel {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl io::Read for Channel {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        Ok(try_io!(unsafe {
            libc::read(
                self.fd,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len() as libc::size_t,
            )
        }) as usize)
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        Ok(try_io!(unsafe {
            libc::readv(
                self.fd,
                bufs.as_ptr() as *const libc::iovec,
                bufs.len() as libc::c_int,
            )
        }) as usize)
    }
}

impl io::Write for Channel {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(try_io!(unsafe {
            libc::write(
                self.fd,
                buf.as_ptr() as *const libc::c_void,
                buf.len() as libc::size_t,
            )
        }) as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        try_io!(unsafe { libc::fsync(self.fd) });
        Ok(())
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        Ok(try_io!(unsafe {
            libc::writev(
                self.fd,
                bufs.as_ptr() as *const libc::iovec,
                bufs.len() as libc::c_int,
            )
        }) as usize)
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        // TODO: OSXFUSE sends ioctl FUSEDEVIOCSETDAEMONDEAD before closing the fd

        // Close the file descriptor first to prevent further operations coming in (e.g. if the
        // filesystem is still mounted at this point, the following call to unmount would block
        // indefinitely since its sync operation requests couldn't be dispatched anymore).
        unsafe {
            libc::close(self.fd);
        }

        // Unmount this channel's mountpoint
        let _ = self.unmount();
    }
}

impl Channel {
    /// Create a new communication channel to the kernel driver using the given file descriptor
    /// obtained by calling fusermount or any other FUSE mount mechanism. When the channel is
    /// dropped, the file descriptor will be closed and the path unmounted.
    pub fn new(fd: RawFd, mountpoint: PathBuf) -> Self {
        Self { fd, mountpoint }
    }

    /// Create a new communication channel to the kernel driver by mounting the given path. The
    /// kernel driver will delegate filesystem operations of the given path to the channel. When the
    /// channel is dropped, the path will be unmounted.
    pub fn mount(mountpoint: &Path, options: &[&OsStr]) -> io::Result<Channel> {
        let mountpoint = mountpoint.canonicalize()?;

        // Convert options to `fuse_args` which requires pointers to C strings
        let args: Vec<CString> = [OsStr::new("fuse-rs")]
            .iter()
            .chain(options.iter())
            .map(|s| CString::new(s.as_bytes()).unwrap())
            .collect();
        let argptrs: Vec<_> = args.iter().map(|s| s.as_ptr()).collect();
        let fuse_args = fuse_args {
            argc: argptrs.len() as i32,
            argv: argptrs.as_ptr(),
            allocated: 0,
        };

        let path = CString::new(mountpoint.as_os_str().as_bytes())?;
        let fd = try_io!(unsafe { fuse_mount_compat25(path.as_ptr(), &fuse_args) });
        Ok(Channel::new(fd, mountpoint))
    }

    /// Returns the path of the mounted filesystem.
    pub fn mountpoint(&self) -> &Path {
        &self.mountpoint
    }

    /// Unmount this channel's mountpoint.
    /// This will in some way or other trigger a call to the kernel's umount syscall and unmount
    /// the mountpoint. The kernel will typically request to sync the filesystem one last time and
    /// shuts down the FUSE kernel driver instance afterwards, which results in the final `destroy`
    /// operation being sent to the channel.
    pub fn unmount(&self) -> io::Result<()> {
        unmount(&self.mountpoint)
    }
}


/// Unmount an arbitrary mount point
// FIXME: This should be moved to `Channel::unmount`, but it's still needed for `BackgroundSession`
pub fn unmount(mountpoint: &Path) -> io::Result<()> {
    // `fuse_unmount_compat22` unfortunately doesn't return a status. Additionally, it attempts
    // to call `realpath`, which in turn calls into the filesystem. So if the filesystem returns
    // an error, the unmount does not take place, with no indication of the error available to
    // the caller. So we call unmount directly (which is what OSXFUSE does anyway), since we
    // already converted to the real path when we first mounted.

    // On macOS and BSD, simply call `libc::unmount` to unmount.
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "bitrig",
        target_os = "netbsd"
    ))]
    #[inline]
    fn unmount(path: &CStr) -> libc::c_int {
        unsafe { libc::unmount(path.as_ptr(), 0) }
    }

    // On Linux, try calling `libc::umount` but fall back to libfuse in case of permission
    // errors.
    #[cfg(not(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "bitrig",
        target_os = "netbsd"
    )))]
    #[inline]
    fn unmount(path: &CStr) -> libc::c_int {
        use fuse_sys::fuse_unmount_compat22;
        use std::io::ErrorKind::PermissionDenied;

        let rc = unsafe { libc::umount(path.as_ptr()) };
        if rc < 0 && io::Error::last_os_error().kind() == PermissionDenied {
            // Linux always returns EPERM for non-root users. We have to let the
            // library go through the setuid-root "fusermount -u" to unmount.
            unsafe {
                fuse_unmount_compat22(path.as_ptr());
            }
            0
        } else {
            rc
        }
    }

    // Unmount this channel's mountpoint
    let path = CString::new(mountpoint.as_os_str().as_bytes())?;
    try_io!(unmount(&path));
    Ok(())
}
