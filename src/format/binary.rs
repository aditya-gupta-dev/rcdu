//! ncdu EX1 wire format: independently compressed blocks, postorder records and a seekable index.
use crate::model::{EntryId, Kind, Model, NONE, Part, Totals};
use crate::os::{self, Observation};
use std::collections::{HashSet, VecDeque};
use std::io::{self, Read, Seek, SeekFrom, Write};

pub const SIGNATURE: &[u8; 8] = b"\xbfncduEX1";
pub const NO_REF: BinaryRef = BinaryRef(u64::MAX);
const MAX_BLOCK: usize = (1 << 24) - 1;
const MAX_INDEX: u64 = 64 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BinaryRef(pub u64);
impl BinaryRef {
    fn checked(value: u64) -> io::Result<Self> {
        if value >> 56 != 0 {
            return Err(os::invalid("binary reference exceeds 56 bits"));
        }
        Ok(Self(value))
    }
}
#[derive(Clone, Debug)]
pub struct Record {
    pub reference: BinaryRef,
    pub previous: BinaryRef,
    pub child: BinaryRef,
    pub name: Vec<u8>,
    pub stat: Observation,
    pub totals: Totals,
    pub read_error: bool,
    pub descendant_error: bool,
    pub has_device: bool,
}
fn head(buffer: &mut Vec<u8>, major: u8, number: u64) {
    if number < 24 {
        buffer.push((major << 5) | number as u8);
    } else if number <= u8::MAX as u64 {
        buffer.extend_from_slice(&[(major << 5) | 24, number as u8]);
    } else if number <= u16::MAX as u64 {
        buffer.push((major << 5) | 25);
        buffer.extend_from_slice(&(number as u16).to_be_bytes());
    } else if number <= u32::MAX as u64 {
        buffer.push((major << 5) | 26);
        buffer.extend_from_slice(&(number as u32).to_be_bytes());
    } else {
        buffer.push((major << 5) | 27);
        buffer.extend_from_slice(&number.to_be_bytes());
    }
}
fn integer(buffer: &mut Vec<u8>, value: i64) {
    if value >= 0 {
        head(buffer, 0, value as u64);
    } else {
        head(buffer, 1, (-1i128 - i128::from(value)) as u64);
    }
}
fn field(buffer: &mut Vec<u8>, key: u8, value: u64) {
    head(buffer, 0, u64::from(key));
    head(buffer, 0, value);
}
fn reference_field(
    buffer: &mut Vec<u8>,
    key: u8,
    value: BinaryRef,
    current: BinaryRef,
) -> io::Result<()> {
    if value == NO_REF {
        return Ok(());
    }
    BinaryRef::checked(value.0)?;
    head(buffer, 0, u64::from(key));
    if value.0 >> 24 == current.0 >> 24 {
        let delta = current
            .0
            .checked_sub(value.0)
            .filter(|value| *value > 0)
            .ok_or_else(|| os::invalid("forward same-block reference"))?;
        head(buffer, 1, delta - 1);
    } else {
        head(buffer, 0, value.0);
    }
    Ok(())
}
pub struct Item<'a> {
    pub name: &'a [u8],
    pub stat: Observation,
    pub previous: BinaryRef,
    pub child: BinaryRef,
    pub totals: Totals,
    pub read_error: bool,
    pub descendant_error: bool,
}
pub struct Writer<W> {
    output: W,
    block: Vec<u8>,
    index: Vec<u64>,
    offset: u64,
    block_size: usize,
    level: i32,
}
impl<W: Write> Writer<W> {
    pub fn new(mut output: W, block_size: usize, level: i32) -> io::Result<Self> {
        output.write_all(SIGNATURE)?;
        if !(4096..=16000 * 1024).contains(&block_size) {
            return Err(os::invalid("binary block size out of range"));
        }
        Ok(Self {
            output,
            block: Vec::with_capacity(block_size),
            index: Vec::new(),
            offset: 8,
            block_size,
            level,
        })
    }
    fn flush_block(&mut self) -> io::Result<()> {
        if self.block.is_empty() {
            return Ok(());
        }
        let compressed = zstd::bulk::compress(&self.block, self.level)?;
        let length = compressed
            .len()
            .checked_add(12)
            .filter(|length| *length <= MAX_BLOCK)
            .ok_or_else(|| os::invalid("compressed block length exceeds 24 bits"))?;
        if self.offset >= 1 << 40 || self.index.len() >= u32::MAX as usize {
            return Err(os::invalid("binary index capacity exceeded"));
        }
        let header = (length as u32).to_be_bytes();
        self.output.write_all(&header)?;
        self.output
            .write_all(&(self.index.len() as u32).to_be_bytes())?;
        self.output.write_all(&compressed)?;
        self.output.write_all(&header)?;
        self.index.push((self.offset << 24) | length as u64);
        self.offset = self
            .offset
            .checked_add(length as u64)
            .ok_or_else(|| os::invalid("file offset overflow"))?;
        self.block.clear();
        Ok(())
    }
    pub fn item(&mut self, item: Item<'_>, extended: bool) -> io::Result<BinaryRef> {
        let Item {
            name,
            stat,
            previous,
            child,
            totals,
            read_error,
            descendant_error,
        } = item;
        let bound = name
            .len()
            .checked_add(256)
            .filter(|length| *length < MAX_BLOCK)
            .ok_or_else(|| os::invalid("binary item too large"))?;
        if self.block.len() + bound > self.block_size {
            self.flush_block()?;
        }
        let current =
            BinaryRef::checked(((self.index.len() as u64) << 24) | self.block.len() as u64)?;
        self.block.push(0xbf);
        head(&mut self.block, 0, 0);
        integer(&mut self.block, stat.kind.wire());
        head(&mut self.block, 0, 1);
        head(&mut self.block, 2, name.len() as u64);
        self.block.extend_from_slice(name);
        reference_field(&mut self.block, 2, previous, current)?;
        field(&mut self.block, 3, stat.apparent);
        field(&mut self.block, 4, stat.blocks.saturating_mul(512));
        if stat.kind == Kind::Directory || stat.kind == Kind::Hardlink {
            field(&mut self.block, 5, stat.device);
        }
        if read_error || descendant_error {
            head(&mut self.block, 0, 6);
            self.block.push(if read_error { 0xf5 } else { 0xf4 });
        }
        if stat.kind == Kind::Directory {
            field(&mut self.block, 7, totals.apparent);
            field(&mut self.block, 8, totals.allocated);
            if totals.shared_apparent != 0 {
                field(&mut self.block, 9, totals.shared_apparent);
            }
            if totals.shared_allocated != 0 {
                field(&mut self.block, 10, totals.shared_allocated);
            }
            field(&mut self.block, 11, totals.items);
            reference_field(&mut self.block, 12, child, current)?;
        }
        if stat.kind == Kind::Hardlink {
            field(&mut self.block, 13, stat.inode);
            field(&mut self.block, 14, u64::from(stat.links));
        }
        if extended {
            let ext = stat.extended;
            if ext.present & 2 != 0 {
                field(&mut self.block, 15, u64::from(ext.uid));
            }
            if ext.present & 4 != 0 {
                field(&mut self.block, 16, u64::from(ext.gid));
            }
            if ext.present & 8 != 0 {
                field(&mut self.block, 17, u64::from(ext.mode));
            }
            if ext.present & 1 != 0 {
                field(&mut self.block, 18, ext.mtime);
            }
        }
        self.block.push(0xff);
        Ok(current)
    }
    pub fn finish(mut self, root: BinaryRef) -> io::Result<W> {
        self.flush_block()?;
        let length = self
            .index
            .len()
            .checked_mul(8)
            .and_then(|length| length.checked_add(16))
            .filter(|length| *length < 1 << 28)
            .ok_or_else(|| os::invalid("index length overflow"))?;
        let header = (0x1000_0000 | length as u32).to_be_bytes();
        self.output.write_all(&header)?;
        for entry in self.index {
            self.output.write_all(&entry.to_be_bytes())?;
        }
        self.output.write_all(&root.0.to_be_bytes())?;
        self.output.write_all(&header)?;
        self.output.flush()?;
        Ok(self.output)
    }
}
pub fn write<W: Write>(
    model: &Model,
    output: W,
    block_size: usize,
    level: i32,
    extended: bool,
) -> io::Result<()> {
    let mut writer = Writer::new(output, block_size, level)?;
    struct Frame {
        id: EntryId,
        next: EntryId,
        previous: BinaryRef,
    }
    let mut stack = vec![Frame {
        id: model.root,
        next: model.directory(model.root).unwrap().first_child,
        previous: NO_REF,
    }];
    let root;
    loop {
        let frame = stack.last_mut().unwrap();
        if frame.next != NONE {
            let id = frame.next;
            frame.next = model.entry(id).next;
            if let Some(dir) = model.directory(id) {
                stack.push(Frame {
                    id,
                    next: dir.first_child,
                    previous: NO_REF,
                });
            } else {
                frame.previous = writer.item(
                    Item {
                        name: model.name(id),
                        stat: model.observation(id),
                        previous: frame.previous,
                        child: NO_REF,
                        totals: Totals::default(),
                        read_error: false,
                        descendant_error: false,
                    },
                    extended,
                )?;
            }
        } else {
            let frame = stack.pop().unwrap();
            let dir = model.directory(frame.id).unwrap();
            let previous = stack.last().map_or(NO_REF, |frame| frame.previous);
            let item = writer.item(
                Item {
                    name: model.name(frame.id),
                    stat: model.observation(frame.id),
                    previous,
                    child: frame.previous,
                    totals: dir.totals,
                    read_error: dir.read_error,
                    descendant_error: dir.descendant_error,
                },
                extended,
            )?;
            if let Some(parent) = stack.last_mut() {
                parent.previous = item;
            } else {
                root = item;
                break;
            }
        }
    }
    writer.finish(root)?;
    Ok(())
}

