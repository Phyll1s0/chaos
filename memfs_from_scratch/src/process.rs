use alloc::collections::BTreeMap;
use alloc::string::String;

use crate::fs::{FileHandle, FileLike};
use crate::{Error, Result};

pub struct Process {
    fd_table: BTreeMap<usize, FileLike>,
    cwd: String,
}

impl Process {
    pub fn new() -> Self {
        Self {
            fd_table: BTreeMap::new(),
            cwd: String::from("/"),
        }
    }

    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    pub fn alloc_fd(&self) -> usize {
        let mut fd = 0;
        while self.fd_table.contains_key(&fd) {
            fd += 1;
        }
        fd
    }

    pub fn set_cwd(&mut self, cwd: String) {
        self.cwd = cwd;
    }

    pub fn add_file(&mut self, file: FileLike) -> usize {
        // allocate a fd and insert file into fd_table.
        //todo!("step 17: implement Process::add_file")
        let fd = self.alloc_fd();
        self.fd_table.insert(fd, file);
        fd
    }

    pub fn get_file_like(&self, fd: usize) -> Result<&FileLike> {
        // look up FileLike by fd or return Error::BadFd.
        //todo!("step 18: implement Process::get_file_like")
        self.fd_table.get(&fd).ok_or(Error::BadFd)
    }

    pub fn get_file(&self, fd: usize) -> Result<&FileHandle> {
        // get FileHandle from FileLike.
        //todo!("step 19: implement Process::get_file")
        Ok(self.get_file_like(fd)?.as_file())
    }

    pub fn close(&mut self, fd: usize) -> Result<()> {
        // remove fd from fd_table.
        //todo!("step 20: implement Process::close")
        self.fd_table.remove(&fd).map(|_| ()).ok_or(Error::BadFd)
    }
}

impl Default for Process {
    fn default() -> Self {
        Self::new()
    }
}
