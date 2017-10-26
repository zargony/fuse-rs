use std::io;
use libc::{self, size_t, c_void};
use std::os::unix::io::RawFd;

pub fn read(fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
    let rc = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut c_void, buf.len() as size_t) };
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(rc as usize)
    }
}
