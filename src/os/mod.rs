//! Platform boundary. Unsupported targets fail at compilation, rather than returning fake data.
#[cfg(not(target_os = "linux"))]
compile_error!("rcdu currently supports Linux only");
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

use crate::model::Kind;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Extended {
    pub mtime: u64,
    pub uid: u32,
    pub gid: u32,
    pub mode: u16,
    pub present: u8,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Observation {
    pub kind: Kind,
    pub blocks: u64,
    pub apparent: u64,
    pub device: u64,
    pub inode: u64,
    pub links: u32,
    pub symlink: bool,
    pub extended: Extended,
}
