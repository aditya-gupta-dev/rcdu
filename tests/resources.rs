use rcdu::{
    os,
    scan::{self, Cancellation, ScanOptions},
};
use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
fn tree() -> PathBuf {
    let path = std::env::temp_dir().join(format!("rcdu-resources-{}", std::process::id()));
    fs::create_dir(&path).unwrap();
    for number in 0..128 {
        let dir = path.join(number.to_string());
        fs::create_dir(&dir).unwrap();
        for item in 0..8 {
            fs::write(dir.join(item.to_string()), b"data").unwrap();
        }
    }
    path
}
fn fds() -> usize {
    fs::read_dir("/proc/self/fd").unwrap().count()
}
struct Failing {
    dropped: Arc<AtomicBool>,
    bytes: usize,
}
impl Write for Failing {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.bytes > 100 {
            return Err(io::Error::other("injected output failure"));
        }
        self.bytes += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Drop for Failing {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Relaxed);
    }
}
#[test]
fn cancellation_panic_and_output_failure_join_workers_and_close_descriptors() {
    let path = tree();
    let before = fds();
    let options = ScanOptions {
        workers: 4,
        ..Default::default()
    };
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(scan::scan_with_progress(&path, &options, cancel, |_| {}).is_err());
    assert_eq!(fds(), before);
    let panic = std::panic::catch_unwind(|| {
        scan::scan_with_progress(&path, &options, Cancellation::default(), |_| {
            panic!("injected progress failure")
        })
    });
    assert!(panic.is_err());
    assert_eq!(fds(), before);
    let dropped = Arc::new(AtomicBool::new(false));
    assert!(
        scan::binary(
            &path,
            &options,
            Box::new(Failing {
                dropped: Arc::clone(&dropped),
                bytes: 0
            }),
            4096,
            1,
            Cancellation::default()
        )
        .is_err()
    );
    assert!(dropped.load(Ordering::Relaxed));
    assert_eq!(fds(), before);
    for _ in 0..8 {
        let model = scan::scan(&path, &options).unwrap();
        assert_eq!(model.totals(model.root).items, 128 + 128 * 8);
        drop(model);
        assert_eq!(fds(), before);
    }
    fs::remove_dir_all(path).unwrap();
    assert!(os::scan_descriptor_budget(4).is_ok());
}
