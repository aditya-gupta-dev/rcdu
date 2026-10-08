//! Linux ABI calls. Only owned descriptors cross into the scheduler.
use super::Metadata;
use std::ffi::{CStr, CString, OsString};
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn invalid(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}
pub fn path(bytes: &[u8]) -> PathBuf {
    OsString::from_vec(bytes.to_vec()).into()
}
pub fn bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_bytes()
}
pub fn name(bytes: &[u8]) -> io::Result<CString> {
    CString::new(bytes).map_err(|_| invalid("NUL in filename"))
}
fn descriptor(result: i32) -> io::Result<OwnedFd> {
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful open/fcntl returns a new descriptor transferred exactly once.
    Ok(unsafe { OwnedFd::from_raw_fd(result) })
}
pub fn open_root(path: &Path) -> io::Result<OwnedFd> {
    let path = name(bytes(path))?;
    // SAFETY: terminated CString lives throughout open; no variadic mode is required.
    descriptor(unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    })
}
pub fn open_directory(parent: BorrowedFd<'_>, name: &CStr) -> io::Result<OwnedFd> {
    // SAFETY: parent remains live and name terminated. NOFOLLOW rejects replaced directory links.
    descriptor(unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    })
}
pub fn stat(parent: BorrowedFd<'_>, name: &CStr, follow: bool) -> io::Result<Metadata> {
    // SAFETY: zero initialization includes padding; fstatat fills aligned writable storage.
    let mut value: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: parent and terminated name outlive this call, and value is writable.
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            &mut value,
            if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW },
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(Metadata {
        blocks: value.st_blocks.max(0) as u64,
        apparent: value.st_size.max(0) as u64,
        device: value.st_dev,
        inode: value.st_ino,
        links: (value.st_nlink as u64).min(0x7fff_ffff) as u32,
        mode: value.st_mode,
        mtime: value.st_mtime.max(0) as u64,
        uid: value.st_uid,
        gid: value.st_gid,
    })
}
pub fn stat_directory(fd: BorrowedFd<'_>) -> io::Result<Metadata> {
    stat(fd, c".", true)
}
pub fn pool_budget(workers: usize) -> io::Result<usize> {
    // SAFETY: rlimit consists of integer fields; getrlimit writes initialized aligned storage.
    let mut limits: libc::rlimit = unsafe { std::mem::zeroed() };
    // SAFETY: limits pointer is valid during getrlimit.
    let available = if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limits) } == 0 {
        usize::try_from(limits.rlim_cur)
            .unwrap_or(usize::MAX)
            .saturating_sub(16)
    } else {
        48
    };
    let spare = available
        .checked_sub(workers.saturating_mul(3))
        .filter(|spare| *spare > 0)
        .ok_or_else(|| io::Error::other("too many scan workers for open-file limit; reduce -t"))?;
    Ok(spare.min(workers.saturating_mul(2)).clamp(1, 32))
}

/// Ancestry is shared only by directory jobs; regular entries never allocate full paths.
pub struct Location {
    pub parent: Option<Arc<Location>>,
    pub name: CString,
    pub device: u64,
    pub inode: u64,
}
impl Location {
    pub fn open(&self, root: BorrowedFd<'_>) -> io::Result<OwnedFd> {
        let mut components = Vec::new();
        let mut current = self;
        while let Some(parent) = &current.parent {
            components.push(current);
            current = parent;
        }
        let mut fd = open_directory(root, c".")?;
        for component in components.into_iter().rev() {
            fd = open_directory(fd.as_fd(), &component.name)?;
            let observed = stat_directory(fd.as_fd())?;
            if (observed.device, observed.inode) != (component.device, component.inode) {
                return Err(io::Error::other("directory changed during scan"));
            }
        }
        Ok(fd)
    }
}
impl Drop for Location {
    fn drop(&mut self) {
        let mut parent = self.parent.take();
        while let Some(node) = parent {
            match Arc::try_unwrap(node) {
                Ok(mut node) => parent = node.parent.take(),
                Err(_) => break,
            }
        }
    }
}

pub struct DirectoryReader {
    buffer: Vec<u8>,
    start: usize,
    end: usize,
}
impl Default for DirectoryReader {
    fn default() -> Self {
        Self {
            buffer: vec![0; 32768],
            start: 0,
            end: 0,
        }
    }
}
impl DirectoryReader {
    pub fn reset(&mut self) {
        self.start = 0;
        self.end = 0;
    }
    pub fn next(&mut self, fd: BorrowedFd<'_>) -> io::Result<Option<&CStr>> {
        loop {
            if self.start == self.end {
                // SAFETY: buffer is writable, initialized, and syscall cannot write past its length.
                let result = unsafe {
                    libc::syscall(
                        libc::SYS_getdents64,
                        fd.as_raw_fd(),
                        self.buffer.as_mut_ptr(),
                        self.buffer.len(),
                    )
                };
                if result < 0 {
                    let error = io::Error::last_os_error();
                    if error.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(error);
                }
                if result == 0 {
                    return Ok(None);
                }
                self.start = 0;
                self.end = result as usize;
            }
            let start = self.start;
            if self.end - start < 20 {
                return Err(invalid("short directory record"));
            }
            let length = u16::from_ne_bytes(self.buffer[start + 16..start + 18].try_into().unwrap())
                as usize;
            if length < 20 || length > self.end - start {
                return Err(invalid("bad directory record length"));
            }
            self.start += length;
            let name = CStr::from_bytes_until_nul(&self.buffer[start + 19..start + length])
                .map_err(|_| invalid("unterminated directory record"))?;
            if name == c"." || name == c".." {
                continue;
            }
            return Ok(Some(
                CStr::from_bytes_until_nul(&self.buffer[start + 19..start + length])
                    .map_err(|_| invalid("unterminated directory record"))?,
            ));
        }
    }
}
