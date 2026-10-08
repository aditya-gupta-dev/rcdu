use rcdu::{
    model::Kind,
    os,
    scan::{self, Cancellation, Options},
};
use std::os::unix::{
    ffi::OsStringExt,
    fs::{MetadataExt, symlink},
};
use std::{
    fs, io,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
static SERIAL: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "rcdu-batch-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn sparse_sizes_links_in_siblings_and_outside_root() {
    let fixture = Fixture::new();
    let root = fixture.0.join("tree");
    fs::create_dir(&root).unwrap();
    for name in ["a", "b"] {
        fs::create_dir(root.join(name)).unwrap();
    }
    fs::write(root.join("a/h1"), b"12345").unwrap();
    fs::hard_link(root.join("a/h1"), root.join("a/h2")).unwrap();
    fs::hard_link(root.join("a/h1"), root.join("b/h3")).unwrap();
    fs::hard_link(root.join("a/h1"), fixture.0.join("outside")).unwrap();
    let sparse = fs::File::create(root.join("b/sparse")).unwrap();
    sparse.set_len(1 << 30).unwrap();
    let link = fs::metadata(root.join("a/h1")).unwrap();
    for threads in [1, 2, 4, 0] {
        let model = scan::scan(
            &root,
            &Options {
                threads,
                ..Options::default()
            },
        )
        .unwrap();
        let totals = model.directories[0].totals;
        let directories = [
            fs::metadata(&root).unwrap(),
            fs::metadata(root.join("a")).unwrap(),
            fs::metadata(root.join("b")).unwrap(),
        ];
        assert_eq!(
            totals.apparent,
            directories.iter().map(|m| m.size()).sum::<u64>() + 5 + (1 << 30)
        );
        assert_eq!(
            totals.allocated,
            directories.iter().map(|m| m.blocks() * 512).sum::<u64>() + link.blocks() * 512
        );
        assert_eq!(totals.shared_apparent, 5);
        assert_eq!(totals.shared_allocated, link.blocks() * 512);
        assert_eq!(totals.items, 6);
        for entry in model.children(0) {
            let dir = model.directory_id(entry).unwrap();
            assert_eq!(model.directories[dir as usize].totals.shared_apparent, 5);
        }
    }
}
#[test]
fn flat_directory_shares_metadata_and_preserves_raw_names() {
    let fixture = Fixture::new();
    for index in 0..8192 {
        fs::File::create(fixture.0.join(format!("file-{index}"))).unwrap();
    }
    let raw = std::ffi::OsString::from_vec(b"raw\xff\n".to_vec());
    fs::write(fixture.0.join(&raw), b"x").unwrap();
    let model = scan::scan(
        &fixture.0,
        &Options {
            threads: 4,
            ..Options::default()
        },
    )
    .unwrap();
    assert_eq!(model.directories[0].totals.items, 8193);
    assert_eq!(model.len(), 8194);
    assert!(
        model
            .parts
            .iter()
            .filter(|part| !part.entries.is_empty())
            .count()
            > 1,
        "flat directory metadata must reach multiple workers"
    );
    let entry = model
        .children(0)
        .find(|entry| model.name(*entry) == b"raw\xff\n")
        .unwrap();
    assert_eq!(model.path(entry), fixture.0.join(raw));
    assert_eq!(model.children(0).count(), 8193);
    assert_eq!(std::mem::size_of::<rcdu::model::Entry>(), 24);
}
#[test]
fn symlinks_do_not_recurse_and_cancellation_joins_workers() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("file"), b"data").unwrap();
    symlink("file", fixture.0.join("link")).unwrap();
    symlink(&fixture.0, fixture.0.join("cycle")).unwrap();
    let model = scan::scan(
        &fixture.0,
        &Options {
            threads: 4,
            follow_symlinks: true,
            ..Options::default()
        },
    )
    .unwrap();
    let cycle = model
        .children(0)
        .find(|entry| model.name(*entry) == b"cycle")
        .unwrap();
    assert_eq!(model.entry(cycle).kind(), Kind::Other);
    let link = model
        .children(0)
        .find(|entry| model.name(*entry) == b"link")
        .unwrap();
    assert_eq!(model.entry(link).apparent, 4);
    let cancel = Cancellation::default();
    let trigger = cancel.clone();
    let result = scan::scan_with_progress(
        &fixture.0,
        &Options {
            threads: 4,
            ..Options::default()
        },
        cancel,
        |_| trigger.cancel(),
    );
    assert_eq!(result.err().unwrap().kind(), io::ErrorKind::Interrupted);
    let panic = std::panic::catch_unwind(|| {
        let _ = scan::scan_with_progress(
            &fixture.0,
            &Options {
                threads: 4,
                ..Options::default()
            },
            Cancellation::default(),
            |_| panic!("hook"),
        );
    });
    assert!(panic.is_err());
}
#[test]
fn broad_and_deep_subtrees_have_stable_spans_and_directory_totals() {
    let fixture = Fixture::new();
    for index in 0..120 {
        let dir = fixture.0.join(index.to_string());
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("leaf"), b"x").unwrap();
    }
    let mut path = fixture.0.join("deep");
    fs::create_dir(&path).unwrap();
    for _ in 0..80 {
        path.push("d");
        fs::create_dir(&path).unwrap();
    }
    let one = scan::scan(
        &fixture.0,
        &Options {
            threads: 1,
            ..Options::default()
        },
    )
    .unwrap();
    for _ in 0..5 {
        let many = scan::scan(
            &fixture.0,
            &Options {
                threads: 4,
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(one.directories[0].totals, many.directories[0].totals);
        let mut expected: Vec<_> = one
            .directories
            .iter()
            .map(|dir| (os::bytes(&one.path(dir.entry)).to_vec(), dir.totals))
            .collect();
        let mut observed: Vec<_> = many
            .directories
            .iter()
            .map(|dir| (os::bytes(&many.path(dir.entry)).to_vec(), dir.totals))
            .collect();
        expected.sort_by(|a, b| a.0.cmp(&b.0));
        observed.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(expected, observed);
    }
}
