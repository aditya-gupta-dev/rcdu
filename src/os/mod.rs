//! Safe interfaces for the Linux syscall boundary.
mod linux;
pub use linux::*;

#[derive(Clone, Copy, Default, Debug)]
pub struct Metadata {
    pub blocks: u64,
    pub apparent: u64,
    pub device: u64,
    pub inode: u64,
    pub links: u32,
    pub mode: u32,
    pub mtime: u64,
    pub uid: u32,
    pub gid: u32,
    pub present: u8,
}
impl Metadata {
    pub fn directory(self) -> bool {
        self.mode & libc::S_IFMT == libc::S_IFDIR
    }
    pub fn symlink(self) -> bool {
        self.mode & libc::S_IFMT == libc::S_IFLNK
    }
    pub fn allocated(self) -> u64 {
        self.blocks.saturating_mul(512)
    }
}
