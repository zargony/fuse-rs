//! This is intended to demonstrate the ability to run filesystem operations asynchronously.
//! 
//! When listed it appears to have no files.  But if you attempt to read a file whose name is an
//! integer (less than a maximum), it will delay for that number seconds before returning EOF.  The
//! async nature can be demonstrating by seeing a shorter-delayed read return before a
//! longer-delayed one that was started first.

extern mod fuse;

use std::libc::{off_t, size_t, ENOENT, S_IFDIR, S_IFREG};
use std::default::Default;
use std::from_str::from_str;
use std::task;
use std::io::timer::sleep;
use fuse::{Request, Reply, fuse_open_out};
use fuse::consts::FOPEN_DIRECT_IO;

// The root is inode INO_ROOT, and the file that delays N seconds is inode N+INO_ROOT.
static INO_ROOT:u64 = 1;

// We won't delay by more than this, as a sanity check
static MAX_DELAY:u64 = 60;

struct DelayFS;

fn root_dir_attr () -> fuse::fuse_attr {
	fuse::fuse_attr {
		ino: 1, mode: S_IFDIR as u32 | 0o755, nlink: 2, uid: 501, gid: 20, ..Default::default()
	}
}

fn file_contents(num:u64) -> ~str {
	format!("This file took {:u} seconds to read.\n", num)
}

fn file_attr(num:u64) -> fuse::fuse_attr {
	let content = file_contents(num);
	fuse::fuse_attr {
		ino: INO_ROOT+num, size: content.as_bytes().len() as u64, mode: S_IFREG as u32 | 0o644, nlink: 1, uid: 501, gid: 20, ..Default::default()
	}
}

impl fuse::Filesystem for DelayFS {
	fn lookup(&mut self, req: fuse::Request<~fuse::fuse_entry_out>, parent: u64, name: &[u8]) -> fuse::Reply {
		if parent != INO_ROOT {
			return req.reply(Err(ENOENT));
		}

		let fname = std::str::from_utf8_opt(name);
		debug!("Looking up {:?}", fname);
		let delay_secs_opt:Option<u64> = fname.and_then(|s| from_str(s));
		let result = match delay_secs_opt {
			Some(delay_secs) if delay_secs <= MAX_DELAY => Ok(~fuse::fuse_entry_out { nodeid: INO_ROOT+delay_secs, generation: 0, attr: file_attr(delay_secs), entry_valid: 1, entry_valid_nsec: 0, attr_valid: 1, attr_valid_nsec: 0 }),
			_ => Err(ENOENT)
		};
		req.reply(result)
	}

	fn getattr (&mut self, req: fuse::Request<~fuse::fuse_attr_out>, ino: u64) -> fuse::Reply {
		let result = if ino <= MAX_DELAY+INO_ROOT {
			Ok(~fuse::fuse_attr_out { attr_valid: 1, attr_valid_nsec: 0, dummy: 0, attr: if ino == INO_ROOT { root_dir_attr() } else { file_attr(ino-INO_ROOT) } })
		} else { Err(ENOENT) };
		req.reply(result)
	}

	fn read (&mut self, req: fuse::Request<~[u8]>, ino: u64, _fh: u64, offset: off_t, _size: size_t) -> fuse::Reply {
		if ino <= INO_ROOT || ino > INO_ROOT+MAX_DELAY {
			return req.reply(Err(ENOENT));
		}
		
		do req.reply_async(task::SingleThreaded) |req| {
			let num_secs = ino - INO_ROOT;
			if offset == 0 {
				info!("Yawn...zzzzzz");
				sleep((num_secs)*1000);
				info!("Wakey wakey!");
			}
			let result = file_contents(num_secs);
			req.reply(Ok(result.as_bytes().tailn(offset as uint).into_owned()))
		}
	}

	fn readdir (&mut self, req: fuse::Request<~fuse::DirBuffer>, ino: u64, _fh: u64, _offset: off_t, buffer: ~fuse::DirBuffer) -> fuse::Reply {
		req.reply(if ino != INO_ROOT {
				Err(ENOENT)
			} else { 
				Ok(buffer)
			})
	}

	fn open (&mut self, req: Request<~fuse_open_out>, _ino: u64, _flags: uint) -> Reply { 
		req.reply(Ok(~fuse_open_out { fh: 0, open_flags: FOPEN_DIRECT_IO, padding: 0 })) 
	}
}

fn main () {
	let mountpoint = Path::new(::std::os::args()[1]);
	fuse::mount(DelayFS, &mountpoint, []);
}

