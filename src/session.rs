//! Session owner: signal guards, scanner progress, curses transitions and binary entry point.
use crate::{
    browser::Browser,
    cli::{self, Config},
    format::json,
    model::Model,
    os::{self, Curses, Key},
    scan::{self, Cancellation},
};
use std::{
    ffi::OsString,
    io,
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::Path,
    time::{Duration, Instant},
};
pub fn scan(config: &Config, root: &Path, terminal: &mut Option<Curses>) -> io::Result<Model> {
    let cancel = Cancellation::default();
    let trigger = cancel.clone();
    let mut failure = None;
    let mut last = Instant::now().checked_sub(Duration::from_secs(3)).unwrap();
    let interval = Duration::from_millis(if config.slow { 2000 } else { 100 });
    let result = scan::scan_with_progress(root, &config.scan, cancel, |progress| {
        if os::interrupted() {
            trigger.cancel();
        }
        if let Some(terminal) = terminal {
            let status = (|| -> io::Result<()> {
                if last.elapsed() >= interval {
                    terminal.clear();
                    terminal.text(0, 0, "rcdu — scanning...", false, true, 0)?;
                    terminal.text(
                        2,
                        2,
                        &crate::display::escape(os::bytes(root)),
                        false,
                        false,
                        0,
                    )?;
                    terminal.text(
                        4,
                        2,
                        &format!("{} items observed", progress.entries),
                        false,
                        false,
                        0,
                    )?;
                    terminal.text(6, 2, "Press q to abort", false, false, 0)?;
                    terminal.refresh()?;
                    last = Instant::now();
                }
                if matches!(terminal.key(0)?, Some(Key::Character('q' | '\u{3}'))) {
                    trigger.cancel();
                }
                Ok(())
            })();
            if let Err(error) = status {
                failure = Some(error);
                trigger.cancel();
            }
        } else if config.ui == Some(1) && last.elapsed() >= interval {
            eprintln!("rcdu: {} items observed", progress.entries);
            last = Instant::now();
        }
    });
    if let Some(error) = failure {
        Err(error)
    } else {
        result
    }
}
pub fn run(arguments: &[OsString]) -> io::Result<()> {
    let mut config = cli::parse(arguments, true)?;
    if config.help {
        println!("{}", cli::HELP);
        return Ok(());
    }
    if config.version {
        println!("rcdu {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if config.export.as_ref().is_some_and(|(_, binary)| *binary) {
        return Err(io::Error::other(
            "EX1 binary is still being implemented in the new engine",
        ));
    }
    if !config.quit_after_scan
        && config.import.is_none()
        && config.export.is_none()
        && !std::io::IsTerminal::is_terminal(&std::io::stdin())
    {
        return Err(io::Error::other(
            "stdin is not a tty; use --quit-after-scan or -f",
        ));
    }
    let _signals = os::Signals::install()?;
    let mode = config
        .ui
        .unwrap_or(if config.quit_after_scan || config.export.is_some() {
            0
        } else {
            2
        });
    config.ui = Some(mode);
    let mut terminal = if mode == 2 {
        Some(Curses::open(config.color)?)
    } else {
        None
    };
    let root = config.root.as_deref().unwrap_or(Path::new("."));
    let start = Instant::now();
    let imported = config.import.is_some();
    let model = if let Some(path) = &config.import {
        if path == Path::new("-") {
            read_json(BufReader::new(std::io::stdin()))?
        } else {
            read_json(BufReader::new(std::fs::File::open(path)?))?
        }
    } else {
        scan(&config, root, &mut terminal)?
    };
    if let Some(path) = &config.report {
        let totals = model.directories[0].totals;
        let entries: Vec<_> = model.parts.iter().map(|part| part.entries.len()).collect();
        let bytes = model
            .parts
            .iter()
            .map(|part| part.entries.len() * std::mem::size_of::<crate::model::Entry>())
            .sum::<usize>();
        let mut file = std::fs::File::create(path)?;
        writeln!(
            file,
            "{{\"version\":1,\"elapsed_seconds\":{},\"worker_entries\":{:?},\"directories\":{},\"entries\":{},\"entry_bytes\":{},\"allocated\":{},\"apparent\":{},\"shared_allocated\":{},\"shared_apparent\":{},\"items\":{}}}",
            start.elapsed().as_secs_f64(),
            entries,
            model.directories.len(),
            model.len(),
            bytes,
            totals.allocated,
            totals.apparent,
            totals.shared_allocated,
            totals.shared_apparent,
            totals.items
        )?;
    }
    if let Some((path, _)) = &config.export {
        let output: Box<dyn Write> = if path == Path::new("-") {
            Box::new(std::io::stdout())
        } else {
            Box::new(std::fs::File::create(path)?)
        };
        let mut output = BufWriter::with_capacity(65536, output);
        if config.compress {
            let encoder = zstd::stream::write::Encoder::new(&mut output, config.compress_level)?;
            let mut buffer = BufWriter::with_capacity(65536, encoder);
            json::write(&model, &mut buffer, config.scan.extended)?;
            buffer
                .into_inner()
                .map_err(|error| error.into_error())?
                .finish()?;
        } else {
            json::write(&model, &mut output, config.scan.extended)?;
        }
        output.flush()?;
        return Ok(());
    }
    if config.quit_after_scan {
        return Ok(());
    }
    let mut terminal = if let Some(terminal) = terminal {
        terminal
    } else {
        Curses::open(config.color)?
    };
    Browser::new(model, config, imported).run(&mut terminal)
}

fn read_json(mut input: impl BufRead) -> io::Result<Model> {
    let mut prefix = [0; 4];
    input.read_exact(&mut prefix)?;
    if prefix == [0xbf, b'n', b'c', b'd'] {
        return Err(io::Error::other(
            "EX1 binary is still being implemented in the new engine",
        ));
    }
    let input = std::io::Cursor::new(prefix).chain(input);
    if prefix == [0x28, 0xb5, 0x2f, 0xfd] {
        let mut decoder = zstd::stream::read::Decoder::new(input)?;
        decoder.window_log_max(27)?;
        json::read(BufReader::new(decoder))
    } else {
        json::read(BufReader::new(input))
    }
}
