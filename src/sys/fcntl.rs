use std::io;
use libc::{self, c_int};
use std::os::unix::io::RawFd;

// TODO: only useful for special purpose
pub fn fcntl(fd: RawFd, cmd: c_int, arg1: usize) -> io::Result<()> {
    let res = unsafe { libc::fcntl(fd, cmd, arg1) };
    if res < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
