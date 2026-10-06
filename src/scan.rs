//! Fixed scan pool with a bounded handoff queue and cooperative depth-first overflow.
use crate::exclude::{Exclusions, Match};
use crate::format::binary_pool;
use crate::model::{EntryId, Kind, Model, NONE, Part};
use crate::os::{self, DirectoryCursor, Location, MetadataBackend, Observation};
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[derive(Clone)]
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
    binary: Option<Arc<binary_pool::Directory>>,
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
    queue_capacity: usize,
    retained_frames: usize,
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
        if queue.tasks.len() < self.queue_capacity {
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
enum Collector {
    Memory {
        index: u8,
        part: Part,
        completed: Vec<Completion>,
    },
    Binary(binary_pool::Worker),
}
impl Collector {
    fn add(
        &mut self,
        name: &[u8],
        task: &Task,
        stat: Observation,
        extended: bool,
        head: EntryId,
    ) -> io::Result<(EntryId, Option<Arc<binary_pool::Directory>>)> {
        match self {
            Self::Memory { part, index, .. } => {
                let id = part.add(*index, name, task.id, stat, extended)?;
                part.entries[id.slot()].next = head;
                Ok((id, None))
            }
            Self::Binary(worker) => {
                let parent = task.binary.as_ref().expect("binary task context");
                if stat.kind == Kind::Directory {
                    Ok((
                        NONE,
                        Some(binary_pool::Directory::child(parent, name, stat)?),
                    ))
                } else {
                    worker.file(parent, name, stat)?;
                    Ok((NONE, None))
                }
            }
        }
    }
    fn complete(&mut self, task: &Task, head: EntryId, error: bool) -> io::Result<()> {
        match self {
            Self::Memory { completed, .. } => completed.push(Completion {
                id: task.id,
                head,
                error,
            }),
            Self::Binary(worker) => worker.complete(
                Arc::clone(task.binary.as_ref().expect("binary directory context")),
                error,
            )?,
        }
        Ok(())
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Memory { .. } => Ok(()),
            Self::Binary(worker) => worker.flush(),
        }
    }
}
fn worker(
    mut collector: Collector,
    root: BorrowedFd<'_>,
    options: &ScanOptions,
    shared: &Shared,
) -> io::Result<Collector> {
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
                    collector.complete(&task, NONE, true)?;
                    shared.complete();
                    continue;
                }
            };
            if os::descriptor_metadata(fd.as_fd(), options.backend).map_or(true, |stat| {
                stat.device != task.device || stat.inode != task.inode
            }) {
                collector.complete(&task, NONE, true)?;
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
            match frame.task.location.open(root).and_then(|fd| {
                let stat = os::descriptor_metadata(fd.as_fd(), options.backend)?;
                if stat.device != frame.task.device || stat.inode != frame.task.inode {
                    return Err(io::Error::other("directory replaced while suspended"));
                }
                frame.cursor.resume(fd)
            }) {
                Ok(()) => {}
                Err(_) => {
                    frame.error = true;
                    let frame = frames.pop().unwrap();
                    collector.complete(&frame.task, frame.head, frame.error)?;
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
                collector.complete(&frame.task, frame.head, frame.error)?;
                shared.complete();
                continue;
            }
        };
        let (stat, child_fd, read_error) =
            observe(options, fd, frame.task.device, &frame.task.location, name);
        let (id, binary) = collector.add(
            name.to_bytes(),
            &frame.task,
            stat,
            options.extended,
            frame.head,
        )?;
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
                fd: Some(child_fd),
                binary,
            };
            if let Some(mut task) = shared.publish(task) {
                let child_fd = task.fd.take().expect("cooperative child descriptor");
                // At most 16 suspended ancestors retain descriptors and directory buffers.
                if frames.len() >= shared.retained_frames {
                    let suspended = frames.len() - shared.retained_frames;
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
            let task = Task {
                id,
                device: stat.device,
                inode: stat.inode,
                location: Arc::clone(&frame.task.location),
                fd: None,
                binary,
            };
            collector.complete(&task, NONE, read_error)?;
        }
    }
    shared.entries.fetch_add(count % 256, Ordering::Relaxed);
    collector.flush()?;
    Ok(collector)
}
pub fn scan(path: &Path, options: &ScanOptions) -> io::Result<Model> {
    scan_with_progress(path, options, Cancellation::default(), |_| {})
}
/// The hook runs on the calling thread. Cancellation discards partial models and joins every worker.
pub fn scan_with_progress(
    path: &Path,
    options: &ScanOptions,
    cancel: Cancellation,
    hook: impl FnMut(Progress),
) -> io::Result<Model> {
    scan_destination(path, options, cancel, hook, None, true)?
        .ok_or_else(|| os::invalid("missing memory model"))
}
/// JSON staging preserves observations but skips totals that its writer never consumes.
pub(crate) fn stage_with_progress(
    path: &Path,
    options: &ScanOptions,
    cancel: Cancellation,
    hook: impl FnMut(Progress),
) -> io::Result<Model> {
    scan_destination(path, options, cancel, hook, None, false)?
        .ok_or_else(|| os::invalid("missing staged model"))
}
/// Parallel, bounded-memory binary export; the output is owned until every worker flushes.
pub fn binary(
    path: &Path,
    options: &ScanOptions,
    output: Box<dyn std::io::Write + Send>,
    block_size: usize,
    level: i32,
    cancel: Cancellation,
) -> io::Result<()> {
    binary_with_progress(path, options, output, block_size, level, cancel, |_| {})
}
pub fn binary_with_progress(
    path: &Path,
    options: &ScanOptions,
    output: Box<dyn std::io::Write + Send>,
    block_size: usize,
    level: i32,
    cancel: Cancellation,
    mut hook: impl FnMut(Progress),
) -> io::Result<()> {
    let output = binary_pool::Output::new(output, block_size, level, options.extended)?;
    let interruption = cancel.clone();
    scan_destination(
        path,
        options,
        cancel,
        |progress| {
            if os::interrupted() {
                interruption.cancel();
            }
            hook(progress);
        },
        Some(Arc::clone(&output)),
        false,
    )?;
    output.finish()
}
fn scan_destination(
    path: &Path,
    options: &ScanOptions,
    cancel: Cancellation,
    mut hook: impl FnMut(Progress),
    output: Option<Arc<binary_pool::Output>>,
    aggregate: bool,
) -> io::Result<Option<Model>> {
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
    let (queue_capacity, retained_frames) = os::scan_descriptor_budget(workers)?;
    let mut first = Part::default();
    let root = if output.is_none() {
        first.add(0, os::path_bytes(&path), NONE, stat, options.extended)?
    } else {
        NONE
    };
    let binary = output
        .as_ref()
        .map(|_| binary_pool::Directory::root(os::path_bytes(&path), stat));
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
                binary,
            }],
            outstanding: 1,
        }),
        ready: Condvar::new(),
        cancel,
        entries: AtomicU64::new(0),
        queue_capacity,
        retained_frames,
    };
    let (send, receive) = std::sync::mpsc::sync_channel(workers);
    let results = std::thread::scope(|scope| -> io::Result<Vec<Collector>> {
        let mut handles = Vec::new();
        let mut error = None;
        for index in 0..workers {
            let collector = if let Some(output) = &output {
                Collector::Binary(binary_pool::Worker::new(Arc::clone(output)))
            } else {
                Collector::Memory {
                    index: index as u8,
                    part: if index == 0 {
                        std::mem::take(&mut first)
                    } else {
                        Part::default()
                    },
                    completed: Vec::new(),
                }
            };
            let send = send.clone();
            let shared = &shared;
            let fd = root_fd.as_fd();
            match std::thread::Builder::new()
                .name(format!("scan-{index}"))
                .stack_size(256 * 1024)
                .spawn_scoped(scope, move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        worker(collector, fd, options, shared)
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
        let mut hook_panic = None;
        while finished < handles.len() {
            if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                hook(Progress {
                    entries: shared.entries.load(Ordering::Relaxed),
                })
            })) {
                shared.failed();
                hook_panic = Some(panic);
                break;
            }
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
        if let Some(panic) = hook_panic {
            std::panic::resume_unwind(panic);
        }
        if let Some(error) = error {
            Err(error)
        } else {
            Ok(results)
        }
    })?;
    let mut parts = Vec::with_capacity(workers);
    let mut completions = Vec::new();
    if output.is_some() {
        return Ok(None);
    }
    for collector in results {
        if let Collector::Memory {
            part, completed, ..
        } = collector
        {
            parts.push(part);
            completions.extend(completed);
        }
    }
    let mut model = Model { parts, root };
    for completion in completions {
        let dir = model.directory_mut(completion.id);
        dir.first_child = completion.head;
        dir.read_error = completion.error;
    }
    if aggregate {
        model.recount_with_cancel(|| shared.cancel.cancelled() || os::interrupted())?;
    }
    hook(Progress {
        entries: model.len().saturating_sub(1) as u64,
    });
    if shared.cancel.cancelled() || os::interrupted() {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "scan cancelled"));
    }
    Ok(Some(model))
}

