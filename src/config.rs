use crate::{os, scan::ScanOptions};
use std::{io, path::PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortField {
    Name,
    Allocated,
    Apparent,
    Items,
    Mtime,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SharedColumn {
    Off,
    Shared,
    Unique,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphStyle {
    Hash,
    Half,
    Eighth,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    Off,
    Dark,
    DarkBackground,
}
#[derive(Clone)]
pub struct Config {
    pub scan: ScanOptions,
    pub root: Option<PathBuf>,
    pub import: Option<PathBuf>,
    pub export: Option<(PathBuf, bool)>,
    pub quit_after_scan: bool,
    pub help: bool,
    pub version: bool,
    pub ui: Option<u8>,
    pub slow_ui: bool,
    pub can_delete: Option<bool>,
    pub can_shell: Option<bool>,
    pub can_refresh: Option<bool>,
    pub hidden: bool,
    pub itemcount: bool,
    pub mtime: bool,
    pub graph: bool,
    pub percent: bool,
    pub directories_first: bool,
    pub natural_sort: bool,
    pub sort: SortField,
    pub descending: bool,
    pub shared: SharedColumn,
    pub apparent: bool,
    pub si: bool,
    pub compress: bool,
    pub compression_level: i32,
    pub block_size: usize,
    pub confirm_quit: bool,
    pub confirm_delete: bool,
    pub delete_command: Option<Vec<u8>>,
    pub graph_style: GraphStyle,
    pub color: Color,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            scan: ScanOptions {
                workers: 1,
                ..Default::default()
            },
            root: None,
            import: None,
            export: None,
            quit_after_scan: false,
            help: false,
            version: false,
            ui: None,
            slow_ui: false,
            can_delete: None,
            can_shell: None,
            can_refresh: None,
            hidden: true,
            itemcount: false,
            mtime: false,
            graph: true,
            percent: false,
            directories_first: false,
            natural_sort: true,
            sort: SortField::Allocated,
            descending: true,
            shared: SharedColumn::Shared,
            apparent: false,
            si: false,
            compress: false,
            compression_level: 4,
            block_size: 64 * 1024,
            confirm_quit: false,
            confirm_delete: true,
            delete_command: None,
            graph_style: GraphStyle::Hash,
            color: Color::Off,
        }
    }
}
fn number(bytes: &[u8], minimum: usize, maximum: usize) -> io::Result<usize> {
    let number = std::str::from_utf8(bytes)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|number| *number >= minimum && *number <= maximum);
    number.ok_or_else(|| os::invalid("option number out of range"))
}
impl Config {
    pub fn apply(&mut self, option: &[u8], value: Option<&[u8]>, infile: bool) -> io::Result<()> {
        let arg = || value.ok_or_else(|| os::invalid("missing option value"));
        let boolean = match option {
            b"-q" | b"--slow-ui-updates" => Some((&mut self.slow_ui, true)),
            b"--fast-ui-updates" => Some((&mut self.slow_ui, false)),
            b"-x" | b"--one-file-system" => Some((&mut self.scan.same_filesystem, true)),
            b"--cross-file-system" => Some((&mut self.scan.same_filesystem, false)),
            b"-e" | b"--extended" => Some((&mut self.scan.extended, true)),
            b"--no-extended" => Some((&mut self.scan.extended, false)),
            b"--show-hidden" => Some((&mut self.hidden, true)),
            b"--hide-hidden" => Some((&mut self.hidden, false)),
            b"--show-itemcount" => Some((&mut self.itemcount, true)),
            b"--hide-itemcount" => Some((&mut self.itemcount, false)),
            b"--show-mtime" => Some((&mut self.mtime, true)),
            b"--hide-mtime" => Some((&mut self.mtime, false)),
            b"--show-graph" => Some((&mut self.graph, true)),
            b"--hide-graph" => Some((&mut self.graph, false)),
            b"--show-percent" => Some((&mut self.percent, true)),
            b"--hide-percent" => Some((&mut self.percent, false)),
            b"--group-directories-first" => Some((&mut self.directories_first, true)),
            b"--no-group-directories-first" => Some((&mut self.directories_first, false)),
            b"--enable-natsort" => Some((&mut self.natural_sort, true)),
            b"--disable-natsort" => Some((&mut self.natural_sort, false)),
            b"--apparent-size" => Some((&mut self.apparent, true)),
            b"--disk-usage" => Some((&mut self.apparent, false)),
            b"--si" => Some((&mut self.si, true)),
            b"--no-si" => Some((&mut self.si, false)),
            b"-L" | b"--follow-symlinks" => Some((&mut self.scan.follow_symlinks, true)),
            b"--no-follow-symlinks" => Some((&mut self.scan.follow_symlinks, false)),
            b"--exclude-caches" => Some((&mut self.scan.exclude_caches, true)),
            b"--include-caches" => Some((&mut self.scan.exclude_caches, false)),
            b"--exclude-kernfs" => Some((&mut self.scan.exclude_kernel, true)),
            b"--include-kernfs" => Some((&mut self.scan.exclude_kernel, false)),
            b"-c" | b"--compress" => Some((&mut self.compress, true)),
            b"--no-compress" => Some((&mut self.compress, false)),
            b"--confirm-quit" => Some((&mut self.confirm_quit, true)),
            b"--no-confirm-quit" => Some((&mut self.confirm_quit, false)),
            b"--confirm-delete" => Some((&mut self.confirm_delete, true)),
            b"--no-confirm-delete" => Some((&mut self.confirm_delete, false)),
            _ => None,
        };
        if let Some((field, setting)) = boolean {
            if value.is_some() {
                return Err(os::invalid("unexpected option value"));
            }
            *field = setting;
            return Ok(());
        }
        match option {
            b"-r" => {
                if self.can_delete == Some(false) {
                    self.can_shell = Some(false);
                } else {
                    self.can_delete = Some(false);
                }
            }
            b"--enable-shell" => self.can_shell = Some(true),
            b"--disable-shell" => self.can_shell = Some(false),
            b"--enable-delete" => self.can_delete = Some(true),
            b"--disable-delete" => self.can_delete = Some(false),
            b"--enable-refresh" => self.can_refresh = Some(true),
            b"--disable-refresh" => self.can_refresh = Some(false),
            b"-0" => self.ui = Some(0),
            b"-1" => self.ui = Some(1),
            b"-2" => self.ui = Some(2),
            b"--graph-style" => {
                self.graph_style = match arg()? {
                    b"hash" => GraphStyle::Hash,
                    b"half-block" => GraphStyle::Half,
                    b"eighth-block" | b"eigth-block" => GraphStyle::Eighth,
                    _ => return Err(os::invalid("unknown graph style")),
                }
            }
            b"--shared-column" => {
                self.shared = match arg()? {
                    b"off" => SharedColumn::Off,
                    b"shared" => SharedColumn::Shared,
                    b"unique" => SharedColumn::Unique,
                    _ => return Err(os::invalid("unknown shared column")),
                }
            }
            b"--color" => {
                self.color = match arg()? {
                    b"off" => Color::Off,
                    b"dark" => Color::Dark,
                    b"dark-bg" => Color::DarkBackground,
                    _ => return Err(os::invalid("unknown color")),
                }
            }
            b"--sort" => {
                let mut field = arg()?;
                let order = if field.ends_with(b"-asc") {
                    field = &field[..field.len() - 4];
                    Some(false)
                } else if field.ends_with(b"-desc") {
                    field = &field[..field.len() - 5];
                    Some(true)
                } else {
                    None
                };
                self.sort = match field {
                    b"name" => SortField::Name,
                    b"disk-usage" => SortField::Allocated,
                    b"apparent-size" => SortField::Apparent,
                    b"itemcount" => SortField::Items,
                    b"mtime" => SortField::Mtime,
                    _ => return Err(os::invalid("unknown sort column")),
                };
                self.descending =
                    order.unwrap_or(!matches!(self.sort, SortField::Name | SortField::Mtime));
            }
            b"--exclude" => {
                let value = if infile {
                    os::expand_user(arg()?)
                } else {
                    arg()?.to_vec()
                };
                self.scan.exclusions.add(&value)?;
            }
            b"-X" | b"--exclude-from" => {
                let value = if infile {
                    os::expand_user(arg()?)
                } else {
                    arg()?.to_vec()
                };
                let bytes = os::read_config(&os::byte_path(&value))?;
                for line in bytes.split(|byte| *byte == b'\n') {
                    if !line.is_empty() {
                        self.scan.exclusions.add(line)?;
                    }
                }
            }
            b"--compress-level" => self.compression_level = number(arg()?, 1, 20)? as i32,
            b"--export-block-size" => self.block_size = number(arg()?, 4, 16000)? * 1024,
            b"-t" | b"--threads" => self.scan.workers = number(arg()?, 0, 255)?,
            b"--delete-command" => {
                os::c_name(arg()?)?;
                self.delete_command = Some(arg()?.to_vec());
            }
            b"--metadata-backend" => {
                self.scan.backend = match arg()? {
                    b"fstatat" => os::MetadataBackend::Fstatat,
                    b"statx" => os::MetadataBackend::Statx,
                    _ => return Err(os::invalid("unknown metadata backend")),
                }
            }
            _ => {
                return Err(os::invalid(&format!(
                    "unknown option {}",
                    String::from_utf8_lossy(option)
                )));
            }
        }
        if !crate::cli::takes_value(option) && value.is_some() {
            return Err(os::invalid("unexpected option value"));
        }
        Ok(())
    }
    pub fn config_bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
        for (line_number, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
            if line.len() > 4096 {
                return Err(os::invalid("configuration line exceeds 4096 bytes"));
            }
            let line = trim(line);
            let (optional, line) = if let Some(line) = line.strip_prefix(b"@") {
                (true, line)
            } else {
                (false, line)
            };
            if line.is_empty() || line.starts_with(b"#") {
                continue;
            }
            let end = line
                .iter()
                .position(|byte| matches!(byte, b' ' | b'\t' | b'='))
                .unwrap_or(line.len());
            let value = (end < line.len()).then(|| trim(&line[end + 1..]));
            let mut candidate = self.clone();
            match candidate.apply(&line[..end], value, true) {
                Ok(()) => *self = candidate,
                Err(_) if optional => {}
                Err(error) => {
                    return Err(os::invalid(&format!(
                        "config line {}: {error}",
                        line_number + 1
                    )));
                }
            }
        }
        Ok(())
    }
}
fn trim(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |index| index + 1);
    &bytes[start..end]
}
