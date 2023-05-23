extern crate env_logger;
extern crate fuse;
extern crate libc;
extern crate time;

use std::env;
use std::ffi::OsStr;
use libc::ENOENT;
use time::Timespec;
use fuse::{FileType, FileAttr, Filesystem, Request, ReplyData, ReplyEntry, ReplyAttr, ReplyDirectory, ReplyWrite, ReplyOpen, ReplyCreate, ReplyEmpty};

const TTL: Timespec = Timespec { sec: 1, nsec: 0 };                     // 1 second

const CREATE_TIME: Timespec = Timespec { sec: 1381237736, nsec: 0 };    // 2013-10-08 08:56

const HELLO_DIR_ATTR: FileAttr = FileAttr {
    ino: 1,
    size: 0,
    blocks: 0,
    atime: CREATE_TIME,
    mtime: CREATE_TIME,
    ctime: CREATE_TIME,
    crtime: CREATE_TIME,
    kind: FileType::Directory,
    perm: 0o755,
    nlink: 2,
    uid: 501,
    gid: 20,
    rdev: 0,
    flags: 0,
};

const HELLO_TXT_ATTR: FileAttr = FileAttr {
    ino: 2,
    size: 13,
    blocks: 1,
    atime: CREATE_TIME,
    mtime: CREATE_TIME,
    ctime: CREATE_TIME,
    crtime: CREATE_TIME,
    kind: FileType::RegularFile,
    perm: 0o644,
    nlink: 1,
    uid: 501,
    gid: 20,
    rdev: 0,
    flags: 0,
};

struct HelloFS {
    hello_txt_content: Vec<u8>,
}

impl HelloFS {
    fn hello_txt_attr(&self) -> FileAttr {
        let mut attr = HELLO_TXT_ATTR;
        attr.size = self.hello_txt_content.len() as u64;
        attr
    }
}

impl Filesystem for HelloFS {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        if parent == 1 && name.to_str() == Some("hello.txt") {
            let attr = self.hello_txt_attr();
            reply.entry(&TTL, &attr, 0);
        } else {
            reply.error(ENOENT);
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        match ino {
            1 => reply.attr(&TTL, &HELLO_DIR_ATTR),
            2 => reply.attr(&TTL, &self.hello_txt_attr()),
            _ => reply.error(ENOENT),
        }
    }

    fn setattr(&mut self, _req: &Request, ino: u64, _mode: Option<u32>, _uid: Option<u32>, _gid: Option<u32>, _size: Option<u64>, _atime: Option<Timespec>, _mtime: Option<Timespec>, _fh: Option<u64>, _crtime: Option<Timespec>, _chgtime: Option<Timespec>, _bkuptime: Option<Timespec>, _flags: Option<u32>, reply: ReplyAttr) {
        match ino {
            1 => reply.attr(&TTL, &HELLO_DIR_ATTR),
            2 => reply.attr(&TTL, &self.hello_txt_attr()),
            _ => reply.error(ENOENT),
        }
    }

    fn open(&mut self, _req: &Request, ino: u64, flags: u32, reply: ReplyOpen) {
        if ino == 2 {
            reply.opened(0, flags)
        } else {
            reply.error(ENOENT);
        }
    }

    fn read(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, size: u32, reply: ReplyData) {
        if ino == 2 {
            let offset = offset as usize;
            let size = size as usize;
            let read_size = size.min(self.hello_txt_content.len() - offset);
            reply.data(&self.hello_txt_content.as_slice()[offset .. offset + read_size]);
        } else {
            reply.error(ENOENT);
        }
    }

    fn write(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, data: &[u8], _flags: u32, reply: ReplyWrite) {
        if ino == 2 {
            let offset = offset as usize;
            let overwrite_len = data.len().min(self.hello_txt_content.len() - offset);
            self.hello_txt_content.as_mut_slice()[offset .. offset + overwrite_len].copy_from_slice(&data[.. overwrite_len]);
            self.hello_txt_content.extend_from_slice(&data[overwrite_len ..]);
            reply.written(data.len() as u32)
        } else {
            reply.error(ENOENT);
        }
    }

    fn readdir(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, mut reply: ReplyDirectory) {
        if ino == 1 {
            if offset == 0 {
                reply.add(1, 0, FileType::Directory, ".");
                reply.add(1, 1, FileType::Directory, "..");
                reply.add(2, 2, FileType::RegularFile, "hello.txt");
            }
            reply.ok();
        } else {
            reply.error(ENOENT);
        }
    }
}

fn main() {
    env_logger::init().unwrap();
    let mountpoint = env::args_os().nth(1).unwrap();
    fuse::mount(HelloFS { hello_txt_content: vec![] }, &mountpoint, &[]).unwrap();
}

#[test]
fn stale_data_bug() {
    use std::io::Read;
    use std::io::Write;
    let mut file_read = std::fs::File::open("/tmp/mnt/hello.txt").unwrap();
    let mut file_write = std::fs::File::create("/tmp/mnt/hello.txt").unwrap();
    file_write.write_all("Init".as_bytes()).unwrap();
    let mut buffer1 = vec![];
    file_read.read_to_end(&mut buffer1).unwrap();
    println!("buffer1 {:?}", buffer1);
    file_write.write_all("Hello World!".as_bytes()).unwrap();
    let mut buffer2 = vec![];
    file_read.read_to_end(&mut buffer2).unwrap();
    println!("buffer2 {:?}", buffer2);
}