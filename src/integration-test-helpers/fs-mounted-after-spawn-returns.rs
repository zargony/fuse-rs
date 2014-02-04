extern mod fuse;

// This is a regression test against issue #9.

use std::libc::{c_int, S_IFDIR};
use std::path::Path;
use std::os;
use std::io::fs::stat;

/// Responds to any lookup with the same set of attributes.  The only purpose is to behave in a way
/// such that the test can tell it's in effect.
struct TestFS;

fn dir_attr () -> fuse::fuse_attr {
	fuse::fuse_attr {
		ino: 999, mode: S_IFDIR as u32 | 0o755, nlink: 2, uid: 501, gid: 20, ..Default::default()
	}
}

impl fuse::Filesystem for TestFS {

	fn getattr(&mut self, _ino: u64) -> Result<~fuse::fuse_attr_out, c_int> {
		Ok(~fuse::fuse_attr_out { attr_valid: 1, attr_valid_nsec: 0, dummy: 0, attr: dir_attr() })
	}
}

fn main() {
	let mountpoint = Path::new(os::args()[1]);
	let _mounter = fuse::spawn_mount(TestFS, &mountpoint, []);
	assert_eq!(stat(&mountpoint).unstable.inode, 999);
}
