//! FUSE kernel driver communication
//!
//! Raw communication channel to the FUSE kernel driver.
//!
//! TODO: This module is meant to go away soon in favor of `lowlevel::Channel`.

use std::io;
use std::os::unix::io::{AsRawFd, RawFd};


macro_rules! try_io {
    ($x:expr) => {
        match $x {
            rc if rc < 0 => return Err(io::Error::last_os_error()),
            rc => rc,
        }
    };
}


#[derive(Clone, Copy, Debug)]
pub struct ChannelSender {
    fd: RawFd,
}

impl io::Write for ChannelSender {
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

impl ChannelSender {
    pub fn new<T: AsRawFd>(fd: &T) -> Self {
        Self { fd: fd.as_raw_fd() }
    }
}
