//! Rebuild reachable observations after a refresh or a partially successful deletion.
use crate::{
    model::{Builder, EntryId, Kind, Model},
    os,
};
use std::{
    collections::{HashMap, HashSet},
    io,
};

pub fn rebuild(
    model: &Model,
    replacement: Option<(u32, &Model)>,
    removed: &HashSet<EntryId>,
    links: &HashMap<(u64, u64), u32>,
) -> io::Result<Model> {
    let source = replacement
        .filter(|(directory, _)| *directory == 0)
        .map_or(model, |(_, source)| source);
    let root = source.directories[0].entry;
    let mut builder = Builder::root(
        model.name(model.directories[0].entry),
        source.metadata(root),
    )?;
    builder.errors(
        0,
        source.directories[0].read_error,
        source.directories[0].descendant_error,
    );
    let mut stack = vec![(source, source.children(0), 0)];
    while let Some((source, children, parent)) = stack.last_mut() {
        let Some(entry) = children.next() else {
            stack.pop();
            continue;
        };
        if std::ptr::eq(*source, model) && removed.contains(&entry) {
            continue;
        }
        if os::interrupted() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "rebuild interrupted",
            ));
        }
        let name = source.name(entry);
        let mut stat = source.metadata(entry);
        let mut kind = source.entry(entry).kind();
        if let Some(count) = links
            .get(&(stat.device, stat.inode))
            .filter(|_| kind == Kind::Hardlink)
        {
            stat.links = *count;
            if *count == 1 {
                kind = Kind::Regular;
            }
        }
        let mut child_source = *source;
        let mut child_directory = source.directory_id(entry);
        if std::ptr::eq(*source, model) {
            if let Some((directory, other)) =
                replacement.filter(|(directory, _)| Some(*directory) == child_directory)
            {
                let _ = directory;
                child_source = other;
                child_directory = Some(0);
                stat = other.metadata(other.directories[0].entry);
            }
        }
        let (_, new_directory) = builder.add(*parent, name, kind, stat)?;
        if let (Some(old), Some(new)) = (child_directory, new_directory) {
            let directory = &child_source.directories[old as usize];
            builder.errors(new, directory.read_error, directory.descendant_error);
            stack.push((child_source, child_source.children(old), new));
        }
    }
    builder.finish()
}
