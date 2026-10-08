//! CLI and ncdu configuration. Parsing preserves raw Linux argument bytes.
use crate::{os, scan::Options};
use std::os::unix::ffi::OsStrExt;
use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Name,
    Allocated,
    Apparent,
    Items,
    Mtime,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Shared {
    Off,
    Shared,
    Unique,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Graph {
    Hash,
    Half,
    Eighth,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Off,
    Dark,
    DarkBackground,
}
#[derive(Clone)]
pub struct Config {
    pub scan: Options,
    pub root: Option<PathBuf>,
    pub import: Option<PathBuf>,
    pub export: Option<(PathBuf, bool)>,
    pub report: Option<PathBuf>,
    pub ui: Option<u8>,
    pub help: bool,
    pub version: bool,
    pub quit_after_scan: bool,
    pub slow: bool,
    pub apparent: bool,
    pub si: bool,
    pub hidden: bool,
    pub items: bool,
    pub mtime: bool,
    pub graph: bool,
    pub percent: bool,
    pub directories_first: bool,
    pub natural: bool,
    pub sort: Sort,
    pub descending: bool,
    pub shared: Shared,
    pub graph_style: Graph,
    pub color: Color,
    pub can_delete: Option<bool>,
    pub can_shell: Option<bool>,
    pub can_refresh: Option<bool>,
    pub confirm_delete: bool,
    pub confirm_quit: bool,
    pub delete_command: Option<Vec<u8>>,
    pub compress: bool,
    pub compress_level: i32,
    pub block_size: usize,
    readonly: usize,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            scan: Options::default(),
            root: None,
            import: None,
            export: None,
            report: None,
            ui: None,
            help: false,
            version: false,
            quit_after_scan: false,
            slow: false,
            apparent: false,
            si: false,
            hidden: true,
            items: false,
            mtime: false,
            graph: true,
            percent: false,
            directories_first: false,
            natural: true,
            sort: Sort::Allocated,
            descending: true,
            shared: Shared::Shared,
            graph_style: Graph::Hash,
            color: Color::Off,
            can_delete: None,
            can_shell: None,
            can_refresh: None,
            confirm_delete: true,
            confirm_quit: false,
            delete_command: None,
            compress: false,
            compress_level: 1,
            block_size: 65536,
            readonly: 0,
        }
    }
}
fn requires_value(option: &[u8]) -> bool {
    matches!(
        option,
        b"--threads"
            | b"--exclude"
            | b"--exclude-from"
            | b"--sort"
            | b"--color"
            | b"--shared-column"
            | b"--graph-style"
            | b"--delete-command"
            | b"--compress-level"
            | b"--export-block-size"
            | b"--scan-report"
            | b"-t"
            | b"-f"
            | b"-o"
            | b"-O"
            | b"-X"
    )
}
fn number(value: &[u8], minimum: usize, maximum: usize) -> io::Result<usize> {
    std::str::from_utf8(value)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value >= minimum && *value <= maximum)
        .ok_or_else(|| os::invalid("option numeric value out of range"))
}
impl Config {
    fn set(&mut self, option: &[u8], value: Option<&[u8]>, config: bool) -> io::Result<()> {
        if requires_value(option) != value.is_some() {
            return Err(os::invalid("missing or unexpected option argument"));
        }
        let value = value.unwrap_or_default();
        match option {
            b"-h" | b"-?" | b"--help" => self.help = true,
            b"-v" | b"-V" | b"--version" => self.version = true,
            b"-t" | b"--threads" => self.scan.threads = number(value, 0, 255)?,
            b"-x" | b"--one-file-system" => self.scan.same_filesystem = true,
            b"--cross-file-system" => self.scan.same_filesystem = false,
            b"-e" | b"--extended" => self.scan.extended = true,
            b"--no-extended" => self.scan.extended = false,
            b"-L" | b"--follow-symlinks" => self.scan.follow_symlinks = true,
            b"--no-follow-symlinks" => self.scan.follow_symlinks = false,
            b"--exclude-caches" => self.scan.exclude_caches = true,
            b"--include-caches" => self.scan.exclude_caches = false,
            b"--exclude-kernfs" => self.scan.exclude_kernel = true,
            b"--include-kernfs" => self.scan.exclude_kernel = false,
            b"--exclude" => self.scan.exclusions.add(&if config {
                os::expand_home(value)?
            } else {
                value.to_vec()
            })?,
            b"-X" | b"--exclude-from" => {
                let expanded = if config {
                    os::expand_home(value)?
                } else {
                    value.to_vec()
                };
                for line in std::fs::read(os::path(&expanded))?.split(|byte| *byte == b'\n') {
                    let line = line.strip_suffix(b"\r").unwrap_or(line);
                    self.scan.exclusions.add(&if config {
                        os::expand_home(line)?
                    } else {
                        line.to_vec()
                    })?;
                }
            }
            b"-0" => self.ui = Some(0),
            b"-1" => self.ui = Some(1),
            b"-2" => self.ui = Some(2),
            b"-q" | b"--slow-ui-updates" => self.slow = true,
            b"--fast-ui-updates" => self.slow = false,
            b"--quit-after-scan" => self.quit_after_scan = true,
            b"--ignore-config" => {}
            b"-r" => {
                self.readonly += 1;
                self.can_delete = Some(false);
                if self.readonly > 1 {
                    self.can_shell = Some(false);
                }
            }
            b"-f" => {
                if self.import.is_some() {
                    return Err(os::invalid("multiple imports"));
                }
                self.import = Some(os::path(value));
            }
            b"-o" | b"-O" => {
                if self.export.is_some() {
                    return Err(os::invalid("multiple outputs"));
                }
                self.export = Some((os::path(value), option == b"-O"));
            }
            b"--scan-report" => self.report = Some(os::path(value)),
            b"-c" | b"--compress" => self.compress = true,
            b"--no-compress" => self.compress = false,
            b"--compress-level" => self.compress_level = number(value, 1, 20)? as i32,
            b"--export-block-size" => self.block_size = number(value, 4, 16000)? * 1024,
            b"--delete-command" => self.delete_command = Some(value.to_vec()),
            b"--apparent-size" => {
                self.apparent = true;
                if self.sort == Sort::Allocated {
                    self.sort = Sort::Apparent;
                }
            }
            b"--disk-usage" => {
                self.apparent = false;
                if self.sort == Sort::Apparent {
                    self.sort = Sort::Allocated;
                }
            }
            b"--si" => self.si = true,
            b"--no-si" => self.si = false,
            b"--show-hidden" => self.hidden = true,
            b"--hide-hidden" => self.hidden = false,
            b"--show-itemcount" => self.items = true,
            b"--hide-itemcount" => self.items = false,
            b"--show-mtime" => self.mtime = true,
            b"--hide-mtime" => self.mtime = false,
            b"--show-graph" => self.graph = true,
            b"--hide-graph" => self.graph = false,
            b"--show-percent" => self.percent = true,
            b"--hide-percent" => self.percent = false,
            b"--group-directories-first" => self.directories_first = true,
            b"--no-group-directories-first" => self.directories_first = false,
            b"--enable-natsort" => self.natural = true,
            b"--disable-natsort" => self.natural = false,
            b"--enable-delete" => self.can_delete = Some(true),
            b"--disable-delete" => self.can_delete = Some(false),
            b"--enable-shell" => self.can_shell = Some(true),
            b"--disable-shell" => self.can_shell = Some(false),
            b"--enable-refresh" => self.can_refresh = Some(true),
            b"--disable-refresh" => self.can_refresh = Some(false),
            b"--confirm-delete" => self.confirm_delete = true,
            b"--no-confirm-delete" => self.confirm_delete = false,
            b"--confirm-quit" => self.confirm_quit = true,
            b"--no-confirm-quit" => self.confirm_quit = false,
            b"--shared-column" => {
                self.shared = match value {
                    b"off" => Shared::Off,
                    b"shared" => Shared::Shared,
                    b"unique" => Shared::Unique,
                    _ => return Err(os::invalid("invalid shared column")),
                }
            }
            b"--graph-style" => {
                self.graph_style = match value {
                    b"hash" => Graph::Hash,
                    b"half-block" => Graph::Half,
                    b"eighth-block" | b"eigth-block" => Graph::Eighth,
                    _ => return Err(os::invalid("invalid graph style")),
                }
            }
            b"--color" => {
                self.color = match value {
                    b"off" => Color::Off,
                    b"dark" => Color::Dark,
                    b"dark-bg" => Color::DarkBackground,
                    _ => return Err(os::invalid("invalid color")),
                }
            }
            b"--sort" => {
                let (field, descending) = if let Some(field) = value.strip_suffix(b"-asc") {
                    (field, false)
                } else if let Some(field) = value.strip_suffix(b"-desc") {
                    (field, true)
                } else {
                    (value, value != b"name")
                };
                self.sort = match field {
                    b"name" => Sort::Name,
                    b"disk-usage" => Sort::Allocated,
                    b"apparent-size" => Sort::Apparent,
                    b"itemcount" => Sort::Items,
                    b"mtime" => Sort::Mtime,
                    _ => return Err(os::invalid("invalid sort field")),
                };
                self.descending = descending;
            }
            _ => {
                return Err(os::invalid(&format!(
                    "unknown option {}",
                    String::from_utf8_lossy(option)
                )));
            }
        }
        Ok(())
    }
    pub fn config_bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
        for line in bytes.split(|byte| *byte == b'\n') {
            if line.len() > 4096 {
                return Err(os::invalid("configuration line too long"));
            }
            let line = line.trim_ascii();
            if line.is_empty() || line.starts_with(b"#") {
                continue;
            }
            let suppressed = line.starts_with(b"@");
            let line = if suppressed { &line[1..] } else { line };
            let separator = line
                .iter()
                .position(|byte| byte.is_ascii_whitespace() || *byte == b'=');
            let (option, value) = if let Some(at) = separator {
                (&line[..at], Some(line[at + 1..].trim_ascii()))
            } else {
                (line, None)
            };
            let mut candidate = self.clone();
            match candidate.set(option, value, true) {
                Ok(()) => *self = candidate,
                Err(_) if suppressed => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}
fn optional_config(config: &mut Config, path: &Path) -> io::Result<()> {
    match std::fs::read(path) {
        Ok(bytes) => config.config_bytes(&bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
pub fn parse(arguments: &[OsString], load_config: bool) -> io::Result<Config> {
    let mut config = Config::default();
    if load_config
        && !arguments
            .iter()
            .any(|arg| arg.as_bytes() == b"--ignore-config")
    {
        optional_config(&mut config, Path::new("/etc/ncdu.conf"))?;
        let home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
        if let Some(home) = home {
            optional_config(&mut config, &home.join("ncdu/config"))?;
        }
    }
    let mut index = 0;
    let mut positional = false;
    while index < arguments.len() {
        let value = arguments[index].as_bytes();
        index += 1;
        if !positional && value == b"--" {
            positional = true;
            continue;
        }
        if positional || !value.starts_with(b"-") {
            if config.root.is_some() {
                return Err(os::invalid("multiple scan directories"));
            }
            config.root = Some(os::path(value));
            continue;
        }
        if value.starts_with(b"--") {
            let split = value.iter().position(|byte| *byte == b'=');
            let (option, mut parameter) = if let Some(at) = split {
                (&value[..at], Some(&value[at + 1..]))
            } else {
                (value, None)
            };
            if requires_value(option) && parameter.is_none() {
                parameter = Some(
                    arguments
                        .get(index)
                        .ok_or_else(|| os::invalid("missing option value"))?
                        .as_bytes(),
                );
                index += 1;
            }
            config.set(option, parameter, false)?;
        } else {
            if value.len() == 1 {
                return Err(os::invalid("invalid option '-'"));
            }
            let mut at = 1;
            while at < value.len() {
                let option = [b'-', value[at]];
                at += 1;
                let parameter = if requires_value(&option) {
                    if at < value.len() {
                        let parameter = &value[at..];
                        at = value.len();
                        Some(parameter)
                    } else {
                        let parameter = arguments
                            .get(index)
                            .ok_or_else(|| os::invalid("missing option value"))?
                            .as_bytes();
                        index += 1;
                        Some(parameter)
                    }
                } else {
                    None
                };
                config.set(&option, parameter, false)?;
            }
        }
    }
    if config.root.is_some() && config.import.is_some() {
        return Err(os::invalid("scan root and import are mutually exclusive"));
    }
    Ok(config)
}
pub const HELP: &str = "rcdu 0.2 — Linux ncurses disk usage browser\nUsage: rcdu [options] [directory]\n  -t, --threads N     Workers: 0 automatic (default), 1..255 explicit\n  -x / -L / -e        One filesystem / follow file links / extended metadata\n  -0 / -1 / -2        No / line / curses scan UI\n  -f FILE             Import ncdu JSON, zstd JSON or EX1 binary\n  -o FILE [-c]        Export JSON, optionally zstd; '-' means stdout\n  -O FILE             Export indexed ncdu EX1 binary\n  --exclude PATTERN / -X FILE / --exclude-caches / --exclude-kernfs\n  -r / -rr            Disable deletion / deletion and shell\n  --ignore-config / --quit-after-scan / --scan-report FILE\n  --sort name|disk-usage|apparent-size|itemcount|mtime[-asc|-desc]\n  --apparent-size / --disk-usage / --si / --no-si\n  --shared-column off|shared|unique / --color off|dark|dark-bg\n  --graph-style hash|half-block|eighth-block\n  --compress-level 1..20 / --export-block-size 4..16000 (KiB)\n  --[enable|disable]-shell|-delete|-refresh|-natsort\n  --[show|hide]-hidden|-itemcount|-mtime|-graph|-percent\n  --[no-]group-directories-first / --[no-]confirm-delete|-quit\n  --delete-command COMMAND / --fast-ui-updates / --slow-ui-updates\n  -h, --help / -v, --version\nBrowser: arrows/hjkl, q, ?, i, d, r, b, n/s/C/M, a/e/t/c/m/g/u.\n";
