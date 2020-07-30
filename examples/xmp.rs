//! Analogue of fusexmp
//!
//! See also a more high-level example: https://github.com/wfraser/fuse-mt/tree/master/example

use fuse::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty,
    ReplyEntry, ReplyOpen, ReplyWrite, Request,
};
use libc::c_int;
use libc::{EINVAL, EIO, ENOENT, ENOSYS, EPERM};
use libc::{O_ACCMODE, O_APPEND, O_CREAT, O_EXCL, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY};
use std::env;
use std::ffi::{OsStr, OsString};
use std::time::{Duration, UNIX_EPOCH};

use std::collections::HashMap;
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::SystemTime;

use log::{error};

const TTL: Duration = Duration::from_secs(1); // 1 second

struct DirInfo {
    ino: u64,
    name: OsString,
    kind: FileType,
}

struct XmpFS {
    /// I don't want to include `slab` in dev-dependencies, so using a counter instead.
    /// This provides a source of new inodes and filehandles
    counter: u64,

    inode_to_path: HashMap<u64, OsString>,
    path_to_inode: HashMap<OsString, u64>,

    opened_directories: HashMap<u64, Vec<DirInfo>>,
    opened_files: HashMap<u64, std::fs::File>,
}

impl XmpFS {
    pub fn new() -> XmpFS {
        XmpFS {
            counter: 1,
            inode_to_path: HashMap::with_capacity(1024),
            path_to_inode: HashMap::with_capacity(1024),
            opened_directories: HashMap::with_capacity(2),
            opened_files: HashMap::with_capacity(2),
        }
    }

    pub fn populate_root_dir(&mut self) {
        let rootino = self.add_inode(OsStr::from_bytes(b"/"));
        assert_eq!(rootino, 1);
    }

    pub fn add_inode(&mut self, path: &OsStr) -> u64 {
        let ino = self.counter;
        self.counter += 1;
        self.path_to_inode.insert(path.to_os_string(), ino);
        self.inode_to_path.insert(ino, path.to_os_string());
        ino
    }

    pub fn add_or_create_inode(&mut self, path: impl AsRef<Path>) -> u64 {
        if let Some(x) = self.path_to_inode.get(path.as_ref().as_os_str()) {
            return *x;
        }

        self.add_inode(path.as_ref().as_os_str())
    }
    pub fn get_inode(&self, path: impl AsRef<Path>) -> Option<u64> {
        self.path_to_inode
            .get(path.as_ref().as_os_str())
            .map(|x| *x)
    }

    pub fn unregister_ino(&mut self, ino: u64) {
        if !self.inode_to_path.contains_key(&ino) {
            return;
        }
        self.path_to_inode.remove(&self.inode_to_path[&ino]);
        self.inode_to_path.remove(&ino);
    }
}

fn ft2ft(t: std::fs::FileType) -> FileType {
    match t {
        x if x.is_symlink() => FileType::Symlink,
        x if x.is_dir() => FileType::Directory,
        x if x.is_file() => FileType::RegularFile,
        x if x.is_fifo() => FileType::NamedPipe,
        x if x.is_char_device() => FileType::CharDevice,
        x if x.is_block_device() => FileType::BlockDevice,
        x if x.is_socket() => FileType::Socket,
        _ => FileType::RegularFile,
    }
}

fn meta2attr(m: &std::fs::Metadata, ino: u64) -> FileAttr {
    use std::convert::TryInto;
    FileAttr {
        ino,
        size: m.size(),
        blocks: m.blocks(),
        atime: m.accessed().unwrap_or(UNIX_EPOCH),
        mtime: m.modified().unwrap_or(UNIX_EPOCH),
        ctime: UNIX_EPOCH + Duration::from_secs(m.ctime().try_into().unwrap_or(0)),
        crtime: m.created().unwrap_or(UNIX_EPOCH),
        kind: ft2ft(m.file_type()),
        perm: m.permissions().mode() as u16,
        nlink: m.nlink() as u32,
        uid: m.uid(),
        gid: m.gid(),
        rdev: m.rdev() as u32,
        flags: 0,
    }
}

fn errhandle(e: std::io::Error, not_found: impl FnOnce() -> ()) -> libc::c_int {
    match e.kind() {
        ErrorKind::PermissionDenied => EPERM,
        ErrorKind::NotFound => {
            not_found();
            ENOENT
        }
        e => {
            error!("{:?}", e);
            EIO
        }
    }
}

