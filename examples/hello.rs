#![feature(globs)]
extern crate libc;
extern crate time;
extern crate fuse;
//use libc::size_t;
//use std::c_str::CString;
use fuse::*;
//mod fuse;


//extern crate fuse;

use std::io::{TypeFile, TypeDirectory, UserFile, UserDir};
use std::os;
use libc::ENOENT;
use time::Timespec;
use fuse::{FileAttr, Filesystem, Request, ReplyData, ReplyEntry, ReplyAttr, ReplyDirectory};

static TTL: Timespec = Timespec { sec: 1, nsec: 0 };					// 1 second

static CREATE_TIME: Timespec = Timespec { sec: 1381237736, nsec: 0 };	// 2013-10-08 08:56

static HELLO_DIR_ATTR: FileAttr = FileAttr {
	ino: 1,
	size: 0,
	blocks: 0,
	atime: CREATE_TIME,
	mtime: CREATE_TIME,
	ctime: CREATE_TIME,
	crtime: CREATE_TIME,
	kind: TypeDirectory,
	perm: UserDir,
	nlink: 2,
	uid: 501,
	gid: 20,
	rdev: 0,
	flags: 0,
};

static HELLO_TXT_CONTENT: &'static str = "Hello World!\n";

static HELLO_TXT_ATTR: FileAttr = FileAttr {
	ino: 2,
	size: 13,
	blocks: 1,
	atime: CREATE_TIME,
	mtime: CREATE_TIME,
	ctime: CREATE_TIME,
	crtime: CREATE_TIME,
	kind: TypeFile,
	perm: UserFile,
	nlink: 1,
	uid: 501,
	gid: 20,
	rdev: 0,
	flags: 0,
};

struct HelloFS;

impl Filesystem for HelloFS {
	fn lookup (&mut self, _req: &Request, parent: u64, name: &PosixPath, reply: ReplyEntry) {
		if parent == 1 && name.as_str() == Some("hello.txt") {
			reply.entry(&TTL, &HELLO_TXT_ATTR, 0);
		} else {
			reply.error(ENOENT);
		}
	}

	fn getattr (&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
		match ino {
			1 => reply.attr(&TTL, &HELLO_DIR_ATTR),
			2 => reply.attr(&TTL, &HELLO_TXT_ATTR),
			_ => reply.error(ENOENT),
		}
	}

	fn read (&mut self, _req: &Request, ino: u64, _fh: u64, offset: u64, _size: uint, reply: ReplyData) {
		if ino == 2 {
			reply.data(HELLO_TXT_CONTENT.as_bytes().slice_from(offset as uint));
		} else {
			reply.error(ENOENT);
		}
	}

	fn readdir (&mut self, _req: &Request, ino: u64, _fh: u64, offset: u64, mut reply: ReplyDirectory) {
		if ino == 1 {
			if offset == 0 {
				reply.add(1, 0, TypeDirectory, &PosixPath::new("."));
				reply.add(1, 1, TypeDirectory, &PosixPath::new(".."));
				reply.add(2, 2, TypeFile, &PosixPath::new("hello.txt"));
			}
			reply.ok();
		} else {
			reply.error(ENOENT);
		}
	}
}

fn main () {
	let mountpoint = Path::new(os::args()[1].as_slice());
	fuse::mount(HelloFS, &mountpoint, []);
}
