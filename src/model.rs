//! Compact append-only worker storage with coarse directory spans and observation-time subtotals.
use crate::os::{self, Metadata};
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

pub const NO_PARENT: u32 = u32::MAX;
const BLOCK_MASK: u64 = (1 << 60) - 1;
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Directory,
    Regular,
    Other,
    Hardlink,
    Error,
    Excluded,
    OtherFs,
    KernelFs,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EntryId(pub u32);
impl EntryId {
    pub fn worker(self) -> usize {
        (self.0 >> 24) as usize
    }
    pub fn slot(self) -> usize {
        (self.0 & 0xffffff) as usize
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    packed: u64,
    pub apparent: u64,
    pub name: u32,
    pub parent: u32,
}
impl Entry {
    pub fn kind(self) -> Kind {
        match self.packed >> 60 {
            0 => Kind::Directory,
            1 => Kind::Regular,
            2 => Kind::Other,
            3 => Kind::Hardlink,
            4 => Kind::Error,
            5 => Kind::Excluded,
            6 => Kind::OtherFs,
            _ => Kind::KernelFs,
        }
    }
    pub fn blocks(self) -> u64 {
        self.packed & BLOCK_MASK
    }
    pub fn allocated(self) -> u64 {
        self.blocks().saturating_mul(512)
    }
}
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Totals {
    pub allocated: u64,
    pub apparent: u64,
    pub shared_allocated: u64,
    pub shared_apparent: u64,
    pub items: u64,
}
impl Totals {
    pub fn add(&mut self, other: Self) {
        self.allocated = self.allocated.saturating_add(other.allocated);
        self.apparent = self.apparent.saturating_add(other.apparent);
        self.shared_allocated = self.shared_allocated.saturating_add(other.shared_allocated);
        self.shared_apparent = self.shared_apparent.saturating_add(other.shared_apparent);
        self.items = self.items.saturating_add(other.items);
    }
}
#[derive(Clone, Copy)]
pub struct Span {
    pub worker: u8,
    pub start: u32,
    pub length: u32,
}
pub struct Directory {
    pub entry: EntryId,
    pub parent: u32,
    pub device: u64,
    pub inode: u64,
    pub spans: Vec<Span>,
    pub totals: Totals,
    pub read_error: bool,
    pub descendant_error: bool,
}
impl Directory {
    pub fn new(entry: EntryId, parent: u32, stat: Metadata) -> Self {
        Self {
            entry,
            parent,
            device: stat.device,
            inode: stat.inode,
            spans: Vec::new(),
            totals: Totals {
                allocated: stat.allocated(),
                apparent: stat.apparent,
                ..Totals::default()
            },
            read_error: false,
            descendant_error: false,
        }
    }
}
#[derive(Clone, Copy)]
pub struct Link {
    pub entry: EntryId,
    pub device: u64,
    pub inode: u64,
    pub nlink: u32,
}
#[derive(Default)]
pub struct Part {
    pub entries: Vec<Entry>,
    pub names: Vec<u8>,
    pub directories: Vec<(u32, u32)>,
    pub links: Vec<Link>,
    pub extended: Vec<(u32, Metadata)>,
}
impl Part {
    pub fn add(
        &mut self,
        worker: u8,
        name: &[u8],
        parent: u32,
        kind: Kind,
        stat: Metadata,
        extended: bool,
    ) -> io::Result<EntryId> {
        if name.is_empty()
            || name.contains(&0)
            || (parent != NO_PARENT && (name.contains(&b'/') || name == b"." || name == b".."))
        {
            return Err(os::invalid("invalid entry basename"));
        }
        if worker == 255
            || self.entries.len() >= 1 << 24
            || self
                .names
                .len()
                .checked_add(name.len() + 1)
                .is_none_or(|length| length > u32::MAX as usize)
        {
            return Err(os::invalid("worker arena capacity exceeded"));
        }
        let id = EntryId((u32::from(worker) << 24) | self.entries.len() as u32);
        self.entries.push(Entry {
            packed: (stat.blocks.min(BLOCK_MASK)) | ((kind as u64) << 60),
            apparent: stat.apparent,
            name: self.names.len() as u32,
            parent,
        });
        self.names.extend_from_slice(name);
        self.names.push(0);
        if kind == Kind::Hardlink {
            self.links.push(Link {
                entry: id,
                device: stat.device,
                inode: stat.inode,
                nlink: stat.links,
            });
        }
        if extended {
            self.extended.push((id.slot() as u32, stat));
        }
        Ok(id)
    }
}
pub struct Model {
    pub parts: Vec<Part>,
    pub directories: Vec<Directory>,
}
impl Model {
    pub fn entry(&self, id: EntryId) -> &Entry {
        &self.parts[id.worker()].entries[id.slot()]
    }
    pub fn name(&self, id: EntryId) -> &[u8] {
        let part = &self.parts[id.worker()];
        let remaining = &part.names[self.entry(id).name as usize..];
        &remaining[..remaining.iter().position(|byte| *byte == 0).unwrap()]
    }
    pub fn directory_id(&self, id: EntryId) -> Option<u32> {
        let mappings = &self.parts[id.worker()].directories;
        mappings
            .binary_search_by_key(&(id.slot() as u32), |mapping| mapping.0)
            .ok()
            .map(|index| mappings[index].1)
    }
    pub fn extended(&self, id: EntryId) -> Option<Metadata> {
        let values = &self.parts[id.worker()].extended;
        values
            .binary_search_by_key(&(id.slot() as u32), |value| value.0)
            .ok()
            .map(|index| values[index].1)
    }
    pub fn totals(&self, id: EntryId) -> Totals {
        self.directory_id(id).map_or_else(
            || Totals {
                allocated: self.entry(id).allocated(),
                apparent: self.entry(id).apparent,
                ..Totals::default()
            },
            |dir| self.directories[dir as usize].totals,
        )
    }
    pub fn children(&self, dir: u32) -> impl Iterator<Item = EntryId> + '_ {
        self.directories[dir as usize]
            .spans
            .iter()
            .flat_map(|span| {
                (span.start..span.start + span.length)
                    .map(|slot| EntryId((u32::from(span.worker) << 24) | slot))
            })
    }
    pub fn path(&self, id: EntryId) -> PathBuf {
        let mut names = vec![self.name(id)];
        let mut parent = self.entry(id).parent;
        while parent != NO_PARENT {
            let dir = &self.directories[parent as usize];
            names.push(self.name(dir.entry));
            parent = dir.parent;
        }
        let mut result = os::path(names.pop().unwrap());
        for name in names.into_iter().rev() {
            result.push(os::path(name));
        }
        result
    }
    pub fn len(&self) -> usize {
        self.parts.iter().map(|part| part.entries.len()).sum()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Ordinary file sizes were accumulated during scan. This pass visits directories only.
    pub fn finish_accounting(&mut self, cancelled: impl Fn() -> bool) -> io::Result<()> {
        for index in (1..self.directories.len()).rev() {
            if cancelled() {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "accounting cancelled",
                ));
            }
            let (parents, child) = self.directories.split_at_mut(index);
            let child = &child[0];
            let parent = &mut parents[child.parent as usize];
            parent.totals.add(child.totals);
            parent.descendant_error |= child.read_error || child.descendant_error;
        }
        let mut groups: HashMap<(u64, u64), Vec<Link>> = HashMap::new();
        for part in &self.parts {
            for link in &part.links {
                groups
                    .entry((link.device, link.inode))
                    .or_default()
                    .push(*link);
            }
        }
        for links in groups.values() {
            if cancelled() {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "accounting cancelled",
                ));
            }
            let representative = *self.entry(links[0].entry);
            let nlink =
                if links[0].nlink == 0 || links.iter().any(|link| link.nlink != links[0].nlink) {
                    links.len() as u64
                } else {
                    u64::from(links[0].nlink)
                };
            let mut ancestors: HashMap<u32, u64> = HashMap::new();
            for (position, link) in links.iter().enumerate() {
                if position % 1024 == 0 && cancelled() {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "accounting cancelled",
                    ));
                }
                let mut parent = self.entry(link.entry).parent;
                while parent != NO_PARENT {
                    *ancestors.entry(parent).or_default() += 1;
                    parent = self.directories[parent as usize].parent;
                }
            }
            for (dir, count) in ancestors {
                let totals = &mut self.directories[dir as usize].totals;
                totals.add(Totals {
                    allocated: representative.allocated(),
                    apparent: representative.apparent,
                    shared_allocated: if count < nlink {
                        representative.allocated()
                    } else {
                        0
                    },
                    shared_apparent: if count < nlink {
                        representative.apparent
                    } else {
                        0
                    },
                    ..Totals::default()
                });
            }
        }
        Ok(())
    }
}
