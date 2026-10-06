//! Mutations operate on scanned children through no-follow parent descriptors.
use crate::{
    model::{EntryId, Kind, Model, NONE, Part},
    os,
    scan::Cancellation,
};
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::path::PathBuf;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorChoice {
    Abort,
    Ignore,
    IgnoreAll,
}
#[derive(Default, Debug)]
pub struct DeleteReport {
    pub removed: usize,
    pub failed: usize,
    pub aborted: bool,
}
fn open_directory(model: &Model, id: EntryId) -> io::Result<OwnedFd> {
    let mut ancestors = Vec::new();
    let mut current = id;
    while current != model.root {
        ancestors.push(current);
        current = model.parent(current);
        if current == NONE {
            return Err(os::invalid("detached deletion target"));
        }
    }
    os::validate_action_root(&model.path(model.root))?;
    let mut fd = os::root_directory(&model.path(model.root))?;
    verify_directory(model, model.root, &fd)?;
    for id in ancestors.into_iter().rev() {
        fd = os::child_directory(fd.as_fd(), &os::c_name(model.name(id))?)?;
        verify_directory(model, id, &fd)?;
    }
    Ok(fd)
}
fn verify_directory(model: &Model, id: EntryId, fd: &OwnedFd) -> io::Result<()> {
    let dir = model
        .directory(id)
        .ok_or_else(|| os::invalid("expected scanned directory"))?;
    let stat = os::descriptor_metadata(fd.as_fd(), os::MetadataBackend::Fstatat)?;
    if dir.inode != 0 && (stat.inode != dir.inode || stat.device != dir.device) {
        return Err(io::Error::other("directory was replaced since scanning"));
    }
    Ok(())
}
pub fn detach(model: &mut Model, id: EntryId) -> io::Result<()> {
    let parent = model.parent(id);
    if parent == NONE {
        return Err(os::invalid("cannot delete root"));
    }
    let next = model.entry(id).next;
    let head = model.directory(parent).unwrap().first_child;
    if head == id {
        model.directory_mut(parent).first_child = next;
    } else {
        let previous = model
            .children(parent)
            .find(|previous| model.entry(*previous).next == id)
            .ok_or_else(|| os::invalid("deletion target no longer listed"))?;
        model.entry_mut(previous).next = next;
    }
    Ok(())
}
/// Confirmation belongs to the browser. The callback receives each actual OS failure.
/// Successful deletions detach immediately; failures keep surviving model records.
pub fn remove(
    model: &mut Model,
    target: EntryId,
    cancel: &Cancellation,
    mut on_error: impl FnMut(&PathBuf, &io::Error) -> ErrorChoice,
) -> io::Result<DeleteReport> {
    remove_with_events(model, target, cancel, |event| match event {
        DeleteEvent::Error { path, error } => on_error(path, error),
        DeleteEvent::Progress { .. } => ErrorChoice::Ignore,
    })
}
pub enum DeleteEvent<'a> {
    Error {
        path: &'a PathBuf,
        error: &'a io::Error,
    },
    Progress {
        path: &'a PathBuf,
        removed: usize,
        failed: usize,
    },
}
pub fn remove_with_events(
    model: &mut Model,
    target: EntryId,
    cancel: &Cancellation,
    mut on_event: impl FnMut(DeleteEvent<'_>) -> ErrorChoice,
) -> io::Result<DeleteReport> {
    if target == model.root || model.parent(target) == NONE {
        return Err(os::invalid("cannot delete root"));
    }
    struct Frame {
        id: EntryId,
        entered: bool,
        next: EntryId,
    }
    let mut stack = vec![Frame {
        id: target,
        entered: false,
        next: NONE,
    }];
    let mut report = DeleteReport::default();
    let mut ignore_all = false;
    let mut steps = 0usize;
    let mut unlinked = std::collections::HashMap::new();
    while let Some(frame) = stack.last_mut() {
        if cancel.cancelled() || os::interrupted() {
            report.aborted = true;
            break;
        }
        let id = frame.id;
        if steps % 128 == 0
            && on_event(DeleteEvent::Progress {
                path: &model.path(id),
                removed: report.removed,
                failed: report.failed,
            }) == ErrorChoice::Abort
        {
            report.aborted = true;
            break;
        }
        steps += 1;
        if !frame.entered && model.entry(id).kind() == Kind::Directory {
            let result = open_directory(model, id);
            if let Err(error) = result {
                report.failed += 1;
                let choice = if ignore_all {
                    ErrorChoice::Ignore
                } else {
                    on_event(DeleteEvent::Error {
                        path: &model.path(id),
                        error: &error,
                    })
                };
                ignore_all |= choice == ErrorChoice::IgnoreAll;
                if choice == ErrorChoice::Abort {
                    report.aborted = true;
                    break;
                }
                stack.pop();
                continue;
            }
            frame.next = model.directory(id).unwrap().first_child;
            frame.entered = true;
        }
        if frame.next != NONE {
            let child = frame.next;
            frame.next = model.entry(child).next;
            stack.push(Frame {
                id: child,
                entered: false,
                next: NONE,
            });
            continue;
        }
        let result = open_directory(model, model.parent(id)).and_then(|fd| {
            let name = os::c_name(model.name(id))?;
            let before = os::metadata(fd.as_fd(), &name, false, os::MetadataBackend::Fstatat).ok();
            os::unlink_at(fd.as_fd(), &name, model.entry(id).kind().directory_like())?;
            Ok(before)
        });
        match result {
            Ok(before) => {
                if let Some(stat) = before {
                    if stat.kind != Kind::Directory
                        && !stat.symlink
                        && (model.entry(id).kind() == Kind::Hardlink || stat.links > 1)
                    {
                        unlinked.insert(
                            crate::model::InodeKey {
                                device: stat.device,
                                inode: stat.inode,
                            },
                            stat.links.saturating_sub(1),
                        );
                    }
                }
                detach(model, id)?;
                report.removed += 1;
            }
            Err(error) => {
                report.failed += 1;
                let choice = if ignore_all {
                    ErrorChoice::Ignore
                } else {
                    on_event(DeleteEvent::Error {
                        path: &model.path(id),
                        error: &error,
                    })
                };
                ignore_all |= choice == ErrorChoice::IgnoreAll;
                if choice == ErrorChoice::Abort {
                    report.aborted = true;
                    break;
                }
            }
        }
        stack.pop();
    }
    for part in &mut model.parts {
        for link in &mut part.hardlinks {
            if let Some(links) = unlinked.get(&link.key) {
                link.links = *links;
                if *links == 1 {
                    part.entries[link.entry.slot()].set_kind(Kind::Regular);
                }
            }
        }
    }
    model.recount();
    Ok(report)
}
/// Restat after a custom command. Only ENOENT proves absence; permission/I/O errors retain data.
pub fn confirmed_missing(path: &std::path::Path) -> io::Result<bool> {
    match os::path_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
    }
}
/// Rebuild reachable observations to reclaim arena generations and stale mutation records.
/// The replacement root's own metadata replaces the selected directory transactionally.
pub fn replace_subtree(old: &Model, target: EntryId, replacement: &Model) -> io::Result<Model> {
    if target == old.root {
        return compact(replacement);
    }
    rebuild(old, Some((target, replacement)))
}
pub fn compact(model: &Model) -> io::Result<Model> {
    rebuild(model, None)
}
fn rebuild(model: &Model, replace: Option<(EntryId, &Model)>) -> io::Result<Model> {
    let mut part = Part::default();
    let root = part.add(
        0,
        model.name(model.root),
        NONE,
        model.observation(model.root),
        true,
    )?;
    let mut result = Model {
        parts: vec![part],
        root,
    };
    result.directory_mut(root).read_error = model.directory(model.root).unwrap().read_error;
    let mut pending: Vec<(&Model, EntryId, EntryId)> = model
        .children(model.root)
        .map(|id| (model, id, root))
        .collect();
    while let Some((source, source_id, parent)) = pending.pop() {
        let (source, source_id, name) = if replace
            .is_some_and(|(target, _)| std::ptr::eq(source, model) && source_id == target)
        {
            let fresh = replace.unwrap().1;
            (fresh, fresh.root, model.name(source_id))
        } else {
            (source, source_id, source.name(source_id))
        };
        let id = result.parts[0].add(0, name, parent, source.observation(source_id), true)?;
        result.entry_mut(id).next = result.directory(parent).unwrap().first_child;
        result.directory_mut(parent).first_child = id;
        if let Some(directory) = source.directory(source_id) {
            result.directory_mut(id).read_error = directory.read_error;
            pending.extend(source.children(source_id).map(|child| (source, child, id)));
        }
    }
    result.recount();
    Ok(result)
}
