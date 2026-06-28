#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    BadFd,
    NotFound,
    AlreadyExists,
    NotFile,
    NotDir,
    IsDir,
    Invalid,
    Permission,
}

pub type Result<T> = core::result::Result<T, Error>;
