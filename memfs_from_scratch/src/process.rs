use alloc::collections::BTreeMap;
use alloc::string::String;

use crate::fs::{FileHandle, FileLike};
use crate::Result;

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
        let _ = file;
        // TODO(you): allocate a fd and insert file into fd_table.
        todo!("step 17: implement Process::add_file")
    }

    pub fn get_file_like(&self, fd: usize) -> Result<&FileLike> {
        let _ = fd;
        // TODO(you): look up FileLike by fd or return Error::BadFd.
        todo!("step 18: implement Process::get_file_like")
    }

    pub fn get_file(&self, fd: usize) -> Result<&FileHandle> {
        let _ = fd;
        // TODO(you): get FileHandle from FileLike.
        todo!("step 19: implement Process::get_file")
    }

    pub fn close(&mut self, fd: usize) -> Result<()> {
        let _ = fd;
        // TODO(you): remove fd from fd_table.
        todo!("step 20: implement Process::close")
    }
}

impl Default for Process {
    fn default() -> Self {
        Self::new()
    }
}
