//! Disk usage navigation and curses layout over compact directory spans.
use crate::{
    cli::{Color, Config, Graph, Shared, Sort},
    display,
    model::{EntryId, Kind, Model, NO_PARENT},
    os::{self, Curses, Key},
};
use std::{collections::HashMap, io};

enum Overlay {
    None,
    Help { tab: usize, offset: usize },
    Info { links: bool, selected: usize },
    Quit,
    Delete(EntryId),
    Message(String),
}
pub struct Browser {
    pub model: Model,
    pub config: Config,
    pub directory: u32,
    rows: Vec<Option<EntryId>>,
    selected: usize,
    top: usize,
    saved: HashMap<u32, (Option<Vec<u8>>, usize)>,
    overlay: Overlay,
}
impl Browser {
    pub fn new(model: Model, config: Config) -> Self {
        let mut browser = Self {
            model,
            config,
            directory: 0,
            rows: Vec::new(),
            selected: 0,
            top: 0,
            saved: HashMap::new(),
            overlay: Overlay::None,
        };
        browser.reload(None);
        browser
    }
    fn selected_entry(&self) -> Option<EntryId> {
        self.rows.get(self.selected).copied().flatten()
    }
    fn reload(&mut self, name: Option<&[u8]>) {
        self.rows = self
            .model
            .children(self.directory)
            .filter(|id| {
                let name = self.model.name(*id);
                let kind = self.model.entry(*id).kind();
                self.config.hidden
                    || !(name.starts_with(b".")
                        || name.ends_with(b"~")
                        || matches!(kind, Kind::Excluded | Kind::OtherFs | Kind::KernelFs))
            })
            .map(Some)
            .collect();
        let model = &self.model;
        let config = &self.config;
        self.rows
            .sort_unstable_by(|a, b| compare(model, config, a.unwrap(), b.unwrap()));
        if self.model.directories[self.directory as usize].parent != NO_PARENT {
            self.rows.insert(0, None);
        }
        self.selected = name
            .and_then(|name| {
                self.rows
                    .iter()
                    .position(|id| id.is_some_and(|id| self.model.name(id) == name))
            })
            .unwrap_or(self.selected.min(self.rows.len().saturating_sub(1)));
    }
    fn navigate(&mut self, directory: u32) {
        let selected = self
            .selected_entry()
            .map(|entry| self.model.name(entry).to_vec());
        self.saved.insert(self.directory, (selected, self.top));
        self.directory = directory;
        let (name, top) = self.saved.get(&directory).cloned().unwrap_or((None, 0));
        self.selected = 0;
        self.top = top;
        self.reload(name.as_deref());
    }
    fn sort(&mut self, field: Sort) {
        self.config.descending = if self.config.sort == field {
            !self.config.descending
        } else {
            field != Sort::Name
        };
        self.config.sort = field;
        let name = self
            .selected_entry()
            .map(|entry| self.model.name(entry).to_vec());
        self.reload(name.as_deref());
    }
    fn selection(&mut self, key: Key, page: usize) -> bool {
        move_selection(key, &mut self.selected, self.rows.len(), page)
    }
    fn links(&self) -> Vec<EntryId> {
        let Some(entry) = self.selected_entry() else {
            return Vec::new();
        };
        if self.model.entry(entry).kind() != Kind::Hardlink {
            return Vec::new();
        }
        let stat = self.model.metadata(entry);
        let mut links: Vec<_> = self
            .model
            .parts
            .iter()
            .flat_map(|part| &part.links)
            .filter(|link| (link.device, link.inode) == (stat.device, stat.inode))
            .map(|link| link.entry)
            .collect();
        links.sort_unstable_by_key(|entry| self.model.path(*entry));
        links
    }
    fn flag(&self, entry: EntryId) -> char {
        if let Some(id) = self.model.directory_id(entry) {
            let directory = &self.model.directories[id as usize];
            if directory.read_error {
                '!'
            } else if directory.descendant_error {
                '.'
            } else if directory.totals.items == 0 {
                'e'
            } else {
                ' '
            }
        } else {
            match self.model.entry(entry).kind() {
                Kind::Hardlink => 'H',
                Kind::Error => '!',
                Kind::Excluded => '<',
                Kind::OtherFs => '>',
                Kind::KernelFs => '^',
                Kind::Other => '@',
                _ => ' ',
            }
        }
    }
    pub fn draw(&mut self, terminal: &mut Curses) -> io::Result<()> {
        let (height, width) = terminal.dimensions();
        let page = height.saturating_sub(3).max(1);
        if self.selected < self.top {
            self.top = self.selected;
        }
        if self.selected >= self.top + page {
            self.top = self.selected + 1 - page;
        }
        terminal.clear();
        let banner = format!(
            "rcdu {} ~ Use the arrow keys to navigate, press ? for help",
            env!("CARGO_PKG_VERSION")
        );
        terminal.text(0, 0, &" ".repeat(width), true, false, 0)?;
        terminal.text(0, 0, &display::shorten(&banner, width), true, false, 0)?;
        let badge = if self.config.can_delete == Some(false) {
            "[readonly]"
        } else {
            ""
        };
        terminal.text(0, width.saturating_sub(badge.len()), badge, true, false, 0)?;
        terminal.text(1, 0, &"-".repeat(width), false, false, 0)?;
        let current = &self.model.directories[self.directory as usize];
        let path = display::escape(os::bytes(&self.model.path(current.entry)));
        terminal.text(
            1,
            3,
            &format!(" {} ", display::shorten(&path, width.saturating_sub(5))),
            false,
            false,
            if self.config.color == Color::Off {
                0
            } else {
                1
            },
        )?;
        let metric = |totals: crate::model::Totals| {
            if self.config.apparent {
                totals.apparent
            } else {
                totals.allocated
            }
        };
        let maximum = self
            .rows
            .iter()
            .flatten()
            .map(|entry| metric(self.model.totals(*entry)))
            .max()
            .unwrap_or(0);
        let total = metric(current.totals);
        let show_shared = self.config.shared != Shared::Off
            && self.rows.iter().flatten().any(|entry| {
                let totals = self.model.totals(*entry);
                if self.config.apparent {
                    totals.shared_apparent > 0
                } else {
                    totals.shared_allocated > 0
                }
            });
        for (row, entry) in self.rows.iter().skip(self.top).take(page).enumerate() {
            let selected = self.top + row == self.selected;
            if selected {
                terminal.text(row + 2, 0, &" ".repeat(width), true, false, 0)?;
            }
            let Some(entry) = entry else {
                terminal.text(
                    row + 2,
                    2,
                    &format!(
                        "{:width$}/..",
                        "",
                        width = if self.config.si { 9 } else { 10 }
                    ),
                    selected,
                    false,
                    0,
                )?;
                continue;
            };
            let totals = self.model.totals(*entry);
            let usage = metric(totals);
            let mut line = format!(
                "{} {}",
                self.flag(*entry),
                display::size(usage, self.config.si)
            );
            if show_shared {
                let shared = if self.config.apparent {
                    totals.shared_apparent
                } else {
                    totals.shared_allocated
                };
                let (letter, value) = if self.config.shared == Shared::Unique {
                    ('U', usage.saturating_sub(shared))
                } else {
                    ('S', shared)
                };
                line.push_str(&if value > 0 {
                    format!(" {letter} {}", display::size(value, self.config.si))
                } else {
                    " ".repeat(if self.config.si { 11 } else { 12 })
                });
            }
            let bars = (width / 7).max(10);
            if (self.config.graph || self.config.percent)
                && UnicodeWidthStr::width(line.as_str()) + 20 <= width
            {
                line.push_str(" [");
                if self.config.percent {
                    let tenths = ((u128::from(usage) * 1000 + u128::from(total) / 2)
                        / u128::from(total.max(1)))
                    .min(1000);
                    line.push_str(&format!("{:>3}.{}%", tenths / 10, tenths % 10));
                    if self.config.graph {
                        line.push(' ');
                    }
                }
                if self.config.graph {
                    let eighths = if maximum == 0 {
                        0
                    } else {
                        (u128::from(usage) * bars as u128 * 8 / u128::from(maximum)) as usize
                    };
                    for cell in 0..bars {
                        let fill = eighths.saturating_sub(cell * 8).min(8);
                        line.push(match self.config.graph_style {
                            Graph::Hash => {
                                if fill == 8 {
                                    '#'
                                } else {
                                    ' '
                                }
                            }
                            Graph::Half => {
                                if fill == 8 {
                                    '█'
                                } else if fill >= 4 {
                                    '▌'
                                } else {
                                    ' '
                                }
                            }
                            Graph::Eighth => [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'][fill],
                        });
                    }
                }
                line.push(']');
            }
            if self.config.items && UnicodeWidthStr::width(line.as_str()) + 10 <= width {
                if self.model.directory_id(*entry).is_some() {
                    line.push_str(&format!(" {:>6}", totals.items));
                } else {
                    line.push_str("       ");
                }
            }
            if self.config.mtime
                && self.config.scan.extended
                && UnicodeWidthStr::width(line.as_str()) + 37 <= width
            {
                let time = self
                    .model
                    .directory_id(*entry)
                    .and_then(|id| self.model.directories[id as usize].latest_mtime)
                    .or_else(|| {
                        self.model
                            .extended(*entry)
                            .filter(|stat| stat.present & 1 != 0)
                            .map(|stat| stat.mtime)
                    });
                line.push_str(&format!(
                    " {}",
                    time.map(os::timestamp)
                        .unwrap_or_else(|| "                 no mtime".into())
                ));
            }
            line.push(' ');
            line.push(
                if matches!(
                    self.model.entry(*entry).kind(),
                    Kind::Directory | Kind::OtherFs | Kind::KernelFs
                ) {
                    '/'
                } else {
                    ' '
                },
            );
            let remaining = width.saturating_sub(UnicodeWidthStr::width(line.as_str()) + 1);
            line.push_str(&display::shorten(
                &display::escape(self.model.name(*entry)),
                remaining,
            ));
            terminal.text(row + 2, 0, &line, selected, false, 0)?;
        }
        let totals = current.totals;
        let footer = format!(
            "{}Total disk usage: {}  {}Apparent size: {}   Items: {}",
            if self.config.apparent { ' ' } else { '*' },
            display::size(totals.allocated, self.config.si),
            if self.config.apparent { '*' } else { ' ' },
            display::size(totals.apparent, self.config.si),
            totals.items
        );
        terminal.text(height - 1, 0, &" ".repeat(width), true, false, 0)?;
        terminal.text(
            height - 1,
            0,
            &display::shorten(&footer, width),
            true,
            false,
            0,
        )?;
        self.draw_overlay(terminal)?;
        terminal.refresh()
    }
    fn draw_overlay(&self, terminal: &mut Curses) -> io::Result<()> {
        match &self.overlay {
            Overlay::None => Ok(()),
            Overlay::Delete(entry) => popup(
                terminal,
                "Confirm deletion",
                &[
                    display::escape(self.model.name(*entry)),
                    "Delete this item? (y/N)".into(),
                ],
            ),
            Overlay::Quit => popup(terminal, "Confirm quit", &["Really quit? (y/N)".into()]),
            Overlay::Message(message) => {
                let mut lines: Vec<String> = message.lines().map(str::to_owned).collect();
                lines.push("Press any key to continue".into());
                popup(terminal, "Message", &lines)
            }
            Overlay::Help { tab, offset } => {
                let text: Vec<String> = match tab {
                    0 => HELP_KEYS
                        .iter()
                        .skip(*offset)
                        .take(10)
                        .map(|text| (*text).into())
                        .collect(),
                    1 => [
                        "!  Error reading directory",
                        ".  Error reading a subdirectory",
                        "<  Excluded from statistics",
                        "e  Empty directory",
                        ">  Other filesystem",
                        "@  Symlink, socket or other nonregular entry",
                        "^  Excluded Linux pseudo-filesystem",
                        "H  Hard link",
                    ]
                    .map(Into::into)
                    .to_vec(),
                    _ => vec![
                        "Rust disk usage browser".into(),
                        format!("rcdu {}", env!("CARGO_PKG_VERSION")),
                        "Fresh Rust implementation by Aditya Gupta".into(),
                        "https://github.com/aditya-gupta-dev/rcdu".into(),
                    ],
                };
                let mut lines = vec![format!(
                    "[1 Keys{}] [2 Format{}] [3 About{}]",
                    if *tab == 0 { "*" } else { "" },
                    if *tab == 1 { "*" } else { "" },
                    if *tab == 2 { "*" } else { "" }
                )];
                lines.extend(text);
                lines.push("Press q to close".into());
                popup(terminal, "rcdu help", &lines)
            }
            Overlay::Info { links, selected } => {
                let Some(entry) = self.selected_entry() else {
                    return Ok(());
                };
                let stat = self.model.metadata(entry);
                let totals = self.model.totals(entry);
                let mut lines = Vec::new();
                if self.model.entry(entry).kind() == Kind::Hardlink {
                    lines.push("[1 Info] [2 Links]".into());
                }
                if *links {
                    for (index, link) in self
                        .links()
                        .iter()
                        .enumerate()
                        .skip(selected.saturating_sub(4))
                        .take(8)
                    {
                        lines.push(format!(
                            "{} {}",
                            if index == *selected { '>' } else { ' ' },
                            display::escape(os::bytes(&self.model.path(*link)))
                        ));
                    }
                } else {
                    lines.push(format!("Name: {}", display::escape(self.model.name(entry))));
                    if stat.present != 0 {
                        lines.push(format!(
                            "Mode: {}  UID: {} GID: {}",
                            display::permissions(stat.mode),
                            stat.uid,
                            stat.gid
                        ));
                    } else {
                        lines.push(format!("Type: {:?}", self.model.entry(entry).kind()));
                    }
                    if stat.present & 1 != 0 {
                        lines.push(format!("Last modified: {}", os::timestamp(stat.mtime)));
                    }
                    lines.push(format!(
                        "   Disk usage: {} ({})",
                        display::size(totals.allocated, self.config.si),
                        totals.allocated
                    ));
                    if totals.shared_allocated > 0 {
                        lines.push(format!("     > shared: {}", totals.shared_allocated));
                        lines.push(format!(
                            "     > unique: {}",
                            totals.allocated.saturating_sub(totals.shared_allocated)
                        ));
                    }
                    lines.push(format!(
                        "Apparent size: {} ({})",
                        display::size(totals.apparent, self.config.si),
                        totals.apparent
                    ));
                    if totals.shared_apparent > 0 {
                        lines.push(format!("     > shared: {}", totals.shared_apparent));
                        lines.push(format!(
                            "     > unique: {}",
                            totals.apparent.saturating_sub(totals.shared_apparent)
                        ));
                    }
                    if self.model.directory_id(entry).is_some() {
                        lines.push(format!("    Sub items: {}", totals.items));
                    }
                    if self.model.entry(entry).kind() == Kind::Hardlink {
                        lines.push(format!("Link count: {}  Inode: {}", stat.links, stat.inode));
                    }
                }
                lines.push("Press i to close this window".into());
                popup(terminal, "Item info", &lines)
            }
        }
    }
    fn key(&mut self, key: Key, terminal: &mut Curses) -> io::Result<bool> {
        let page = terminal.dimensions().0.saturating_sub(3).max(1);
        if let Overlay::Delete(entry) = self.overlay {
            self.overlay = Overlay::None;
            if matches!(key, Key::Character('y' | 'Y')) {
                if let Err(error) = self.delete_entry(entry, terminal) {
                    self.overlay = Overlay::Message(error.to_string());
                }
            }
            return Ok(false);
        }
        match &mut self.overlay {
            Overlay::Delete(_) => unreachable!(),
            Overlay::Quit => {
                let quit = matches!(key, Key::Character('y' | 'Y'));
                self.overlay = Overlay::None;
                return Ok(quit);
            }
            Overlay::Message(_) => {
                self.overlay = Overlay::None;
                return Ok(false);
            }
            Overlay::Help { tab, offset } => {
                let previous = *tab;
                match key {
                    Key::Character('1'..='3') => {
                        if let Key::Character(value) = key {
                            *tab = value as usize - '1' as usize;
                        }
                    }
                    Key::Left | Key::Character('h') => *tab = tab.saturating_sub(1),
                    Key::Right | Key::Character('l') => *tab = (*tab + 1).min(2),
                    Key::Down | Key::PageDown | Key::Character('j' | ' ') => {
                        if *tab == 0 {
                            *offset = (*offset + 1).min(HELP_KEYS.len().saturating_sub(10));
                        }
                    }
                    Key::Up | Key::PageUp | Key::Character('k') => {
                        *offset = offset.saturating_sub(1)
                    }
                    Key::Resize => {}
                    _ => self.overlay = Overlay::None,
                }
                if let Overlay::Help { tab, offset } = &mut self.overlay {
                    if *tab != previous {
                        *offset = 0;
                    }
                }
                return Ok(false);
            }
            Overlay::Info { .. } => {
                if matches!(key, Key::Character('i' | 'q') | Key::Escape) {
                    self.overlay = Overlay::None;
                    return Ok(false);
                }
                let links = self.links();
                if let Overlay::Info {
                    links: showing_links,
                    selected,
                } = &mut self.overlay
                {
                    if !links.is_empty() {
                        match key {
                            Key::Left | Key::Character('1' | 'h') => {
                                *showing_links = false;
                                return Ok(false);
                            }
                            Key::Right | Key::Character('2' | 'l') => {
                                *showing_links = true;
                                return Ok(false);
                            }
                            _ => {}
                        }
                        if *showing_links {
                            if move_selection(key, selected, links.len(), 5) {
                                return Ok(false);
                            }
                            if key == Key::Enter {
                                let entry = links[*selected];
                                let parent = self.model.entry(entry).parent;
                                let name = self.model.name(entry).to_vec();
                                self.overlay = Overlay::None;
                                self.navigate(parent);
                                self.reload(Some(&name));
                                return Ok(false);
                            }
                        }
                    }
                }
            }
            Overlay::None => {}
        }
        match key {
            Key::Character('q') => {
                if self.config.confirm_quit {
                    self.overlay = Overlay::Quit;
                } else {
                    return Ok(true);
                }
            }
            Key::Character('?') => self.overlay = Overlay::Help { tab: 0, offset: 0 },
            Key::Character('i') => {
                if self.selected_entry().is_some() {
                    self.overlay = Overlay::Info {
                        links: false,
                        selected: 0,
                    };
                }
            }
            Key::Right | Key::Enter | Key::Character('l') => {
                let next = if let Some(entry) = self.selected_entry() {
                    self.model.directory_id(entry)
                } else {
                    (self.model.directories[self.directory as usize].parent != NO_PARENT)
                        .then_some(self.model.directories[self.directory as usize].parent)
                };
                if let Some(next) = next {
                    self.navigate(next);
                    self.overlay = Overlay::None;
                }
            }
            Key::Left | Key::Backspace | Key::Character('h' | '<') => {
                let parent = self.model.directories[self.directory as usize].parent;
                if parent != NO_PARENT {
                    self.navigate(parent);
                    self.overlay = Overlay::None;
                }
            }
            Key::Character('n') => self.sort(Sort::Name),
            Key::Character('s') => self.sort(if self.config.apparent {
                Sort::Apparent
            } else {
                Sort::Allocated
            }),
            Key::Character('C') => self.sort(Sort::Items),
            Key::Character('M') => {
                if self.config.scan.extended {
                    self.sort(Sort::Mtime);
                }
            }
            Key::Character('a') => {
                self.config.apparent = !self.config.apparent;
                if matches!(self.config.sort, Sort::Allocated | Sort::Apparent) {
                    self.config.sort = if self.config.apparent {
                        Sort::Apparent
                    } else {
                        Sort::Allocated
                    };
                    self.reload(None);
                }
            }
            Key::Character('e') => {
                self.config.hidden = !self.config.hidden;
                self.reload(None);
                self.overlay = Overlay::None;
            }
            Key::Character('t') => {
                self.config.directories_first = !self.config.directories_first;
                self.reload(None);
            }
            Key::Character('c') => self.config.items = !self.config.items,
            Key::Character('m') => {
                if self.config.scan.extended {
                    self.config.mtime = !self.config.mtime;
                }
            }
            Key::Character('g') => {
                (self.config.graph, self.config.percent) =
                    match (self.config.graph, self.config.percent) {
                        (false, false) => (true, false),
                        (true, false) => (false, true),
                        (false, true) => (true, true),
                        (true, true) => (false, false),
                    }
            }
            Key::Character('u') => {
                self.config.shared = match self.config.shared {
                    Shared::Off => Shared::Shared,
                    Shared::Shared => Shared::Unique,
                    Shared::Unique => Shared::Off,
                }
            }
            Key::Character('r') => {
                if let Err(error) = self.refresh(terminal) {
                    self.overlay = Overlay::Message(error.to_string());
                }
            }
            Key::Character('b') => {
                if let Err(error) = self.shell(terminal) {
                    self.overlay = Overlay::Message(error.to_string());
                }
            }
            Key::Character('d' | 'D') => {
                if !self.config.can_delete.unwrap_or(true) {
                    self.overlay =
                        Overlay::Message("Deletion is disabled in configuration.".into());
                } else if let Some(entry) = self.selected_entry() {
                    if self.config.confirm_delete {
                        self.overlay = Overlay::Delete(entry);
                    } else if let Err(error) = self.delete_entry(entry, terminal) {
                        self.overlay = Overlay::Message(error.to_string());
                    }
                }
            }
            _ => {
                self.selection(key, page);
            }
        }
        Ok(false)
    }
    fn install_model(&mut self, model: Model) {
        let path = self
            .model
            .path(self.model.directories[self.directory as usize].entry);
        let name = self
            .selected_entry()
            .map(|entry| self.model.name(entry).to_vec());
        self.model = model;
        self.directory = self.model.directory_at_path(&path).unwrap_or(0);
        self.saved.clear();
        self.reload(name.as_deref());
    }
    fn refresh(&mut self, terminal: &mut Curses) -> io::Result<()> {
        if !self.config.can_refresh.unwrap_or(true) {
            return Err(io::Error::other("Refresh is disabled in configuration."));
        }
        let descriptor = os::open_model_directory(&self.model, self.directory)?;
        let path = self
            .model
            .path(self.model.directories[self.directory as usize].entry);
        let cancel = crate::scan::Cancellation::default();
        let trigger = cancel.clone();
        let mut failure = None;
        let mut last = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(3))
            .unwrap();
        let replacement = crate::scan::scan_descriptor(
            &path,
            descriptor,
            &self.config.scan,
            cancel,
            |progress| {
                if os::interrupted() {
                    trigger.cancel();
                }
                let result = (|| -> io::Result<()> {
                    if last.elapsed()
                        >= std::time::Duration::from_millis(if self.config.slow {
                            2000
                        } else {
                            100
                        })
                    {
                        terminal.clear();
                        terminal.text(0, 0, "rcdu — recalculating...", false, true, 0)?;
                        terminal.text(
                            2,
                            2,
                            &format!("{} items observed; q to abort", progress.entries),
                            false,
                            false,
                            0,
                        )?;
                        terminal.refresh()?;
                        last = std::time::Instant::now();
                    }
                    if matches!(terminal.key(0)?, Some(Key::Character('q'))) {
                        trigger.cancel();
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    failure = Some(error);
                    trigger.cancel();
                }
            },
        )?;
        if let Some(error) = failure {
            return Err(error);
        }
        let model = crate::mutation::rebuild(
            &self.model,
            Some((self.directory, &replacement)),
            &Default::default(),
            &Default::default(),
        )?;
        self.install_model(model);
        Ok(())
    }
    fn shell(&mut self, terminal: &mut Curses) -> io::Result<()> {
        if !self.config.can_shell.unwrap_or(true) {
            return Err(io::Error::other("Shell is disabled in configuration."));
        }
        let descriptor = os::open_model_directory(&self.model, self.directory)?;
        let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
        let mut command = std::process::Command::new(shell);
        os::command_in_directory(&mut command, descriptor);
        os::terminal_stdio(&mut command)?;
        run_command(terminal, &mut command)
    }
    fn delete_entry(&mut self, entry: EntryId, terminal: &mut Curses) -> io::Result<()> {
        let terminal = std::cell::RefCell::new(terminal);
        let tty_failure = std::cell::RefCell::new(None);
        let steps = std::cell::Cell::new(0usize);
        let last_draw = std::cell::Cell::new(
            std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(1))
                .unwrap(),
        );
        let outcome = crate::delete::remove_with_progress(
            &self.model,
            entry,
            |entry, error| {
                let result = (|| -> io::Result<crate::delete::ErrorAction> {
                    loop {
                        let mut terminal = terminal.borrow_mut();
                        terminal.clear();
                        popup(
                            &mut terminal,
                            "Deletion error",
                            &[
                                display::escape(self.model.name(entry)),
                                error.to_string(),
                                "a: abort, i: ignore, I: ignore all".into(),
                            ],
                        )?;
                        terminal.refresh()?;
                        if os::interrupted() {
                            return Ok(crate::delete::ErrorAction::Abort);
                        }
                        match terminal.key(100)? {
                            Some(Key::Character('i')) => {
                                return Ok(crate::delete::ErrorAction::Ignore);
                            }
                            Some(Key::Character('I')) => {
                                return Ok(crate::delete::ErrorAction::IgnoreAll);
                            }
                            Some(Key::Character('a' | 'q') | Key::Escape | Key::Enter) => {
                                return Ok(crate::delete::ErrorAction::Abort);
                            }
                            _ => {}
                        }
                    }
                })();
                match result {
                    Ok(action) => action,
                    Err(error) => {
                        *tty_failure.borrow_mut() = Some(error);
                        crate::delete::ErrorAction::Abort
                    }
                }
            },
            |entry| {
                let count = steps.get() + 1;
                steps.set(count);
                if count % 32 != 1
                    && last_draw.get().elapsed() < std::time::Duration::from_millis(100)
                {
                    return !os::interrupted();
                }
                let result = (|| -> io::Result<bool> {
                    let mut terminal = terminal.borrow_mut();
                    if last_draw.get().elapsed() >= std::time::Duration::from_millis(100) {
                        terminal.clear();
                        terminal.text(0, 0, "rcdu — deleting...", false, true, 0)?;
                        let width = terminal.dimensions().1.saturating_sub(4);
                        terminal.text(
                            2,
                            2,
                            &display::shorten(
                                &display::escape(os::bytes(&self.model.path(entry))),
                                width,
                            ),
                            false,
                            false,
                            0,
                        )?;
                        terminal.text(
                            4,
                            2,
                            &format!("{count} steps; q to stop"),
                            false,
                            false,
                            0,
                        )?;
                        terminal.refresh()?;
                        last_draw.set(std::time::Instant::now());
                    }
                    Ok(!matches!(
                        terminal.key(0)?,
                        Some(Key::Character('q' | '\u{3}'))
                    ))
                })();
                match result {
                    Ok(keep_going) => keep_going && !os::interrupted(),
                    Err(error) => {
                        *tty_failure.borrow_mut() = Some(error);
                        false
                    }
                }
            },
        );
        let model = crate::mutation::rebuild(&self.model, None, &outcome.removed, &outcome.links)?;
        self.install_model(model);
        if let Some(error) = tty_failure.into_inner() {
            return Err(error);
        }
        let removed_target = outcome.removed.contains(&entry);
        let refresh_error =
            if !removed_target && !outcome.aborted && self.config.can_refresh.unwrap_or(true) {
                self.refresh(terminal.into_inner()).err()
            } else {
                None
            };
        if outcome.aborted || !removed_target || outcome.failures > 0 || refresh_error.is_some() {
            let status = if outcome.aborted {
                "stopped"
            } else if removed_target {
                "finished with errors"
            } else {
                "incomplete"
            };
            self.overlay = Overlay::Message(format!(
                "Deletion {}\n{} removed; {} failed.{}{}",
                status,
                outcome.removed.len(),
                outcome.failures,
                if !removed_target {
                    "\nSelected item remains."
                } else {
                    ""
                },
                refresh_error
                    .map(|error| format!("\nRefresh failed: {error}"))
                    .unwrap_or_default()
            ));
        }
        Ok(())
    }
    pub fn run(&mut self, terminal: &mut Curses) -> io::Result<()> {
        loop {
            if os::interrupted() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "interrupted"));
            }
            self.draw(terminal)?;
            if let Some(key) = terminal.key(100)? {
                if self.key(key, terminal)? {
                    return Ok(());
                }
            }
        }
    }
}
use unicode_width::UnicodeWidthStr;
pub fn compare(
    model: &Model,
    config: &Config,
    left: EntryId,
    right: EntryId,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if config.directories_first {
        let group = (model.entry(right).kind() == Kind::Directory)
            .cmp(&(model.entry(left).kind() == Kind::Directory));
        if group != Ordering::Equal {
            return group;
        }
    }
    let a = model.totals(left);
    let b = model.totals(right);
    let order = match config.sort {
        Sort::Name => Ordering::Equal,
        Sort::Allocated => a
            .allocated
            .cmp(&b.allocated)
            .then(a.apparent.cmp(&b.apparent)),
        Sort::Apparent => a
            .apparent
            .cmp(&b.apparent)
            .then(a.allocated.cmp(&b.allocated)),
        Sort::Items => a
            .items
            .cmp(&b.items)
            .then(a.allocated.cmp(&b.allocated))
            .then(a.apparent.cmp(&b.apparent)),
        Sort::Mtime => {
            let time = |entry| {
                model
                    .directory_id(entry)
                    .and_then(|id| model.directories[id as usize].latest_mtime)
                    .or_else(|| {
                        model
                            .extended(entry)
                            .filter(|stat| stat.present & 1 != 0)
                            .map(|stat| stat.mtime)
                    })
            };
            time(left).cmp(&time(right))
        }
    }
    .then_with(|| {
        if config.natural {
            display::natural(model.name(left), model.name(right))
        } else {
            Ordering::Equal
        }
    })
    .then_with(|| model.name(left).cmp(model.name(right)));
    if config.descending {
        order.reverse()
    } else {
        order
    }
}
fn move_selection(key: Key, selected: &mut usize, length: usize, page: usize) -> bool {
    match key {
        Key::Down | Key::Character('j') => {
            *selected = (*selected + 1).min(length.saturating_sub(1))
        }
        Key::Up | Key::Character('k') => *selected = selected.saturating_sub(1),
        Key::Home => *selected = 0,
        Key::End => *selected = length.saturating_sub(1),
        Key::PageDown => *selected = (*selected + page).min(length.saturating_sub(1)),
        Key::PageUp => *selected = selected.saturating_sub(page),
        _ => return false,
    }
    true
}
pub fn popup(terminal: &mut Curses, title: &str, lines: &[String]) -> io::Result<()> {
    let (height, width) = terminal.dimensions();
    let box_width = 60.min(width);
    let box_height = (lines.len() + 4).min(height);
    let left = (width - box_width) / 2;
    let top = (height - box_height) / 2;
    let border = format!("+{}+", "-".repeat(box_width.saturating_sub(2)));
    terminal.text(top, left, &border, false, false, 0)?;
    terminal.text(
        top,
        left + 2,
        &display::shorten(title, box_width.saturating_sub(4)),
        false,
        true,
        0,
    )?;
    for row in 1..box_height.saturating_sub(1) {
        terminal.text(
            top + row,
            left,
            &format!("|{}|", " ".repeat(box_width.saturating_sub(2))),
            false,
            false,
            0,
        )?;
        if let Some(line) = lines.get(row.saturating_sub(2)).filter(|_| row >= 2) {
            terminal.text(
                top + row,
                left + 2,
                &display::shorten(line, box_width.saturating_sub(4)),
                false,
                false,
                0,
            )?;
        }
    }
    terminal.text(
        top + box_height.saturating_sub(1),
        left,
        &border,
        false,
        false,
        0,
    )
}
const HELP_KEYS: &[&str] = &[
    "arrows / hjkl  Navigate",
    "n   Sort by name",
    "s   Sort by size",
    "C   Sort by items",
    "M   Sort by mtime (-e)",
    "D/d Delete selected file or directory",
    "y   Confirm deletion; other key cancels",
    "q   Stop an active deletion",
    "a/i/I  Abort / ignore / ignore all on error",
    "t   Directories before files",
    "g   Percentage / graph",
    "u   Hard-link shared sizes",
    "a   Apparent / allocated bytes",
    "c   Child item counts",
    "m   Latest mtime (-e)",
    "e   Hidden / excluded entries",
    "i   Selected item information",
    "r   Recalculate directory",
    "b   Shell in current directory",
    "q   Quit rcdu",
];

fn run_command(terminal: &mut Curses, command: &mut std::process::Command) -> io::Result<()> {
    terminal.suspend()?;
    let result = command.status();
    terminal.resume()?;
    let status = result?;
    if !status.success() {
        return Err(io::Error::other(format!("Command exited with {status}")));
    }
    Ok(())
}
