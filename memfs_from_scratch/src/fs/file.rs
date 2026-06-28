use alloc::sync::Arc;
use spin::Mutex;

use crate::fs::{Inode, Metadata};
use crate::Result;

bitflags::bitflags! {
    pub struct OpenFlags: u32 {
        const WRONLY = 1 << 0;
        const RDWR = 1 << 1;
        const CREATE = 1 << 6;
        const TRUNCATE = 1 << 9;
        const APPEND = 1 << 10;
    }
}

#[derive(Clone, Copy, Debug)]
pub struct OpenOptions {
    pub read: bool,
    pub write: bool,
    pub append: bool,
}

impl OpenOptions {
    pub const fn read_only() -> Self {
        Self {
            read: true,
            write: false,
            append: false,
        }
    }

    pub const fn write_only() -> Self {
        Self {
            read: false,
            write: true,
            append: false,
        }
    }

    pub const fn read_write() -> Self {
        Self {
            read: true,
            write: true,
            append: false,
        }
    }

    pub fn from_flags(flags: OpenFlags) -> Self {
        let read = !flags.contains(OpenFlags::WRONLY) || flags.contains(OpenFlags::RDWR);
        let write = flags.contains(OpenFlags::WRONLY) || flags.contains(OpenFlags::RDWR);
        Self {
            read,
            write,
            append: flags.contains(OpenFlags::APPEND),
        }
    }
}

pub struct FileHandle {
    inode: Arc<dyn Inode>,
    offset: Mutex<usize>,
    options: OpenOptions,
}

pub trait File {
    fn read(&self, buf: &mut [u8]) -> Result<usize>;
    fn write(&self, buf: &[u8]) -> Result<usize>;
    fn metadata(&self) -> Result<Metadata>;
}

impl FileHandle {
    pub fn new(inode: Arc<dyn Inode>, options: OpenOptions) -> Self {
        Self {
            inode,
            offset: Mutex::new(0),
            options,
        }
    }

    pub fn inode(&self) -> Arc<dyn Inode> {
        self.inode.clone()
    }

    pub fn read(&self, buf: &mut [u8]) -> Result<usize> {
        let _ = buf;
        // TODO(you): check read permission, read at current offset, update offset.
        todo!("step 10: implement FileHandle::read")
    }

    pub fn write(&self, buf: &[u8]) -> Result<usize> {
        let _ = buf;
        // TODO(you): check write permission, handle append, write, update offset.
        todo!("step 11: implement FileHandle::write")
    }

    pub fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize> {
        let _ = (offset, buf);
        // TODO(you): permission-checking wrapper around inode.read_at.
        todo!("step 12: implement FileHandle::read_at")
    }

    pub fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize> {
        let _ = (offset, buf);
        // TODO(you): permission-checking wrapper around inode.write_at.
        todo!("step 13: implement FileHandle::write_at")
    }

    pub fn metadata(&self) -> Result<Metadata> {
        // TODO(you): return inode metadata.
        todo!("step 14: implement FileHandle::metadata")
    }

    pub fn seek_set(&self, offset: usize) {
        let _ = offset;
        // TODO(you): set this handle's offset.
        todo!("step 15: implement FileHandle::seek_set")
    }

    pub fn offset(&self) -> usize {
        // TODO(you): read this handle's offset.
        todo!("step 16: implement FileHandle::offset")
    }
}

impl File for FileHandle {
    fn read(&self, buf: &mut [u8]) -> Result<usize> {
        FileHandle::read(self, buf)
    }

    fn write(&self, buf: &[u8]) -> Result<usize> {
        FileHandle::write(self, buf)
    }

    fn metadata(&self) -> Result<Metadata> {
        FileHandle::metadata(self)
    }
}

pub enum FileLike {
    File(FileHandle),
}

impl FileLike {
    pub fn read(&self, buf: &mut [u8]) -> Result<usize> {
        match self {
            FileLike::File(file) => file.read(buf),
        }
    }

    pub fn write(&self, buf: &[u8]) -> Result<usize> {
        match self {
            FileLike::File(file) => file.write(buf),
        }
    }

    pub fn as_file(&self) -> &FileHandle {
        match self {
            FileLike::File(file) => file,
        }
    }
}
