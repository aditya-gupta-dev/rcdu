//! Session owner: signal guards, scanner progress and curses transitions.
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
    if !config.quit_after_scan && !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return Err(io::Error::other(
            "stdin is not a tty; use --quit-after-scan",
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
    let model = scan(&config, root, &mut terminal)?;
    if config.quit_after_scan {
        // Drop curses before writing the result, including when -2 was explicit.
        drop(terminal);
        let totals = model.directories[0].totals;
        println!(
            "{}: {} allocated bytes, {} apparent bytes, {} items",
            crate::display::escape(os::bytes(&model.path(model.directories[0].entry))),
            totals.allocated,
            totals.apparent,
            totals.items
        );
        return Ok(());
    }
    let mut terminal = if let Some(terminal) = terminal {
        terminal
    } else {
        Curses::open(config.color)?
    };
    Browser::new(model, config).run(&mut terminal)
}
