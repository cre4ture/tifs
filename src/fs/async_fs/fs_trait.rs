use std::time::SystemTime;

use async_trait::async_trait;
use bytestring::ByteString;
use fuser::{KernelConfig, TimeOrNow};

use crate::fs::{error::FsError, reply::*};

use crate::fs::error::Result;


#[allow(clippy::too_many_arguments)]
#[async_trait]
pub trait AsyncFileSystem: Send + Sync {
    /// Initialize filesystem.
    /// Called before any other filesystem method.
    /// The kernel module connection can be configured using the KernelConfig object
    async fn init(&self, _gid: u32, _uid: u32, _config: &mut KernelConfig) -> Result<()> {
        Ok(())
    }

    /// Clean up filesystem.
    /// Called on filesystem exit.
    async fn destroy(&self) {}

    /// Look up a directory entry by name and get its attributes.
    async fn lookup(&self, _parent: u64, _name: ByteString) -> Result<Entry> {
        Err(FsError::unimplemented())
    }

    /// Forget about an inode.
    /// The nlookup parameter indicates the number of lookups previously performed on
    /// this inode. If the filesystem implements inode lifetimes, it is recommended that
    /// inodes acquire a single reference on each lookup, and lose nlookup references on
    /// each forget. The filesystem may ignore forget calls, if the inodes don't need to
    /// have a limited lifetime. On unmount it is not guaranteed, that all referenced
    /// inodes will receive a forget message.
    async fn forget(&self, _ino: u64, _nlookup: u64) {}

    /// Get file attributes.
    async fn getattr(&self, _ino: u64) -> Result<Attr> {
        Err(FsError::unimplemented())
    }

    /// Set file attributes.
    async fn setattr(
        &self,
        _ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        _size: Option<u64>,
        _atime: Option<TimeOrNow>,
        _mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
    ) -> Result<Attr> {
        Err(FsError::unimplemented())
    }

    /// Read symbolic link.
    async fn readlink(&self, _ino: u64) -> Result<Data> {
        Err(FsError::unimplemented())
    }

    /// Create file node.
    /// Create a regular file, character device, block device, fifo or socket node.
    async fn mknod(
        &self,
        _parent: u64,
        _name: ByteString,
        _mode: u32,
        _gid: u32,
        _uid: u32,
        _umask: u32,
        _rdev: u32,
    ) -> Result<Entry> {
        Err(FsError::unimplemented())
    }

    /// Create a directory.
    async fn mkdir(
        &self,
        _parent: u64,
        _name: ByteString,
        _mode: u32,
        _gid: u32,
        _uid: u32,
        _umask: u32,
    ) -> Result<Entry> {
        Err(FsError::unimplemented())
    }

    /// Remove a file.
    async fn unlink(&self, _parent: u64, _name: ByteString) -> Result<()> {
        Err(FsError::unimplemented())
    }

    /// Remove a directory.
    async fn rmdir(&self, _parent: u64, _name: ByteString) -> Result<()> {
        Err(FsError::unimplemented())
    }

    /// Create a symbolic link.
    async fn symlink(
        &self,
        _gid: u32,
        _uid: u32,
        _parent: u64,
        _name: ByteString,
        _link: ByteString,
    ) -> Result<Entry> {
        Err(FsError::unimplemented())
    }

    /// Rename a file.
    async fn rename(
        &self,
        _parent: u64,
        _name: ByteString,
        _newparent: u64,
        _newname: ByteString,
        _flags: u32,
    ) -> Result<()> {
        Err(FsError::unimplemented())
    }

    /// Create a hard link.
    async fn link(&self, _ino: u64, _newparent: u64, _newname: ByteString) -> Result<Entry> {
        Err(FsError::unimplemented())
    }

    /// Open a file.
    /// Open flags (with the exception of O_CREAT, O_EXCL, O_NOCTTY and O_TRUNC) are
    /// available in flags. Filesystem may store an arbitrary file handle (pointer, index,
    /// etc) in fh, and use this in other all other file operations (read, write, flush,
    /// release, fsync). Filesystem may also implement stateless file I/O and not store
    /// anything in fh. There are also some flags (direct_io, keep_cache) which the
    /// filesystem may set, to change the way the file is opened. See fuse_file_info
    /// structure in <fuse_common.h> for more details.
    async fn open(&self, _ino: u64, _flags: i32) -> Result<Open> {
        Ok(Open::new(0, 0))
    }

    /// Read data.
    /// Read should send exactly the number of bytes requested except on EOF or error,
    /// otherwise the rest of the data will be substituted with zeroes. An exception to
    /// this is when the file has been opened in 'direct_io' mode, in which case the
    /// return value of the read system call will reflect the return value of this
    /// operation. fh will contain the value set by the open method, or will be undefined
    /// if the open method didn't set any value.
    ///
    /// flags: these are the file flags, such as O_SYNC. Only supported with ABI >= 7.9
    /// lock_owner: only supported with ABI >= 7.9
    async fn read(
        &self,
        _ino: u64,
        _fh: u64,
        _offset: i64,
        _size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
    ) -> Result<Data> {
        Err(FsError::unimplemented())
    }

