use crate::os;
use std::io;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Character(char),
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Enter,
    Escape,
    Backspace,
}
pub struct Terminal {
    pub handle: os::TerminalHandle,
    pending: Vec<u8>,
    queued_key: Option<Key>,
}
impl Terminal {
    pub fn open() -> io::Result<Self> {
        let handle = os::TerminalHandle::open()?;
        handle.panic_restore_hook()?;
        Ok(Self {
            handle,
            pending: Vec::new(),
            queued_key: None,
        })
    }
    pub fn key(&mut self, timeout_ms: i32) -> io::Result<Option<Key>> {
        if let Some(key) = self.queued_key.take() {
            return Ok(Some(key));
        }
        if self.pending.is_empty() {
            self.pending.extend(self.handle.input(timeout_ms)?);
        }
        if self.pending.is_empty() {
            return Ok(None);
        }
        if self.pending[0] == 27 {
            // A terminal may deliver an escape sequence in several reads.
            for _ in 0..3 {
                if self.pending.len() >= 3
                    && (self.pending[2].is_ascii_alphabetic() || self.pending.last() == Some(&b'~'))
                {
                    break;
                }
                let before = self.pending.len();
                self.pending.extend(self.handle.input(15)?);
                if before == self.pending.len() {
                    break;
                }
            }
            for (sequence, key) in [
                (b"\x1b[A".as_slice(), Key::Up),
                (b"\x1b[B", Key::Down),
                (b"\x1b[C", Key::Right),
                (b"\x1b[D", Key::Left),
                (b"\x1b[H", Key::Home),
                (b"\x1b[F", Key::End),
                (b"\x1b[1~", Key::Home),
                (b"\x1b[4~", Key::End),
                (b"\x1b[5~", Key::PageUp),
                (b"\x1b[6~", Key::PageDown),
            ] {
                if self.pending.starts_with(sequence) {
                    self.pending.drain(..sequence.len());
                    return Ok(Some(key));
                }
            }
        }
        let byte = self.pending.remove(0);
        Ok(Some(match byte {
            27 => Key::Escape,
            10 | 13 => Key::Enter,
            8 | 127 => Key::Backspace,
            byte => Key::Character(char::from(byte)),
        }))
    }
    pub fn queue_key(&mut self, key: Key) {
        self.queued_key = Some(key);
    }
    pub fn frame(
        &mut self,
        lines: &[String],
        selected: Option<usize>,
        color: bool,
    ) -> io::Result<()> {
        let (width, height) = self.handle.dimensions()?;
        let mut bytes = b"\x1b[H".to_vec();
        for (row, line) in lines.iter().take(height as usize).enumerate() {
            if selected == Some(row) {
                bytes.extend_from_slice(b"\x1b[7m");
            } else if color && row == 0 {
                bytes.extend_from_slice(b"\x1b[1;36m");
            }
            bytes.extend_from_slice(super::display::shorten(line, width as usize).as_bytes());
            bytes.extend_from_slice(b"\x1b[0m\x1b[K");
            if row + 1 < height as usize {
                bytes.extend_from_slice(b"\r\n");
            }
        }
        bytes.extend_from_slice(b"\x1b[J");
        self.handle.write(&bytes)
    }
    /// The default answer is no. Session-only 'a' enables later deletion without another prompt.
    pub fn confirm(&mut self, message: &str, allow_all: bool) -> io::Result<Option<bool>> {
        let mut yes = false;
        loop {
            self.frame(
                &[
                    message.to_owned(),
                    format!(
                        "{}   {}{}",
                        if yes { "[Yes]" } else { " Yes " },
                        if yes { " No " } else { "[No]" },
                        if allow_all {
                            "    a: don't ask again"
                        } else {
                            ""
                        }
                    ),
                ],
                Some(if yes { 0 } else { 1 }),
                false,
            )?;
            if os::interrupted() {
                return Ok(None);
            }
            match self.key(100)? {
                Some(Key::Character('y')) => return Ok(Some(false)),
                Some(Key::Character('a')) if allow_all => return Ok(Some(true)),
                Some(Key::Enter) if yes => return Ok(Some(false)),
                Some(Key::Enter | Key::Escape | Key::Character('n' | 'q')) => return Ok(None),
                Some(Key::Left | Key::Right | Key::Character('h' | 'l')) => yes = !yes,
                _ => {}
            }
        }
    }
}
