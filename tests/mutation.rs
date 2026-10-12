use rcdu::{
    delete::{self, ErrorAction},
    model::Kind,
    mutation, os, scan,
};
use std::{
    collections::{HashMap, HashSet},
    os::unix::fs::MetadataExt,
    path::PathBuf,
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "rcdu-mutation-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn deletion_preserves_new_children_and_recounts_surviving_hardlinks() {
    let fixture = Fixture::new();
    let root = fixture.0.join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(root.join("a")).unwrap();
    std::fs::create_dir(root.join("b")).unwrap();
    std::fs::write(root.join("a/file"), b"hello").unwrap();
    std::fs::hard_link(root.join("a/file"), root.join("b/link")).unwrap();
    let model = scan::scan(
        &root,
        &scan::Options {
            threads: 4,
            ..Default::default()
        },
    )
    .unwrap();
    let selected = model
        .children(0)
        .find(|id| model.name(*id) == b"a")
        .unwrap();
    std::fs::write(root.join("a/new"), b"not observed").unwrap();
    let outcome = delete::remove(&model, selected, |_, _| ErrorAction::IgnoreAll);
    assert!(!root.join("a/file").exists());
    assert!(root.join("a/new").exists());
    assert!(root.join("a").exists());
    let updated = mutation::rebuild(&model, None, &outcome.removed, &outcome.links).unwrap();
    let survivor = updated.parts.iter().flat_map(|part| &part.links).count();
    assert_eq!(survivor, 0);
    let b = updated
        .children(0)
        .find(|id| updated.name(*id) == b"b")
        .unwrap();
    let b = updated.directory_id(b).unwrap();
    let link = updated.children(b).next().unwrap();
    assert_eq!(updated.entry(link).kind(), Kind::Regular);
    assert_eq!(updated.directories[0].totals.shared_allocated, 0);
}
#[test]
fn replaced_directory_symlink_cannot_escape_owned_scan_and_refresh_replaces_subtree() {
    let fixture = Fixture::new();
    let root = fixture.0.join("root");
    std::fs::create_dir(&root).unwrap();
    let child = root.join("child");
    std::fs::create_dir(&child).unwrap();
    std::fs::write(child.join("old"), b"old").unwrap();
    let outside = fixture.0.join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("keep"), b"keep").unwrap();
    let model = scan::scan(
        &root,
        &scan::Options {
            threads: 2,
            ..Default::default()
        },
    )
    .unwrap();
    let selected = model.children(0).next().unwrap();
    let directory = model.directory_id(selected).unwrap();
    std::fs::rename(&child, root.join("moved")).unwrap();
    std::os::unix::fs::symlink(&outside, &child).unwrap();
    assert!(os::open_model_directory(&model, directory).is_err());
    let outcome = delete::remove(&model, selected, |_, _| ErrorAction::Abort);
    assert!(outcome.aborted);
    assert!(outcome.removed.is_empty());
    assert!(outside.join("keep").exists());
    std::fs::remove_file(&child).unwrap();
    std::fs::rename(root.join("moved"), &child).unwrap();
    std::fs::remove_file(child.join("old")).unwrap();
    std::fs::write(child.join("new"), b"replacement").unwrap();
    let replacement = scan::scan(
        &child,
        &scan::Options {
            threads: 2,
            ..Default::default()
        },
    )
    .unwrap();
    let refreshed = mutation::rebuild(
        &model,
        Some((directory, &replacement)),
        &HashSet::new(),
        &HashMap::new(),
    )
    .unwrap();
    assert_eq!(refreshed.len(), 3);
    let new_directory = refreshed.directory_at_path(&child).unwrap();
    assert_eq!(
        refreshed.name(refreshed.children(new_directory).next().unwrap()),
        b"new"
    );
    assert_eq!(
        refreshed.directories[0].totals,
        scan::scan(
            &root,
            &scan::Options {
                threads: 2,
                ..Default::default()
            }
        )
        .unwrap()
        .directories[0]
            .totals
    );
}