    /// Write data.
    /// Write should return exactly the number of bytes requested except on error. An
    /// exception to this is when the file has been opened in 'direct_io' mode, in
    /// which case the return value of the write system call will reflect the return
    /// value of this operation. fh will contain the value set by the open method, or
    /// will be undefined if the open method didn't set any value.
    ///
    /// write_flags: will contain FUSE_WRITE_CACHE, if this write is from the page cache. If set,
    /// the pid, uid, gid, and fh may not match the value that would have been sent if write cachin
    /// is disabled
    /// flags: these are the file flags, such as O_SYNC. Only supported with ABI >= 7.9
    /// lock_owner: only supported with ABI >= 7.9
    async fn write(
        &self,
        _ino: u64,
        _fh: u64,
        _offset: i64,
        _data: Vec<u8>,
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
    ) -> Result<Write> {
        Err(FsError::unimplemented())
    }

    /// Flush method.
    /// This is called on each close() of the opened file. Since file descriptors can
    /// be duplicated (dup, dup2, fork), for one open call there may be many flush
    /// calls. Filesystems shouldn't assume that flush will always be called after some
    /// writes, or that if will be called at all. fh will contain the value set by the
    /// open method, or will be undefined if the open method didn't set any value.
    /// NOTE: the name of the method is misleading, since (unlike fsync) the filesystem
    /// is not forced to flush pending writes. One reason to flush data, is if the
    /// filesystem wants to return write errors. If the filesystem supports file locking
    /// operations (setlk, getlk) it should remove all locks belonging to 'lock_owner'.
    async fn flush(&self, _ino: u64, _fh: u64, _lock_owner: u64) -> Result<()> {
        Err(FsError::unimplemented())
    }

    /// Release an open file.
    /// Release is called when there are no more references to an open file: all file
    /// descriptors are closed and all memory mappings are unmapped. For every open
    /// call there will be exactly one release call. The filesystem may reply with an
    /// error, but error values are not returned to close() or munmap() which triggered
    /// the release. fh will contain the value set by the open method, or will be undefined
    /// if the open method didn't set any value. flags will contain the same flags as for
    /// open.
    async fn release(
        &self,
        _ino: u64,
        _fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
    ) -> Result<()> {
        Ok(())
    }

    /// Synchronize file contents.
    /// If the datasync parameter is non-zero, then only the user data should be flushed,
    /// not the meta data.
    async fn fsync(&self, _ino: u64, _fh: u64, _datasync: bool) -> Result<()> {
        Err(FsError::unimplemented())
    }

    /// Open a directory.
    /// Filesystem may store an arbitrary file handle (pointer, index, etc) in fh, and
    /// use this in other all other directory stream operations (readdir, releasedir,
    /// fsyncdir). Filesystem may also implement stateless directory I/O and not store
    /// anything in fh, though that makes it impossible to implement standard conforming
    /// directory stream operations in case the contents of the directory can change
    /// between opendir and releasedir.
    async fn opendir(&self, _ino: u64, _flags: i32) -> Result<Open> {
        Ok(Open::new(0, 0))
    }

    /// Read directory.
    /// Send a buffer filled using buffer.fill(), with size not exceeding the
    /// requested size. Send an empty buffer on end of stream. fh will contain the
    /// value set by the opendir method, or will be undefined if the opendir method
    /// didn't set any value.
    async fn readdir(&self, _ino: u64, _fh: u64, offset: i64) -> Result<Dir> {
        Ok(Dir::offset(offset as usize))
    }

    /// Read directory.
    /// Send a buffer filled using buffer.fill(), with size not exceeding the
    /// requested size. Send an empty buffer on end of stream. fh will contain the
    /// value set by the opendir method, or will be undefined if the opendir method
    /// didn't set any value.
    async fn readdirplus(&self, _ino: u64, _fh: u64, offset: i64) -> Result<DirPlus> {
        Ok(DirPlus::offset(offset as usize))
    }

    /// Release an open directory.
    /// For every opendir call there will be exactly one releasedir call. fh will
    /// contain the value set by the opendir method, or will be undefined if the
    /// opendir method didn't set any value.
    async fn releasedir(&self, _ino: u64, _fh: u64, _flags: i32) -> Result<()> {
        Err(FsError::unimplemented())
    }

    /// Synchronize directory contents.
    /// If the datasync parameter is set, then only the directory contents should
    /// be flushed, not the meta data. fh will contain the value set by the opendir
    /// method, or will be undefined if the opendir method didn't set any value.
    async fn fsyncdir(&self, _ino: u64, _fh: u64, _datasync: bool) -> Result<()> {
        Err(FsError::unimplemented())
    }