impl Filesystem for XmpFS {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        if !self.inode_to_path.contains_key(&parent) {
            return reply.error(ENOENT);
        }

        let parent_path = Path::new(&self.inode_to_path[&parent]);
        let entry_path = parent_path.join(name);

        let entry_inode = self.get_inode(&entry_path);

        match std::fs::symlink_metadata(entry_path) {
            Err(e) => {
                reply.error(errhandle(e, || {
                    // if not found:
                    if let Some(ino) = entry_inode {
                        self.unregister_ino(ino);
                    }
                }));
            }
            Ok(m) => {
                let ino = match entry_inode {
                    Some(x) => x,
                    None => {
                        let parent_path = Path::new(&self.inode_to_path[&parent]);
                        let entry_path = parent_path.join(name);
                        self.add_or_create_inode(entry_path)
                    }
                };

                let attr: FileAttr = meta2attr(&m, ino);

                reply.entry(&TTL, &attr, 1);
            }
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        if !self.inode_to_path.contains_key(&ino) {
            return reply.error(ENOENT);
        }

        let entry_path = Path::new(&self.inode_to_path[&ino]);

        match std::fs::symlink_metadata(entry_path) {
            Err(e) => {
                reply.error(errhandle(e, || {
                    // if not found:
                    self.unregister_ino(ino);
                }));
            }
            Ok(m) => {
                let attr: FileAttr = meta2attr(&m, ino);
                reply.attr(&TTL, &attr);
            }
        }
    }

    fn open(&mut self, _req: &Request, ino: u64, flags: u32, reply: ReplyOpen) {
        if !self.inode_to_path.contains_key(&ino) {
            return reply.error(ENOENT);
        }

        let entry_path = Path::new(&self.inode_to_path[&ino]);

        let mut oo = std::fs::OpenOptions::new();

        let fl = flags as c_int;
        match fl & O_ACCMODE {
            O_RDONLY => {
                oo.read(true);
                oo.write(false);
            }
            O_WRONLY => {
                oo.read(false);
                oo.write(true);
            }
            O_RDWR => {
                oo.read(true);
                oo.write(true);
            }
            _ => return reply.error(EINVAL),
        }

        oo.create(false);
        if fl & (O_EXCL | O_CREAT) != 0 {
            error!("Wrong flags on open");
            return reply.error(EIO);
        }

        oo.append(fl & O_APPEND == O_APPEND);
        oo.truncate(fl & O_TRUNC == O_TRUNC);

        match oo.open(entry_path) {
            Err(e) => reply.error(errhandle(e, || self.unregister_ino(ino))),
            Ok(f) => {
                let fh = self.counter;
                self.counter += 1;

                self.opened_files.insert(fh, f);
                reply.opened(fh, 0);
            }
        }
    }

    fn create(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        mode: u32,
        flags: u32,
        reply: ReplyCreate,
    ) {
        if !self.inode_to_path.contains_key(&parent) {
            return reply.error(ENOENT);
        }

        let parent_path = Path::new(&self.inode_to_path[&parent]);
        let entry_path = parent_path.join(name);

        let ino = self.add_or_create_inode(&entry_path);

        let mut oo = std::fs::OpenOptions::new();

        let fl = flags as c_int;
        match fl & O_ACCMODE {
            O_RDONLY => {
                oo.read(true);
                oo.write(false);
            }
            O_WRONLY => {
                oo.read(false);
                oo.write(true);
            }
            O_RDWR => {
                oo.read(true);
                oo.write(true);
            }
            _ => return reply.error(EINVAL),
        }

        oo.create(fl & O_CREAT == O_CREAT);
        oo.create_new(fl & O_EXCL == O_EXCL);
        oo.append(fl & O_APPEND == O_APPEND);
        oo.truncate(fl & O_TRUNC == O_TRUNC);
        oo.mode(mode);

        match oo.open(&entry_path) {
            Err(e) => return reply.error(errhandle(e, || self.unregister_ino(ino))),
            Ok(f) => {
                let meta = match std::fs::symlink_metadata(entry_path) {
                    Err(e) => {
                        return reply.error(errhandle(e, || self.unregister_ino(ino)));
                    }
                    Ok(m) => meta2attr(&m, ino),
                };
                let fh = self.counter;
                self.counter += 1;

                self.opened_files.insert(fh, f);
                reply.created(&TTL, &meta, 1, fh, 0);
            }
        }
    }

