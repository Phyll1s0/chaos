#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod error;
pub mod fs;
pub mod process;
pub mod syscall;

pub use error::{Error, Result};
