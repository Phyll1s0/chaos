use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::{
    split_parent, FileHandle, FileLike, FileType, Inode, MemFS, Metadata, OpenFlags, OpenOptions,
    PathCursor,
};
use crate::process::Process;
use crate::{Error, Result};

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
        let inode = match self.lookup(path) {
            Ok(inode) => inode,
            Err(Error::NotFound) if create => self.create_file(path)?,
            Err(err) => return Err(err),
        };
        let file = FileHandle::new(inode, options);
        Ok(self.process.add_file(FileLike::File(file)))
    }

    pub fn sys_open_flags(&mut self, path: &str, flags: OpenFlags) -> Result<usize> {
        let fd = self.sys_open(
            path,
            flags.contains(OpenFlags::CREATE),
            OpenOptions::from_flags(flags),
        )?;
        if flags.contains(OpenFlags::TRUNCATE) {
            self.sys_ftruncate(fd, 0)?;
        }
        Ok(fd)
    }

    pub fn sys_read(&self, fd: usize, buf: &mut [u8]) -> Result<usize> {
        self.process.get_file_like(fd)?.read(buf)
    }

    pub fn sys_write(&self, fd: usize, buf: &[u8]) -> Result<usize> {
        self.process.get_file_like(fd)?.write(buf)
    }

    pub fn sys_close(&mut self, fd: usize) -> Result<()> {
        self.process.close(fd)
    }

    pub fn sys_mkdir(&self, path: &str) -> Result<()> {
        let (parent, name) = self.parent_dir(path)?;
        parent.create(name, FileType::Dir).map(|_| ())
    }

    pub fn sys_unlink(&self, path: &str) -> Result<()> {
        let (parent, name) = self.parent_dir(path)?;
        parent.unlink(name)
    }

    pub fn sys_fstat(&self, fd: usize) -> Result<Metadata> {
        self.process.get_file(fd)?.metadata()
    }

    pub fn sys_ftruncate(&self, fd: usize, len: usize) -> Result<()> {
        self.process.get_file(fd)?.inode().resize(len)
    }

    pub fn sys_lseek(&self, fd: usize, offset: usize) -> Result<()> {
        self.process.get_file(fd)?.seek_set(offset);
        Ok(())
    }

    pub fn sys_getdents(&self, path: &str) -> Result<Vec<String>> {
        self.lookup(path)?.list()
    }

    pub fn sys_chdir(&mut self, path: &str) -> Result<()> {
        let inode = self.lookup(path)?;
        if inode.file_type() != FileType::Dir {
            return Err(Error::NotDir);
        }
        self.process.set_cwd(self.absolute_path(path));
        Ok(())
    }

    fn lookup(&self, path: &str) -> Result<Arc<dyn Inode>> {
        let path = self.absolute_path(path);
        let mut inode = self.fs.root_inode();
        if path == "/" {
            return Ok(inode);
        }
        for part in PathCursor::new(&path) {
            if part == ".." {
                return Err(Error::Invalid);
            }
            inode = inode.find(part)?;
        }
        Ok(inode)
    }

    fn create_file(&self, path: &str) -> Result<Arc<dyn Inode>> {
        let path = self.absolute_path(path);
        let mut inode = self.fs.root_inode();
        let mut parts = PathCursor::new(&path).peekable();
        while let Some(part) = parts.next() {
            if part == ".." {
                return Err(Error::Invalid);
            }
            if parts.peek().is_none() {
                return inode.create(part, FileType::File);
            }
            inode = inode.find(part)?;
        }
        Err(Error::Invalid)
    }

    fn parent_dir<'a>(&self, path: &'a str) -> Result<(Arc<dyn Inode>, &'a str)> {
        let (parent_path, name) = split_parent(path);
        if name.is_empty() || name == "." || name == ".." {
            return Err(Error::Invalid);
        }
        let parent = self.lookup(parent_path)?;
        if parent.file_type() != FileType::Dir {
            return Err(Error::NotDir);
        }
        Ok((parent, name))
    }

    fn absolute_path(&self, path: &str) -> String {
        if path.starts_with('/') {
            path.to_string()
        } else if self.process.cwd() == "/" {
            let mut abs = String::from("/");
            abs.push_str(path);
            abs
        } else {
            let mut abs = self.process.cwd().to_string();
            abs.push('/');
            abs.push_str(path);
            abs
        }
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