    fn read(
        &mut self,
        _req: &Request,
        _ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        reply: ReplyData,
    ) {
        if !self.opened_files.contains_key(&fh) {
            return reply.error(EIO);
        }
        let size = size as usize;

        let f = self.opened_files.get_mut(&fh).unwrap();

        let mut b = Vec::with_capacity(size);
        b.resize(size, 0);

        use std::os::unix::fs::FileExt;

        let mut bo = 0;
        while bo < size {
            match f.read_at(&mut b[bo..], offset as u64) {
                Err(e) => return reply.error(errhandle(e, || ())),
                Ok(0) => {
                    b.resize(bo, 0);
                    break;
                }
                Ok(ret) => {
                    bo += ret;
                }
            };
        }

        reply.data(&b[..]);
    }

    fn write(
        &mut self,
        _req: &Request,
        _ino: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        _flags: u32,
        reply: ReplyWrite,
    ) {
        if !self.opened_files.contains_key(&fh) {
            return reply.error(EIO);
        }

        let f = self.opened_files.get_mut(&fh).unwrap();

        use std::os::unix::fs::FileExt;

        match f.write_all_at(data, offset as u64) {
            Err(e) => return reply.error(errhandle(e, || ())),
            Ok(()) => {
                reply.written(data.len() as u32);
            }
        };
    }

    fn fsync(&mut self, _req: &Request, _ino: u64, fh: u64, datasync: bool, reply: ReplyEmpty) {
        if !self.opened_files.contains_key(&fh) {
            return reply.error(EIO);
        }

        let f = self.opened_files.get_mut(&fh).unwrap();

        match if datasync {
            f.sync_data()
        } else {
            f.sync_all()
        } {
            Err(e) => return reply.error(errhandle(e, || ())),
            Ok(()) => {
                reply.ok();
            }
        }
    }

    fn fsyncdir(
        &mut self,
        _req: &Request,
        _ino: u64,
        _fh: u64,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        // I'm not sure how to do I with libstd
        reply.ok();
    }

    fn release(
        &mut self,
        _req: &Request,
        _ino: u64,
        fh: u64,
        _flags: u32,
        _lock_owner: u64,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        if !self.opened_files.contains_key(&fh) {
            return reply.error(EIO);
        }

        self.opened_files.remove(&fh);
        reply.ok();
    }

    fn opendir(&mut self, _req: &Request, ino: u64, _flags: u32, reply: ReplyOpen) {
        if !self.inode_to_path.contains_key(&ino) {
            return reply.error(ENOENT);
        }

        let entry_path = Path::new(&self.inode_to_path[&ino]).to_owned();

        match std::fs::read_dir(&entry_path) {
            Err(e) => {
                reply.error(errhandle(e, || ()));
            }
            Ok(x) => {
                let mut v: Vec<DirInfo> = Vec::with_capacity(x.size_hint().0);

                let parent_ino: u64 = if ino == 1 {
                    1
                } else {
                    match entry_path.parent() {
                        None => ino,
                        Some(x) => *self.path_to_inode.get(x.as_os_str()).unwrap_or(&ino),
                    }
                };

                v.push(DirInfo {
                    ino: ino,
                    kind: FileType::Directory,
                    name: OsStr::from_bytes(b".").to_os_string(),
                });
                v.push(DirInfo {
                    ino: parent_ino,
                    kind: FileType::Directory,
                    name: OsStr::from_bytes(b"..").to_os_string(),
                });

                for dee in x {
                    match dee {
                        Err(e) => {
                            reply.error(errhandle(e, || ()));
                            return;
                        }
                        Ok(de) => {
                            let name = de.file_name().to_os_string();
                            let kind = de.file_type().map(ft2ft).unwrap_or(FileType::RegularFile);
                            let jp = entry_path.join(&name);
                            let ino = self.add_or_create_inode(jp);

                            v.push(DirInfo { ino, kind, name });
                        }
                    }
                }
                let fh = self.counter;
                self.opened_directories.insert(fh, v);
                self.counter += 1;
                reply.opened(fh, 0);
            }
        }
    }

