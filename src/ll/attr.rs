//! Low-level filesystem attributes.

use std::convert::TryFrom;
use std::os::unix::fs::FileTypeExt;
use std::time::SystemTime;
use std::{error, fmt, fs};


/// Error type returned when a `FileAttr` conversion fails.
#[derive(Debug)]
pub struct FileAttrTryFromError;

impl fmt::Display for FileAttrTryFromError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Could not convert invalid file attributes")
    }
}

impl error::Error for FileAttrTryFromError {}


/// File attributes.
///
/// Holds metadata required to represent a file in a filesystem. Besides the
/// inode number, which uniquely identifies a file, attributes contain more
/// useful metadata like file size, ownership information and permissions that
/// users and query and act upon.
///
/// This is the filesystem side representation of file metadata. On the user
/// side, Rust abstracts this information in `std::fs::Metadata`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileAttr {
    /// Inode number.
    pub ino: u64,
    /// Size in bytes.
    pub size: u64,
    /// Size in blocks.
    pub blocks: u64,
    /// Time of last access.
    pub atime: SystemTime,
    /// Time of last modification.
    pub mtime: SystemTime,
    /// Time of last change.
    pub ctime: SystemTime,
    /// macOS only: Time of creation.
    #[cfg(target_os = "macos")]
    pub crtime: SystemTime,
    /// Type of the file (e.g. regular file, directory, pipe, etc).
    pub ftype: FileType,
    /// File permissions.
    pub perm: u16,
    /// Number of hard links.
    pub nlink: u32,
    /// User id of file owner.
    pub uid: u32,
    /// Group id of file owner.
    pub gid: u32,
    /// Rdev.
    pub rdev: u32,
    /// macOS only: Flags (see chflags(2)).
    #[cfg(target_os = "macos")]
    pub flags: u32,
}

// TODO: Convert `std::fs::Metadata` to `FileAttr` if ever possible


/// Error type returned when a `FileType` conversion fails.
#[derive(Debug)]
pub struct FileTypeTryFromError;

impl fmt::Display for FileTypeTryFromError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Could not convert unknown file type")
    }
}

impl error::Error for FileTypeTryFromError {}


/// File type.
///
/// Determines the type of a file (e.g. wether it's a regular file or a
/// symlink).
///
/// This is the filesystem side representation of the type of a file. On the
/// user side, Rust abstracts this information in `std::fs::FileType`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FileType {
    /// Named pipe (FIFO).
    ///
    /// Also known as `S_IFIFO` in libc.
    NamedPipe,
    /// Character device.
    ///
    /// Also known as `S_IFCHR` in libc.
    CharDevice,
    /// Directory.
    ///
    /// Also known as `S_IFDIR` in libc.
    Directory,
    /// Block device.
    ///
    /// Also known as `S_IFBLK` in libc.
    BlockDevice,
    /// Regular file.
    ///
    /// Also known as `S_IFREG` in libc.
    RegularFile,
    /// Symbolic link.
    ///
    /// Also known as `S_IFLNK` in libc.
    Symlink,
    /// Unix domain socket.
    ///
    /// Also known as `S_IFSOCK` in libc.
    Socket,
}

impl TryFrom<fs::FileType> for FileType {
    type Error = FileTypeTryFromError;

    fn try_from(ft: fs::FileType) -> Result<Self, Self::Error> {
        if ft.is_fifo() {
            Ok(FileType::NamedPipe)
        } else if ft.is_char_device() {
            Ok(FileType::CharDevice)
        } else if ft.is_dir() {
            Ok(FileType::Directory)
        } else if ft.is_block_device() {
            Ok(FileType::BlockDevice)
        } else if ft.is_file() {
            Ok(FileType::RegularFile)
        } else if ft.is_symlink() {
            Ok(FileType::Symlink)
        } else if ft.is_socket() {
            Ok(FileType::Socket)
        } else {
            Err(FileTypeTryFromError)
        }
    }
}
