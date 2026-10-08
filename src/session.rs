//! Session owner: signal guards, scanner progress, curses transitions and binary entry point.
use crate::{
    browser::Browser,
    cli::{self, Config},
    model::Model,
    os::{self, Curses, Key},
    scan::{self, Cancellation},
};
use std::{
    ffi::OsString,
    io,
    io::Write,
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
    if config.import.is_some() || config.export.is_some() {
        return Err(io::Error::other(
            "codecs are still being implemented in the new engine",
        ));
    }
    if !config.quit_after_scan && !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return Err(io::Error::other(
            "stdin is not a tty; use --quit-after-scan or -f",
        ));
    }
    let _signals = os::Signals::install()?;
    let mode = config
        .ui
        .unwrap_or(if config.quit_after_scan { 0 } else { 2 });
    config.ui = Some(mode);
    let mut terminal = if mode == 2 {
        Some(Curses::open(config.color)?)
    } else {
        None
    };
    let root = config.root.as_deref().unwrap_or(Path::new("."));
    let start = Instant::now();
    let model = scan(&config, root, &mut terminal)?;
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
    if config.quit_after_scan {
        return Ok(());
    }
    let mut terminal = if let Some(terminal) = terminal {
        terminal
    } else {
        Curses::open(config.color)?
    };
    Browser::new(model, config, false).run(&mut terminal)
}
