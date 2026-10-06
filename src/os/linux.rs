//! Audited Linux ABI boundary. Names stay byte strings; open directory handles are owned.
use super::{Extended, Observation};
use crate::model::Kind;
use std::ffi::{CStr, CString, OsStr, OsString};
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
pub fn c_name(name: &[u8]) -> io::Result<CString> {
    CString::new(name).map_err(|_| invalid("interior NUL in path"))
}
pub fn path_bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_bytes()
}
pub fn byte_path(bytes: &[u8]) -> PathBuf {
    OsString::from_vec(bytes.to_vec()).into()
}
pub fn argument_bytes(arg: &OsStr) -> &[u8] {
    arg.as_bytes()
}
pub fn absolute(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}
pub fn input(path: &Path) -> io::Result<File> {
    File::open(path)
}
pub fn output(path: &Path) -> io::Result<File> {
    File::create(path)
}
pub fn read_config(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}

fn owned_fd(result: libc::c_int) -> io::Result<OwnedFd> {
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful open/dup returns a fresh descriptor; ownership transfers once.
    Ok(unsafe { OwnedFd::from_raw_fd(result) })
}
pub fn root_directory(path: &Path) -> io::Result<OwnedFd> {
    let path = c_name(path_bytes(path))?;
    // SAFETY: CString is terminated and valid for the call; no variadic mode is needed.
    owned_fd(unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    })
}
pub fn child_directory(parent: BorrowedFd<'_>, name: &CStr) -> io::Result<OwnedFd> {
    // SAFETY: borrowed parent stays open; name is terminated. O_NOFOLLOW rejects replacement links.
    owned_fd(unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    })
}
pub fn duplicate(fd: BorrowedFd<'_>) -> io::Result<OwnedFd> {
    // SAFETY: fcntl duplicates a live borrowed descriptor and returns independent ownership.
    owned_fd(unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MetadataBackend {
    #[default]
    Fstatat,
    Statx,
}

fn kind(mode: u32, links: u32) -> Kind {
    if mode & libc::S_IFMT == libc::S_IFDIR {
        Kind::Directory
    } else if links > 1 {
        Kind::Hardlink
    } else if mode & libc::S_IFMT == libc::S_IFREG {
        Kind::Regular
    } else {
        Kind::NonRegular
    }
}
pub fn metadata(
    parent: BorrowedFd<'_>,
    name: &CStr,
    follow: bool,
    backend: MetadataBackend,
) -> io::Result<Observation> {
    let flags = if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
    if backend == MetadataBackend::Statx {
        // SAFETY: zero initializes padding/reserved fields; pointer is aligned and writable.
        let mut stat: libc::statx = unsafe { std::mem::zeroed() };
        // SAFETY: CString and live borrowed fd outlive the syscall; stat is initialized storage.
        let result = unsafe {
            libc::statx(
                parent.as_raw_fd(),
                name.as_ptr(),
                flags | libc::AT_NO_AUTOMOUNT,
                libc::STATX_BASIC_STATS,
                &mut stat,
            )
        };
        if result == 0 && stat.stx_mask & libc::STATX_BASIC_STATS == libc::STATX_BASIC_STATS {
            return Ok(Observation {
                kind: kind(u32::from(stat.stx_mode), stat.stx_nlink),
                blocks: stat.stx_blocks,
                apparent: stat.stx_size,
                device: libc::makedev(stat.stx_dev_major, stat.stx_dev_minor),
                inode: stat.stx_ino,
                links: stat.stx_nlink.min(0x7fff_ffff),
                symlink: u32::from(stat.stx_mode) & libc::S_IFMT == libc::S_IFLNK,
                extended: Extended {
                    mtime: stat.stx_mtime.tv_sec.max(0) as u64,
                    uid: stat.stx_uid,
                    gid: stat.stx_gid,
                    mode: stat.stx_mode,
                    present: 15,
                },
            });
        }
        if result != 0 {
            let error = io::Error::last_os_error();
            if !matches!(
                error.raw_os_error(),
                Some(libc::ENOSYS | libc::EINVAL | libc::EPERM)
            ) {
                return Err(error);
            }
        }
    }
    // SAFETY: zeroed stat has valid integer fields/padding. The syscall fills it on success.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: all pointers and the descriptor are valid for this call.
    if unsafe { libc::fstatat(parent.as_raw_fd(), name.as_ptr(), &mut stat, flags) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let links = (stat.st_nlink as u64).min(0x7fff_ffff) as u32;
    Ok(Observation {
        kind: kind(stat.st_mode, links),
        blocks: stat.st_blocks.max(0) as u64,
        apparent: stat.st_size.max(0) as u64,
        device: stat.st_dev,
        inode: stat.st_ino,
        links,
        symlink: stat.st_mode & libc::S_IFMT == libc::S_IFLNK,
        extended: Extended {
            mtime: stat.st_mtime.max(0) as u64,
            uid: stat.st_uid,
            gid: stat.st_gid,
            mode: stat.st_mode as u16,
            present: 15,
        },
    })
}
pub fn descriptor_metadata(
    fd: BorrowedFd<'_>,
    backend: MetadataBackend,
) -> io::Result<Observation> {
    metadata(fd, c".", true, backend)
}

/// Transient directory ancestry. Only directories allocate a path node; files never do.
#[derive(Debug)]
pub struct Location {
    pub parent: Option<Arc<Location>>,
    pub name: Vec<u8>,
}
impl Location {
    pub fn open(&self, root: BorrowedFd<'_>) -> io::Result<OwnedFd> {
        let mut names = Vec::new();
        let mut node = self;
        while let Some(parent) = &node.parent {
            names.push(&node.name);
            node = parent;
        }
        let mut fd = duplicate(root)?;
        for name in names.into_iter().rev() {
            fd = child_directory(std::os::fd::AsFd::as_fd(&fd), &c_name(name)?)?;
        }
        Ok(fd)
    }
    pub fn components(&self) -> Vec<&[u8]> {
        let mut parts = Vec::new();
        let mut node = self;
        loop {
            parts.push(node.name.as_slice());
            match &node.parent {
                Some(parent) => node = parent,
                None => break,
            }
        }
        parts.reverse();
        parts
    }
}

/// Buffered getdents64 parser. The last processed d_off cookie permits bounded-FD suspension.
pub struct DirectoryCursor {
    pub fd: Option<OwnedFd>,
    buffer: Vec<u8>,
    position: usize,
    end: usize,
    cookie: i64,
}
impl DirectoryCursor {
    pub fn new(fd: OwnedFd) -> Self {
        Self {
            fd: Some(fd),
            buffer: vec![0; 8192],
            position: 0,
            end: 0,
            cookie: 0,
        }
    }
    pub fn suspend(&mut self) {
        self.fd = None;
        self.buffer = Vec::new();
        self.position = 0;
        self.end = 0;
    }
    pub fn resume(&mut self, fd: OwnedFd) -> io::Result<()> {
        // SAFETY: descriptor is live and cookie came from this directory's getdents64 record.
        if unsafe { libc::lseek(fd.as_raw_fd(), self.cookie, libc::SEEK_SET) } < 0 {
            return Err(io::Error::last_os_error());
        }
        self.fd = Some(fd);
        self.buffer.resize(8192, 0);
        Ok(())
    }
    pub fn next_name(&mut self) -> io::Result<Option<&CStr>> {
        loop {
            if self.position == self.end {
                let fd = self
                    .fd
                    .as_ref()
                    .ok_or_else(|| invalid("suspended cursor"))?;
                // SAFETY: kernel writes at most buffer.len() initialized bytes; no record references survive refill.
                let count = unsafe {
                    libc::syscall(
                        libc::SYS_getdents64,
                        fd.as_raw_fd(),
                        self.buffer.as_mut_ptr(),
                        self.buffer.len(),
                    )
                };
                if count < 0 {
                    let error = io::Error::last_os_error();
                    if error.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(error);
                }
                if count == 0 {
                    return Ok(None);
                }
                self.position = 0;
                self.end = count as usize;
            }
            let start = self.position;
            if self.end - start < 20 {
                return Err(invalid("short getdents64 record"));
            }
            let length = u16::from_ne_bytes(self.buffer[start + 16..start + 18].try_into().unwrap())
                as usize;
            if length < 20 || length > self.end - start {
                return Err(invalid("invalid getdents64 length"));
            }
            self.cookie =
                i64::from_ne_bytes(self.buffer[start + 8..start + 16].try_into().unwrap());
            self.position += length;
            let name = CStr::from_bytes_until_nul(&self.buffer[start + 19..start + length])
                .map_err(|_| invalid("unterminated getdents64 name"))?;
            let is_dot = name.to_bytes() == b"." || name.to_bytes() == b"..";
            if is_dot {
                continue;
            }
            return Ok(Some(
                CStr::from_bytes_until_nul(&self.buffer[start + 19..start + length])
                    .map_err(|_| invalid("unterminated getdents64 name"))?,
            ));
        }
    }
}
pub fn cache_directory(fd: BorrowedFd<'_>) -> bool {
    let signature = b"Signature: 8a477f597d28d172789f06886806bc55";
    // SAFETY: fd is borrowed/live; fixed C name is terminated; successful fd is owned below.
    let Ok(fd) = owned_fd(unsafe {
        libc::openat(
            fd.as_raw_fd(),
            c"CACHEDIR.TAG".as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    }) else {
        return false;
    };
    let mut file = File::from(fd);
    let mut bytes = [0; 43];
    file.read_exact(&mut bytes).is_ok() && bytes == *signature
}
pub fn kernel_filesystem(fd: BorrowedFd<'_>) -> bool {
    // SAFETY: zeroed initialized statfs is valid and writable for fstatfs.
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: fd is live; stat outlives the call.
    if unsafe { libc::fstatfs(fd.as_raw_fd(), &mut stat) } != 0 {
        return false;
    }
    matches!(
        stat.f_type as u32,
        0x42494e4d
            | 0xcafe4a11
            | 0x27e0eb
            | 0x63677270
            | 0x64626720
            | 0x1cd1
            | 0x9fa0
            | 0x6165676c
            | 0x73636673
            | 0xf97cff8c
            | 0x62656572
            | 0x74726163
    )
}
pub fn component_matches(pattern: &CStr, name: &CStr) -> bool {
    // SAFETY: both byte strings are NUL terminated; fnmatch does not retain pointers.
    unsafe { libc::fnmatch(pattern.as_ptr(), name.as_ptr(), libc::FNM_PATHNAME) == 0 }
}
pub fn available_parallelism() -> usize {
    std::thread::available_parallelism().map_or(1, usize::from)
}

pub fn expand_user(bytes: &[u8]) -> Vec<u8> {
    if !bytes.starts_with(b"~") {
        return bytes.to_vec();
    }
    let end = bytes
        .iter()
        .position(|byte| *byte == b'/')
        .unwrap_or(bytes.len());
    if end == 1 {
        if let Some(home) = std::env::var_os("HOME") {
            let mut result = home.as_bytes().to_vec();
            while result.ends_with(b"/") {
                result.pop();
            }
            result.extend_from_slice(&bytes[end..]);
            return result;
        }
    }
    let Ok(name) = c_name(&bytes[1..end]) else {
        return bytes.to_vec();
    };
    // SAFETY: zeroed passwd is a valid output struct; storage remains live until home is copied.
    let mut password: libc::passwd = unsafe { std::mem::zeroed() };
    let mut storage = vec![0u8; 65536];
    let mut result = std::ptr::null_mut();
    // SAFETY: output pointers and scratch buffer are aligned/valid; reentrant lookups retain no pointers.
    let code = unsafe {
        if end == 1 {
            libc::getpwuid_r(
                libc::getuid(),
                &mut password,
                storage.as_mut_ptr().cast(),
                storage.len(),
                &mut result,
            )
        } else {
            libc::getpwnam_r(
                name.as_ptr(),
                &mut password,
                storage.as_mut_ptr().cast(),
                storage.len(),
                &mut result,
            )
        }
    };
    if code == 0 && !result.is_null() && !password.pw_dir.is_null() {
        // SAFETY: successful passwd lookup places a terminated home string in live storage.
        let mut home = unsafe { CStr::from_ptr(password.pw_dir) }
            .to_bytes()
            .to_vec();
        while home.ends_with(b"/") {
            home.pop();
        }
        home.extend_from_slice(&bytes[end..]);
        return home;
    }
    bytes.to_vec()
}

pub fn unlink_at(parent: BorrowedFd<'_>, name: &CStr, directory: bool) -> io::Result<()> {
    // SAFETY: parent is live; name is a terminated basename; unlinkat never follows the target link.
    if unsafe {
        libc::unlinkat(
            parent.as_raw_fd(),
            name.as_ptr(),
            if directory { libc::AT_REMOVEDIR } else { 0 },
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
pub fn path_metadata(path: &Path) -> io::Result<Observation> {
    let parent = root_directory(Path::new("/"))?;
    metadata(
        std::os::fd::AsFd::as_fd(&parent),
        &c_name(path_bytes(path))?,
        false,
        MetadataBackend::Fstatat,
    )
}
pub fn terminal_input() -> bool {
    // SAFETY: isatty only queries the process's standard descriptor.
    unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }
}
static INTERRUPTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
extern "C" fn interrupt_signal(_: libc::c_int) {
    INTERRUPTED.store(true, std::sync::atomic::Ordering::Relaxed);
}
pub fn interrupted() -> bool {
    INTERRUPTED.load(std::sync::atomic::Ordering::Relaxed)
}
pub struct SignalGuard {
    previous: Vec<(libc::c_int, libc::sigaction)>,
}
impl SignalGuard {
    pub fn install() -> io::Result<Self> {
        INTERRUPTED.store(false, std::sync::atomic::Ordering::Relaxed);
        let mut guard = Self {
            previous: Vec::new(),
        };
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            // SAFETY: zeroed sigaction has valid scalar/pointer fields; sigemptyset initializes its mask.
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = interrupt_signal as *const () as usize;
            // SAFETY: action and old are valid writable structs; handler does only an atomic store.
            let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
            // SAFETY: pointers remain valid; signal numbers are catchable and valid.
            if unsafe {
                libc::sigemptyset(&mut action.sa_mask);
                libc::sigaction(signal, &action, &mut old)
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            guard.previous.push((signal, old));
        }
        Ok(guard)
    }
}
impl Drop for SignalGuard {
    fn drop(&mut self) {
        for (signal, old) in &self.previous {
            // SAFETY: restore a previously valid signal action on its original signal.
            unsafe {
                libc::sigaction(*signal, old, std::ptr::null_mut());
            }
        }
    }
}

pub struct TerminalHandle {
    file: File,
    saved: libc::termios,
    raw: bool,
}
impl TerminalHandle {
    pub fn open() -> io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")?;
        // SAFETY: zeroed termios is writable output for tcgetattr.
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: descriptor is live, saved is aligned/writable.
        if unsafe { libc::tcgetattr(file.as_raw_fd(), &mut saved) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut handle = Self {
            file,
            saved,
            raw: false,
        };
        handle.resume()?;
        Ok(handle)
    }
    pub fn resume(&mut self) -> io::Result<()> {
        let mut raw = self.saved;
        // SAFETY: raw is initialized termios; descriptor remains open. cfmakeraw modifies only this struct.
        unsafe {
            libc::cfmakeraw(&mut raw);
        }
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        // SAFETY: applying valid initialized terminal attributes on the owned fd.
        if unsafe { libc::tcsetattr(self.file.as_raw_fd(), libc::TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        self.raw = true;
        use std::io::Write;
        self.file.write_all(b"\x1b[?1049h\x1b[?25l")?;
        self.file.flush()
    }
    pub fn suspend(&mut self) -> io::Result<()> {
        if self.raw {
            use std::io::Write;
            let output = self
                .file
                .write_all(b"\x1b[0m\x1b[?25h\x1b[?1049l")
                .and_then(|()| self.file.flush());
            // SAFETY: saved attributes came from tcgetattr on the same owned descriptor.
            if unsafe { libc::tcsetattr(self.file.as_raw_fd(), libc::TCSANOW, &self.saved) } != 0 {
                return Err(io::Error::last_os_error());
            }
            self.raw = false;
            output?;
        }
        Ok(())
    }
    pub fn dimensions(&self) -> io::Result<(u16, u16)> {
        // SAFETY: initialized winsize is valid writable ioctl output.
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        // SAFETY: TIOCGWINSZ expects this exact pointer type and retains nothing.
        if unsafe { libc::ioctl(self.file.as_raw_fd(), libc::TIOCGWINSZ, &mut size) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((size.ws_col.max(1), size.ws_row.max(1)))
    }
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write;
        self.file.write_all(bytes)?;
        self.file.flush()
    }
    pub fn input(&mut self, timeout_ms: i32) -> io::Result<Vec<u8>> {
        let mut poll = libc::pollfd {
            fd: self.file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll points to one initialized struct, descriptor is live; timeout is bounded by caller.
        let result = unsafe { libc::poll(&mut poll, 1, timeout_ms) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                return Ok(Vec::new());
            }
            return Err(error);
        }
        if poll.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            return Err(io::Error::other("lost controlling terminal"));
        }
        if result == 0 {
            return Ok(Vec::new());
        }
        let mut bytes = [0; 128];
        let length = self.file.read(&mut bytes)?;
        if length == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "lost controlling terminal",
            ));
        }
        Ok(bytes[..length].to_vec())
    }
    pub fn run_command(
        &mut self,
        command: &[Vec<u8>],
        cwd: Option<&Path>,
        delete_path: Option<&Path>,
    ) -> io::Result<std::process::ExitStatus> {
        self.suspend()?;
        let result = (|| {
            let mut process = std::process::Command::new(OsString::from_vec(command[0].clone()));
            for argument in &command[1..] {
                process.arg(OsString::from_vec(argument.clone()));
            }
            if let Some(cwd) = cwd {
                process.current_dir(cwd);
            }
            if let Some(path) = delete_path {
                process.env("NCDU_DELETE_PATH", path);
            }
            let level = std::env::var("NCDU_LEVEL")
                .ok()
                .and_then(|value| value.as_bytes().first().copied())
                .filter(u8::is_ascii_digit)
                .map_or(1, |byte| (byte - b'0' + 1).min(9));
            process.env("NCDU_LEVEL", level.to_string());
            process.stdin(self.file.try_clone()?);
            process.stdout(self.file.try_clone()?);
            process.stderr(self.file.try_clone()?);
            process.status()
        })();
        self.resume()?;
        result
    }
    pub fn panic_restore_hook(&self) -> io::Result<PanicRestoreGuard> {
        let fd = self.file.try_clone()?;
        let saved = self.saved;
        let previous = Arc::new(std::panic::take_hook());
        let invoke_previous = Arc::clone(&previous);
        std::panic::set_hook(Box::new(move |information| {
            // SAFETY: hook owns the duplicate fd and initialized saved attributes for its lifetime.
            unsafe {
                libc::tcsetattr(fd.as_raw_fd(), libc::TCSANOW, &saved);
                libc::write(fd.as_raw_fd(), b"\x1b[?25h\x1b[?1049l".as_ptr().cast(), 14);
            }
            invoke_previous(information);
        }));
        Ok(PanicRestoreGuard {
            previous: Some(previous),
        })
    }
}
type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync + 'static>;
pub struct PanicRestoreGuard {
    previous: Option<Arc<PanicHook>>,
}
impl Drop for PanicRestoreGuard {
    fn drop(&mut self) {
        // Rust forbids changing hooks while unwinding. The already-installed hook restored the tty.
        if !std::thread::panicking() {
            drop(std::panic::take_hook());
            if let Some(previous) = self.previous.take() {
                if let Ok(previous) = Arc::try_unwrap(previous) {
                    std::panic::set_hook(previous);
                }
            }
        }
    }
}
impl Drop for TerminalHandle {
    fn drop(&mut self) {
        let _ = self.suspend();
    }
}

pub fn timestamp(seconds: u64) -> String {
    let Ok(seconds) = libc::time_t::try_from(seconds) else {
        return "invalid timestamp".into();
    };
    // SAFETY: zeroed tm is valid writable output; localtime_r keeps pointers in caller storage.
    let mut time: libc::tm = unsafe { std::mem::zeroed() };
    let mut buffer = [0u8; 80];
    // SAFETY: seconds/time/buffer all remain valid and aligned for their respective calls.
    unsafe {
        if libc::localtime_r(&seconds, &mut time).is_null() {
            return "invalid timestamp".into();
        }
        let length = libc::strftime(
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            c"%Y-%m-%d %H:%M:%S %Z".as_ptr(),
            &time,
        );
        String::from_utf8_lossy(&buffer[..length]).into_owned()
    }
}
pub fn account(id: u32, group: bool) -> String {
    let mut storage = vec![0u8; 65536];
    if group {
        // SAFETY: initialized group and scratch storage outlive the reentrant lookup and name copy.
        let mut entry: libc::group = unsafe { std::mem::zeroed() };
        let mut result = std::ptr::null_mut();
        // SAFETY: all output/scratch pointers are valid and aligned; function retains nothing.
        if unsafe {
            libc::getgrgid_r(
                id,
                &mut entry,
                storage.as_mut_ptr().cast(),
                storage.len(),
                &mut result,
            )
        } == 0
            && !result.is_null()
            && !entry.gr_name.is_null()
        {
            // SAFETY: successful lookup returns a terminated name backed by live scratch storage.
            return format!(
                "{} ({id})",
                sanitize_account(unsafe { CStr::from_ptr(entry.gr_name) }.to_bytes())
            );
        }
    } else {
        // SAFETY: initialized passwd and scratch storage outlive lookup/name copy.
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result = std::ptr::null_mut();
        // SAFETY: valid initialized output/scratch pointers; no retained references.
        if unsafe {
            libc::getpwuid_r(
                id,
                &mut entry,
                storage.as_mut_ptr().cast(),
                storage.len(),
                &mut result,
            )
        } == 0
            && !result.is_null()
            && !entry.pw_name.is_null()
        {
            // SAFETY: successful lookup returns a terminated name in live scratch storage.
            return format!(
                "{} ({id})",
                sanitize_account(unsafe { CStr::from_ptr(entry.pw_name) }.to_bytes())
            );
        }
    }
    id.to_string()
}
fn sanitize_account(bytes: &[u8]) -> String {
    crate::ui::display::sanitize(bytes)
}

/// Reserve standard/output/TTY descriptors and transient open/cache handles before budgeting frames.
pub fn scan_descriptor_budget(workers: usize) -> io::Result<(usize, usize)> {
    // SAFETY: initialized rlimit is aligned writable getrlimit output.
    let mut limits: libc::rlimit = unsafe { std::mem::zeroed() };
    // SAFETY: valid resource and output pointer, retained for the call only.
    let available = if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limits) } == 0 {
        usize::try_from(limits.rlim_cur)
            .unwrap_or(usize::MAX)
            .saturating_sub(16)
    } else {
        48
    };
    if available < workers.saturating_mul(3) + 1 {
        return Err(io::Error::other(
            "scan worker count exceeds the descriptor budget; reduce -t",
        ));
    }
    let queue = 16.min(available - workers * 3).max(1);
    let retained = ((available - queue) / workers)
        .saturating_sub(2)
        .clamp(1, 16);
    Ok((queue, retained))
}
impl Drop for Location {
    fn drop(&mut self) {
        let mut parent = self.parent.take();
        while let Some(node) = parent {
            if let Ok(mut node) = Arc::try_unwrap(node) {
                parent = node.parent.take();
            } else {
                break;
            }
        }
    }
}

/// Legacy imports may have arbitrary display roots. Filesystem actions require a normalized absolute root.
pub fn validate_action_root(path: &Path) -> io::Result<()> {
    if !path.is_absolute()
        || path_bytes(path)
            .split(|byte| *byte == b'/')
            .any(|component| component == b"." || component == b"..")
    {
        return Err(invalid(
            "import root is unsuitable for filesystem actions; use a normalized absolute path",
        ));
    }
    Ok(())
}