    /// Get file system statistics.
    async fn statfs(&self, _ino: u64) -> Result<StatFs> {
        Ok(StatFs::new(0, 0, 0, 0, 0, 512, 255, 0))
    }

    /// Set an extended attribute.
    async fn setxattr(
        &self,
        _ino: u64,
        _name: ByteString,
        _value: Vec<u8>,
        _flags: i32,
        _position: u32,
    ) -> Result<()> {
        Err(FsError::unimplemented())
    }

    /// Get an extended attribute.
    /// If `size` is 0, the size of the value should be sent with `reply.size()`.
    /// If `size` is not 0, and the value fits, send it with `reply.data()`, or
    /// `reply.error(ERANGE)` if it doesn't.
    async fn getxattr(&self, _ino: u64, _name: ByteString, _size: u32) -> Result<Xattr> {
        Err(FsError::unimplemented())
    }

    /// List extended attribute names.
    /// If `size` is 0, the size of the value should be sent with `reply.size()`.
    /// If `size` is not 0, and the value fits, send it with `reply.data()`, or
    /// `reply.error(ERANGE)` if it doesn't.
    async fn listxattr(&self, _ino: u64, _size: u32) -> Result<Xattr> {
        Err(FsError::unimplemented())
    }

    /// Remove an extended attribute.
    async fn removexattr(&self, _ino: u64, _name: ByteString) -> Result<()> {
        Err(FsError::unimplemented())
    }

    /// Check file access permissions.
    /// This will be called for the access() system call. If the 'default_permissions'
    /// mount option is given, this method is not called. This method is not called
    /// under Linux kernel versions 2.4.x
    async fn access(&self, _ino: u64, _mask: i32) -> Result<()> {
        Err(FsError::unimplemented())
    }

    /// Create and open a file.
    /// If the file does not exist, first create it with the specified mode, and then
    /// open it. Open flags (with the exception of O_NOCTTY) are available in flags.
    /// Filesystem may store an arbitrary file handle (pointer, index, etc) in fh,
    /// and use this in other all other file operations (read, write, flush, release,
    /// fsync). There are also some flags (direct_io, keep_cache) which the
    /// filesystem may set, to change the way the file is opened. See fuse_file_info
    /// structure in <fuse_common.h> for more details. If this method is not
    /// implemented or under Linux kernel versions earlier than 2.6.15, the mknod()
    /// and open() methods will be called instead.
    async fn create(
        &self,
        _uid: u32,
        _gid: u32,
        _parent: u64,
        _name: ByteString,
        _mode: u32,
        _umask: u32,
        _flags: i32,
    ) -> Result<Create> {
        Err(FsError::unimplemented())
    }

    /// Test for a POSIX file lock.
    async fn getlk(
        &self,
        _ino: u64,
        _fh: u64,
        _lock_owner: u64,
        _start: u64,
        _end: u64,
        _typ: i32,
        _pid: u32,
    ) -> Result<Lock> {
        Err(FsError::unimplemented())
    }

    /// Acquire, modify or release a POSIX file lock.
    /// For POSIX threads (NPTL) there's a 1-1 relation between pid and owner, but
    /// otherwise this is not always the case.  For checking lock ownership,
    /// 'fi->owner' must be used. The l_pid field in 'struct flock' should only be
    /// used to fill in this field in getlk(). Note: if the locking methods are not
    /// implemented, the kernel will still allow file locking to work locally.
    /// Hence these are only interesting for network filesystems and similar.
    async fn setlk(
        &self,
        _ino: u64,
        _fh: u64,
        _lock_owner: u64,
        _start: u64,
        _end: u64,
        _typ: i32,
        _pid: u32,
        _sleep: bool,
    ) -> Result<()> {
        Err(FsError::unimplemented())
    }

    /// Map block index within file to block index within device.
    /// Note: This makes sense only for block device backed filesystems mounted
    /// with the 'blkdev' option
    async fn bmap(&self, _ino: u64, _blocksize: u32, _idx: u64) -> Result<Bmap> {
        Err(FsError::unimplemented())
    }

    /// Preallocate or deallocate space to a file
    async fn fallocate(
        &self,
        _ino: u64,
        _fh: u64,
        _offset: i64,
        _length: i64,
        _mode: i32,
    ) -> Result<()> {
        Err(FsError::unimplemented())
    }
/*
    /// Reposition read/write file offset
    async fn lseek(&self, _ino: u64, _fh: u64, _offset: i64, _whence: i32) -> Result<Lseek> {
        Err(FsError::unimplemented())
    }
*/

    /// Copy the specified range from the source inode to the destination inode
    async fn copy_file_range(
        &self,
        _ino_in: u64,
        _fh_in: u64,
        _offset_in: i64,
        _ino_out: u64,
        _fh_out: u64,
        _offset_out: i64,
        _len: u64,
        _flags: u32,
    ) -> Result<Write> {
        Err(FsError::unimplemented())
    }
}
