//! Process orchestration; reusable scanners/codecs/browser policies return errors to this boundary.
use crate::{
    browser::{Browser, Source},
    cli,
    config::Config,
    format::{binary, json},
    os,
    scan::{self, Cancellation},
    ui::{
        display,
        terminal::{Key, Terminal},
    },
};
use std::{
    ffi::OsString,
    io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::Path,
    time::{Duration, Instant},
};

fn read_json(reader: impl Read) -> io::Result<crate::model::Model> {
    let mut reader = BufReader::new(reader);
    let mut prefix = [0; 4];
    reader.read_exact(&mut prefix)?;
    let reader = std::io::Cursor::new(prefix).chain(reader);
    if prefix == [0x28, 0xb5, 0x2f, 0xfd] {
        json::read(BufReader::new(zstd::stream::read::Decoder::new(reader)?))
    } else {
        json::read(BufReader::new(reader))
    }
}
fn load(config: &Config, terminal: &mut Option<Terminal>) -> io::Result<Source> {
    if let Some(path) = &config.import {
        if path == Path::new("-") {
            return Ok(Source::Memory(read_json(std::io::stdin())?));
        }
        let mut file = os::input(path)?;
        let mut signature = [0; 8];
        file.read_exact(&mut signature)?;
        file.seek(SeekFrom::Start(0))?;
        if &signature == binary::SIGNATURE {
            let mut reader = binary::Reader::open(file)?;
            if config.export.is_some() {
                return Ok(Source::Memory(reader.import()?));
            }
            return Source::indexed(reader);
        }
        return Ok(Source::Memory(read_json(file)?));
    }
    let cancel = Cancellation::default();
    let mut ui_error = None;
    let mut last = Instant::now().checked_sub(Duration::from_secs(3)).unwrap();
    let interval = Duration::from_millis(if config.slow_ui { 2000 } else { 100 });
    let path = config.root.as_deref().unwrap_or(Path::new("."));
    let scanner = if config.export.is_some() {
        scan::stage_with_progress
    } else {
        scan::scan_with_progress
    };
    let model = scanner(
        path,
        &config.scan,
        cancel.clone(),
        |progress: scan::Progress| {
            if os::interrupted() {
                cancel.cancel();
            }
            if let Some(terminal) = terminal {
                if last.elapsed() >= interval {
                    if let Err(error) = terminal.frame(
                        &[
                            "rcdu — scanning".into(),
                            display::sanitize(os::path_bytes(path)),
                            format!("{} entries observed", progress.entries),
                            "q: abort scan".into(),
                        ],
                        None,
                        false,
                    ) {
                        ui_error = Some(error);
                        cancel.cancel();
                    }
                    last = Instant::now();
                }
                match terminal.key(0) {
                    Ok(Some(Key::Character('q' | '\u{3}'))) => cancel.cancel(),
                    Err(error) => {
                        ui_error = Some(error);
                        cancel.cancel();
                    }
                    _ => {}
                }
            } else if config.ui == Some(1) && last.elapsed() >= interval {
                eprintln!("rcdu: {} entries observed", progress.entries);
                last = Instant::now();
            }
        },
    );
    if let Some(error) = ui_error {
        return Err(error);
    }
    Ok(Source::Memory(model?))
}
fn export(config: &Config, model: &crate::model::Model) -> io::Result<()> {
    let (path, binary) = config.export.as_ref().unwrap();
    let output: Box<dyn Write> = if path == Path::new("-") {
        Box::new(io::stdout())
    } else {
        Box::new(os::output(path)?)
    };
    let mut output = BufWriter::with_capacity(65536, output);
    if *binary {
        binary::write(
            model,
            &mut output,
            config.block_size,
            config.compression_level,
            config.scan.extended,
        )?;
    } else if config.compress {
        let mut encoder = zstd::stream::write::Encoder::new(&mut output, config.compression_level)?;
        json::write(model, &mut encoder, config.scan.extended)?;
        encoder.finish()?;
    } else {
        json::write(model, &mut output, config.scan.extended)?;
    }
    output.flush()
}
pub fn run(args: Vec<OsString>) -> io::Result<()> {
    let mut config = cli::parse(&args, true)?;
    if config.help {
        print!("{}", cli::HELP);
        return Ok(());
    }
    if config.version {
        println!("rcdu {} (ncdu 2.9 compatible)", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if !os::terminal_input()
        && config.import.is_none()
        && config.export.is_none()
        && !config.quit_after_scan
    {
        return Err(io::Error::other(
            "stdin is not a TTY; use -f - to import or --quit-after-scan",
        ));
    }
    let _signals = os::SignalGuard::install()?;
    let scan_ui = config
        .ui
        .unwrap_or(if config.export.is_some() || config.quit_after_scan {
            0
        } else {
            2
        });
    config.ui = Some(scan_ui);
    let mut terminal = if scan_ui == 2 {
        Some(Terminal::open()?)
    } else {
        None
    };
    if config.import.is_none()
        && config
            .export
            .as_ref()
            .is_some_and(|(_, binary)| *binary || config.scan.workers == 1)
    {
        return export_scan(&config, &mut terminal);
    }
    let source = load(&config, &mut terminal)?;
    if config.export.is_some() {
        if let Source::Memory(model) = &source {
            export(&config, model)?;
        }
        return Ok(());
    }
    if config.quit_after_scan {
        return Ok(());
    }
    let imported = config.import.is_some();
    let mut browser = Browser::new(source, config, imported);
    let mut terminal = if let Some(terminal) = terminal {
        terminal
    } else {
        Terminal::open()?
    };
    browser.run(&mut terminal)
}

fn progress_hook<'a>(
    config: &'a Config,
    terminal: &'a mut Option<Terminal>,
    cancel: Cancellation,
    error: &'a mut Option<io::Error>,
) -> impl FnMut(scan::Progress) + 'a {
    let mut last = Instant::now().checked_sub(Duration::from_secs(3)).unwrap();
    let interval = Duration::from_millis(if config.slow_ui { 2000 } else { 100 });
    let path = display::sanitize(os::path_bytes(
        config.root.as_deref().unwrap_or(Path::new(".")),
    ));
    move |progress| {
        if os::interrupted() {
            cancel.cancel();
        }
        if let Some(terminal) = terminal {
            if last.elapsed() >= interval {
                if let Err(failure) = terminal.frame(
                    &[
                        "rcdu — scanning".into(),
                        path.clone(),
                        format!("{} entries observed", progress.entries),
                        "q: abort scan".into(),
                    ],
                    None,
                    false,
                ) {
                    *error = Some(failure);
                    cancel.cancel();
                }
                last = Instant::now();
            }
            match terminal.key(0) {
                Ok(Some(Key::Character('q' | '\u{3}'))) => cancel.cancel(),
                Err(failure) => {
                    *error = Some(failure);
                    cancel.cancel();
                }
                _ => {}
            }
        } else if config.ui == Some(1) && last.elapsed() >= interval {
            eprintln!("rcdu: {} entries observed", progress.entries);
            last = Instant::now();
        }
    }
}
fn export_scan(config: &Config, terminal: &mut Option<Terminal>) -> io::Result<()> {
    let (path, binary) = config.export.as_ref().unwrap();
    let output: Box<dyn Write + Send> = if path == Path::new("-") {
        Box::new(io::stdout())
    } else {
        Box::new(os::output(path)?)
    };
    let mut output = BufWriter::with_capacity(65536, output);
    let cancel = Cancellation::default();
    let mut ui_error = None;
    let hook = progress_hook(config, terminal, cancel.clone(), &mut ui_error);
    let root = config.root.as_deref().unwrap_or(Path::new("."));
    let result = if *binary {
        scan::binary_with_progress(
            root,
            &config.scan,
            Box::new(output),
            config.block_size,
            config.compression_level,
            cancel,
            hook,
        )
    } else if config.compress {
        let mut encoder = zstd::stream::write::Encoder::new(&mut output, config.compression_level)?;
        let mut sink = crate::sink::Json::new(&mut encoder, config.scan.extended)?;
        scan::stream_with_progress(root, &config.scan, &mut sink, &cancel, hook)?;
        sink.finish()?;
        encoder.finish()?;
        output.flush()
    } else {
        let mut sink = crate::sink::Json::new(&mut output, config.scan.extended)?;
        scan::stream_with_progress(root, &config.scan, &mut sink, &cancel, hook)?;
        sink.finish()?;
        output.flush()
    };
    if let Some(error) = ui_error {
        return Err(error);
    }
    result
}
