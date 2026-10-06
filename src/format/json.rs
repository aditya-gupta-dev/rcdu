//! Legacy ncdu JSON deliberately permits raw non-UTF-8 string bytes.
use crate::model::{EntryId, Kind, Model, NONE, Part};
use crate::os::{self, Observation};
use std::io::{self, BufRead, Write};

pub struct Parser<R> {
    reader: R,
    pub position: u64,
}
impl<R: BufRead> Parser<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            position: 0,
        }
    }
    fn error(&self, message: &str) -> io::Error {
        os::invalid(&format!("JSON byte {}: {message}", self.position))
    }
    fn peek(&mut self) -> io::Result<Option<u8>> {
        Ok(self.reader.fill_buf()?.first().copied())
    }
    fn byte(&mut self) -> io::Result<u8> {
        let byte = self.peek()?.ok_or_else(|| self.error("unexpected EOF"))?;
        self.reader.consume(1);
        self.position += 1;
        Ok(byte)
    }
    fn whitespace(&mut self) -> io::Result<()> {
        while self
            .peek()?
            .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
        {
            self.byte()?;
        }
        Ok(())
    }
    fn significant(&mut self) -> io::Result<Option<u8>> {
        self.whitespace()?;
        self.peek()
    }
    fn expect(&mut self, expected: u8) -> io::Result<()> {
        self.whitespace()?;
        if self.byte()? != expected {
            return Err(self.error("unexpected token"));
        }
        Ok(())
    }
    fn literal(&mut self, bytes: &[u8]) -> io::Result<()> {
        for expected in bytes {
            if self.byte()? != *expected {
                return Err(self.error("invalid literal"));
            }
        }
        Ok(())
    }
    fn hex(&mut self) -> io::Result<u16> {
        let mut value = 0;
        for _ in 0..4 {
            let byte = self.byte()?;
            let digit = (byte as char)
                .to_digit(16)
                .ok_or_else(|| self.error("invalid unicode escape"))?;
            value = (value << 4) | digit as u16;
        }
        Ok(value)
    }
    pub fn string(&mut self) -> io::Result<Vec<u8>> {
        self.expect(b'"')?;
        let mut bytes = Vec::new();
        loop {
            match self.byte()? {
                b'"' => return Ok(bytes),
                b'\\' => match self.byte()? {
                    b'"' => bytes.push(b'"'),
                    b'\\' => bytes.push(b'\\'),
                    b'/' => bytes.push(b'/'),
                    b'b' => bytes.push(8),
                    b'f' => bytes.push(12),
                    b'n' => bytes.push(10),
                    b'r' => bytes.push(13),
                    b't' => bytes.push(9),
                    b'u' => {
                        let first = self.hex()?;
                        let scalar = if (0xd800..=0xdbff).contains(&first) {
                            self.literal(b"\\u")?;
                            let second = self.hex()?;
                            if !(0xdc00..=0xdfff).contains(&second) {
                                return Err(self.error("invalid surrogate pair"));
                            }
                            0x10000 + ((u32::from(first) - 0xd800) << 10) + u32::from(second)
                                - 0xdc00
                        } else {
                            u32::from(first)
                        };
                        let character = char::from_u32(scalar)
                            .ok_or_else(|| self.error("invalid unicode scalar"))?;
                        bytes.extend_from_slice(character.encode_utf8(&mut [0; 4]).as_bytes());
                    }
                    _ => return Err(self.error("invalid escape")),
                },
                byte if byte < 32 => return Err(self.error("unescaped control byte")),
                byte => bytes.push(byte),
            }
            if bytes.len() > 1024 * 1024 {
                return Err(self.error("string resource limit exceeded"));
            }
        }
    }
    pub fn unsigned(&mut self) -> io::Result<u64> {
        self.whitespace()?;
        let first = self.byte()?;
        if !first.is_ascii_digit() {
            return Err(self.error("expected unsigned integer"));
        }
        let mut value = u64::from(first - b'0');
        while let Some(byte) = self.peek()? {
            if !byte.is_ascii_digit() {
                break;
            }
            if first == b'0' {
                return Err(self.error("leading zero"));
            }
            value = value
                .checked_mul(10)
                .and_then(|value| value.checked_add(u64::from(byte - b'0')))
                .ok_or_else(|| self.error("integer overflow"))?;
            self.byte()?;
        }
        Ok(value)
    }
    fn boolean(&mut self) -> io::Result<bool> {
        self.whitespace()?;
        match self.byte()? {
            b't' => {
                self.literal(b"rue")?;
                Ok(true)
            }
            b'f' => {
                self.literal(b"alse")?;
                Ok(false)
            }
            _ => Err(self.error("expected boolean")),
        }
    }
    fn skip(&mut self, depth: usize) -> io::Result<()> {
        if depth > 128 {
            return Err(self.error("extension nesting limit exceeded"));
        }
        match self.significant()?.ok_or_else(|| self.error("EOF"))? {
            b'"' => {
                self.string()?;
            }
            b't' | b'f' => {
                self.boolean()?;
            }
            b'n' => self.literal(b"null")?,
            b'[' => {
                self.byte()?;
                if self.significant()? != Some(b']') {
                    loop {
                        self.skip(depth + 1)?;
                        if self.significant()? == Some(b']') {
                            break;
                        }
                        self.expect(b',')?;
                    }
                }
                self.expect(b']')?;
            }
            b'{' => {
                self.byte()?;
                if self.significant()? != Some(b'}') {
                    loop {
                        self.string()?;
                        self.expect(b':')?;
                        self.skip(depth + 1)?;
                        if self.significant()? == Some(b'}') {
                            break;
                        }
                        self.expect(b',')?;
                    }
                }
                self.expect(b'}')?;
            }
            b'-' | b'0'..=b'9' => {
                if self.peek()? == Some(b'-') {
                    self.byte()?;
                }
                self.unsigned()?;
                if self.peek()? == Some(b'.') {
                    self.byte()?;
                    self.digits()?;
                }
                if matches!(self.peek()?, Some(b'e' | b'E')) {
                    self.byte()?;
                    if matches!(self.peek()?, Some(b'+' | b'-')) {
                        self.byte()?;
                    }
                    self.digits()?;
                }
            }
            _ => return Err(self.error("unknown value")),
        }
        Ok(())
    }
    fn digits(&mut self) -> io::Result<()> {
        if !self.peek()?.is_some_and(|byte| byte.is_ascii_digit()) {
            return Err(self.error("missing digits"));
        }
        while self.peek()?.is_some_and(|byte| byte.is_ascii_digit()) {
            self.byte()?;
        }
        Ok(())
    }
    fn record(&mut self, device: u64, root: bool) -> io::Result<Record> {
        let array = self.significant()? == Some(b'[');
        if array {
            self.byte()?;
        }
        self.expect(b'{')?;
        let mut stat = Observation {
            kind: if array {
                Kind::Directory
            } else {
                Kind::Regular
            },
            device,
            ..Observation::default()
        };
        let mut name = None;
        let mut excluded = None;
        let mut read_error = false;
        let mut link = false;
        let mut nonregular = false;
        if self.significant()? != Some(b'}') {
            loop {
                let key = self.string()?;
                self.expect(b':')?;
                match key.as_slice() {
                    b"name" => {
                        if name.is_some() {
                            return Err(self.error("duplicate name"));
                        }
                        name = Some(self.string()?);
                    }
                    b"asize" => stat.apparent = self.unsigned()?,
                    b"dsize" => stat.blocks = self.unsigned()? >> 9,
                    b"dev" => stat.device = self.unsigned()?,
                    b"ino" => stat.inode = self.unsigned()?,
                    b"nlink" => {
                        stat.links = u32::try_from(self.unsigned()?)
                            .ok()
                            .filter(|value| *value <= 0x7fff_ffff)
                            .ok_or_else(|| self.error("nlink out of range"))?;
                    }
                    b"hlnkc" => link = self.boolean()?,
                    b"notreg" => nonregular = self.boolean()?,
                    b"read_error" => read_error = self.boolean()?,
                    b"excluded" => {
                        excluded = Some(match self.string()?.as_slice() {
                            b"otherfs" | b"othfs" => Kind::OtherFs,
                            b"kernfs" => Kind::KernelFs,
                            _ => Kind::Pattern,
                        })
                    }
                    b"mtime" => {
                        stat.extended.mtime = self.unsigned()?;
                        if self.peek()? == Some(b'.') {
                            self.byte()?;
                            self.digits()?;
                        }
                        stat.extended.present |= 1;
                    }
                    b"uid" => {
                        stat.extended.uid = u32::try_from(self.unsigned()?)
                            .map_err(|_| self.error("uid out of range"))?;
                        stat.extended.present |= 2;
                    }
                    b"gid" => {
                        stat.extended.gid = u32::try_from(self.unsigned()?)
                            .map_err(|_| self.error("gid out of range"))?;
                        stat.extended.present |= 4;
                    }
                    b"mode" => {
                        stat.extended.mode = u16::try_from(self.unsigned()?)
                            .map_err(|_| self.error("mode out of range"))?;
                        stat.extended.present |= 8;
                    }
                    _ => self.skip(0)?,
                }
                if self.significant()? == Some(b'}') {
                    break;
                }
                self.expect(b',')?;
            }
        }
        self.expect(b'}')?;
        let name = name.ok_or_else(|| self.error("missing name"))?;
        crate::model::validate_name(&name, root)?;
        // Resolve type after all fields: malformed ordering must not override exclusions/errors.
        stat.kind = if let Some(kind) = excluded {
            kind
        } else if array {
            Kind::Directory
        } else if read_error {
            Kind::Error
        } else if link || stat.links > 1 {
            Kind::Hardlink
        } else if nonregular {
            Kind::NonRegular
        } else {
            Kind::Regular
        };
        if root && stat.kind != Kind::Directory {
            return Err(self.error("root must be a directory"));
        }
        if stat.kind.excluded() || stat.kind == Kind::Error {
            stat.apparent = 0;
            stat.blocks = 0;
        }
        if array && stat.kind != Kind::Directory {
            self.expect(b']')?;
        }
        Ok(Record {
            name,
            stat,
            read_error,
        })
    }
}
struct Record {
    name: Vec<u8>,
    stat: Observation,
    read_error: bool,
}
pub fn read<R: BufRead>(reader: R) -> io::Result<Model> {
    let mut parser = Parser::new(reader);
    parser.expect(b'[')?;
    if parser.unsigned()? != 1 {
        return Err(os::invalid("unsupported ncdu JSON major version"));
    }
    parser.expect(b',')?;
    parser.unsigned()?;
    parser.expect(b',')?;
    parser.skip(0)?;
    parser.expect(b',')?;
    let root_record = parser.record(0, true)?;
    let mut part = Part::default();
    let root = part.add(0, &root_record.name, NONE, root_record.stat, true)?;
    part.directories[0].read_error = root_record.read_error;
    let mut model = Model {
        parts: vec![part],
        root,
    };
    let mut stack = vec![(root, root_record.stat.device)];
    while let Some((parent, device)) = stack.last().copied() {
        if parser.significant()? == Some(b']') {
            parser.byte()?;
            stack.pop();
            continue;
        }
        parser.expect(b',')?;
        let record = parser.record(device, false)?;
        let id = model.parts[0].add(0, &record.name, parent, record.stat, true)?;
        model.entry_mut(id).next = model.directory(parent).unwrap().first_child;
        model.directory_mut(parent).first_child = id;
        if record.stat.kind == Kind::Directory {
            model.directory_mut(id).read_error = record.read_error;
            if stack.len() >= 4096 {
                return Err(os::invalid("directory nesting limit exceeded"));
            }
            stack.push((id, record.stat.device));
        }
    }
    while parser.significant()? == Some(b',') {
        parser.byte()?;
        parser.skip(0)?;
    }
    parser.expect(b']')?;
    if parser.significant()?.is_some() {
        return Err(os::invalid("trailing JSON bytes"));
    }
    model.recount();
    Ok(model)
}

