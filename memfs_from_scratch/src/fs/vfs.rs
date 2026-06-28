use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileType {
    File,
    Dir,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Metadata {
    pub file_type: FileType,
    pub len: usize,
}

pub trait Inode: Send + Sync {
    fn file_type(&self) -> FileType;

    fn metadata(&self) -> Metadata {
        Metadata {
            file_type: self.file_type(),
            len: self.len(),
        }
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize>;
    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize>;
    fn len(&self) -> usize;
    fn resize(&self, len: usize) -> Result<()>;
    fn find(&self, name: &str) -> Result<Arc<dyn Inode>>;
    fn create(&self, name: &str, file_type: FileType) -> Result<Arc<dyn Inode>>;
    fn unlink(&self, name: &str) -> Result<()>;
    fn list(&self) -> Result<Vec<String>>;
}
