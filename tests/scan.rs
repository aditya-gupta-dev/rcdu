use rcdu::{
    model::Kind,
    os,
    scan::{self, ScanOptions},
};
use std::ffi::OsString;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    fs,
    os::unix::{
        ffi::OsStringExt,
        fs::{MetadataExt, symlink},
    },
    path::PathBuf,
};
static SERIAL: AtomicUsize = AtomicUsize::new(0);
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rcdu-test-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn find(
    model: &rcdu::model::Model,
    parent: rcdu::model::EntryId,
    name: &[u8],
) -> rcdu::model::EntryId {
    model
        .children(parent)
        .find(|id| model.name(*id) == name)
        .unwrap()
}
#[test]
fn hardlinks_in_siblings_and_outside_root() {
    let tree = Tree::new();
    let root = tree.0.join("root");
    fs::create_dir(&root).unwrap();
    for dir in ["a", "b"] {
        fs::create_dir(root.join(dir)).unwrap();
    }
    fs::write(root.join("a/file"), b"12345").unwrap();
    fs::hard_link(root.join("a/file"), root.join("b/link")).unwrap();
    fs::hard_link(root.join("a/file"), tree.0.join("outside")).unwrap();
    for workers in [1, 2, 4] {
        let model = scan::scan(
            &root,
            &ScanOptions {
                workers,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(model.totals(model.root).items, 4);
        for name in [b"a", b"b"] {
            let id = find(&model, model.root, name);
            assert_eq!(model.totals(id).shared_apparent, 5);
            assert_eq!(
                model.totals(id).apparent,
                fs::metadata(root.join(os::byte_path(name))).unwrap().size() + 5
            );
        }
        assert_eq!(model.totals(model.root).shared_apparent, 5);
        assert_eq!(
            model.totals(model.root).apparent,
            fs::metadata(&root).unwrap().size()
                + fs::metadata(root.join("a")).unwrap().size()
                + fs::metadata(root.join("b")).unwrap().size()
                + 5
        );
    }
}
#[test]
fn sparse_raw_names_symlinks_and_backend_equivalence() {
    let tree = Tree::new();
    let path = tree.0.join(OsString::from_vec(b"bad\xff\nname".to_vec()));
    let file = fs::File::create(&path).unwrap();
    file.set_len(1 << 30).unwrap();
    symlink(&path, tree.0.join("link")).unwrap();
    symlink("missing", tree.0.join("dangling")).unwrap();
    symlink(&tree.0, tree.0.join("dirlink")).unwrap();
    let first = scan::scan(
        &tree.0,
        &ScanOptions {
            workers: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let second = scan::scan(
        &tree.0,
        &ScanOptions {
            workers: 4,
            backend: os::MetadataBackend::Statx,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(first.totals(first.root), second.totals(second.root));
    let id = find(&first, first.root, b"bad\xff\nname");
    assert_eq!(first.entry(id).apparent, 1 << 30);
    assert_eq!(first.entry(id).allocated(), 0);
    assert_eq!(
        first.entry(find(&first, first.root, b"link")).kind(),
        Kind::NonRegular
    );
    let followed = scan::scan(
        &tree.0,
        &ScanOptions {
            workers: 2,
            follow_symlinks: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        followed
            .entry(find(&followed, followed.root, b"link"))
            .apparent,
        1 << 30
    );
    assert_eq!(
        followed
            .entry(find(&followed, followed.root, b"dirlink"))
            .kind(),
        Kind::NonRegular
    );
}
#[test]
fn queue_saturation_and_deep_completion() {
    let tree = Tree::new();
    for n in 0..96 {
        let path = tree.0.join(n.to_string());
        fs::create_dir(&path).unwrap();
        for m in 0..4 {
            fs::create_dir(path.join(m.to_string())).unwrap();
            fs::write(path.join(m.to_string()).join("f"), b"x").unwrap();
        }
    }
    let mut deep = tree.0.join("deep");
    fs::create_dir(&deep).unwrap();
    for _ in 0..80 {
        deep.push("d");
        fs::create_dir(&deep).unwrap();
    }
    let one = scan::scan(
        &tree.0,
        &ScanOptions {
            workers: 1,
            ..Default::default()
        },
    )
    .unwrap();
    for _ in 0..5 {
        let many = scan::scan(
            &tree.0,
            &ScanOptions {
                workers: 4,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(one.len(), many.len());
        assert_eq!(one.totals(one.root), many.totals(many.root));
    }
}
#[test]
fn cache_signature_and_directory_only_exclusions() {
    let tree = Tree::new();
    for name in ["valid", "invalid", "empty"] {
        fs::create_dir(tree.0.join(name)).unwrap();
    }
    fs::write(
        tree.0.join("valid/CACHEDIR.TAG"),
        b"Signature: 8a477f597d28d172789f06886806bc55rest",
    )
    .unwrap();
    fs::write(tree.0.join("invalid/CACHEDIR.TAG"), b"short").unwrap();
    fs::write(tree.0.join("empty-file"), b"x").unwrap();
    let mut options = ScanOptions {
        workers: 2,
        exclude_caches: true,
        ..Default::default()
    };
    options.exclusions.add(b"empty*/").unwrap();
    let model = scan::scan(&tree.0, &options).unwrap();
    assert_eq!(
        model.entry(find(&model, model.root, b"valid")).kind(),
        Kind::Pattern
    );
    assert_eq!(
        model.entry(find(&model, model.root, b"empty")).kind(),
        Kind::Pattern
    );
    assert_eq!(
        model.entry(find(&model, model.root, b"empty-file")).kind(),
        Kind::Regular
    );
    assert_eq!(
        model.entry(find(&model, model.root, b"invalid")).kind(),
        Kind::Directory
    );
}
