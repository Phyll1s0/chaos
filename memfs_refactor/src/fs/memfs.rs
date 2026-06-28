use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::RwLock;

use crate::fs::{FileType, Inode};
use crate::{Error, Result};

pub struct MemFS {
    root: Arc<MemInode>,
}

impl MemFS {
    pub fn new() -> Self {
        Self {
            root: MemInode::new_dir(),
        }
    }

    pub fn root_inode(&self) -> Arc<dyn Inode> {
        self.root.clone()
    }
}

impl Default for MemFS {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MemInode {
    inner: RwLock<MemInodeInner>,
}

enum MemInodeInner {
    File {
        data: Vec<u8>,
    },
    Dir {
        entries: BTreeMap<String, Arc<MemInode>>,
    },
}

impl MemInode {
    pub fn new_file() -> Arc<Self> {
        Arc::new(Self {
            inner: RwLock::new(MemInodeInner::File { data: Vec::new() }),
        })
    }

    pub fn new_dir() -> Arc<Self> {
        Arc::new(Self {
            inner: RwLock::new(MemInodeInner::Dir {
                entries: BTreeMap::new(),
            }),
        })
    }
}

impl Inode for MemInode {
    fn file_type(&self) -> FileType {
        match &*self.inner.read() {
            MemInodeInner::File { .. } => FileType::File,
            MemInodeInner::Dir { .. } => FileType::Dir,
        }
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize> {
        match &*self.inner.read() {
            MemInodeInner::File { data } => {
                if offset >= data.len() {
                    return Ok(0);
                }
                let end = core::cmp::min(offset + buf.len(), data.len());
                let src = &data[offset..end];
                buf[..src.len()].copy_from_slice(src);
                Ok(src.len())
            }
            MemInodeInner::Dir { .. } => Err(Error::IsDir),
        }
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize> {
        match &mut *self.inner.write() {
            MemInodeInner::File { data } => {
                let end = offset + buf.len();
                if end > data.len() {
                    data.resize(end, 0);
                }
                data[offset..end].copy_from_slice(buf);
                Ok(buf.len())
            }
            MemInodeInner::Dir { .. } => Err(Error::IsDir),
        }
    }

    fn len(&self) -> usize {
        match &*self.inner.read() {
            MemInodeInner::File { data } => data.len(),
            MemInodeInner::Dir { entries } => entries.len(),
        }
    }

    fn resize(&self, len: usize) -> Result<()> {
        match &mut *self.inner.write() {
            MemInodeInner::File { data } => {
                data.resize(len, 0);
                Ok(())
            }
            MemInodeInner::Dir { .. } => Err(Error::IsDir),
        }
    }

    fn find(&self, name: &str) -> Result<Arc<dyn Inode>> {
        match &*self.inner.read() {
            MemInodeInner::Dir { entries } => entries
                .get(name)
                .cloned()
                .map(|inode| inode as Arc<dyn Inode>)
                .ok_or(Error::NotFound),
            MemInodeInner::File { .. } => Err(Error::NotDir),
        }
    }

    fn create(&self, name: &str, file_type: FileType) -> Result<Arc<dyn Inode>> {
        match &mut *self.inner.write() {
            MemInodeInner::Dir { entries } => {
                if entries.contains_key(name) {
                    return Err(Error::AlreadyExists);
                }
                let inode = match file_type {
                    FileType::File => MemInode::new_file(),
                    FileType::Dir => MemInode::new_dir(),
                };
                entries.insert(name.to_string(), inode.clone());
                Ok(inode)
            }
            MemInodeInner::File { .. } => Err(Error::NotDir),
        }
    }

    fn unlink(&self, name: &str) -> Result<()> {
        match &mut *self.inner.write() {
            MemInodeInner::Dir { entries } => {
                entries.remove(name).map(|_| ()).ok_or(Error::NotFound)
            }
            MemInodeInner::File { .. } => Err(Error::NotDir),
        }
    }

    fn list(&self) -> Result<Vec<String>> {
        match &*self.inner.read() {
            MemInodeInner::Dir { entries } => Ok(entries.keys().cloned().collect()),
            MemInodeInner::File { .. } => Err(Error::NotDir),
        }
    }
}
