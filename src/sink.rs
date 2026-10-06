//! Serial event lifecycle. Each begun directory ends exactly once after all its children.
use crate::{
    format::{
        binary::{self, BinaryRef, NO_REF},
        json,
    },
    model::{InodeKey, Kind, Totals},
    os::{self, Observation},
};
use std::{
    collections::HashMap,
    io::{self, Write},
};
pub trait Sink {
    fn begin(
        &mut self,
        name: &[u8],
        stat: Observation,
        parent_device: u64,
        read_error: bool,
    ) -> io::Result<()>;
    fn file(&mut self, name: &[u8], stat: Observation, parent_device: u64) -> io::Result<()>;
    fn end(&mut self, read_error: bool) -> io::Result<()>;
}

pub struct Json<W> {
    output: W,
    extended: bool,
    depth: usize,
    pending: Option<bool>,
}
impl<W: Write> Json<W> {
    pub fn new(mut output: W, extended: bool) -> io::Result<Self> {
        json::header(&mut output)?;
        Ok(Self {
            output,
            extended,
            depth: 0,
            pending: None,
        })
    }
    fn close_pending(&mut self, error: bool) -> io::Result<()> {
        if let Some(already_error) = self.pending.take() {
            if error && !already_error {
                self.output.write_all(b",\"read_error\":true")?;
            }
            self.output.write_all(b"}")?;
        }
        Ok(())
    }
    pub fn finish(mut self) -> io::Result<W> {
        if self.depth != 0 || self.pending.is_some() {
            return Err(os::invalid("unfinished JSON sink"));
        }
        self.output.write_all(b"]\n")?;
        self.output.flush()?;
        Ok(self.output)
    }
}
impl<W: Write> Sink for Json<W> {
    fn begin(
        &mut self,
        name: &[u8],
        stat: Observation,
        parent_device: u64,
        error: bool,
    ) -> io::Result<()> {
        self.close_pending(false)?;
        if self.depth > 0 {
            self.output.write_all(b",")?;
        }
        self.output.write_all(b"[")?;
        json::object_fields(
            &mut self.output,
            name,
            stat,
            parent_device,
            error,
            self.extended,
        )?;
        self.pending = Some(error);
        self.depth += 1;
        Ok(())
    }
    fn file(&mut self, name: &[u8], stat: Observation, parent_device: u64) -> io::Result<()> {
        self.close_pending(false)?;
        self.output.write_all(b",")?;
        if stat.kind.directory_like() {
            self.output.write_all(b"[")?;
        }
        json::object(
            &mut self.output,
            name,
            stat,
            parent_device,
            false,
            self.extended,
        )?;
        if stat.kind.directory_like() {
            self.output.write_all(b"]")?;
        }
        Ok(())
    }
    fn end(&mut self, error: bool) -> io::Result<()> {
        self.close_pending(error)?;
        self.output.write_all(b"]")?;
        self.depth = self
            .depth
            .checked_sub(1)
            .ok_or_else(|| os::invalid("unbalanced JSON sink"))?;
        Ok(())
    }
}
#[derive(Clone)]
pub(crate) struct LinkCount {
    pub blocks: u64,
    pub apparent: u64,
    pub links: u32,
    pub count: u64,
    pub inconsistent: bool,
}
pub(crate) fn add_link(links: &mut HashMap<InodeKey, LinkCount>, stat: Observation) {
    let key = InodeKey {
        device: stat.device,
        inode: stat.inode,
    };
    if let Some(link) = links.get_mut(&key) {
        link.count = link.count.saturating_add(1);
        link.inconsistent |= stat.links != link.links;
    } else {
        links.insert(
            key,
            LinkCount {
                blocks: stat.blocks,
                apparent: stat.apparent,
                links: stat.links,
                count: 1,
                inconsistent: false,
            },
        );
    }
}
pub(crate) fn merge_links(
    target: &mut HashMap<InodeKey, LinkCount>,
    source: HashMap<InodeKey, LinkCount>,
) {
    for (key, link) in source {
        if let Some(previous) = target.get_mut(&key) {
            previous.count = previous.count.saturating_add(link.count);
            previous.inconsistent |= link.inconsistent || previous.links != link.links;
        } else {
            target.insert(key, link);
        }
    }
}
pub(crate) fn link_totals(
    ordinary: Totals,
    stat: Observation,
    links: &HashMap<InodeKey, LinkCount>,
) -> Totals {
    let mut totals = ordinary;
    totals.allocated = totals
        .allocated
        .saturating_add(stat.blocks.saturating_mul(512));
    totals.apparent = totals.apparent.saturating_add(stat.apparent);
    for link in links.values() {
        let bytes = link.blocks.saturating_mul(512);
        totals.allocated = totals.allocated.saturating_add(bytes);
        totals.apparent = totals.apparent.saturating_add(link.apparent);
        let total_links = if link.inconsistent || link.links == 0 {
            link.count
        } else {
            u64::from(link.links)
        };
        if link.count < total_links {
            totals.shared_allocated = totals.shared_allocated.saturating_add(bytes);
            totals.shared_apparent = totals.shared_apparent.saturating_add(link.apparent);
        }
    }
    totals
}
struct Directory {
    name: Vec<u8>,
    stat: Observation,
    ordinary: Totals,
    links: HashMap<InodeKey, LinkCount>,
    previous: BinaryRef,
    read_error: bool,
    descendant_error: bool,
}
pub struct Binary<W> {
    writer: binary::Writer<W>,
    frames: Vec<Directory>,
    root: Option<BinaryRef>,
    extended: bool,
}
impl<W: Write> Binary<W> {
    pub fn new(output: W, block_size: usize, level: i32, extended: bool) -> io::Result<Self> {
        Ok(Self {
            writer: binary::Writer::new(output, block_size, level)?,
            frames: Vec::new(),
            root: None,
            extended,
        })
    }
    pub fn finish(self) -> io::Result<W> {
        if !self.frames.is_empty() {
            return Err(os::invalid("unfinished binary sink"));
        }
        self.writer.finish(
            self.root
                .ok_or_else(|| os::invalid("missing binary root"))?,
        )
    }
}
impl<W: Write> Sink for Binary<W> {
    fn begin(
        &mut self,
        name: &[u8],
        stat: Observation,
        _: u64,
        read_error: bool,
    ) -> io::Result<()> {
        if let Some(parent) = self.frames.last_mut() {
            parent.ordinary.items = parent.ordinary.items.saturating_add(1);
            parent.ordinary.allocated = parent
                .ordinary
                .allocated
                .saturating_add(stat.blocks.saturating_mul(512));
            parent.ordinary.apparent = parent.ordinary.apparent.saturating_add(stat.apparent);
        }
        self.frames.push(Directory {
            name: name.to_vec(),
            stat,
            ordinary: Totals::default(),
            links: HashMap::new(),
            previous: NO_REF,
            read_error,
            descendant_error: false,
        });
        Ok(())
    }
    fn file(&mut self, name: &[u8], stat: Observation, _: u64) -> io::Result<()> {
        let parent = self
            .frames
            .last_mut()
            .ok_or_else(|| os::invalid("file outside directory"))?;
        parent.ordinary.items = parent.ordinary.items.saturating_add(1);
        if stat.kind == Kind::Hardlink {
            add_link(&mut parent.links, stat);
        } else {
            parent.ordinary.allocated = parent
                .ordinary
                .allocated
                .saturating_add(stat.blocks.saturating_mul(512));
            parent.ordinary.apparent = parent.ordinary.apparent.saturating_add(stat.apparent);
        }
        parent.descendant_error |= stat.kind == Kind::Error;
        parent.previous = self.writer.item(
            binary::Item {
                name,
                stat,
                previous: parent.previous,
                child: NO_REF,
                totals: Totals::default(),
                read_error: false,
                descendant_error: false,
            },
            self.extended,
        )?;
        Ok(())
    }
    fn end(&mut self, read_error: bool) -> io::Result<()> {
        let directory = self
            .frames
            .pop()
            .ok_or_else(|| os::invalid("unbalanced binary sink"))?;
        let totals = link_totals(directory.ordinary, directory.stat, &directory.links);
        let previous = self.frames.last().map_or(NO_REF, |frame| frame.previous);
        let reference = self.writer.item(
            binary::Item {
                name: &directory.name,
                stat: directory.stat,
                previous,
                child: directory.previous,
                totals,
                read_error: read_error || directory.read_error,
                descendant_error: directory.descendant_error,
            },
            self.extended,
        )?;
        if let Some(parent) = self.frames.last_mut() {
            parent.previous = reference;
            parent.ordinary.allocated = parent
                .ordinary
                .allocated
                .saturating_add(directory.ordinary.allocated);
            parent.ordinary.apparent = parent
                .ordinary
                .apparent
                .saturating_add(directory.ordinary.apparent);
            parent.ordinary.items = parent
                .ordinary
                .items
                .saturating_add(directory.ordinary.items);
            parent.descendant_error |=
                read_error || directory.read_error || directory.descendant_error;
            merge_links(&mut parent.links, directory.links);
        } else {
            self.root = Some(reference);
        }
        Ok(())
    }
}
