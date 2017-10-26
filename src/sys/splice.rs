use std::io;
use libc::{self, c_uint, loff_t};
use std::os::unix::io::RawFd;
use std::ptr;

pub fn splice(from: RawFd, from_offset: Option<&mut loff_t>, to: RawFd, to_offset: Option<&mut loff_t>, len: usize, flags: c_uint) -> io::Result<usize> {
    let rc = unsafe {
        let from_offset = from_offset.map(|offset| offset as *mut _).unwrap_or(ptr::null_mut());
        let to_offset = to_offset.map(|offset| offset as *mut _).unwrap_or(ptr::null_mut());

        libc::splice(from, from_offset, to, to_offset, len, flags)
    };
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(rc as usize)
    }
}