pub(crate) fn observe(
    options: &ScanOptions,
    fd: BorrowedFd<'_>,
    parent_device: u64,
    location: &Location,
    name: &std::ffi::CStr,
) -> (Observation, Option<OwnedFd>, bool) {
    let exclusion = options.exclusions.matches(location, name);
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
                if target.device != parent_device && target.kind == Kind::Hardlink {
                    target.kind = Kind::Regular;
                    target.links = 1;
                }
                stat = target;
            }
        }
    }
    if options.same_filesystem
        && !stat.kind.excluded()
        && stat.kind != Kind::Error
        && stat.device != parent_device
    {
        stat = Observation {
            kind: Kind::OtherFs,
            ..Observation::default()
        };
    }
    if stat.kind == Kind::Directory && exclusion == Match::Directory {
        stat = Observation {
            kind: Kind::Pattern,
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
                        let cache = options.exclude_caches && os::cache_directory(opened.as_fd());
                        let kernel = !cache
                            && options.exclude_kernel
                            && stat.device != parent_device
                            && os::kernel_filesystem(opened.as_fd());
                        if cache || kernel {
                            stat = Observation {
                                kind: if cache { Kind::Pattern } else { Kind::KernelFs },
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
    (stat, child_fd, read_error)
}

/// Low-memory depth-first source for a serial sink. Successful completion balances every directory.
pub fn stream(
    path: &Path,
    options: &ScanOptions,
    sink: &mut impl crate::sink::Sink,
    cancel: &Cancellation,
) -> io::Result<()> {
    stream_with_progress(path, options, sink, cancel, |_| {})
}
pub fn stream_with_progress(
    path: &Path,
    options: &ScanOptions,
    sink: &mut impl crate::sink::Sink,
    cancel: &Cancellation,
    mut hook: impl FnMut(Progress),
) -> io::Result<()> {
    let path = os::absolute(path)?;
    let root_fd = os::root_directory(&path)?;
    let stat = os::descriptor_metadata(root_fd.as_fd(), options.backend)?;
    let location = Arc::new(Location {
        parent: None,
        name: os::path_bytes(&path).to_vec(),
    });
    let (_, retained_frames) = os::scan_descriptor_budget(1)?;
    let mut count = 0;
    sink.begin(os::path_bytes(&path), stat, 0, false)?;
    let mut frames = vec![Frame {
        task: Task {
            id: NONE,
            device: stat.device,
            inode: stat.inode,
            location,
            fd: None,
            binary: None,
        },
        cursor: DirectoryCursor::new(os::child_directory(root_fd.as_fd(), c".")?),
        head: NONE,
        error: false,
    }];
    loop {
        if cancel.cancelled() || os::interrupted() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "scan cancelled"));
        }
        let Some(frame) = frames.last_mut() else {
            hook(Progress { entries: count });
            return Ok(());
        };
        if frame.cursor.fd.is_none()
            && frame
                .task
                .location
                .open(root_fd.as_fd())
                .and_then(|fd| {
                    let stat = os::descriptor_metadata(fd.as_fd(), options.backend)?;
                    if stat.device != frame.task.device || stat.inode != frame.task.inode {
                        return Err(io::Error::other("directory replaced while suspended"));
                    }
                    frame.cursor.resume(fd)
                })
                .is_err()
        {
            frames.pop();
            sink.end(true)?;
            continue;
        }
        let raw_fd = frame.cursor.fd.as_ref().unwrap().as_raw_fd();
        // SAFETY: cursor owns this live fd throughout the borrowed name and observation calls.
        let fd = unsafe { BorrowedFd::borrow_raw(raw_fd) };
        let name = match frame.cursor.next_name() {
            Ok(Some(name)) => name,
            result => {
                let error = result.is_err();
                let frame = frames.pop().unwrap();
                sink.end(error || frame.error)?;
                continue;
            }
        };
        let (stat, child, error) =
            observe(options, fd, frame.task.device, &frame.task.location, name);
        count += 1;
        if count % 256 == 0 {
            hook(Progress { entries: count });
        }
        if stat.kind == Kind::Directory {
            sink.begin(name.to_bytes(), stat, frame.task.device, error)?;
            if let Some(fd) = child {
                let location = Arc::new(Location {
                    parent: Some(Arc::clone(&frame.task.location)),
                    name: name.to_bytes().to_vec(),
                });
                if frames.len() >= retained_frames {
                    let suspended = frames.len() - retained_frames;
                    frames[suspended].cursor.suspend();
                }
                frames.push(Frame {
                    task: Task {
                        id: NONE,
                        device: stat.device,
                        inode: stat.inode,
                        location,
                        fd: None,
                        binary: None,
                    },
                    cursor: DirectoryCursor::new(fd),
                    head: NONE,
                    error: false,
                });
            } else {
                sink.end(error)?;
            }
        } else {
            sink.file(name.to_bytes(), stat, frame.task.device)?;
        }
    }
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            workers: 1,
            extended: false,
            same_filesystem: false,
            follow_symlinks: false,
            exclude_caches: false,
            exclude_kernel: false,
            exclusions: Exclusions::default(),
            backend: MetadataBackend::default(),
        }
    }
}
