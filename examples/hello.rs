extern crate fuse;

use std::default::Default;
use std::libc::{c_int, ENOENT, S_IFDIR, S_IFREG};
use std::io::{TypeFile, TypeDirectory};
use std::os;
use fuse::{Filesystem, Request, fuse_attr, fuse_entry_out, fuse_attr_out, DirBuffer};

struct HelloFS;

static HELLO_WORLD: &'static str = "Hello World!\n";

fn hello_dir_attr () -> fuse_attr {
	fuse_attr {
		ino: 1, mode: S_IFDIR as u32 | 0o755, nlink: 2, uid: 501, gid: 20, ..Default::default()
	}
}

fn hello_txt_attr () -> fuse_attr {
	fuse_attr {
		ino: 2, size: 13, mode: S_IFREG as u32 | 0o644, nlink: 1, uid: 501, gid: 20, ..Default::default()
	}
}

impl Filesystem for HelloFS {
	fn lookup (&mut self, _req: &Request, parent: u64, name: &PosixPath) -> Result<fuse_entry_out, c_int> {
		if parent == 1 && name.as_str() == Some("hello.txt") {
			Ok(fuse_entry_out { nodeid: 2, generation: 0, attr: hello_txt_attr(), entry_valid: 1, entry_valid_nsec: 0, attr_valid: 1, attr_valid_nsec: 0 })
		} else {
			Err(ENOENT)
		}
	}

	fn getattr (&mut self, _req: &Request, ino: u64) -> Result<fuse_attr_out, c_int> {
		match ino {
			1 => Ok(fuse_attr_out { attr_valid: 1, attr_valid_nsec: 0, dummy: 0, attr: hello_dir_attr() }),
			2 => Ok(fuse_attr_out { attr_valid: 1, attr_valid_nsec: 0, dummy: 0, attr: hello_txt_attr() }),
			_ => Err(ENOENT),
		}
	}

	fn read (&mut self, _req: &Request, ino: u64, _fh: u64, offset: u64, _size: uint) -> Result<Vec<u8>, c_int> {
		if ino == 2 {
			Ok(Vec::from_slice(HELLO_WORLD.as_bytes().tailn(offset as uint)))
		} else {
			Err(ENOENT)
		}
	}

	fn readdir (&mut self, _req: &Request, ino: u64, _fh: u64, offset: u64, mut buffer: DirBuffer) -> Result<DirBuffer, c_int> {
		if ino == 1 {
			if offset == 0 {
				buffer.fill(1, 0, TypeDirectory, &PosixPath::new("."));
				buffer.fill(1, 1, TypeDirectory, &PosixPath::new(".."));
				buffer.fill(2, 2, TypeFile, &PosixPath::new("hello.txt"));
			}
			Ok(buffer)
		} else {
			Err(ENOENT)
		}
	}
}

fn main () {
	let mountpoint = Path::new(os::args()[1]);
	fuse::mount(HelloFS, &mountpoint, []);
}