    fn readdir(
        &mut self,
        _req: &Request,
        _ino: u64,
        fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        if !self.opened_directories.contains_key(&fh) {
            error!("no fh {} for readdir", fh);
            return reply.error(EIO);
        }

        let entries = &self.opened_directories[&fh];

        for (i, entry) in entries.iter().enumerate().skip(offset as usize) {
            // i + 1 means the index of the next entry
            if reply.add(entry.ino, (i + 1) as i64, entry.kind, &entry.name) {
                break;
            }
        }
        reply.ok();
    }

    fn releasedir(&mut self, _req: &Request, _ino: u64, fh: u64, _flags: u32, reply: ReplyEmpty) {
        if !self.opened_directories.contains_key(&fh) {
            return reply.error(EIO);
        }

        self.opened_directories.remove(&fh);
        reply.ok();
    }

    fn readlink(&mut self, _req: &Request, ino: u64, reply: ReplyData) {
        if !self.inode_to_path.contains_key(&ino) {
            return reply.error(ENOENT);
        }

        let entry_path = Path::new(&self.inode_to_path[&ino]);

        match std::fs::read_link(entry_path) {
            Err(e) => reply.error(errhandle(e, || self.unregister_ino(ino))),
            Ok(x) => {
                reply.data(x.as_os_str().as_bytes());
            }
        }
    }

    fn mkdir(&mut self, _req: &Request, parent: u64, name: &OsStr, _mode: u32, reply: ReplyEntry) {
        if !self.inode_to_path.contains_key(&parent) {
            return reply.error(ENOENT);
        }

        let parent_path = Path::new(&self.inode_to_path[&parent]);
        let entry_path = parent_path.join(name);

        let ino = self.add_or_create_inode(&entry_path);
        match std::fs::create_dir(&entry_path) {
            Err(e) => reply.error(errhandle(e, || ())),
            Ok(()) => {
                let attr = match std::fs::symlink_metadata(entry_path) {
                    Err(e) => {
                        return reply.error(errhandle(e, || self.unregister_ino(ino)));
                    }
                    Ok(m) => meta2attr(&m, ino),
                };

                reply.entry(&TTL, &attr, 1);
            }
        }
    }

    fn unlink(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        if !self.inode_to_path.contains_key(&parent) {
            return reply.error(ENOENT);
        }

        let parent_path = Path::new(&self.inode_to_path[&parent]);
        let entry_path = parent_path.join(name);

        match std::fs::remove_file(entry_path) {
            Err(e) => reply.error(errhandle(e, || ())),
            Ok(()) => {
                reply.ok();
            }
        }
    }

    fn rmdir(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        if !self.inode_to_path.contains_key(&parent) {
            return reply.error(ENOENT);
        }

        let parent_path = Path::new(&self.inode_to_path[&parent]);
        let entry_path = parent_path.join(name);

        match std::fs::remove_dir(entry_path) {
            Err(e) => reply.error(errhandle(e, || ())),
            Ok(()) => {
                reply.ok();
            }
        }
    }

    fn symlink(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        link: &Path,
        reply: ReplyEntry,
    ) {
        if !self.inode_to_path.contains_key(&parent) {
            return reply.error(ENOENT);
        }

        let parent_path = Path::new(&self.inode_to_path[&parent]);
        let entry_path = parent_path.join(name);
        let ino = self.add_or_create_inode(&entry_path);

        match std::os::unix::fs::symlink(&entry_path, link) {
            Err(e) => reply.error(errhandle(e, || self.unregister_ino(ino))),
            Ok(()) => {
                let attr = match std::fs::symlink_metadata(entry_path) {
                    Err(e) => {
                        return reply.error(errhandle(e, || self.unregister_ino(ino)));
                    }
                    Ok(m) => meta2attr(&m, ino),
                };

                reply.entry(&TTL, &attr, 1);
            }
        }
    }

    fn rename(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        reply: ReplyEmpty,
    ) {
        if !self.inode_to_path.contains_key(&parent) {
            return reply.error(ENOENT);
        }
        if !self.inode_to_path.contains_key(&newparent) {
            return reply.error(ENOENT);
        }

        let parent_path = Path::new(&self.inode_to_path[&parent]);
        let newparent_path = Path::new(&self.inode_to_path[&newparent]);
        let entry_path = parent_path.join(name);
        let newentry_path = newparent_path.join(newname);

        if entry_path == newentry_path {
            return reply.ok();
        }

        let ino = self.add_or_create_inode(&entry_path);

        match std::fs::rename(&entry_path, &newentry_path) {
            Err(e) => reply.error(errhandle(e, || self.unregister_ino(ino))),
            Ok(()) => {
                self.inode_to_path
                    .insert(ino, newentry_path.as_os_str().to_os_string());
                self.path_to_inode.remove(entry_path.as_os_str());
                self.path_to_inode
                    .insert(newentry_path.as_os_str().to_os_string(), ino);
                reply.ok();
            }
        }
    }

