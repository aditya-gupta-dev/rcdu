//! Legacy ncdu byte strings and streaming syntax; no whole-document JSON DOM.
use crate::{
    model::{Builder, Kind, Model},
    os::{self, Metadata},
};
use std::io::{self, BufRead, Write};
const TOKEN_LIMIT: usize = 1 << 20;
struct Parser<R> {
    input: R,
    offset: u64,
}
impl<R: BufRead> Parser<R> {
    fn error(&self, text: &str) -> io::Error {
        os::invalid(&format!("JSON byte {}: {text}", self.offset))
    }
    fn peek(&mut self) -> io::Result<Option<u8>> {
        Ok(self.input.fill_buf()?.first().copied())
    }
    fn byte(&mut self) -> io::Result<u8> {
        let byte = self.peek()?.ok_or_else(|| self.error("unexpected end"))?;
        self.input.consume(1);
        self.offset += 1;
        Ok(byte)
    }
    fn space(&mut self) -> io::Result<Option<u8>> {
        while self.peek()?.is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.byte()?;
        }
        self.peek()
    }
    fn expect(&mut self, expected: u8) -> io::Result<()> {
        if self.space()? != Some(expected) {
            return Err(self.error("unexpected token"));
        }
        self.byte()?;
        Ok(())
    }
    fn literal(&mut self, value: &[u8]) -> io::Result<()> {
        self.space()?;
        for expected in value {
            if self.byte()? != *expected {
                return Err(self.error("invalid literal"));
            }
        }
        Ok(())
    }
    fn boolean(&mut self) -> io::Result<bool> {
        if self.space()? == Some(b't') {
            self.literal(b"true")?;
            Ok(true)
        } else {
            self.literal(b"false")?;
            Ok(false)
        }
    }
    fn hex4(&mut self) -> io::Result<u32> {
        let mut value = 0;
        for _ in 0..4 {
            let byte = self.byte()?;
            value = value * 16
                + match byte {
                    b'0'..=b'9' => u32::from(byte - b'0'),
                    b'a'..=b'f' => u32::from(byte - b'a' + 10),
                    b'A'..=b'F' => u32::from(byte - b'A' + 10),
                    _ => return Err(self.error("bad Unicode escape")),
                };
        }
        Ok(value)
    }
    fn string(&mut self) -> io::Result<Vec<u8>> {
        self.expect(b'"')?;
        let mut output = Vec::new();
        loop {
            match self.byte()? {
                b'"' => return Ok(output),
                b'\\' => match self.byte()? {
                    b'"' => output.push(b'"'),
                    b'\\' => output.push(b'\\'),
                    b'/' => output.push(b'/'),
                    b'b' => output.push(8),
                    b'f' => output.push(12),
                    b'n' => output.push(10),
                    b'r' => output.push(13),
                    b't' => output.push(9),
                    b'u' => {
                        let mut point = self.hex4()?;
                        if (0xd800..=0xdbff).contains(&point) {
                            if self.byte()? != b'\\' || self.byte()? != b'u' {
                                return Err(self.error("missing low surrogate"));
                            }
                            let low = self.hex4()?;
                            if !(0xdc00..=0xdfff).contains(&low) {
                                return Err(self.error("bad low surrogate"));
                            }
                            point = 0x10000 + ((point - 0xd800) << 10) + low - 0xdc00;
                        }
                        let point = char::from_u32(point)
                            .ok_or_else(|| self.error("invalid Unicode point"))?;
                        let mut encoded = [0; 4];
                        output.extend_from_slice(point.encode_utf8(&mut encoded).as_bytes());
                    }
                    _ => return Err(self.error("unknown string escape")),
                },
                byte if byte < 32 => return Err(self.error("unescaped string control")),
                byte => output.push(byte),
            }
            if output.len() > TOKEN_LIMIT {
                return Err(self.error("string token too large"));
            }
        }
    }
    fn unsigned(&mut self) -> io::Result<u64> {
        self.space()?;
        let mut result = 0u64;
        let mut digits = 0;
        while let Some(byte @ b'0'..=b'9') = self.peek()? {
            self.byte()?;
            result = result
                .checked_mul(10)
                .and_then(|value| value.checked_add(u64::from(byte - b'0')))
                .ok_or_else(|| self.error("integer overflow"))?;
            digits += 1;
        }
        if digits == 0 {
            return Err(self.error("expected unsigned integer"));
        }
        Ok(result)
    }
    fn digits(&mut self) -> io::Result<()> {
        let start = self.offset;
        while self.peek()?.is_some_and(|byte| byte.is_ascii_digit()) {
            self.byte()?;
            if self.offset - start > TOKEN_LIMIT as u64 {
                return Err(self.error("number token too large"));
            }
        }
        if self.offset == start {
            return Err(self.error("expected digits"));
        }
        Ok(())
    }
    fn skip(&mut self, depth: usize) -> io::Result<()> {
        if depth > 128 {
            return Err(self.error("extension nesting too deep"));
        }
        match self.space()? {
            Some(b'"') => {
                self.string()?;
            }
            Some(b't') => self.literal(b"true")?,
            Some(b'f') => self.literal(b"false")?,
            Some(b'n') => self.literal(b"null")?,
            Some(b'[') => {
                self.byte()?;
                if self.space()? != Some(b']') {
                    loop {
                        self.skip(depth + 1)?;
                        if self.space()? == Some(b']') {
                            break;
                        }
                        self.expect(b',')?;
                    }
                }
                self.expect(b']')?;
            }
            Some(b'{') => {
                self.byte()?;
                if self.space()? != Some(b'}') {
                    loop {
                        self.string()?;
                        self.expect(b':')?;
                        self.skip(depth + 1)?;
                        if self.space()? == Some(b'}') {
                            break;
                        }
                        self.expect(b',')?;
                    }
                }
                self.expect(b'}')?;
            }
            Some(b'-' | b'0'..=b'9') => {
                if self.peek()? == Some(b'-') {
                    self.byte()?;
                }
                self.digits()?;
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
            _ => return Err(self.error("invalid extension value")),
        }
        Ok(())
    }
    fn record(&mut self, device: u64) -> io::Result<Record> {
        let array = self.space()? == Some(b'[');
        if array {
            self.byte()?;
        }
        self.expect(b'{')?;
        let mut stat = Metadata {
            device,
            ..Metadata::default()
        };
        let mut name = None;
        let mut hardlink = false;
        let mut other = false;
        let mut error = false;
        let mut excluded = None;
        if self.space()? != Some(b'}') {
            loop {
                let field = self.string()?;
                self.expect(b':')?;
                match field.as_slice() {
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
                            .filter(|value| *value <= 0x7fffffff)
                            .ok_or_else(|| self.error("link count out of range"))?
                    }
                    b"hlnkc" => hardlink = self.boolean()?,
                    b"notreg" => other = self.boolean()?,
                    b"read_error" => error = self.boolean()?,
                    b"excluded" => {
                        excluded = Some(match self.string()?.as_slice() {
                            b"otherfs" | b"othfs" => Kind::OtherFs,
                            b"kernfs" => Kind::KernelFs,
                            _ => Kind::Excluded,
                        })
                    }
                    b"mtime" => {
                        stat.mtime = self.unsigned()?;
                        if self.peek()? == Some(b'.') {
                            self.byte()?;
                            self.digits()?;
                        }
                        stat.present |= 1;
                    }
                    b"uid" => {
                        stat.uid = u32::try_from(self.unsigned()?)
                            .map_err(|_| self.error("UID out of range"))?;
                        stat.present |= 2;
                    }
                    b"gid" => {
                        stat.gid = u32::try_from(self.unsigned()?)
                            .map_err(|_| self.error("GID out of range"))?;
                        stat.present |= 4;
                    }
                    b"mode" => {
                        stat.mode = u16::try_from(self.unsigned()?)
                            .map_err(|_| self.error("mode out of range"))?
                            .into();
                        stat.present |= 8;
                    }
                    _ => self.skip(0)?,
                }
                if self.space()? == Some(b'}') {
                    break;
                }
                self.expect(b',')?;
            }
        }
        self.expect(b'}')?;
        let kind = if let Some(excluded) = excluded {
            excluded
        } else if array {
            Kind::Directory
        } else if error {
            Kind::Error
        } else if hardlink || stat.links > 1 {
            Kind::Hardlink
        } else if other {
            Kind::Other
        } else {
            Kind::Regular
        };
        if matches!(
            kind,
            Kind::Error | Kind::Excluded | Kind::OtherFs | Kind::KernelFs
        ) {
            stat.apparent = 0;
            stat.blocks = 0;
        }
        if array && kind != Kind::Directory {
            self.expect(b']')?;
        }
        Ok(Record {
            name: name.ok_or_else(|| self.error("missing name"))?,
            stat,
            kind,
            error,
        })
    }
}
struct Record {
    name: Vec<u8>,
    stat: Metadata,
    kind: Kind,
    error: bool,
}
pub fn read(input: impl BufRead) -> io::Result<Model> {
    let mut parser = Parser { input, offset: 0 };
    parser.expect(b'[')?;
    if parser.unsigned()? != 1 {
        return Err(parser.error("unsupported major version"));
    }
    parser.expect(b',')?;
    parser.unsigned()?;
    parser.expect(b',')?;
    parser.skip(0)?;
    parser.expect(b',')?;
    let root = parser.record(0)?;
    if root.kind != Kind::Directory {
        return Err(parser.error("root is not a directory"));
    }
    let mut builder = Builder::root(&root.name, root.stat)?;
    builder.errors(0, root.error, false);
    let mut stack = vec![(0, root.stat.device)];
    while let Some((parent, device)) = stack.last().copied() {
        if parser.space()? == Some(b']') {
            parser.byte()?;
            stack.pop();
            continue;
        }
        parser.expect(b',')?;
        let record = parser.record(device)?;
        let (_, directory) = builder.add(parent, &record.name, record.kind, record.stat)?;
        if let Some(directory) = directory {
            builder.errors(directory, record.error, false);
            if stack.len() >= 4096 {
                return Err(parser.error("directory nesting too deep"));
            }
            stack.push((directory, record.stat.device));
        }
    }
    while parser.space()? == Some(b',') {
        parser.byte()?;
        parser.skip(0)?;
    }
    parser.expect(b']')?;
    if parser.space()?.is_some() {
        return Err(parser.error("trailing bytes"));
    }
    builder.finish()
}
pub fn string(output: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    output.write_all(b"\"")?;
    for byte in bytes {
        match byte {
            b'"' | b'\\' => {
                output.write_all(b"\\")?;
                output.write_all(&[*byte])?;
            }
            0..=31 | 127 => write!(output, "\\u{byte:04x}")?,
            byte => output.write_all(&[*byte])?,
        }
    }
    output.write_all(b"\"")
}
pub fn object(
    output: &mut impl Write,
    name: &[u8],
    kind: Kind,
    stat: Metadata,
    parent_device: u64,
    read_error: bool,
    extended: bool,
) -> io::Result<()> {
    output.write_all(b"{\"name\":")?;
    string(output, name)?;
    if stat.apparent != 0 {
        write!(output, ",\"asize\":{}", stat.apparent)?;
    }
    if stat.blocks != 0 {
        write!(output, ",\"dsize\":{}", stat.allocated())?;
    }
    if matches!(kind, Kind::Directory | Kind::Hardlink) && stat.device != parent_device {
        write!(output, ",\"dev\":{}", stat.device)?;
    }
    if kind == Kind::Hardlink {
        write!(output, ",\"hlnkc\":true,\"ino\":{}", stat.inode)?;
        if stat.links != 0 {
            write!(output, ",\"nlink\":{}", stat.links)?;
        }
    }
    if kind == Kind::Other {
        output.write_all(b",\"notreg\":true")?;
    }
    if read_error || kind == Kind::Error {
        output.write_all(b",\"read_error\":true")?;
    }
    if matches!(kind, Kind::Excluded | Kind::OtherFs | Kind::KernelFs) {
        write!(
            output,
            ",\"excluded\":\"{}\"",
            match kind {
                Kind::OtherFs => "otherfs",
                Kind::KernelFs => "kernfs",
                _ => "pattern",
            }
        )?;
    }
    if extended {
        for (bit, field, value) in [
            (1, "mtime", stat.mtime),
            (2, "uid", u64::from(stat.uid)),
            (4, "gid", u64::from(stat.gid)),
            (8, "mode", u64::from(stat.mode)),
        ] {
            if stat.present & bit != 0 {
                write!(output, ",\"{field}\":{value}")?;
            }
        }
    }
    output.write_all(b"}")
}
pub fn header(output: &mut impl Write) -> io::Result<()> {
    write!(
        output,
        "[1,2,{{\"progname\":\"rcdu\",\"progver\":\"{}\"}},",
        env!("CARGO_PKG_VERSION")
    )
}
pub fn write(model: &Model, output: &mut impl Write, extended: bool) -> io::Result<()> {
    header(output)?;
    output.write_all(b"[")?;
    let root = &model.directories[0];
    object(
        output,
        model.name(root.entry),
        Kind::Directory,
        model.metadata(root.entry),
        0,
        root.read_error,
        extended,
    )?;
    let mut stack = vec![(0, model.children(0))];
    while let Some((parent, children)) = stack.last_mut() {
        if let Some(entry) = children.next() {
            output.write_all(b",")?;
            let kind = model.entry(entry).kind();
            let directory = model.directory_id(entry);
            let array = matches!(kind, Kind::Directory | Kind::OtherFs | Kind::KernelFs);
            if array {
                output.write_all(b"[")?;
            }
            object(
                output,
                model.name(entry),
                kind,
                model.metadata(entry),
                model.directories[*parent as usize].device,
                directory.is_some_and(|id| model.directories[id as usize].read_error),
                extended,
            )?;
            if let Some(directory) = directory {
                stack.push((directory, model.children(directory)));
            } else if array {
                output.write_all(b"]")?;
            }
        } else {
            output.write_all(b"]")?;
            stack.pop();
        }
    }
    output.write_all(b"]\n")
}
