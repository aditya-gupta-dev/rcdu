//! Component-oriented ncdu patterns, anchored to the filesystem root when starting with '/'.
use crate::os::{self, Location};
use std::ffi::{CStr, CString};
use std::io;
#[derive(Default, Clone)]
pub struct Exclusions {
    patterns: Vec<Pattern>,
}
#[derive(Clone)]
struct Pattern {
    components: Vec<CString>,
    anchored: bool,
    directories_only: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Match {
    None,
    Any,
    Directory,
}
impl Exclusions {
    pub fn add(&mut self, bytes: &[u8]) -> io::Result<()> {
        let components = bytes
            .split(|byte| *byte == b'/')
            .filter(|part| !part.is_empty())
            .map(os::c_name)
            .collect::<io::Result<Vec<_>>>()?;
        if !components.is_empty() {
            self.patterns.push(Pattern {
                components,
                anchored: bytes.starts_with(b"/"),
                directories_only: bytes.ends_with(b"/"),
            });
        }
        Ok(())
    }
    pub fn matches(&self, location: &Location, name: &CStr) -> Match {
        if self.patterns.is_empty() {
            return Match::None;
        }
        let mut path = Vec::new();
        for part in location.components() {
            path.extend(
                part.split(|byte| *byte == b'/')
                    .filter(|part| !part.is_empty()),
            );
        }
        path.push(name.to_bytes());
        let mut found = Match::None;
        for pattern in &self.patterns {
            if pattern.components.len() > path.len() {
                continue;
            }
            let start = path.len() - pattern.components.len();
            if pattern.anchored && start != 0 {
                continue;
            }
            if pattern
                .components
                .iter()
                .zip(&path[start..])
                .all(|(pattern, component)| {
                    os::component_matches(
                        pattern,
                        &os::c_name(component).expect("validated components"),
                    )
                })
            {
                if !pattern.directories_only {
                    return Match::Any;
                }
                found = Match::Directory;
            }
        }
        found
    }
}
