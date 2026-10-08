//! Fixed growth chunks avoid copying every earlier record/name at a Vec capacity expansion.
use crate::os;
use std::{io, ops::Index};
const ENTRIES_PER_CHUNK: usize = 4096;
const NAME_CHUNK: usize = 65536;
pub struct Arena<T> {
    chunks: Vec<Vec<T>>,
    length: usize,
}
impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self {
            chunks: Vec::new(),
            length: 0,
        }
    }
}
impl<T> Arena<T> {
    pub fn len(&self) -> usize {
        self.length
    }
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }
    pub fn push(&mut self, value: T) {
        if self
            .chunks
            .last()
            .is_none_or(|chunk| chunk.len() == ENTRIES_PER_CHUNK)
        {
            self.chunks.push(Vec::with_capacity(ENTRIES_PER_CHUNK));
        }
        self.chunks.last_mut().unwrap().push(value);
        self.length += 1;
    }
}
impl<T> Index<usize> for Arena<T> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        &self.chunks[index / ENTRIES_PER_CHUNK][index % ENTRIES_PER_CHUNK]
    }
}
#[derive(Default)]
pub struct Names {
    chunks: Vec<Vec<u8>>,
    used: usize,
}
impl Names {
    pub fn len(&self) -> usize {
        self.used
    }
    pub fn is_empty(&self) -> bool {
        self.used == 0
    }
    pub fn add(&mut self, name: &[u8]) -> io::Result<u32> {
        if name.len() >= NAME_CHUNK {
            return Err(os::invalid("name exceeds storage chunk"));
        }
        if self
            .chunks
            .last()
            .is_none_or(|chunk| chunk.len() + name.len() + 1 > NAME_CHUNK)
        {
            if self.chunks.len() == 1 << 16 {
                return Err(os::invalid("name arena capacity exceeded"));
            }
            self.chunks.push(Vec::with_capacity(NAME_CHUNK));
        }
        let number = self.chunks.len() - 1;
        let chunk = self.chunks.last_mut().unwrap();
        let offset = (number * NAME_CHUNK + chunk.len()) as u32;
        chunk.extend_from_slice(name);
        chunk.push(0);
        self.used += name.len() + 1;
        Ok(offset)
    }
    pub fn get(&self, offset: u32) -> &[u8] {
        let remaining = &self.chunks[offset as usize / NAME_CHUNK][offset as usize % NAME_CHUNK..];
        &remaining[..remaining
            .iter()
            .position(|byte| *byte == 0)
            .expect("owned terminated name")]
    }
}
