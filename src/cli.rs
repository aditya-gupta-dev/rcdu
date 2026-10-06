use crate::{config::Config, os};
use std::{ffi::OsString, io};
pub fn takes_value(option: &[u8]) -> bool {
    matches!(
        option,
        b"-o"
            | b"-O"
            | b"-f"
            | b"-X"
            | b"-t"
            | b"--threads"
            | b"--graph-style"
            | b"--sort"
            | b"--shared-column"
            | b"--exclude"
            | b"--exclude-from"
            | b"--compress-level"
            | b"--export-block-size"
            | b"--delete-command"
            | b"--color"
            | b"--metadata-backend"
    )
}
pub fn parse(args: &[OsString], load_config: bool) -> io::Result<Config> {
    let args: Vec<&[u8]> = args.iter().map(|arg| os::argument_bytes(arg)).collect();
    let mut config = Config::default();
    if load_config && !args.contains(&b"--ignore-config".as_slice()) {
        let mut paths = vec![std::path::PathBuf::from("/etc/ncdu.conf")];
        if let Some(home) = std::env::var_os("XDG_CONFIG_HOME") {
            paths.push(std::path::PathBuf::from(home).join("ncdu/config"));
        } else if let Some(home) = std::env::var_os("HOME") {
            paths.push(std::path::PathBuf::from(home).join(".config/ncdu/config"));
        }
        for path in paths {
            match os::read_config(&path) {
                Ok(bytes) => config.config_bytes(&bytes)?,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) => {}
                Err(error) => return Err(error),
            }
        }
    }
    let mut index = 0;
    let mut positional = false;
    while index < args.len() {
        let token = args[index];
        index += 1;
        if !positional && token == b"--" {
            positional = true;
            continue;
        }
        if positional || !token.starts_with(b"-") || token == b"-" {
            if config.root.is_some() {
                return Err(os::invalid("multiple scan directories"));
            }
            config.root = Some(os::byte_path(token));
            continue;
        }
        if token.starts_with(b"--") {
            let end = token
                .iter()
                .position(|byte| *byte == b'=')
                .unwrap_or(token.len());
            let option = &token[..end];
            let mut value = (end < token.len()).then(|| &token[end + 1..]);
            if takes_value(option) && value.is_none() {
                value = Some(
                    *args
                        .get(index)
                        .ok_or_else(|| os::invalid("missing option value"))?,
                );
                index += 1;
            }
            dispatch(&mut config, option, value)?;
        } else {
            let mut offset = 1;
            while offset < token.len() {
                let option = [b'-', token[offset]];
                offset += 1;
                let mut value = None;
                if takes_value(&option) {
                    if offset < token.len() {
                        value = Some(&token[offset..]);
                        offset = token.len();
                    } else {
                        value = Some(
                            *args
                                .get(index)
                                .ok_or_else(|| os::invalid("missing option value"))?,
                        );
                        index += 1;
                    }
                }
                dispatch(&mut config, &option, value)?;
            }
        }
    }
    if config.root.is_some() && config.import.is_some() {
        return Err(os::invalid(
            "scan directory and import are mutually exclusive",
        ));
    }
    Ok(config)
}
fn dispatch(config: &mut Config, option: &[u8], value: Option<&[u8]>) -> io::Result<()> {
    match option {
        b"-h" | b"-?" | b"--help" => config.help = true,
        b"-v" | b"-V" | b"--version" => config.version = true,
        b"--ignore-config" => {}
        b"--quit-after-scan" => config.quit_after_scan = true,
        b"-f" => {
            if config.import.is_some() {
                return Err(os::invalid("-f can only be given once"));
            }
            config.import = Some(os::byte_path(value.unwrap()));
        }
        b"-o" | b"-O" => {
            if config.export.is_some() {
                return Err(os::invalid("output can only be given once"));
            }
            config.export = Some((os::byte_path(value.unwrap()), option == b"-O"));
        }
        _ => return config.apply(option, value, false),
    }
    if value.is_some() && !takes_value(option) {
        return Err(os::invalid("unexpected option value"));
    }
    Ok(())
}
pub const HELP: &str = "rcdu - Linux disk usage browser (ncdu 2.9 compatible)\nUsage: rcdu [options] [directory]\n  -h, --help       Show help      -v, -V, --version  Version\n  -x               Stay on filesystem    -e  Extended metadata\n  -t, --threads N  Workers (1 default; 0 automatic; maximum 255)\n  -L               Follow file symlinks   -r / -rr  Disable delete / shell\n  -0 / -1 / -2     No / line / full scan UI; -q slows UI updates\n  -f FILE          Import JSON/zstd/indexed binary (JSON stdin: -)\n  -o FILE          Export JSON (stdout: -); -c enables zstd\n  -O FILE          Export indexed ncdu binary (always zstd)\n  -X FILE          Excludes from file; --exclude PATTERN\n  --exclude-caches / --exclude-kernfs / --ignore-config\n  --compress-level 1..20 / --export-block-size 4..16000 (KiB)\n  --sort name|disk-usage|apparent-size|itemcount|mtime[-asc|-desc]\n  --apparent-size / --disk-usage / --si / --no-si\n  --shared-column off|shared|unique / --graph-style hash|half-block|eighth-block\n  --color off|dark|dark-bg / --delete-command COMMAND\n  --[enable|disable]-shell / --[enable|disable]-delete / --[enable|disable]-refresh\n  --[show|hide]-hidden / -itemcount / -mtime / -graph / -percent\n  --[no-]group-directories-first / --[enable|disable]-natsort\n  --[no-]confirm-quit / --[no-]confirm-delete\n  --fast-ui-updates / --slow-ui-updates / --quit-after-scan\n  --metadata-backend fstatat|statx (measurement option)\nEvery boolean scan toggle also has its inverse; see docs/cli.md.\nBrowser: arrows/hjkl, q quit, ? help, i details, d delete, r refresh, b shell.\n";
