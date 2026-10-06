use rcdu::{
    delete::{self, ErrorChoice},
    model::Kind,
    scan::{self, Cancellation, ScanOptions},
};
use std::{
    fs,
    os::unix::fs::symlink,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
static SERIAL: AtomicUsize = AtomicUsize::new(0);
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "rcdu-mutate-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn deletes_only_scanned_children_and_recounts_survivors() {
    let tree = Tree::new();
    fs::create_dir(tree.0.join("dir")).unwrap();
    fs::write(tree.0.join("dir/file"), b"hello").unwrap();
    fs::hard_link(tree.0.join("dir/file"), tree.0.join("link")).unwrap();
    let mut model = scan::scan(&tree.0, &ScanOptions::default()).unwrap();
    let id = model
        .children(model.root)
        .find(|id| model.name(*id) == b"dir")
        .unwrap();
    fs::write(tree.0.join("dir/new-unseen"), b"new").unwrap();
    let report = delete::remove(&mut model, id, &Cancellation::default(), |_, _| {
        ErrorChoice::Ignore
    })
    .unwrap();
    assert_eq!(report.removed, 1);
    assert_eq!(report.failed, 1);
    assert!(tree.0.join("dir/new-unseen").exists());
    assert!(!tree.0.join("dir/file").exists());
    assert_eq!(model.totals(model.root).shared_apparent, 0);
    let replacement = scan::scan(&tree.0.join("dir"), &ScanOptions::default()).unwrap();
    let mut model = delete::replace_subtree(&model, id, &replacement).unwrap();
    let link = model
        .children(model.root)
        .find(|id| model.name(*id) == b"link")
        .unwrap();
    assert_eq!(model.entry(link).kind(), Kind::Regular);
    let id = model
        .children(model.root)
        .find(|id| model.name(*id) == b"dir")
        .unwrap();
    delete::remove(&mut model, id, &Cancellation::default(), |_, _| {
        ErrorChoice::Abort
    })
    .unwrap();
    assert!(!tree.0.join("dir").exists());
    assert!(tree.0.join("link").exists());
}
#[test]
fn replacement_symlink_and_directory_are_never_traversed() {
    let tree = Tree::new();
    fs::create_dir(tree.0.join("dir")).unwrap();
    fs::write(tree.0.join("dir/file"), b"original").unwrap();
    let mut model = scan::scan(&tree.0, &ScanOptions::default()).unwrap();
    let id = model.children(model.root).next().unwrap();
    fs::rename(tree.0.join("dir"), tree.0.join("protected")).unwrap();
    symlink("protected", tree.0.join("dir")).unwrap();
    let report = delete::remove(&mut model, id, &Cancellation::default(), |_, _| {
        ErrorChoice::Abort
    })
    .unwrap();
    assert!(report.aborted);
    assert!(tree.0.join("protected/file").exists());
    fs::remove_file(tree.0.join("dir")).unwrap();
    fs::create_dir(tree.0.join("dir")).unwrap();
    fs::write(tree.0.join("dir/file"), b"replacement").unwrap();
    let report = delete::remove(&mut model, id, &Cancellation::default(), |_, _| {
        ErrorChoice::Abort
    })
    .unwrap();
    assert!(report.aborted);
    assert_eq!(fs::read(tree.0.join("dir/file")).unwrap(), b"replacement");
}
#[test]
fn cancelled_deletion_and_failed_refresh_retain_original_model() {
    let tree = Tree::new();
    fs::write(tree.0.join("file"), b"safe").unwrap();
    let mut model = scan::scan(&tree.0, &ScanOptions::default()).unwrap();
    let target = model.children(model.root).next().unwrap();
    let before = model.totals(model.root);
    let cancel = Cancellation::default();
    cancel.cancel();
    let report = delete::remove(&mut model, target, &cancel, |_, _| ErrorChoice::Abort).unwrap();
    assert!(report.aborted);
    assert_eq!(before, model.totals(model.root));
    assert!(tree.0.join("file").exists());
    assert!(scan::scan(&tree.0.join("absent"), &ScanOptions::default()).is_err());
    assert_eq!(before, model.totals(model.root));
    assert!(delete::confirmed_missing(&tree.0.join("absent")).unwrap());
    assert!(!delete::confirmed_missing(&tree.0.join("file")).unwrap());
}

#[test]
fn permission_errors_do_not_mean_custom_target_disappeared() {
    use std::os::unix::fs::PermissionsExt;
    // The suite is run as an ordinary user; root would bypass this fixture's premise.
    let tree = Tree::new();
    fs::create_dir(tree.0.join("locked")).unwrap();
    fs::write(tree.0.join("locked/file"), b"safe").unwrap();
    fs::set_permissions(tree.0.join("locked"), fs::Permissions::from_mode(0o0)).unwrap();
    let result = delete::confirmed_missing(&tree.0.join("locked/file"));
    fs::set_permissions(tree.0.join("locked"), fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        result.unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert!(tree.0.join("locked/file").exists());
}
#[test]
fn progress_abort_keeps_unvisited_scanned_children() {
    let tree = Tree::new();
    fs::create_dir(tree.0.join("target")).unwrap();
    for number in 0..300 {
        fs::write(tree.0.join("target").join(number.to_string()), b"x").unwrap();
    }
    let mut model = scan::scan(&tree.0, &ScanOptions::default()).unwrap();
    let target = model.children(model.root).next().unwrap();
    let mut polls = 0;
    let report = delete::remove_with_events(
        &mut model,
        target,
        &Cancellation::default(),
        |event| match event {
            delete::DeleteEvent::Progress { .. } => {
                polls += 1;
                if polls > 1 {
                    ErrorChoice::Abort
                } else {
                    ErrorChoice::Ignore
                }
            }
            _ => ErrorChoice::Abort,
        },
    )
    .unwrap();
    assert!(report.aborted);
    assert!(report.removed > 0 && report.removed < 300);
    assert_eq!(
        fs::read_dir(tree.0.join("target")).unwrap().count(),
        300 - report.removed
    );
    assert_eq!(
        model.totals(model.root).items,
        1 + 300 - report.removed as u64
    );
}
