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
        // lookup or create inode, wrap it in FileHandle, insert into fd_table.
        //todo!("step 21: implement Kernel::sys_open")
        let inode = match self.lookup(path) {
            Ok(inode) => inode,
            Err(Error::NotFound) if create => self.create_file(path)?,
            Err(err) => return Err(err),
        };

        let file = FileHandle::new(inode, options);
        Ok(self.process.add_file(FileLike::File(file)))
    }
    

    pub fn sys_open_flags(&mut self, path: &str, flags: OpenFlags) -> Result<usize> {
        let _ = (path, flags);
        // translate OpenFlags, call sys_open, then handle TRUNCATE.
        //todo!("step 22: implement Kernel::sys_open_flags")
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
        // get FileLike from fd and call read.
        //todo!("step 23: implement Kernel::sys_read")
        self.process.get_file_like(fd)?.read(buf)
    }

    pub fn sys_write(&self, fd: usize, buf: &[u8]) -> Result<usize> {
        // get FileLike from fd and call write.
        //todo!("step 24: implement Kernel::sys_write")
        self.process.get_file_like(fd)?.write(buf)
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
        
        // get file metadata through fd.
        //todo!("step 28: implement Kernel::sys_fstat")
        self.process.get_file_like(fd)?.as_file().metadata()
    }

    pub fn sys_ftruncate(&self, fd: usize, len: usize) -> Result<()> {
        let _ = (fd, len);
        // resize inode through file handle.
        //todo!("step 29: implement Kernel::sys_ftruncate")
        self.process.get_file(fd)?.inode().resize(len)
    }

    pub fn sys_lseek(&self, fd: usize, offset: usize) -> Result<()> {
        // set file handle offset.
        //todo!("step 30: implement Kernel::sys_lseek")
        self.process.get_file_like(fd)?.as_file().seek_set(offset);
        Ok(())
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
        // walk from root through each PathCursor part.

        // todo!("step 33: implement Kernel::lookup")
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
        
        // walk to parent and create the final component as File.
        //todo!("step 34: implement Kernel::create_file")

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
        let _ = path;
        // split parent path, lookup parent, ensure it is a directory.
        todo!("step 35: implement Kernel::parent_dir")
    }

    fn absolute_path(&self, path: &str) -> String {
        // convert relative paths using process.cwd().
        //todo!("step 36: implement Kernel::absolute_path")
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
