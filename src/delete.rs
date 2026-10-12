//! Descriptor-relative deletion of the observed tree, with explicit partial success.
use crate::{
    model::{EntryId, Kind, Model, NO_PARENT},
    os,
};
use std::{
    collections::{HashMap, HashSet},
    io,
    os::fd::AsFd,
};
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ErrorAction {
    Abort,
    Ignore,
    IgnoreAll,
}
#[derive(Default)]
pub struct Outcome {
    pub removed: HashSet<EntryId>,
    pub links: HashMap<(u64, u64), u32>,
    pub failures: usize,
    pub aborted: bool,
}
pub fn remove(
    model: &Model,
    selected: EntryId,
    on_error: impl FnMut(EntryId, &io::Error) -> ErrorAction,
) -> Outcome {
    remove_with_progress(model, selected, on_error, |_| true)
}

pub fn remove_with_progress(
    model: &Model,
    selected: EntryId,
    mut on_error: impl FnMut(EntryId, &io::Error) -> ErrorAction,
    mut on_progress: impl FnMut(EntryId) -> bool,
) -> Outcome {
    let mut outcome = Outcome::default();
    if model.entry(selected).parent == NO_PARENT {
        outcome.aborted = true;
        return outcome;
    }
    let mut pending = vec![(selected, false)];
    let mut ignore_all = false;
    while let Some((entry, leaving)) = pending.pop() {
        if os::interrupted() || !on_progress(entry) {
            outcome.aborted = true;
            break;
        }
        let directory = model.directory_id(entry);
        let result = (|| -> io::Result<()> {
            if let Some(id) = directory {
                // Reject replaced directory links before descending or removing the directory.
                let _verified = os::open_model_directory(model, id)?;
                if !leaving {
                    pending.push((entry, true));
                    pending.extend(model.children(id).map(|entry| (entry, false)));
                    return Ok(());
                }
            }
            let parent = os::open_model_directory(model, model.entry(entry).parent)?;
            let name = os::name(model.name(entry))?;
            let live = os::stat(parent.as_fd(), &name, false)?;
            let observed = *model.entry(entry);
            let known = model.metadata(entry);
            if !matches!(observed.kind(), Kind::Excluded | Kind::Error)
                && observed.was_symlink() != live.symlink()
            {
                return Err(os::invalid("entry changed symlink type since scan"));
            }
            if observed.kind() == Kind::Hardlink {
                // With -L, an observed file symlink can be recorded as a hardlink
                // to its target. Unlink the symlink itself, but compare the target
                // identity that was scanned before doing so.
                let observed = if live.symlink() {
                    os::stat(parent.as_fd(), &name, true)?
                } else {
                    live
                };
                if (observed.device, observed.inode) != (known.device, known.inode) {
                    return Err(os::invalid("hardlink replaced since scan"));
                }
            }
            if live.directory() != directory.is_some()
                && (!live.directory()
                    || !matches!(
                        observed.kind(),
                        Kind::Excluded | Kind::OtherFs | Kind::KernelFs
                    ))
            {
                return Err(os::invalid("entry type changed since scan"));
            }
            // Excluded directory entries have no child records. rmdir can remove
            // an empty one, but never recurse into unobserved contents.
            if observed.kind() == Kind::Regular {
                let target = if live.symlink() {
                    os::stat(parent.as_fd(), &name, true)?
                } else {
                    live
                };
                if target.mode & libc::S_IFMT != libc::S_IFREG {
                    return Err(os::invalid("regular file changed type since scan"));
                }
            }
            os::unlink(parent.as_fd(), &name, live.directory())?;
            outcome.removed.insert(entry);
            if live.links > 1 && !live.directory() && !live.symlink() {
                outcome
                    .links
                    .insert((live.device, live.inode), live.links - 1);
            }
            Ok(())
        })();
        if let Err(error) = result {
            outcome.failures += 1;
            if ignore_all {
                continue;
            }
            match on_error(entry, &error) {
                ErrorAction::Abort => {
                    outcome.aborted = true;
                    break;
                }
                ErrorAction::Ignore => {}
                ErrorAction::IgnoreAll => ignore_all = true,
            }
        }
    }
    outcome
}