pub fn string<W: Write>(writer: &mut W, bytes: &[u8]) -> io::Result<()> {
    writer.write_all(b"\"")?;
    for byte in bytes {
        match *byte {
            b'"' => writer.write_all(b"\\\"")?,
            b'\\' => writer.write_all(b"\\\\")?,
            0..=31 | 127 => write!(writer, "\\u{:04x}", byte)?,
            byte => writer.write_all(&[byte])?,
        }
    }
    writer.write_all(b"\"")
}
pub fn object<W: Write>(
    writer: &mut W,
    name: &[u8],
    stat: Observation,
    parent_device: u64,
    error: bool,
    extended: bool,
) -> io::Result<()> {
    writer.write_all(b"{\"name\":")?;
    string(writer, name)?;
    if stat.apparent != 0 {
        write!(writer, ",\"asize\":{}", stat.apparent)?;
    }
    if stat.blocks != 0 {
        write!(writer, ",\"dsize\":{}", stat.blocks.saturating_mul(512))?;
    }
    if stat.kind == Kind::Directory && stat.device != parent_device {
        write!(writer, ",\"dev\":{}", stat.device)?;
    }
    if stat.kind == Kind::Hardlink {
        write!(
            writer,
            ",\"ino\":{},\"hlnkc\":true,\"nlink\":{}",
            stat.inode, stat.links
        )?;
    }
    if stat.kind == Kind::NonRegular {
        writer.write_all(b",\"notreg\":true")?;
    }
    if error || stat.kind == Kind::Error {
        writer.write_all(b",\"read_error\":true")?;
    }
    if stat.kind.excluded() {
        writer.write_all(b",\"excluded\":")?;
        string(
            writer,
            match stat.kind {
                Kind::OtherFs => b"otherfs",
                Kind::KernelFs => b"kernfs",
                _ => b"pattern",
            },
        )?;
    }
    if extended {
        let ext = stat.extended;
        if ext.present & 1 != 0 {
            write!(writer, ",\"mtime\":{}", ext.mtime)?;
        }
        if ext.present & 2 != 0 {
            write!(writer, ",\"uid\":{}", ext.uid)?;
        }
        if ext.present & 4 != 0 {
            write!(writer, ",\"gid\":{}", ext.gid)?;
        }
        if ext.present & 8 != 0 {
            write!(writer, ",\"mode\":{}", ext.mode)?;
        }
    }
    writer.write_all(b"}")
}
pub fn header<W: Write>(writer: &mut W) -> io::Result<()> {
    writer.write_all(b"[1,2,{\"progname\":\"rcdu\",\"progver\":\"0.1.0\"},")
}
fn write_entry<W: Write>(
    writer: &mut W,
    model: &Model,
    id: EntryId,
    extended: bool,
) -> io::Result<()> {
    let stat = model.observation(id);
    if stat.kind.directory_like() {
        writer.write_all(b"[")?;
    }
    let parent_device = if model.entry(id).parent == NONE {
        0
    } else {
        model.directory(model.entry(id).parent).unwrap().device
    };
    object(
        writer,
        model.name(id),
        stat,
        parent_device,
        model.directory(id).is_some_and(|dir| dir.read_error),
        extended,
    )?;
    if stat.kind.directory_like() && stat.kind != Kind::Directory {
        writer.write_all(b"]")?;
    }
    Ok(())
}
pub fn write<W: Write>(model: &Model, mut writer: W, extended: bool) -> io::Result<()> {
    header(&mut writer)?;
    write_entry(&mut writer, model, model.root, extended)?;
    let mut stack = vec![(model.root, model.directory(model.root).unwrap().first_child)];
    while let Some((_, child)) = stack.last_mut() {
        if *child == NONE {
            writer.write_all(b"]")?;
            stack.pop();
            continue;
        }
        let id = *child;
        *child = model.entry(id).next;
        writer.write_all(b",")?;
        write_entry(&mut writer, model, id, extended)?;
        if let Some(dir) = model.directory(id) {
            stack.push((id, dir.first_child));
        }
    }
    writer.write_all(b"]\n")?;
    writer.flush()
}
