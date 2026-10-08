//! Fixed pool sharing both directories and metadata batches from wide directories.
use crate::model::{Directory, Kind, Model, NO_PARENT, Part, Span, Totals};
use crate::os::{self, DirectoryReader, Location, Metadata};
use std::collections::VecDeque;
use std::ffi::CStr;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

const BATCH_ENTRIES: usize = 256;
#[derive(Clone, Default)]
pub struct Options {
    /// Zero selects process-available CPU parallelism; explicit one remains serial.
    pub threads: usize,
    pub extended: bool,
    pub same_filesystem: bool,
    pub follow_symlinks: bool,
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
pub struct Progress {
    pub entries: u64,
}
struct DirectoryWork {
    directory: u32,
    location: Arc<Location>,
}
struct Batch {
    work: Arc<DirectoryWork>,
    descriptor: Arc<OwnedFd>,
    names: Vec<u8>,
    offsets: Vec<u32>,
}
enum Job {
    Directory(u32),
    Metadata(Batch),
}
struct Queue {
    jobs: VecDeque<Job>,
    outstanding: usize,
}
struct Registry {
    directories: Vec<Directory>,
    work: Vec<Option<Arc<DirectoryWork>>>,
}
struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
    registry: Mutex<Registry>,
    root: OwnedFd,
    capacity: usize,
    cancel: Cancellation,
    entries: AtomicU64,
}
impl Shared {
    fn publish(&self, job: Job) -> Option<Job> {
        let mut queue = self.queue.lock().unwrap();
        queue.outstanding += 1;
        if queue.jobs.len() == self.capacity {
            return Some(job);
        }
        queue.jobs.push_back(job);
        self.ready.notify_one();
        None
    }
    fn take(&self) -> Option<Job> {
        let mut queue = self.queue.lock().unwrap();
        loop {
            if self.cancel.cancelled() || queue.outstanding == 0 {
                return None;
            }
            if let Some(job) = queue.jobs.pop_front() {
                return Some(job);
            }
            queue = self
                .ready
                .wait_timeout(queue, Duration::from_millis(50))
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
    fn fail(&self) {
        self.cancel.cancel();
        self.ready.notify_all();
    }
    fn check(&self) -> io::Result<()> {
        if self.cancel.cancelled() {
            Err(io::Error::new(io::ErrorKind::Interrupted, "scan cancelled"))
        } else {
            Ok(())
        }
    }
}
fn classify(stat: Metadata) -> Kind {
    if stat.directory() {
        Kind::Directory
    } else if stat.links > 1 {
        Kind::Hardlink
    } else if stat.mode & libc::S_IFMT == libc::S_IFREG {
        Kind::Regular
    } else {
        Kind::Other
    }
}
fn metadata(options: &Options, batch: &Batch, name: &CStr) -> (Kind, Metadata) {
    let Ok(mut stat) = os::stat(batch.descriptor.as_fd(), name, false) else {
        return (Kind::Error, Metadata::default());
    };
    if options.follow_symlinks && stat.symlink() {
        if let Ok(target) = os::stat(batch.descriptor.as_fd(), name, true) {
            if !target.directory() {
                stat = target;
                if stat.device != batch.work.location.device {
                    stat.links = 1;
                }
            }
        }
    }
    if options.same_filesystem && stat.device != batch.work.location.device {
        (Kind::OtherFs, Metadata::default())
    } else {
        (classify(stat), stat)
    }
}
fn observe_batch(
    index: u8,
    part: &mut Part,
    options: &Options,
    shared: &Shared,
    batch: Batch,
    deferred: &mut Vec<u32>,
) -> io::Result<()> {
    shared.check()?;
    let start = part.entries.len() as u32;
    let mut totals = Totals {
        items: batch.offsets.len() as u64,
        ..Totals::default()
    };
    let mut directories = Vec::new();
    let mut error = false;
    for offset in &batch.offsets {
        let name = CStr::from_bytes_until_nul(&batch.names[*offset as usize..])
            .expect("owned enumerated name");
        let (kind, stat) = metadata(options, &batch, name);
        let entry = part.add(
            index,
            name.to_bytes(),
            batch.work.directory,
            kind,
            stat,
            options.extended,
        )?;
        if kind == Kind::Directory {
            let location = Arc::new(Location {
                parent: Some(Arc::clone(&batch.work.location)),
                name: name.to_owned(),
                device: stat.device,
                inode: stat.inode,
            });
            directories.push((entry, stat, location));
        } else if kind != Kind::Hardlink {
            totals.add(Totals {
                allocated: part.entries[entry.slot()].allocated(),
                apparent: stat.apparent,
                ..Totals::default()
            });
        }
        error |= kind == Kind::Error;
    }
    let new_directories = {
        let mut registry = shared.registry.lock().unwrap();
        let parent = &mut registry.directories[batch.work.directory as usize];
        parent.totals.add(totals);
        parent.descendant_error |= error;
        parent.spans.push(Span {
            worker: index,
            start,
            length: batch.offsets.len() as u32,
        });
        let mut discovered = Vec::with_capacity(directories.len());
        for (entry, stat, location) in directories {
            let id = u32::try_from(registry.directories.len())
                .ok()
                .filter(|id| *id != NO_PARENT)
                .ok_or_else(|| os::invalid("directory capacity exceeded"))?;
            registry
                .directories
                .push(Directory::new(entry, batch.work.directory, stat));
            registry.work.push(Some(Arc::new(DirectoryWork {
                directory: id,
                location,
            })));
            part.directories.push((entry.slot() as u32, id));
            discovered.push(id);
        }
        discovered
    };
    shared
        .entries
        .fetch_add(batch.offsets.len() as u64, Ordering::Relaxed);
    // Publish only after directory records/IDs exist. Overflow stores IDs, never owned descriptors.
    for id in new_directories {
        if shared.publish(Job::Directory(id)).is_some() {
            deferred.push(id);
        }
    }
    Ok(())
}
fn enumerate(
    index: u8,
    part: &mut Part,
    options: &Options,
    shared: &Shared,
    reader: &mut DirectoryReader,
    id: u32,
    deferred: &mut Vec<u32>,
) -> io::Result<()> {
    let work = shared.registry.lock().unwrap().work[id as usize]
        .take()
        .expect("directory enumerated once");
    let descriptor = match work.location.open(shared.root.as_fd()) {
        Ok(descriptor) => Arc::new(descriptor),
        Err(_) => {
            shared.registry.lock().unwrap().directories[id as usize].read_error = true;
            return Ok(());
        }
    };
    reader.reset();
    let mut ended = false;
    while !ended {
        shared.check()?;
        let mut batch = Batch {
            work: Arc::clone(&work),
            descriptor: Arc::clone(&descriptor),
            names: Vec::with_capacity(4096),
            offsets: Vec::with_capacity(BATCH_ENTRIES),
        };
        while batch.offsets.len() < BATCH_ENTRIES {
            match reader.next(descriptor.as_fd()) {
                Ok(Some(name)) => {
                    batch.offsets.push(batch.names.len() as u32);
                    batch.names.extend_from_slice(name.to_bytes_with_nul());
                }
                result => {
                    ended = true;
                    if result.is_err() {
                        shared.registry.lock().unwrap().directories[id as usize].read_error = true;
                    }
                    break;
                }
            }
        }
        if batch.offsets.is_empty() {
            break;
        }
        if let Some(Job::Metadata(batch)) = shared.publish(Job::Metadata(batch)) {
            observe_batch(index, part, options, shared, batch, deferred)?;
            shared.complete();
        }
    }
    Ok(())
}
fn worker(index: u8, mut part: Part, options: &Options, shared: &Shared) -> io::Result<Part> {
    let mut reader = DirectoryReader::default();
    let mut deferred = Vec::new();
    loop {
        shared.check()?;
        let job = if let Some(id) = deferred.pop() {
            Job::Directory(id)
        } else if let Some(job) = shared.take() {
            job
        } else {
            break;
        };
        match job {
            Job::Directory(id) => enumerate(
                index,
                &mut part,
                options,
                shared,
                &mut reader,
                id,
                &mut deferred,
            )?,
            Job::Metadata(batch) => {
                observe_batch(index, &mut part, options, shared, batch, &mut deferred)?
            }
        }
        shared.complete();
    }
    Ok(part)
}

pub fn scan(path: &Path, options: &Options) -> io::Result<Model> {
    scan_with_progress(path, options, Cancellation::default(), |_| {})
}
pub fn scan_with_progress(
    path: &Path,
    options: &Options,
    cancel: Cancellation,
    mut hook: impl FnMut(Progress),
) -> io::Result<Model> {
    let path = std::fs::canonicalize(path)?;
    let root = os::open_root(&path)?;
    let stat = os::stat_directory(root.as_fd())?;
    let threads = if options.threads == 0 {
        std::thread::available_parallelism().map_or(1, usize::from)
    } else {
        options.threads
    };
    if threads > 255 {
        return Err(os::invalid("scan workers must be 0..255"));
    }
    let capacity = os::pool_budget(threads)?;
    let mut first = Part::default();
    let entry = first.add(
        0,
        os::bytes(&path),
        NO_PARENT,
        Kind::Directory,
        stat,
        options.extended,
    )?;
    first.directories.push((entry.slot() as u32, 0));
    let location = Arc::new(Location {
        parent: None,
        name: os::name(os::bytes(&path))?,
        device: stat.device,
        inode: stat.inode,
    });
    let shared = Shared {
        queue: Mutex::new(Queue {
            jobs: VecDeque::from([Job::Directory(0)]),
            outstanding: 1,
        }),
        ready: Condvar::new(),
        registry: Mutex::new(Registry {
            directories: vec![Directory::new(entry, NO_PARENT, stat)],
            work: vec![Some(Arc::new(DirectoryWork {
                directory: 0,
                location,
            }))],
        }),
        root,
        capacity,
        cancel,
        entries: AtomicU64::new(0),
    };
    let parts = std::thread::scope(|scope| -> io::Result<Vec<Part>> {
        let (send, receive) = std::sync::mpsc::sync_channel(threads);
        let mut handles = Vec::new();
        let mut error = None;
        for index in 0..threads {
            let part = if index == 0 {
                std::mem::take(&mut first)
            } else {
                Part::default()
            };
            let send = send.clone();
            let shared = &shared;
            match std::thread::Builder::new()
                .name(format!("scan-{index}"))
                .stack_size(256 * 1024)
                .spawn_scoped(scope, move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        worker(index as u8, part, options, shared)
                    }))
                    .unwrap_or_else(|_| Err(io::Error::other("scan worker panicked")));
                    if result.is_err() {
                        shared.fail();
                    }
                    let _ = send.send(());
                    result
                }) {
                Ok(handle) => handles.push(handle),
                Err(failure) => {
                    shared.fail();
                    error = Some(failure);
                    break;
                }
            }
        }
        drop(send);
        let mut completed = 0;
        let mut panic = None;
        while completed < handles.len() {
            if let Err(failure) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                hook(Progress {
                    entries: shared.entries.load(Ordering::Relaxed),
                })
            })) {
                shared.fail();
                panic = Some(failure);
                break;
            }
            match receive.recv_timeout(Duration::from_millis(50)) {
                Ok(()) => completed += 1,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => break,
            }
        }
        let mut parts = Vec::new();
        for handle in handles {
            match handle.join() {
                Ok(Ok(part)) => parts.push(part),
                Ok(Err(failure)) => {
                    error.get_or_insert(failure);
                }
                Err(_) => {
                    error.get_or_insert_with(|| io::Error::other("worker join panicked"));
                }
            }
        }
        if let Some(panic) = panic {
            std::panic::resume_unwind(panic);
        }
        if let Some(error) = error {
            Err(error)
        } else {
            Ok(parts)
        }
    })?;
    shared.check()?;
    let directories = std::mem::take(&mut shared.registry.lock().unwrap().directories);
    let mut model = Model { parts, directories };
    model.finish_accounting(|| shared.cancel.cancelled())?;
    hook(Progress {
        entries: model.len().saturating_sub(1) as u64,
    });
    shared.check()?;
    Ok(model)
}
