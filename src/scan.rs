//! Fixed scan pool with a bounded handoff queue and cooperative depth-first overflow.
use crate::exclude::{Exclusions, Match};
use crate::model::{EntryId, Kind, Model, NONE, Part};
use crate::os::{self, DirectoryCursor, Location, MetadataBackend, Observation};
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[derive(Clone, Default)]
pub struct ScanOptions {
    pub workers: usize,
    pub extended: bool,
    pub same_filesystem: bool,
    pub follow_symlinks: bool,
    pub exclude_caches: bool,
    pub exclude_kernel: bool,
    pub exclusions: Exclusions,
    pub backend: MetadataBackend,
}
#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}
#[derive(Clone, Copy, Default)]
pub struct Progress {
    pub entries: u64,
}
struct Task {
    id: EntryId,
    device: u64,
    inode: u64,
    location: Arc<Location>,
    fd: Option<OwnedFd>,
}
struct Queue {
    tasks: Vec<Task>,
    outstanding: usize,
}
struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
    cancel: Cancellation,
    entries: AtomicU64,
}
struct Completion {
    id: EntryId,
    head: EntryId,
    error: bool,
}
struct Frame {
    task: Task,
    cursor: DirectoryCursor,
    head: EntryId,
    error: bool,
}
impl Shared {
    fn publish(&self, task: Task) -> Option<Task> {
        let mut queue = self.queue.lock().unwrap();
        // Increment before either publication or cooperative local execution.
        queue.outstanding += 1;
        if queue.tasks.len() < 16 {
            queue.tasks.push(task);
            self.ready.notify_one();
            None
        } else {
            Some(task)
        }
    }
    fn take(&self) -> Option<Task> {
        let mut queue = self.queue.lock().unwrap();
        loop {
            if self.cancel.cancelled() || queue.outstanding == 0 {
                return None;
            }
            if let Some(task) = queue.tasks.pop() {
                return Some(task);
            }
            queue = self
                .ready
                .wait_timeout(queue, Duration::from_millis(100))
                .unwrap()
                .0;
        }
    }
    fn complete(&self) {
        let mut queue = self.queue.lock().unwrap();
        queue.outstanding -= 1;
        if queue.outstanding == 0 {
            self.ready.notify_all();
        }
    }
    fn failed(&self) {
        self.cancel.cancel();
        self.ready.notify_all();
    }
}
fn worker(
    index: u8,
    mut part: Part,
    root: BorrowedFd<'_>,
    options: &ScanOptions,
    shared: &Shared,
) -> io::Result<(Part, Vec<Completion>)> {
    let mut completed = Vec::new();
    let mut frames: Vec<Frame> = Vec::new();
    let mut count = 0;
    loop {
        if shared.cancel.cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "scan cancelled"));
        }
        if frames.is_empty() {
            let Some(mut task) = shared.take() else {
                break;
            };
            let fd = match task
                .fd
                .take()
                .map(Ok)
                .unwrap_or_else(|| task.location.open(root))
            {
                Ok(fd) => fd,
                Err(_) => {
                    completed.push(Completion {
                        id: task.id,
                        head: NONE,
                        error: true,
                    });
                    shared.complete();
                    continue;
                }
            };
            if os::descriptor_metadata(fd.as_fd(), options.backend).map_or(true, |stat| {
                stat.device != task.device || stat.inode != task.inode
            }) {
                completed.push(Completion {
                    id: task.id,
                    head: NONE,
                    error: true,
                });
                shared.complete();
                continue;
            }
            frames.push(Frame {
                task,
                cursor: DirectoryCursor::new(fd),
                head: NONE,
                error: false,
            });
        }
        let frame = frames.last_mut().unwrap();
        if frame.cursor.fd.is_none() {
            match frame
                .task
                .location
                .open(root)
                .and_then(|fd| frame.cursor.resume(fd))
            {
                Ok(()) => {}
                Err(_) => {
                    frame.error = true;
                    let frame = frames.pop().unwrap();
                    completed.push(Completion {
                        id: frame.task.id,
                        head: frame.head,
                        error: frame.error,
                    });
                    shared.complete();
                    continue;
                }
            }
        }
        let raw_fd = frame.cursor.fd.as_ref().unwrap().as_raw_fd();
        // SAFETY: cursor's owned fd remains alive throughout metadata/open calls; next_name borrows only its buffer.
        let fd = unsafe { BorrowedFd::borrow_raw(raw_fd) };
        let name = match frame.cursor.next_name() {
            Ok(Some(name)) => name,
            result => {
                frame.error |= result.is_err();
                let frame = frames.pop().unwrap();
                completed.push(Completion {
                    id: frame.task.id,
                    head: frame.head,
                    error: frame.error,
                });
                shared.complete();
                continue;
            }
        };
        let exclusion = options.exclusions.matches(&frame.task.location, name);
        let mut stat = if exclusion == Match::Any {
            Observation {
                kind: Kind::Pattern,
                ..Observation::default()
            }
        } else {
            os::metadata(fd, name, false, options.backend).unwrap_or_else(|_| Observation {
                kind: Kind::Error,
                ..Observation::default()
            })
        };
        if stat.symlink && options.follow_symlinks {
            if let Ok(mut target) = os::metadata(fd, name, true, options.backend) {
                if target.kind != Kind::Directory {
                    if target.device != frame.task.device && target.kind == Kind::Hardlink {
                        target.kind = Kind::Regular;
                    }
                    stat = target;
                }
            }
        }
        if stat.kind == Kind::Directory && exclusion == Match::Directory {
            stat = Observation {
                kind: Kind::Pattern,
                ..Observation::default()
            };
        }
        if options.same_filesystem
            && stat.kind == Kind::Directory
            && stat.device != frame.task.device
        {
            stat = Observation {
                kind: Kind::OtherFs,
                ..Observation::default()
            };
        }
        let mut child_fd = None;
        let mut read_error = false;
        if stat.kind == Kind::Directory {
            match os::child_directory(fd, name) {
                Ok(opened) => {
                    // Restat the opened object to avoid combining pre-replacement metadata with new children.
                    match os::descriptor_metadata(opened.as_fd(), options.backend) {
                        Ok(observed)
                            if observed.device == stat.device && observed.inode == stat.inode =>
                        {
                            if (options.exclude_caches && os::cache_directory(opened.as_fd()))
                                || (options.exclude_kernel
                                    && stat.device != frame.task.device
                                    && os::kernel_filesystem(opened.as_fd()))
                            {
                                stat = Observation {
                                    kind: if options.exclude_caches
                                        && os::cache_directory(opened.as_fd())
                                    {
                                        Kind::Pattern
                                    } else {
                                        Kind::KernelFs
                                    },
                                    ..Observation::default()
                                };
                            } else {
                                child_fd = Some(opened);
                            }
                        }
                        _ => read_error = true,
                    }
                }
                Err(_) => read_error = true,
            }
        }
        let id = part.add(
            index,
            name.to_bytes(),
            frame.task.id,
            stat,
            options.extended,
        )?;
        part.entries[id.slot()].next = frame.head;
        frame.head = id;
        count += 1;
        if count % 256 == 0 {
            shared.entries.fetch_add(256, Ordering::Relaxed);
        }
        if let Some(child_fd) = child_fd {
            let location = Arc::new(Location {
                parent: Some(Arc::clone(&frame.task.location)),
                name: name.to_bytes().to_vec(),
            });
            let task = Task {
                id,
                device: stat.device,
                inode: stat.inode,
                location,
                fd: None,
            };
            if let Some(task) = shared.publish(task) {
                // At most 16 suspended ancestors retain descriptors and directory buffers.
                if frames.len() >= 16 {
                    let suspended = frames.len() - 16;
                    frames[suspended].cursor.suspend();
                }
                frames.push(Frame {
                    task,
                    cursor: DirectoryCursor::new(child_fd),
                    head: NONE,
                    error: false,
                });
            }
        } else if stat.kind == Kind::Directory {
            completed.push(Completion {
                id,
                head: NONE,
                error: read_error,
            });
        }
    }
    shared.entries.fetch_add(count % 256, Ordering::Relaxed);
    Ok((part, completed))
}
pub fn scan(path: &Path, options: &ScanOptions) -> io::Result<Model> {
    scan_with_progress(path, options, Cancellation::default(), |_| {})
}
/// The hook runs on the calling thread. Cancellation discards partial models and joins every worker.
pub fn scan_with_progress(
    path: &Path,
    options: &ScanOptions,
    cancel: Cancellation,
    mut hook: impl FnMut(Progress),
) -> io::Result<Model> {
    let path = os::absolute(path)?;
    let root_fd = os::root_directory(&path)?;
    let stat = os::descriptor_metadata(root_fd.as_fd(), options.backend)?;
    let workers = if options.workers == 0 {
        os::available_parallelism()
    } else {
        options.workers
    };
    if workers > 255 {
        return Err(os::invalid("scan workers must be 0..255"));
    }
    let mut first = Part::default();
    let root = first.add(0, os::path_bytes(&path), NONE, stat, options.extended)?;
    let location = Arc::new(Location {
        parent: None,
        name: os::path_bytes(&path).to_vec(),
    });
    let shared = Shared {
        queue: Mutex::new(Queue {
            tasks: vec![Task {
                id: root,
                device: stat.device,
                inode: stat.inode,
                location,
                fd: Some(os::child_directory(root_fd.as_fd(), c".")?),
            }],
            outstanding: 1,
        }),
        ready: Condvar::new(),
        cancel,
        entries: AtomicU64::new(0),
    };
    let (send, receive) = std::sync::mpsc::sync_channel(workers);
    let results = std::thread::scope(|scope| -> io::Result<Vec<(Part, Vec<Completion>)>> {
        let mut handles = Vec::new();
        let mut error = None;
        for index in 0..workers {
            let part = if index == 0 {
                std::mem::take(&mut first)
            } else {
                Part::default()
            };
            let send = send.clone();
            let shared = &shared;
            let fd = root_fd.as_fd();
            match std::thread::Builder::new()
                .name(format!("scan-{index}"))
                .stack_size(256 * 1024)
                .spawn_scoped(scope, move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        worker(index as u8, part, fd, options, shared)
                    }))
                    .unwrap_or_else(|_| Err(io::Error::other("scan worker panicked")));
                    if result.is_err() {
                        shared.failed();
                    }
                    let _ = send.send(index);
                    result
                }) {
                Ok(handle) => handles.push(handle),
                Err(failure) => {
                    shared.failed();
                    error = Some(failure);
                    break;
                }
            }
        }
        drop(send);
        let mut finished = 0;
        while finished < handles.len() {
            hook(Progress {
                entries: shared.entries.load(Ordering::Relaxed),
            });
            match receive.recv_timeout(Duration::from_millis(100)) {
                Ok(_) => finished += 1,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => break,
            }
        }
        let mut results = Vec::new();
        for handle in handles {
            match handle.join() {
                Ok(Ok(result)) => results.push(result),
                Ok(Err(failure)) => {
                    error.get_or_insert(failure);
                }
                Err(_) => {
                    shared.failed();
                    error.get_or_insert_with(|| io::Error::other("scan worker panicked"));
                }
            }
        }
        if let Some(error) = error {
            Err(error)
        } else {
            Ok(results)
        }
    })?;
    let mut parts = Vec::with_capacity(workers);
    let mut completions = Vec::new();
    for (part, done) in results {
        parts.push(part);
        completions.extend(done);
    }
    let mut model = Model { parts, root };
    for completion in completions {
        let dir = model.directory_mut(completion.id);
        dir.first_child = completion.head;
        dir.read_error = completion.error;
    }
    model.recount();
    hook(Progress {
        entries: model.len().saturating_sub(1) as u64,
    });
    Ok(model)
}
