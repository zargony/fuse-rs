use std::io;
use libc::{self, iovec, size_t, ssize_t, c_uint};
use std::os::unix::io::RawFd;

pub fn vmsplice(fd: RawFd, iov: &iovec, nr_segs: size_t, flags: c_uint) -> io::Result<ssize_t> {
    let rc = unsafe {
        libc::vmsplice(fd, iov, nr_segs, flags)
    };
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(rc)
    }
}
