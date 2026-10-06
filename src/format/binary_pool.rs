//! Parallel streaming EX1 output. Each worker encodes/compresses its own bounded block.
//! The output lock covers physical writes/index publication; directory locks cover sibling linkage.
use super::binary::{self, BinaryRef, Item, NO_REF, SIGNATURE};
use crate::{
    model::{InodeKey, Kind, Totals},
    os::{self, Observation},
    sink::{self, LinkCount},
};
use std::{
    collections::HashMap,
    io::{self, Write},
    sync::{Arc, Mutex},
};
const MAX_BLOCK: usize = (1 << 24) - 1;
pub struct Output {
    state: Mutex<OutputState>,
    pub block_size: usize,
    pub level: i32,
    pub extended: bool,
}
struct OutputState {
    output: Box<dyn Write + Send>,
    index: Vec<u64>,
    offset: u64,
    root: Option<BinaryRef>,
}
impl Output {
    pub fn new(
        mut output: Box<dyn Write + Send>,
        block_size: usize,
        level: i32,
        extended: bool,
    ) -> io::Result<Arc<Self>> {
        output.write_all(SIGNATURE)?;
        if !(4096..=16000 * 1024).contains(&block_size) {
            return Err(os::invalid("binary block size out of range"));
        }
        Ok(Arc::new(Self {
            state: Mutex::new(OutputState {
                output,
                index: Vec::new(),
                offset: 8,
                root: None,
            }),
            block_size,
            level,
            extended,
        }))
    }
    pub fn finish(&self) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        let root = state
            .root
            .ok_or_else(|| os::invalid("binary root not completed"))?;
        if state.index.contains(&0) {
            return Err(os::invalid("binary block not flushed"));
        }
        let length = state
            .index
            .len()
            .checked_mul(8)
            .and_then(|length| length.checked_add(16))
            .filter(|length| *length <= binary::MAX_INDEX as usize)
            .ok_or_else(|| os::invalid("index length overflow"))?;
        let header = (0x1000_0000 | length as u32).to_be_bytes();
        let index = std::mem::take(&mut state.index);
        state.output.write_all(&header)?;
        for entry in index {
            state.output.write_all(&entry.to_be_bytes())?;
        }
        state.output.write_all(&root.0.to_be_bytes())?;
        state.output.write_all(&header)?;
        state.output.flush()
    }
}
pub struct Worker {
    output: Arc<Output>,
    buffer: Vec<u8>,
    number: Option<u32>,
}
impl Worker {
    pub fn new(output: Arc<Output>) -> Self {
        Self {
            buffer: Vec::with_capacity(output.block_size),
            output,
            number: None,
        }
    }
    pub fn item(&mut self, item: Item<'_>) -> io::Result<BinaryRef> {
        let bound = item
            .name
            .len()
            .checked_add(256)
            .filter(|length| *length < MAX_BLOCK)
            .ok_or_else(|| os::invalid("binary item too large"))?;
        if self.buffer.len() + bound > self.output.block_size {
            self.flush()?;
        }
        let number = if let Some(number) = self.number {
            number
        } else {
            let mut state = self.output.state.lock().unwrap();
            if state.index.len() >= ((binary::MAX_INDEX - 16) / 8) as usize {
                return Err(os::invalid("binary index capacity exceeded"));
            }
            let number = u32::try_from(state.index.len())
                .map_err(|_| os::invalid("block number overflow"))?;
            state.index.push(0);
            self.number = Some(number);
            number
        };
        let current = BinaryRef((u64::from(number) << 24) | self.buffer.len() as u64);
        binary::encode_item(&mut self.buffer, item, current, self.output.extended)?;
        Ok(current)
    }
    pub fn flush(&mut self) -> io::Result<()> {
        let Some(number) = self.number.take() else {
            return Ok(());
        };
        let compressed = zstd::bulk::compress(&self.buffer, self.output.level)?;
        let length = compressed
            .len()
            .checked_add(12)
            .filter(|length| *length <= MAX_BLOCK)
            .ok_or_else(|| os::invalid("binary compressed block too large"))?;
        let header = (length as u32).to_be_bytes();
        let mut state = self.output.state.lock().unwrap();
        if state.offset >= 1 << 40 {
            return Err(os::invalid("binary file offset overflow"));
        }
        let offset = state.offset;
        state.output.write_all(&header)?;
        state.output.write_all(&number.to_be_bytes())?;
        state.output.write_all(&compressed)?;
        state.output.write_all(&header)?;
        state.index[number as usize] = (offset << 24) | length as u64;
        state.offset += length as u64;
        self.buffer.clear();
        Ok(())
    }
    pub fn file(
        &mut self,
        directory: &Directory,
        name: &[u8],
        stat: Observation,
    ) -> io::Result<()> {
        let mut state = directory.state.lock().unwrap();
        state.ordinary.items = state.ordinary.items.saturating_add(1);
        if stat.kind == Kind::Hardlink {
            sink::add_link(&mut state.links, stat);
        } else {
            state.ordinary.allocated = state
                .ordinary
                .allocated
                .saturating_add(stat.blocks.saturating_mul(512));
            state.ordinary.apparent = state.ordinary.apparent.saturating_add(stat.apparent);
        }
        state.descendant_error |= stat.kind == Kind::Error;
        state.previous = self.item(Item {
            name,
            stat,
            previous: state.previous,
            child: NO_REF,
            totals: Totals::default(),
            read_error: false,
            descendant_error: false,
        })?;
        Ok(())
    }
    /// One enumeration token plus one token per child. Finalization happens once at the last token.
    pub fn complete(&mut self, directory: Arc<Directory>, read_error: bool) -> io::Result<()> {
        let mut current = directory;
        let mut error = read_error;
        loop {
            let state = {
                let mut state = current.state.lock().unwrap();
                state.read_error |= error;
                state.pending = state
                    .pending
                    .checked_sub(1)
                    .ok_or_else(|| os::invalid("duplicate directory completion"))?;
                if state.pending != 0 {
                    return Ok(());
                }
                std::mem::take(&mut *state)
            };
            let totals = sink::link_totals(state.ordinary, current.stat, &state.links);
            if let Some(parent) = &current.parent {
                let mut parent_state = parent.state.lock().unwrap();
                let reference = self.item(Item {
                    name: &current.name,
                    stat: current.stat,
                    previous: parent_state.previous,
                    child: state.previous,
                    totals,
                    read_error: state.read_error,
                    descendant_error: state.descendant_error,
                })?;
                parent_state.previous = reference;
                parent_state.ordinary.allocated = parent_state
                    .ordinary
                    .allocated
                    .saturating_add(state.ordinary.allocated);
                parent_state.ordinary.apparent = parent_state
                    .ordinary
                    .apparent
                    .saturating_add(state.ordinary.apparent);
                parent_state.ordinary.items = parent_state
                    .ordinary
                    .items
                    .saturating_add(state.ordinary.items);
                parent_state.descendant_error |= state.read_error || state.descendant_error;
                sink::merge_links(&mut parent_state.links, state.links);
                let next = Arc::clone(parent);
                drop(parent_state);
                current = next;
                error = false;
            } else {
                let reference = self.item(Item {
                    name: &current.name,
                    stat: current.stat,
                    previous: NO_REF,
                    child: state.previous,
                    totals,
                    read_error: state.read_error,
                    descendant_error: state.descendant_error,
                })?;
                let mut output = self.output.state.lock().unwrap();
                if output.root.replace(reference).is_some() {
                    return Err(os::invalid("duplicate binary root"));
                }
                return Ok(());
            }
        }
    }
}
pub struct Directory {
    pub name: Vec<u8>,
    stat: Observation,
    parent: Option<Arc<Directory>>,
    state: Mutex<DirectoryState>,
}
struct DirectoryState {
    pending: usize,
    ordinary: Totals,
    links: HashMap<InodeKey, LinkCount>,
    previous: BinaryRef,
    read_error: bool,
    descendant_error: bool,
}
impl Default for DirectoryState {
    fn default() -> Self {
        Self {
            pending: 0,
            ordinary: Totals::default(),
            links: HashMap::new(),
            previous: NO_REF,
            read_error: false,
            descendant_error: false,
        }
    }
}
impl Directory {
    pub fn root(name: &[u8], stat: Observation) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_vec(),
            stat,
            parent: None,
            state: Mutex::new(DirectoryState {
                pending: 1,
                ..Default::default()
            }),
        })
    }
    pub fn child(parent: &Arc<Self>, name: &[u8], stat: Observation) -> io::Result<Arc<Self>> {
        let mut state = parent.state.lock().unwrap();
        state.pending = state
            .pending
            .checked_add(1)
            .ok_or_else(|| os::invalid("directory pending count overflow"))?;
        state.ordinary.items = state.ordinary.items.saturating_add(1);
        state.ordinary.allocated = state
            .ordinary
            .allocated
            .saturating_add(stat.blocks.saturating_mul(512));
        state.ordinary.apparent = state.ordinary.apparent.saturating_add(stat.apparent);
        Ok(Arc::new(Self {
            name: name.to_vec(),
            stat,
            parent: Some(Arc::clone(parent)),
            state: Mutex::new(DirectoryState {
                pending: 1,
                ..Default::default()
            }),
        }))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let mut parent = self.parent.take();
        while let Some(node) = parent {
            if let Ok(mut node) = Arc::try_unwrap(node) {
                parent = node.parent.take();
            } else {
                break;
            }
        }
    }
}
