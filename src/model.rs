//! Dense worker arenas. Entry IDs encode worker/slot; names and optional data live separately.
use crate::os::{self, Extended, Observation};
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

pub const NONE: EntryId = EntryId(u32::MAX);
const SLOT_BITS: u32 = 24;
pub const BLOCK_LIMIT: u64 = (1 << 60) - 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Directory,
    #[default]
    Regular,
    NonRegular,
    Hardlink,
    Error,
    Pattern,
    OtherFs,
    KernelFs,
}
impl Kind {
    pub fn directory_like(self) -> bool {
        matches!(self, Self::Directory | Self::OtherFs | Self::KernelFs)
    }
    pub fn excluded(self) -> bool {
        matches!(self, Self::Pattern | Self::OtherFs | Self::KernelFs)
    }
    pub fn wire(self) -> i64 {
        match self {
            Self::Error => -1,
            Self::Pattern => -2,
            Self::OtherFs => -3,
            Self::KernelFs => -4,
            _ => self as i64,
        }
    }
    pub fn from_wire(value: i64) -> Self {
        match value {
            0 => Self::Directory,
            1 => Self::Regular,
            2 => Self::NonRegular,
            3 => Self::Hardlink,
            -1 => Self::Error,
            -3 => Self::OtherFs,
            -4 => Self::KernelFs,
            n if n < 0 => Self::Pattern,
            _ => Self::NonRegular,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntryId(pub u32);
impl EntryId {
    pub fn part(self) -> usize {
        (self.0 >> SLOT_BITS) as usize
    }
    pub fn slot(self) -> usize {
        (self.0 & ((1 << SLOT_BITS) - 1)) as usize
    }
    fn new(part: u8, slot: usize) -> io::Result<Self> {
        if part == 255 || slot >= (1 << SLOT_BITS) {
            return Err(os::invalid("entry arena ID capacity exceeded"));
        }
        Ok(Self((u32::from(part) << SLOT_BITS) | slot as u32))
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    packed_blocks: u64,
    pub apparent: u64,
    pub name_offset: u32,
    pub next: EntryId,
}
impl Entry {
    pub fn kind(self) -> Kind {
        Kind::from_wire(match self.packed_blocks >> 60 {
            4 => -1,
            5 => -2,
            6 => -3,
            7 => -4,
            value => value as i64,
        })
    }
    pub fn blocks(self) -> u64 {
        self.packed_blocks & BLOCK_LIMIT
    }
    pub fn allocated(self) -> u64 {
        self.blocks().saturating_mul(512)
    }
    pub fn set_kind(&mut self, kind: Kind) {
        self.packed_blocks = self.blocks() | ((kind as u64) << 60);
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub allocated: u64,
    pub apparent: u64,
    pub shared_allocated: u64,
    pub shared_apparent: u64,
    pub items: u64,
}
#[derive(Clone, Debug)]
pub struct Directory {
    pub entry: EntryId,
    pub device: u64,
    pub inode: u64,
    pub first_child: EntryId,
    pub totals: Totals,
    pub latest_mtime: Option<u64>,
    pub read_error: bool,
    pub descendant_error: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InodeKey {
    pub device: u64,
    pub inode: u64,
}
#[derive(Clone, Debug)]
pub struct Hardlink {
    pub entry: EntryId,
    pub key: InodeKey,
    pub links: u32,
}
#[derive(Clone, Copy, Debug)]
pub struct ParentRun {
    pub start: u32,
    pub parent: EntryId,
}
#[derive(Default)]
pub struct Part {
    pub entries: Vec<Entry>,
    pub parents: Vec<ParentRun>,
    pub names: Vec<u8>,
    pub directories: Vec<Directory>,
    pub hardlinks: Vec<Hardlink>,
    pub extended: Vec<(EntryId, Extended)>,
}
impl Part {
    pub fn add(
        &mut self,
        worker: u8,
        name: &[u8],
        parent: EntryId,
        stat: Observation,
        extended: bool,
    ) -> io::Result<EntryId> {
        validate_name(name, parent == NONE)?;
        let id = EntryId::new(worker, self.entries.len())?;
        let name_offset = u32::try_from(self.names.len())
            .map_err(|_| os::invalid("name arena capacity exceeded"))?;
        if self
            .names
            .len()
            .checked_add(name.len() + 1)
            .is_none_or(|length| length > u32::MAX as usize)
        {
            return Err(os::invalid("name arena capacity exceeded"));
        }
        self.names.extend_from_slice(name);
        self.names.push(0);
        if self.parents.last().is_none_or(|run| run.parent != parent) {
            self.parents.push(ParentRun {
                start: id.slot() as u32,
                parent,
            });
        }
        match stat.kind {
            Kind::Directory => {
                self.directories.push(Directory {
                    entry: id,
                    device: stat.device,
                    inode: stat.inode,
                    first_child: NONE,
                    totals: Totals::default(),
                    latest_mtime: None,
                    read_error: false,
                    descendant_error: false,
                });
            }
            Kind::Hardlink => {
                self.hardlinks.push(Hardlink {
                    entry: id,
                    key: InodeKey {
                        device: stat.device,
                        inode: stat.inode,
                    },
                    links: stat.links,
                });
            }
            _ => {}
        }
        self.entries.push(Entry {
            packed_blocks: stat.blocks.min(BLOCK_LIMIT) | ((stat.kind as u64) << 60),
            apparent: stat.apparent,
            name_offset,
            next: NONE,
        });
        if extended && stat.extended.present != 0 {
            self.extended.push((id, stat.extended));
        }
        Ok(id)
    }
}
pub fn validate_name(name: &[u8], root: bool) -> io::Result<()> {
    if name.is_empty()
        || name.contains(&0)
        || (!root && (name.contains(&b'/') || name == b"." || name == b".."))
    {
        return Err(os::invalid("unsafe or missing entry name"));
    }
    if name.len() > 32768 {
        return Err(os::invalid("entry name exceeds import limit"));
    }
    Ok(())
}
pub struct Model {
    pub parts: Vec<Part>,
    pub root: EntryId,
}
impl Model {
    pub fn entry(&self, id: EntryId) -> &Entry {
        &self.parts[id.part()].entries[id.slot()]
    }
    pub fn entry_mut(&mut self, id: EntryId) -> &mut Entry {
        &mut self.parts[id.part()].entries[id.slot()]
    }
    pub fn name(&self, id: EntryId) -> &[u8] {
        let part = &self.parts[id.part()];
        let start = self.entry(id).name_offset as usize;
        let remaining = &part.names[start..];
        &remaining[..remaining.iter().position(|byte| *byte == 0).unwrap()]
    }
    pub fn directory(&self, id: EntryId) -> Option<&Directory> {
        let entry = self.entry(id);
        if entry.kind() != Kind::Directory {
            return None;
        }
        let directories = &self.parts[id.part()].directories;
        let index = directories
            .binary_search_by_key(&id, |dir| dir.entry)
            .expect("directory side record");
        Some(&directories[index])
    }
    pub fn directory_mut(&mut self, id: EntryId) -> &mut Directory {
        let directories = &mut self.parts[id.part()].directories;
        let index = directories
            .binary_search_by_key(&id, |dir| dir.entry)
            .expect("directory side record");
        &mut directories[index]
    }
    /// Consecutive observations usually share a directory; one run replaces a parent word per file.
    pub fn parent(&self, id: EntryId) -> EntryId {
        let runs = &self.parts[id.part()].parents;
        let index = runs.partition_point(|run| run.start <= id.slot() as u32);
        runs[index - 1].parent
    }
    fn hardlink(&self, id: EntryId) -> &Hardlink {
        let links = &self.parts[id.part()].hardlinks;
        let index = links
            .binary_search_by_key(&id, |link| link.entry)
            .expect("hardlink side record");
        &links[index]
    }
    pub fn extended(&self, id: EntryId) -> Option<Extended> {
        let values = &self.parts[id.part()].extended;
        values
            .binary_search_by_key(&id, |(id, _)| *id)
            .ok()
            .map(|index| values[index].1)
    }
    pub fn children(&self, id: EntryId) -> Children<'_> {
        Children {
            model: self,
            next: self.directory(id).map_or(NONE, |dir| dir.first_child),
        }
    }
    pub fn path(&self, id: EntryId) -> PathBuf {
        let mut components = Vec::new();
        let mut current = id;
        while current != NONE {
            components.push(self.name(current));
            current = self.parent(current);
        }
        let mut path = os::byte_path(components.pop().unwrap_or(b"."));
        for component in components.into_iter().rev() {
            path.push(os::byte_path(component));
        }
        path
    }
    pub fn observation(&self, id: EntryId) -> Observation {
        let entry = *self.entry(id);
        let mut stat = Observation {
            kind: entry.kind(),
            blocks: entry.blocks(),
            apparent: entry.apparent,
            extended: self.extended(id).unwrap_or_default(),
            ..Observation::default()
        };
        if let Some(dir) = self.directory(id) {
            stat.device = dir.device;
            stat.inode = dir.inode;
        } else if entry.kind() == Kind::Hardlink {
            let link = self.hardlink(id);
            stat.device = link.key.device;
            stat.inode = link.key.inode;
            stat.links = link.links;
        }
        stat
    }
    /// Postorder ordinary reduction, then one inode contribution per containing ancestor.
    /// A global first-seen set would incorrectly zero hardlinks in sibling directories.
    pub fn recount(&mut self) {
        let _ = self.recount_with_cancel(|| false);
    }
    pub fn recount_with_cancel(&mut self, mut cancelled: impl FnMut() -> bool) -> io::Result<()> {
        let mut order = Vec::new();
        let mut pending = vec![self.root];
        while let Some(id) = pending.pop() {
            if self.directory(id).is_some() {
                order.push(id);
                pending.extend(
                    self.children(id)
                        .filter(|child| self.directory(*child).is_some()),
                );
            }
        }
        let mut groups: HashMap<InodeKey, Vec<(EntryId, u32)>> = HashMap::new();
        let mut processed = 0usize;
        for id in order.into_iter().rev() {
            if cancelled() {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "accounting cancelled",
                ));
            }
            let entry = *self.entry(id);
            let mut totals = Totals {
                allocated: entry.allocated(),
                apparent: entry.apparent,
                ..Totals::default()
            };
            let mut error = false;
            let mut mtime = self
                .extended(id)
                .filter(|ext| ext.present & 1 != 0)
                .map(|ext| ext.mtime);
            for child in self.children(id) {
                processed += 1;
                if processed % 4096 == 0 && cancelled() {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "accounting cancelled",
                    ));
                }
                let entry = *self.entry(child);
                totals.items = totals.items.saturating_add(1);
                if let Some(dir) = self.directory(child) {
                    totals.allocated = totals.allocated.saturating_add(dir.totals.allocated);
                    totals.apparent = totals.apparent.saturating_add(dir.totals.apparent);
                    totals.items = totals.items.saturating_add(dir.totals.items);
                    error |= dir.read_error || dir.descendant_error;
                    mtime = mtime.max(dir.latest_mtime);
                } else {
                    if entry.kind() == Kind::Hardlink {
                        let link = self.hardlink(child);
                        groups
                            .entry(link.key)
                            .or_default()
                            .push((child, link.links));
                    } else {
                        totals.allocated = totals.allocated.saturating_add(entry.allocated());
                        totals.apparent = totals.apparent.saturating_add(entry.apparent);
                    }
                    error |= entry.kind() == Kind::Error;
                    mtime = mtime.max(
                        self.extended(child)
                            .filter(|ext| ext.present & 1 != 0)
                            .map(|ext| ext.mtime),
                    );
                }
            }
            let dir = self.directory_mut(id);
            dir.totals = totals;
            dir.descendant_error = error;
            dir.latest_mtime = mtime;
        }
        for links in groups.values() {
            if cancelled() {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "accounting cancelled",
                ));
            }
            let first = links[0];
            let effective = if first.1 == 0 || links.iter().any(|link| link.1 != first.1) {
                links.len() as u64
            } else {
                u64::from(first.1)
            };
            let entry = *self.entry(first.0);
            let mut counts: HashMap<EntryId, u64> = HashMap::new();
            for (position, (id, _)) in links.iter().enumerate() {
                if position % 1024 == 0 && cancelled() {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "hardlink accounting cancelled",
                    ));
                }
                let mut parent = self.parent(*id);
                while parent != NONE {
                    *counts.entry(parent).or_default() += 1;
                    parent = self.parent(parent);
                }
            }
            for (id, count) in counts {
                let totals = &mut self.directory_mut(id).totals;
                totals.allocated = totals.allocated.saturating_add(entry.allocated());
                totals.apparent = totals.apparent.saturating_add(entry.apparent);
                if count < effective {
                    totals.shared_allocated =
                        totals.shared_allocated.saturating_add(entry.allocated());
                    totals.shared_apparent = totals.shared_apparent.saturating_add(entry.apparent);
                }
            }
        }
        Ok(())
    }
    pub fn len(&self) -> usize {
        self.parts.iter().map(|part| part.entries.len()).sum()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn totals(&self, id: EntryId) -> Totals {
        self.directory(id).map_or_else(
            || {
                let entry = self.entry(id);
                Totals {
                    allocated: entry.allocated(),
                    apparent: entry.apparent,
                    ..Totals::default()
                }
            },
            |dir| dir.totals,
        )
    }
}
pub struct Children<'a> {
    model: &'a Model,
    next: EntryId,
}
impl Iterator for Children<'_> {
    type Item = EntryId;
    fn next(&mut self) -> Option<Self::Item> {
        if self.next == NONE {
            return None;
        }
        let current = self.next;
        self.next = self.model.entry(current).next;
        Some(current)
    }
}
