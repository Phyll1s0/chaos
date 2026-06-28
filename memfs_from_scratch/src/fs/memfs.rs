use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::RwLock;

use crate::fs::{FileType, Inode};
use crate::Result;

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
        // TODO(you): read self.inner and return FileType::File or FileType::Dir.
        todo!("step 1: implement MemInode::file_type")
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize> {
        let _ = (offset, buf);
        // TODO(you): read bytes from File { data } starting at offset.
        todo!("step 2: implement MemInode::read_at")
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize> {
        let _ = (offset, buf);
        // TODO(you): resize File { data } if needed, then copy buf into it.
        todo!("step 3: implement MemInode::write_at")
    }

    fn len(&self) -> usize {
        // TODO(you): return file byte length or directory entry count.
        todo!("step 4: implement MemInode::len")
    }

    fn resize(&self, len: usize) -> Result<()> {
        let _ = len;
        // TODO(you): resize regular file contents.
        todo!("step 5: implement MemInode::resize")
    }

    fn find(&self, name: &str) -> Result<Arc<dyn Inode>> {
        let _ = name;
        // TODO(you): find child inode in a directory.
        todo!("step 6: implement MemInode::find")
    }

    fn create(&self, name: &str, file_type: FileType) -> Result<Arc<dyn Inode>> {
        let _ = (name, file_type);
        // TODO(you): create file or dir inside a directory.
        todo!("step 7: implement MemInode::create")
    }

    fn unlink(&self, name: &str) -> Result<()> {
        let _ = name;
        // TODO(you): remove a child name from a directory.
        todo!("step 8: implement MemInode::unlink")
    }

    fn list(&self) -> Result<Vec<String>> {
        // TODO(you): list directory entry names.
        todo!("step 9: implement MemInode::list")
    }
}