struct Cbor<'a> {
    bytes: &'a [u8],
    position: usize,
}
#[derive(Clone, Copy)]
struct Head {
    major: u8,
    value: Option<u64>,
}
impl<'a> Cbor<'a> {
    fn take(&mut self, length: usize) -> io::Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| os::invalid("truncated CBOR"))?;
        let value = &self.bytes[self.position..end];
        self.position = end;
        Ok(value)
    }
    fn raw_head(&mut self) -> io::Result<Head> {
        let first = self.take(1)?[0];
        let number = first & 31;
        let value = match number {
            0..=23 => Some(u64::from(number)),
            24 => Some(u64::from(self.take(1)?[0])),
            25 => Some(u64::from(u16::from_be_bytes(
                self.take(2)?.try_into().unwrap(),
            ))),
            26 => Some(u64::from(u32::from_be_bytes(
                self.take(4)?.try_into().unwrap(),
            ))),
            27 => Some(u64::from_be_bytes(self.take(8)?.try_into().unwrap())),
            31 if matches!(first >> 5, 2..=5 | 7) => None,
            _ => return Err(os::invalid("invalid CBOR head")),
        };
        Ok(Head {
            major: first >> 5,
            value,
        })
    }
    fn head(&mut self) -> io::Result<Head> {
        for _ in 0..128 {
            let head = self.raw_head()?;
            if head.major != 6 {
                return Ok(head);
            }
        }
        Err(os::invalid("CBOR tag limit exceeded"))
    }
    fn unsigned(&mut self) -> io::Result<u64> {
        let head = self.head()?;
        if head.major != 0 {
            return Err(os::invalid("expected CBOR unsigned integer"));
        }
        head.value.ok_or_else(|| os::invalid("indefinite integer"))
    }
    fn signed(&mut self) -> io::Result<i64> {
        let head = self.head()?;
        let value = head
            .value
            .ok_or_else(|| os::invalid("indefinite integer"))?;
        match head.major {
            0 => i64::try_from(value).map_err(|_| os::invalid("signed CBOR overflow")),
            1 => i64::try_from(-1i128 - i128::from(value))
                .map_err(|_| os::invalid("negative CBOR overflow")),
            _ => Err(os::invalid("expected CBOR integer")),
        }
    }
    fn reference(&mut self, current: BinaryRef) -> io::Result<BinaryRef> {
        let head = self.head()?;
        let value = head
            .value
            .ok_or_else(|| os::invalid("indefinite reference"))?;
        match head.major {
            0 => BinaryRef::checked(value),
            1 => {
                let delta = value
                    .checked_add(1)
                    .ok_or_else(|| os::invalid("reference delta overflow"))?;
                if delta > current.0 & MAX_BLOCK as u64 {
                    return Err(os::invalid("reference delta crosses block"));
                }
                BinaryRef::checked(current.0 - delta)
            }
            _ => Err(os::invalid("invalid reference")),
        }
    }
    fn bytes(&mut self) -> io::Result<&'a [u8]> {
        let head = self.head()?;
        if !matches!(head.major, 2 | 3) {
            return Err(os::invalid("expected CBOR byte string"));
        }
        let length = usize::try_from(head.value.ok_or_else(|| os::invalid("indefinite name"))?)
            .map_err(|_| os::invalid("byte length overflow"))?;
        self.take(length)
    }
    fn boolean(&mut self) -> io::Result<bool> {
        let head = self.head()?;
        match (head.major, head.value) {
            (7, Some(20)) => Ok(false),
            (7, Some(21)) => Ok(true),
            _ => Err(os::invalid("invalid CBOR boolean")),
        }
    }
    fn skip(&mut self, depth: usize) -> io::Result<()> {
        let head = self.head()?;
        self.skip_head(head, depth)
    }
    fn skip_head(&mut self, head: Head, depth: usize) -> io::Result<()> {
        if depth > 128 {
            return Err(os::invalid("CBOR nesting limit exceeded"));
        }
        match head.major {
            0 | 1 | 7 if head.value.is_some() => {}
            2 | 3 => {
                if let Some(length) = head.value {
                    self.take(
                        usize::try_from(length).map_err(|_| os::invalid("length overflow"))?,
                    )?;
                } else {
                    loop {
                        let chunk = self.raw_head()?;
                        if chunk.major == 7 && chunk.value.is_none() {
                            break;
                        }
                        if chunk.major != head.major || chunk.value.is_none() {
                            return Err(os::invalid("invalid CBOR string chunk"));
                        }
                        self.skip_head(chunk, depth + 1)?;
                    }
                }
            }
            4 | 5 => {
                if let Some(length) = head.value {
                    let count = length
                        .checked_mul(if head.major == 5 { 2 } else { 1 })
                        .filter(|count| *count <= self.bytes.len() as u64)
                        .ok_or_else(|| os::invalid("CBOR container limit"))?;
                    for _ in 0..count {
                        self.skip(depth + 1)?;
                    }
                } else {
                    loop {
                        let child = self.head()?;
                        if child.major == 7 && child.value.is_none() {
                            break;
                        }
                        self.skip_head(child, depth + 1)?;
                        if head.major == 5 {
                            self.skip(depth + 1)?;
                        }
                    }
                }
            }
            _ => return Err(os::invalid("invalid CBOR value")),
        }
        Ok(())
    }
}
pub struct Reader<R> {
    input: R,
    index: Vec<u64>,
    pub root: BinaryRef,
    cache: VecDeque<(u32, Vec<u8>)>,
}
impl<R: Read + Seek> Reader<R> {
    pub fn open(mut input: R) -> io::Result<Self> {
        input.seek(SeekFrom::Start(0))?;
        let mut signature = [0; 8];
        input.read_exact(&mut signature)?;
        if &signature != SIGNATURE {
            return Err(os::invalid("not ncdu binary"));
        }
        let size = input.seek(SeekFrom::End(0))?;
        if size < 24 {
            return Err(os::invalid("truncated binary index"));
        }
        input.seek(SeekFrom::End(-4))?;
        let mut header = [0; 4];
        input.read_exact(&mut header)?;
        let word = u32::from_be_bytes(header);
        let length = u64::from(word & 0x0fff_ffff);
        if word >> 28 != 1
            || length < 24
            || length % 8 != 0
            || length > MAX_INDEX
            || length > size - 8
        {
            return Err(os::invalid("invalid binary index length"));
        }
        let start = size - length;
        input.seek(SeekFrom::Start(start))?;
        let mut first = [0; 4];
        input.read_exact(&mut first)?;
        if first != header {
            return Err(os::invalid("binary index envelope mismatch"));
        }
        let entries = (length - 16) / 8;
        let mut index = Vec::with_capacity(entries as usize);
        for _ in 0..entries {
            let mut bytes = [0; 8];
            input.read_exact(&mut bytes)?;
            let entry = u64::from_be_bytes(bytes);
            let offset = entry >> 24;
            let compressed_length = entry & MAX_BLOCK as u64;
            if offset < 8
                || compressed_length <= 12
                || offset
                    .checked_add(compressed_length)
                    .is_none_or(|end| end > start)
            {
                return Err(os::invalid("invalid data block index range"));
            }
            index.push(entry);
        }
        // Overlapping blocks are invalid even when each individual range is within the file.
        let mut ranges: Vec<_> = index
            .iter()
            .map(|entry| (entry >> 24, (entry >> 24) + (entry & MAX_BLOCK as u64)))
            .collect();
        ranges.sort_unstable();
        if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return Err(os::invalid("overlapping data blocks"));
        }
        let mut root = [0; 8];
        input.read_exact(&mut root)?;
        let root = BinaryRef::checked(u64::from_be_bytes(root))?;
        let mut reader = Self {
            input,
            index,
            root,
            cache: VecDeque::new(),
        };
        let record = reader.get(root)?;
        if record.stat.kind != Kind::Directory || record.previous != NO_REF {
            return Err(os::invalid(
                "binary root must be a directory without siblings",
            ));
        }
        crate::model::validate_name(&record.name, true)?;
        Ok(reader)
    }
    fn block(&mut self, number: u32) -> io::Result<&[u8]> {
        if let Some(position) = self.cache.iter().position(|(cached, _)| *cached == number) {
            let entry = self.cache.remove(position).unwrap();
            self.cache.push_back(entry);
        } else {
            let entry = *self
                .index
                .get(number as usize)
                .ok_or_else(|| os::invalid("binary block number out of range"))?;
            let offset = entry >> 24;
            let length = (entry & MAX_BLOCK as u64) as usize;
            self.input.seek(SeekFrom::Start(offset))?;
            let mut bytes = vec![0; length];
            self.input.read_exact(&mut bytes)?;
            let header = u32::from_be_bytes(bytes[..4].try_into().unwrap());
            if header != length as u32
                || bytes[..4] != bytes[length - 4..]
                || u32::from_be_bytes(bytes[4..8].try_into().unwrap()) != number
            {
                return Err(os::invalid("data block envelope mismatch"));
            }
            let compressed = &bytes[8..length - 4];
            let expected = zstd::zstd_safe::get_frame_content_size(compressed)
                .map_err(|_| os::invalid("invalid zstd frame"))?
                .ok_or_else(|| os::invalid("binary frame must declare content size"))?;
            if expected == 0 || expected > MAX_BLOCK as u64 {
                return Err(os::invalid("binary decompression limit exceeded"));
            }
            let data = zstd::bulk::decompress(compressed, expected as usize)?;
            if data.len() != expected as usize {
                return Err(os::invalid("binary frame size mismatch"));
            }
            while self.cache.len() >= 8
                || self.cache.iter().map(|(_, data)| data.len()).sum::<usize>() + data.len()
                    > 64 * 1024 * 1024
            {
                self.cache.pop_front();
            }
            self.cache.push_back((number, data));
        }
        Ok(&self.cache.back().unwrap().1)
    }
    /// Copies names/fields before another read can evict the borrowed block.
    pub fn get(&mut self, reference: BinaryRef) -> io::Result<Record> {
        BinaryRef::checked(reference.0)?;
        let bytes = self.block((reference.0 >> 24) as u32)?;
        let offset = (reference.0 & MAX_BLOCK as u64) as usize;
        let mut parser = Cbor {
            bytes: bytes
                .get(offset..)
                .ok_or_else(|| os::invalid("item offset out of range"))?,
            position: 0,
        };
        let map = parser.head()?;
        if map.major != 5 {
            return Err(os::invalid("binary item must be a map"));
        }
        if map
            .value
            .is_some_and(|count| count > parser.bytes.len() as u64 / 2)
        {
            return Err(os::invalid("item map length out of range"));
        }
        let mut record = Record {
            reference,
            previous: NO_REF,
            child: NO_REF,
            name: Vec::new(),
            stat: Observation::default(),
            totals: Totals::default(),
            read_error: false,
            descendant_error: false,
            has_device: false,
        };
        let mut has_type = false;
        let mut has_name = false;
        let mut fields = 0;
        loop {
            if map.value == Some(fields) {
                break;
            }
            let key = parser.head()?;
            if key.major == 7 && key.value.is_none() {
                if map.value.is_some() {
                    return Err(os::invalid("unexpected map break"));
                }
                break;
            }
            fields += 1;
            if key.major != 0 || key.value.is_none() {
                parser.skip_head(key, 0)?;
                parser.skip(0)?;
                continue;
            }
            match key.value.unwrap() {
                0 => {
                    if has_type {
                        return Err(os::invalid("duplicate item type"));
                    }
                    has_type = true;
                    record.stat.kind = Kind::from_wire(parser.signed()?);
                }
                1 => {
                    if has_name {
                        return Err(os::invalid("duplicate item name"));
                    }
                    has_name = true;
                    record.name = parser.bytes()?.to_vec();
                }
                2 => record.previous = parser.reference(reference)?,
                3 => record.stat.apparent = parser.unsigned()?,
                4 => record.stat.blocks = parser.unsigned()? >> 9,
                5 => {
                    record.stat.device = parser.unsigned()?;
                    record.has_device = true;
                }
                6 => {
                    record.read_error = parser.boolean()?;
                    record.descendant_error = !record.read_error;
                }
                7 => record.totals.apparent = parser.unsigned()?,
                8 => record.totals.allocated = parser.unsigned()?,
                9 => record.totals.shared_apparent = parser.unsigned()?,
                10 => record.totals.shared_allocated = parser.unsigned()?,
                11 => record.totals.items = parser.unsigned()?,
                12 => record.child = parser.reference(reference)?,
                13 => record.stat.inode = parser.unsigned()?,
                14 => {
                    record.stat.links = u32::try_from(parser.unsigned()?)
                        .ok()
                        .filter(|number| *number <= 0x7fff_ffff)
                        .ok_or_else(|| os::invalid("nlink out of range"))?;
                }
                15 => {
                    record.stat.extended.uid = u32::try_from(parser.unsigned()?)
                        .map_err(|_| os::invalid("uid out of range"))?;
                    record.stat.extended.present |= 2;
                }
                16 => {
                    record.stat.extended.gid = u32::try_from(parser.unsigned()?)
                        .map_err(|_| os::invalid("gid out of range"))?;
                    record.stat.extended.present |= 4;
                }
                17 => {
                    record.stat.extended.mode = u16::try_from(parser.unsigned()?)
                        .map_err(|_| os::invalid("mode out of range"))?;
                    record.stat.extended.present |= 8;
                }
                18 => {
                    record.stat.extended.mtime = parser.unsigned()?;
                    record.stat.extended.present |= 1;
                }
                _ => parser.skip(0)?,
            }
            if fields > 4096 {
                return Err(os::invalid("item field count limit exceeded"));
            }
        }
        if !has_type || !has_name {
            return Err(os::invalid("item type/name missing"));
        }
        crate::model::validate_name(&record.name, reference == self.root)?;
        if record.stat.kind != Kind::Directory && record.child != NO_REF {
            return Err(os::invalid("non-directory with children"));
        }
        if record.stat.kind != Kind::Directory {
            record.totals.allocated = record.stat.blocks.saturating_mul(512);
            record.totals.apparent = record.stat.apparent;
        }
        Ok(record)
    }
    pub fn children(&mut self, parent: &Record) -> io::Result<Vec<Record>> {
        let mut records = Vec::new();
        let mut next = parent.child;
        let mut seen = HashSet::new();
        while next != NO_REF {
            if next == parent.reference || !seen.insert(next) {
                return Err(os::invalid("binary sibling/child cycle"));
            }
            if seen.len() > 4_000_000 {
                return Err(os::invalid("listing resource limit exceeded"));
            }
            let mut record = self.get(next)?;
            if !record.has_device {
                record.stat.device = parent.stat.device;
            }
            next = record.previous;
            records.push(record);
        }
        Ok(records)
    }
    pub fn import(&mut self) -> io::Result<Model> {
        let record = self.get(self.root)?;
        let mut part = Part::default();
        let root = part.add(0, &record.name, NONE, record.stat, true)?;
        let mut model = Model {
            parts: vec![part],
            root,
        };
        model.directory_mut(root).read_error = record.read_error;
        struct Frame {
            id: EntryId,
            next: BinaryRef,
            device: u64,
        }
        let mut frames = vec![Frame {
            id: root,
            next: record.child,
            device: record.stat.device,
        }];
        let mut seen = HashSet::from([self.root]);
        while let Some(frame) = frames.last_mut() {
            if frame.next == NO_REF {
                frames.pop();
                continue;
            }
            let reference = frame.next;
            if !seen.insert(reference) {
                return Err(os::invalid("binary graph cycle or repeated reference"));
            }
            let mut record = self.get(reference)?;
            frame.next = record.previous;
            if !record.has_device {
                record.stat.device = frame.device;
            }
            let id = model.parts[0].add(0, &record.name, frame.id, record.stat, true)?;
            model.entry_mut(id).next = model.directory(frame.id).unwrap().first_child;
            model.directory_mut(frame.id).first_child = id;
            if record.stat.kind == Kind::Directory {
                model.directory_mut(id).read_error = record.read_error;
                if frames.len() >= 4096 {
                    return Err(os::invalid("binary nesting limit exceeded"));
                }
                frames.push(Frame {
                    id,
                    next: record.child,
                    device: record.stat.device,
                });
            }
        }
        model.recount();
        Ok(model)
    }
    pub fn cached_blocks(&self) -> usize {
        self.cache.len()
    }
}