    fn link(
        &mut self,
        _req: &Request,
        ino: u64,
        newparent: u64,
        newname: &OsStr,
        reply: ReplyEntry,
    ) {
        // Not a true hardlink: new inode

        if !self.inode_to_path.contains_key(&ino) {
            return reply.error(ENOENT);
        }
        if !self.inode_to_path.contains_key(&newparent) {
            return reply.error(ENOENT);
        }

        let entry_path = Path::new(&self.inode_to_path[&ino]).to_owned();
        let newparent_path = Path::new(&self.inode_to_path[&newparent]);
        let newentry_path = newparent_path.join(newname);

        let newino = self.add_or_create_inode(&newentry_path);

        match std::fs::hard_link(&entry_path, &newentry_path) {
            Err(e) => reply.error(errhandle(e, || self.unregister_ino(ino))),
            Ok(()) => {
                let attr = match std::fs::symlink_metadata(newentry_path) {
                    Err(e) => {
                        return reply.error(errhandle(e, || self.unregister_ino(newino)));
                    }
                    Ok(m) => meta2attr(&m, newino),
                };

                reply.entry(&TTL, &attr, 1);
            }
        }
    }
    fn mknod(
        &mut self,
        _req: &Request,
        _parent: u64,
        _name: &OsStr,
        _mode: u32,
        _rdev: u32,
        reply: ReplyEntry,
    ) {
        // no mknod lib libstd
        reply.error(ENOSYS);
    }

    fn setattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<SystemTime>,
        _mtime: Option<SystemTime>,
        fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        // Limited to setting file length only

        let (fh, sz) = match (fh, size) {
            (Some(x), Some(y)) => (x, y),
            _ => {
                // only partial for chmod +x, and not the good one

                let entry_path = Path::new(&self.inode_to_path[&ino]).to_owned();

                if let Some(mode) = mode {
                    use std::fs::Permissions;

                    let perm = Permissions::from_mode(mode);
                    match std::fs::set_permissions(&entry_path, perm) {
                        Err(e) => return reply.error(errhandle(e, || self.unregister_ino(ino))),
                        Ok(()) => {
                            let attr = match std::fs::symlink_metadata(entry_path) {
                                Err(e) => {
                                    return reply.error(errhandle(e, || self.unregister_ino(ino)));
                                }
                                Ok(m) => meta2attr(&m, ino),
                            };

                            return reply.attr(&TTL, &attr);
                        }
                    }
                } else {
                    // Just try to do nothing, successfully.
                    let attr = match std::fs::symlink_metadata(entry_path) {
                        Err(e) => {
                            return reply.error(errhandle(e, || self.unregister_ino(ino)));
                        }
                        Ok(m) => meta2attr(&m, ino),
                    };

                    return reply.attr(&TTL, &attr);
                }
            }
        };

        if !self.opened_files.contains_key(&fh) {
            return reply.error(EIO);
        }

        let f = self.opened_files.get_mut(&fh).unwrap();

        match f.set_len(sz) {
            Err(e) => reply.error(errhandle(e, || ())),
            Ok(()) => {
                // pull regular file metadata out of thin air

                let attr = FileAttr {
                    ino,
                    size: sz,
                    blocks: 1,
                    atime: UNIX_EPOCH,
                    mtime: UNIX_EPOCH,
                    ctime: UNIX_EPOCH,
                    crtime: UNIX_EPOCH,
                    kind: FileType::RegularFile,
                    perm: 0o644,
                    nlink: 1,
                    uid: 0,
                    gid: 0,
                    rdev: 0,
                    flags: 0,
                };

                reply.attr(&TTL, &attr);
            }
        }
    }
}

fn main() {
    env_logger::init();
    let mountpoint = env::args_os().nth(1).unwrap();
    let options = ["-o", "rw,default_permissions", "-o", "fsname=xmp"]
        .iter()
        .map(|o| o.as_ref())
        .collect::<Vec<&OsStr>>();
    let mut xmp = XmpFS::new();
    xmp.populate_root_dir();
    fuse::mount(xmp, mountpoint, &options).unwrap();
}
