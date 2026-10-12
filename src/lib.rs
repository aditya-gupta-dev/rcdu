//! Fresh batched scan engine and ncurses disk-usage session.
#[cfg(not(target_os = "linux"))]
compile_error!("rcdu currently supports Linux only");

pub mod browser;
pub mod cli;
pub mod delete;
pub mod display;
pub mod exclude;
pub mod model;
pub mod mutation;
pub mod os;
pub mod scan;
pub mod session;
pub mod storage;
