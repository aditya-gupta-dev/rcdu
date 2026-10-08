//! Byte-preserving component patterns; a leading slash anchors at filesystem root.
use crate::os::{self, Location};
use std::ffi::{CStr, CString};
use std::io;
#[derive(Clone, Default)]
pub struct Exclusions {
    patterns: Vec<Pattern>,
}
#[derive(Clone)]
struct Pattern {
    components: Vec<CString>,
    anchored: bool,
    directory: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Match {
    None,
    Any,
    Directory,
}
impl Exclusions {
    pub fn add(&mut self, value: &[u8]) -> io::Result<()> {
        let components = value
            .split(|byte| *byte == b'/')
            .filter(|part| !part.is_empty())
            .map(os::name)
            .collect::<io::Result<Vec<_>>>()?;
        if !components.is_empty() {
            self.patterns.push(Pattern {
                components,
                anchored: value.starts_with(b"/"),
                directory: value.ends_with(b"/"),
            });
        }
        Ok(())
    }
    pub fn matches(&self, parent: &Location, name: &CStr) -> Match {
        if self.patterns.is_empty() {
            return Match::None;
        }
        let mut ancestry = vec![name.to_bytes()];
        let mut node = Some(parent);
        while let Some(location) = node {
            ancestry.push(location.name.to_bytes());
            node = location.parent.as_deref();
        }
        let components: Vec<_> = ancestry
            .into_iter()
            .rev()
            .flat_map(|part| {
                part.split(|byte| *byte == b'/')
                    .filter(|part| !part.is_empty())
            })
            .collect();
        let mut matched = Match::None;
        for pattern in &self.patterns {
            if components.len() < pattern.components.len() {
                continue;
            }
            let start = components.len() - pattern.components.len();
            if pattern.anchored && start != 0 {
                continue;
            }
            if pattern
                .components
                .iter()
                .zip(&components[start..])
                .all(|(pattern, component)| os::component_matches(pattern, component))
            {
                if !pattern.directory {
                    return Match::Any;
                }
                matched = Match::Directory;
            }
        }
        matched
    }
}
