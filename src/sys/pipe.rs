
use libc;
use std::io;
use std::os::unix::io::RawFd;

pub const DEFAULT_PIPE_SIZE: usize = 4096 * 16; // on linux

pub fn pipe() -> io::Result<[RawFd; 2]> {
    let mut fds: [RawFd; 2] = [0; 2];
    let res = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) };
    if res < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(fds)
    }
}