#[test]
fn followed_file_symlink_deletes_link_without_unlinking_its_hardlink_target() {
    let fixture = Fixture::new();
    let root = fixture.0.join("root");
    std::fs::create_dir(&root).unwrap();
    let target = root.join("target");
    std::fs::write(&target, b"keep this data").unwrap();
    std::fs::hard_link(&target, root.join("hardlink")).unwrap();
    std::os::unix::fs::symlink("target", root.join("followed")).unwrap();
    let model = scan::scan(
        &root,
        &scan::Options {
            threads: 2,
            follow_symlinks: true,
            ..Default::default()
        },
    )
    .unwrap();
    let selected = model
        .children(0)
        .find(|id| model.name(*id) == b"followed")
        .unwrap();
    assert_eq!(model.entry(selected).kind(), Kind::Hardlink);
    let outcome = delete::remove(&model, selected, |_, _| ErrorAction::Abort);
    assert!(!outcome.aborted);
    assert!(outcome.removed.contains(&selected));
    assert!(outcome.links.is_empty());
    assert!(!root.join("followed").exists());
    assert_eq!(std::fs::read(&target).unwrap(), b"keep this data");
    assert_eq!(std::fs::metadata(&target).unwrap().nlink(), 2);
}

#[test]
fn followed_single_link_symlink_deletes_only_link_and_replaced_regular_is_rejected() {
    let fixture = Fixture::new();
    let root = fixture.0.join("root");
    std::fs::create_dir(&root).unwrap();
    let target = root.join("target");
    std::fs::write(&target, b"keep target").unwrap();
    let followed = root.join("followed");
    std::os::unix::fs::symlink("target", &followed).unwrap();
    let ordinary = root.join("ordinary");
    std::fs::write(&ordinary, b"ordinary").unwrap();
    let model = scan::scan(
        &root,
        &scan::Options {
            follow_symlinks: true,
            ..Default::default()
        },
    )
    .unwrap();
    let selected = model
        .children(0)
        .find(|id| model.name(*id) == b"followed")
        .unwrap();
    assert_eq!(model.entry(selected).kind(), Kind::Regular);
    assert!(model.entry(selected).was_symlink());
    let outcome = delete::remove(&model, selected, |_, _| ErrorAction::Abort);
    assert!(outcome.removed.contains(&selected));
    assert_eq!(std::fs::read(&target).unwrap(), b"keep target");

    let ordinary_entry = model
        .children(0)
        .find(|id| model.name(*id) == b"ordinary")
        .unwrap();
    std::fs::remove_file(&ordinary).unwrap();
    std::os::unix::fs::symlink("target", &ordinary).unwrap();
    let outcome = delete::remove(&model, ordinary_entry, |_, _| ErrorAction::Abort);
    assert!(outcome.aborted);
    assert!(outcome.removed.is_empty());
    assert!(ordinary.is_symlink());
    assert_eq!(std::fs::read(&target).unwrap(), b"keep target");
}

#[test]
fn deletion_can_stop_after_partial_success_and_preserves_unobserved_files() {
    let fixture = Fixture::new();
    let root = fixture.0.join("root");
    let target = root.join("directory");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("first"), b"one").unwrap();
    std::fs::write(target.join("second"), b"two").unwrap();
    let model = scan::scan(&root, &scan::Options::default()).unwrap();
    let selected = model.children(0).next().unwrap();
    let mut steps = 0;
    let outcome = delete::remove_with_progress(
        &model,
        selected,
        |_, _| ErrorAction::Abort,
        |_| {
            steps += 1;
            steps <= 2
        },
    );
    assert!(outcome.aborted);
    assert_eq!(outcome.removed.len(), 1);
    assert!(target.exists());
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 1);
    let rebuilt = mutation::rebuild(&model, None, &outcome.removed, &outcome.links).unwrap();
    assert_eq!(rebuilt.directories[0].totals.items, 2);
    assert_eq!(rebuilt.directories[1].totals.items, 1);

    std::fs::write(target.join("new-after-scan"), b"new").unwrap();
    let outcome = delete::remove(&rebuilt, rebuilt.children(0).next().unwrap(), |_, _| {
        ErrorAction::IgnoreAll
    });
    assert!(outcome.failures > 0);
    assert!(target.join("new-after-scan").exists());
    assert!(target.exists());
}

#[test]
fn empty_excluded_directory_can_be_removed_without_traversing_unobserved_contents() {
    let fixture = Fixture::new();
    let root = fixture.0.join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(root.join("excluded")).unwrap();
    let mut options = scan::Options::default();
    options.exclusions.add(b"excluded/").unwrap();
    let model = scan::scan(&root, &options).unwrap();
    let selected = model.children(0).next().unwrap();
    assert_eq!(model.entry(selected).kind(), Kind::Excluded);
    assert!(model.directory_id(selected).is_none());
    let outcome = delete::remove(&model, selected, |_, _| ErrorAction::Abort);
    assert!(outcome.removed.contains(&selected));
    assert!(!root.join("excluded").exists());
}
