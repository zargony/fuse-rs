extern crate fuse;
extern crate libc;
extern crate tempdir;
extern crate time;

use fuse::{scope, spawn_mount, FileAttr, FileType, Filesystem, ReplyDirectory, ReplyEntry, Request};
use libc::ENOENT;
use std::ffi::OsStr;
use tempdir::TempDir;
use time::Timespec;

struct SingleFs {}
impl Filesystem for SingleFs {
    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        if ino != 0 || offset != 0 {
            reply.ok();
        } else {
            reply.add(0, 0, FileType::Directory, &OsStr::new("single"));
            reply.ok();
        }
    }

    fn lookup(&mut self, _req: &Request, _parent: u64, name: &OsStr, reply: ReplyEntry) {
        if name != OsStr::new("single") {
            reply.error(ENOENT);
        } else {
            let ttl = Timespec::new(1, 0);
            let now = time::get_time();
            let attr = FileAttr {
                ino: 2,
                size: 0,
                blocks: 0,
                atime: now,
                mtime: now,
                ctime: now,
                crtime: now,
                kind: FileType::Directory,
                perm: 0o755,
                nlink: 0,
                uid: 0,
                gid: 0,
                rdev: 0,
                flags: 0,
            };
            reply.entry(&ttl, &attr, 0);
        }
    }
}

fn main() {
    let fs = SingleFs {};
    let tempdir = TempDir::new("single").unwrap();
    let mountpoint = tempdir.path();
    let options = ["-o", "ro", "-o", "fsname=single"]
        .iter()
        .map(|o| o.as_ref())
        .collect::<Vec<&OsStr>>();
    scope(|scope| {
        {
            let _session = spawn_mount(fs, &mountpoint, &options, &scope);
            assert!(mountpoint.join("single").exists());
        }
        assert!(!mountpoint.join("single").exists());
    })
    .unwrap();
    println!("ok");
}
