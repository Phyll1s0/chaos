use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::{
    split_parent, FileHandle, FileLike, FileType, Inode, MemFS, Metadata, OpenFlags, OpenOptions,
    PathCursor,
};
use crate::process::Process;
use crate::Result;

pub struct Kernel {
    fs: MemFS,
    process: Process,
}

impl Kernel {
    pub fn new() -> Self {
        Self {
            fs: MemFS::new(),
            process: Process::new(),
        }
    }

    pub fn sys_open(&mut self, path: &str, create: bool, options: OpenOptions) -> Result<usize> {
        let _ = (path, create, options);
        // TODO(you): lookup or create inode, wrap it in FileHandle, insert into fd_table.
        todo!("step 21: implement Kernel::sys_open")
    }

    pub fn sys_open_flags(&mut self, path: &str, flags: OpenFlags) -> Result<usize> {
        let _ = (path, flags);
        // TODO(you): translate OpenFlags, call sys_open, then handle TRUNCATE.
        todo!("step 22: implement Kernel::sys_open_flags")
    }

    pub fn sys_read(&self, fd: usize, buf: &mut [u8]) -> Result<usize> {
        let _ = (fd, buf);
        // TODO(you): get FileLike from fd and call read.
        todo!("step 23: implement Kernel::sys_read")
    }

    pub fn sys_write(&self, fd: usize, buf: &[u8]) -> Result<usize> {
        let _ = (fd, buf);
        // TODO(you): get FileLike from fd and call write.
        todo!("step 24: implement Kernel::sys_write")
    }

    pub fn sys_close(&mut self, fd: usize) -> Result<()> {
        let _ = fd;
        // TODO(you): close fd in current process.
        todo!("step 25: implement Kernel::sys_close")
    }

    pub fn sys_mkdir(&self, path: &str) -> Result<()> {
        let _ = path;
        // TODO(you): find parent dir and create a dir inode.
        todo!("step 26: implement Kernel::sys_mkdir")
    }

    pub fn sys_unlink(&self, path: &str) -> Result<()> {
        let _ = path;
        // TODO(you): find parent dir and remove child entry.
        todo!("step 27: implement Kernel::sys_unlink")
    }

    pub fn sys_fstat(&self, fd: usize) -> Result<Metadata> {
        let _ = fd;
        // TODO(you): get file metadata through fd.
        todo!("step 28: implement Kernel::sys_fstat")
    }

    pub fn sys_ftruncate(&self, fd: usize, len: usize) -> Result<()> {
        let _ = (fd, len);
        // TODO(you): resize inode through file handle.
        todo!("step 29: implement Kernel::sys_ftruncate")
    }

    pub fn sys_lseek(&self, fd: usize, offset: usize) -> Result<()> {
        let _ = (fd, offset);
        // TODO(you): set file handle offset.
        todo!("step 30: implement Kernel::sys_lseek")
    }

    pub fn sys_getdents(&self, path: &str) -> Result<Vec<String>> {
        let _ = path;
        // TODO(you): lookup dir inode and list entries.
        todo!("step 31: implement Kernel::sys_getdents")
    }

    pub fn sys_chdir(&mut self, path: &str) -> Result<()> {
        let _ = path;
        // TODO(you): lookup path, ensure it is a dir, update cwd.
        todo!("step 32: implement Kernel::sys_chdir")
    }

    fn lookup(&self, path: &str) -> Result<Arc<dyn Inode>> {
        let _ = path;
        // TODO(you): walk from root through each PathCursor part.
        todo!("step 33: implement Kernel::lookup")
    }

    fn create_file(&self, path: &str) -> Result<Arc<dyn Inode>> {
        let _ = path;
        // TODO(you): walk to parent and create the final component as File.
        todo!("step 34: implement Kernel::create_file")
    }

    fn parent_dir<'a>(&self, path: &'a str) -> Result<(Arc<dyn Inode>, &'a str)> {
        let _ = path;
        // TODO(you): split parent path, lookup parent, ensure it is a directory.
        todo!("step 35: implement Kernel::parent_dir")
    }

    fn absolute_path(&self, path: &str) -> String {
        let _ = path;
        // TODO(you): convert relative paths using process.cwd().
        todo!("step 36: implement Kernel::absolute_path")
    }
}

impl Default for Kernel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::{OpenFlags, OpenOptions};

    #[test]
    fn create_write_seek_read() {
        let mut kernel = Kernel::new();
        let fd = kernel
            .sys_open("/hello", true, OpenOptions::read_write())
            .unwrap();

        assert_eq!(kernel.sys_write(fd, b"abc").unwrap(), 3);
        kernel.sys_lseek(fd, 0).unwrap();

        let mut buf = [0; 4];
        assert_eq!(kernel.sys_read(fd, &mut buf).unwrap(), 3);
        assert_eq!(&buf[..3], b"abc");
    }

    #[test]
    fn two_opens_share_inode_but_not_offset() {
        let mut kernel = Kernel::new();
        let fd1 = kernel
            .sys_open("/note", true, OpenOptions::read_write())
            .unwrap();
        assert_eq!(kernel.sys_write(fd1, b"abcdef").unwrap(), 6);

        let fd2 = kernel
            .sys_open("/note", false, OpenOptions::read_only())
            .unwrap();

        let mut first = [0; 3];
        assert_eq!(kernel.sys_read(fd2, &mut first).unwrap(), 3);
        assert_eq!(&first, b"abc");

        let mut second = [0; 3];
        assert_eq!(kernel.sys_read(fd2, &mut second).unwrap(), 3);
        assert_eq!(&second, b"def");

        let mut empty = [0; 1];
        assert_eq!(kernel.sys_read(fd1, &mut empty).unwrap(), 0);
    }

    #[test]
    fn directory_create_list_and_unlink() {
        let mut kernel = Kernel::new();
        kernel.sys_mkdir("/tmp").unwrap();
        kernel
            .sys_open("/tmp/a", true, OpenOptions::read_write())
            .unwrap();
        kernel
            .sys_open("/tmp/b", true, OpenOptions::read_write())
            .unwrap();

        let mut entries = kernel.sys_getdents("/tmp").unwrap();
        entries.sort();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], "a");
        assert_eq!(entries[1], "b");

        kernel.sys_unlink("/tmp/a").unwrap();
        let entries = kernel.sys_getdents("/tmp").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], "b");
    }

    #[test]
    fn truncate_changes_inode_length() {
        let mut kernel = Kernel::new();
        let fd = kernel
            .sys_open("/data", true, OpenOptions::read_write())
            .unwrap();
        kernel.sys_write(fd, b"abcdef").unwrap();
        kernel.sys_ftruncate(fd, 2).unwrap();
        kernel.sys_lseek(fd, 0).unwrap();

        let mut buf = [0; 8];
        assert_eq!(kernel.sys_read(fd, &mut buf).unwrap(), 2);
        assert_eq!(&buf[..2], b"ab");
    }

    #[test]
    fn open_flags_create_rdwr_and_truncate() {
        let mut kernel = Kernel::new();
        let fd = kernel
            .sys_open_flags("/log", OpenFlags::CREATE | OpenFlags::RDWR)
            .unwrap();
        kernel.sys_write(fd, b"old").unwrap();

        let fd2 = kernel
            .sys_open_flags("/log", OpenFlags::TRUNCATE | OpenFlags::RDWR)
            .unwrap();
        assert_eq!(kernel.sys_fstat(fd2).unwrap().len, 0);

        kernel.sys_write(fd2, b"new").unwrap();
        kernel.sys_lseek(fd2, 0).unwrap();
        let mut buf = [0; 3];
        kernel.sys_read(fd2, &mut buf).unwrap();
        assert_eq!(&buf, b"new");
    }
}
