//! Browser policy operates on arena IDs. Indexed imports replace only the current listing.
use crate::{
    config::{Color, Config, GraphStyle, SharedColumn, SortField},
    delete::{self, ErrorChoice},
    format::binary::{self, BinaryRef, NO_REF},
    model::{EntryId, Kind, Model, NONE, Part},
    os,
    scan::{self, Cancellation},
    ui::{
        display,
        terminal::{Key, Terminal},
    },
};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io,
    path::PathBuf,
};

pub enum Source {
    Memory(Model),
    Indexed {
        reader: Box<binary::Reader<File>>,
        listing: Model,
        references: Vec<BinaryRef>,
        current: BinaryRef,
        ancestors: Vec<(BinaryRef, Vec<u8>, u64)>,
    },
}
impl Source {
    pub fn indexed(mut reader: binary::Reader<File>) -> io::Result<Self> {
        let current = reader.root;
        let (listing, references) = load_indexed(&mut reader, current, &[])?;
        Ok(Self::Indexed {
            reader: Box::new(reader),
            listing,
            references,
            current,
            ancestors: Vec::new(),
        })
    }
    fn model(&self) -> &Model {
        match self {
            Self::Memory(model) => model,
            Self::Indexed { listing, .. } => listing,
        }
    }
    fn lazy(&self) -> bool {
        matches!(self, Self::Indexed { .. })
    }
}
fn load_indexed<R: io::Read + io::Seek>(
    reader: &mut binary::Reader<R>,
    reference: BinaryRef,
    ancestors: &[(BinaryRef, Vec<u8>, u64)],
) -> io::Result<(Model, Vec<BinaryRef>)> {
    if ancestors
        .iter()
        .any(|(ancestor, _, _)| *ancestor == reference)
    {
        return Err(os::invalid("binary navigation cycle"));
    }
    let mut root_record = reader.get(reference)?;
    if !root_record.has_device {
        root_record.stat.device = ancestors.last().map_or(0, |(_, _, device)| *device);
    }
    if root_record.stat.kind != Kind::Directory {
        return Err(os::invalid("expected indexed directory"));
    }
    let mut part = Part::default();
    let root = part.add(0, &root_record.name, NONE, root_record.stat, true)?;
    part.directories[0].totals = root_record.totals;
    part.directories[0].read_error = root_record.read_error;
    part.directories[0].descendant_error = root_record.descendant_error;
    part.directories[0].latest_mtime =
        (root_record.stat.extended.present & 1 != 0).then_some(root_record.stat.extended.mtime);
    let mut model = Model {
        parts: vec![part],
        root,
    };
    let mut references = vec![reference];
    let mut next = root_record.child;
    let mut seen = HashSet::new();
    while next != NO_REF {
        if next == reference
            || ancestors.iter().any(|(ancestor, _, _)| *ancestor == next)
            || !seen.insert(next)
        {
            return Err(os::invalid("binary listing cycle"));
        }
        if seen.len() > 4_000_000 {
            return Err(os::invalid("binary listing resource limit exceeded"));
        }
        let mut record = reader.get(next)?;
        if !record.has_device {
            record.stat.device = root_record.stat.device;
        }
        let id = model.parts[0].add(0, &record.name, root, record.stat, true)?;
        model.entry_mut(id).next = model.directory(root).unwrap().first_child;
        model.directory_mut(root).first_child = id;
        if record.stat.kind == Kind::Directory {
            let directory = model.directory_mut(id);
            directory.totals = record.totals;
            directory.read_error = record.read_error;
            directory.descendant_error = record.descendant_error;
            directory.latest_mtime =
                (record.stat.extended.present & 1 != 0).then_some(record.stat.extended.mtime);
        }
        references.push(next);
        next = record.previous;
    }
    Ok((model, references))
}
pub fn compare(
    model: &Model,
    config: &Config,
    left: EntryId,
    right: EntryId,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let a = model.entry(left);
    let b = model.entry(right);
    if config.directories_first {
        let order = b.kind().directory_like().cmp(&a.kind().directory_like());
        if order != Ordering::Equal {
            return order;
        }
    }
    let a_total = model.totals(left);
    let b_total = model.totals(right);
    let order = match config.sort {
        SortField::Name => Ordering::Equal,
        SortField::Allocated => a_total
            .allocated
            .cmp(&b_total.allocated)
            .then(a_total.apparent.cmp(&b_total.apparent)),
        SortField::Apparent => a_total
            .apparent
            .cmp(&b_total.apparent)
            .then(a_total.allocated.cmp(&b_total.allocated)),
        SortField::Items => a_total
            .items
            .cmp(&b_total.items)
            .then(a_total.allocated.cmp(&b_total.allocated))
            .then(a_total.apparent.cmp(&b_total.apparent)),
        SortField::Mtime => {
            let time = |id| {
                model
                    .directory(id)
                    .and_then(|dir| dir.latest_mtime)
                    .or_else(|| {
                        model
                            .extended(id)
                            .filter(|ext| ext.present & 1 != 0)
                            .map(|ext| ext.mtime)
                    })
            };
            let x = time(left);
            let y = time(right);
            if x.is_some() != y.is_some() {
                return y.is_some().cmp(&x.is_some());
            }
            x.cmp(&y)
        }
    }
    .then_with(|| {
        if config.natural_sort {
            display::natural(model.name(left), model.name(right))
                .then(model.name(left).cmp(model.name(right)))
        } else {
            model.name(left).cmp(model.name(right))
        }
    });
    if config.descending {
        order.reverse()
    } else {
        order
    }
}
pub struct Browser {
    pub source: Source,
    pub config: Config,
    pub current: EntryId,
    pub rows: Vec<Option<EntryId>>,
    pub selected: usize,
    scroll: usize,
    saved: HashMap<PathBuf, (Vec<u8>, usize)>,
    imported: bool,
    message: Option<String>,
}
impl Browser {
    pub fn new(source: Source, config: Config, imported: bool) -> Self {
        let current = source.model().root;
        let mut browser = Self {
            source,
            config,
            current,
            rows: Vec::new(),
            selected: 0,
            scroll: 0,
            saved: HashMap::new(),
            imported,
            message: None,
        };
        browser.reload(None);
        browser
    }
    pub fn path(&self) -> PathBuf {
        match &self.source {
            Source::Memory(model) => model.path(self.current),
            Source::Indexed {
                listing, ancestors, ..
            } => {
                let mut path = if let Some((_, name, _)) = ancestors.first() {
                    os::byte_path(name)
                } else {
                    os::byte_path(listing.name(listing.root))
                };
                for (_, name, _) in ancestors.iter().skip(1) {
                    path.push(os::byte_path(name));
                }
                if !ancestors.is_empty() {
                    path.push(os::byte_path(listing.name(listing.root)));
                }
                path
            }
        }
    }
    fn save(&mut self) {
        let name = self
            .rows
            .get(self.selected)
            .copied()
            .flatten()
            .map(|id| self.source.model().name(id).to_vec())
            .unwrap_or_default();
        self.saved.insert(self.path(), (name, self.scroll));
    }
    fn has_parent(&self) -> bool {
        match &self.source {
            Source::Memory(model) => self.current != model.root,
            Source::Indexed { ancestors, .. } => !ancestors.is_empty(),
        }
    }
    fn reload(&mut self, preferred: Option<&[u8]>) {
        let model = self.source.model();
        self.rows.clear();
        if self.has_parent() {
            self.rows.push(None);
        }
        let mut children: Vec<_> = model
            .children(self.current)
            .filter(|id| {
                self.config.hidden
                    || (!model.name(*id).starts_with(b".")
                        && !model.name(*id).ends_with(b"~")
                        && !model.entry(*id).kind().excluded())
            })
            .collect();
        children.sort_unstable_by(|left, right| compare(model, &self.config, *left, *right));
        self.rows.extend(children.into_iter().map(Some));
        let stored = self.saved.get(&self.path());
        let name = preferred.or_else(|| stored.map(|(name, _)| name.as_slice()));
        self.selected = name
            .and_then(|name| {
                self.rows
                    .iter()
                    .position(|id| id.is_some_and(|id| model.name(id) == name))
            })
            .unwrap_or(0);
        self.scroll = stored.map_or(0, |(_, scroll)| *scroll);
    }
    fn enter(&mut self, id: EntryId) -> io::Result<()> {
        if self.source.model().directory(id).is_none() {
            return Ok(());
        }
        self.save();
        match &mut self.source {
            Source::Memory(_) => self.current = id,
            Source::Indexed {
                reader,
                listing,
                references,
                current,
                ancestors,
            } => {
                let target = references[id.slot()];
                let mut next_ancestors = ancestors.clone();
                next_ancestors.push((
                    *current,
                    listing.name(listing.root).to_vec(),
                    listing.observation(listing.root).device,
                ));
                let (new_listing, new_refs) = load_indexed(reader, target, &next_ancestors)?;
                *listing = new_listing;
                *references = new_refs;
                *current = target;
                *ancestors = next_ancestors;
                self.current = listing.root;
            }
        }
        self.reload(None);
        Ok(())
    }
    fn parent(&mut self) -> io::Result<()> {
        if !self.has_parent() {
            return Ok(());
        }
        self.save();
        let name = self.source.model().name(self.current).to_vec();
        match &mut self.source {
            Source::Memory(model) => self.current = model.parent(self.current),
            Source::Indexed {
                reader,
                listing,
                references,
                current,
                ancestors,
            } => {
                let (reference, _, _) = ancestors.last().unwrap();
                let reference = *reference;
                let (new_listing, new_refs) =
                    load_indexed(reader, reference, &ancestors[..ancestors.len() - 1])?;
                *listing = new_listing;
                *references = new_refs;
                *current = reference;
                ancestors.pop();
                self.current = listing.root;
            }
        }
        self.reload(Some(&name));
        Ok(())
    }
    fn draw(&mut self, terminal: &mut Terminal) -> io::Result<()> {
        let (width, height) = terminal.handle.dimensions()?;
        let page = usize::from(height).saturating_sub(3).max(1);
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        if self.selected < self.scroll {
            self.scroll = self.selected;
        }
        if self.selected >= self.scroll + page {
            self.scroll = self.selected.saturating_sub(page - 1);
        }
        let model = self.source.model();
        let mut lines = vec![
            format!(
                "rcdu {}  {}",
                env!("CARGO_PKG_VERSION"),
                if self.source.lazy() {
                    "[indexed file]"
                } else if self.imported {
                    "[imported]"
                } else {
                    ""
                }
            ),
            display::sanitize(os::path_bytes(&self.path())),
        ];
        let max = model
            .children(self.current)
            .map(|id| {
                let totals = model.totals(id);
                if self.config.apparent {
                    totals.apparent
                } else {
                    totals.allocated
                }
            })
            .max()
            .unwrap_or(0);
        let parent = model.totals(self.current);
        let total = if self.config.apparent {
            parent.apparent
        } else {
            parent.allocated
        };
        for row in self.rows.iter().skip(self.scroll).take(page) {
            if let Some(id) = row {
                let entry = model.entry(*id);
                let totals = model.totals(*id);
                let usage = if self.config.apparent {
                    totals.apparent
                } else {
                    totals.allocated
                };
                let flag = if entry.kind() == Kind::Error
                    || model.directory(*id).is_some_and(|dir| dir.read_error)
                {
                    '!'
                } else if model.directory(*id).is_some_and(|dir| dir.descendant_error) {
                    '.'
                } else if entry.kind() == Kind::OtherFs {
                    '>'
                } else if entry.kind() == Kind::KernelFs {
                    '^'
                } else if entry.kind() == Kind::Pattern {
                    '<'
                } else if model
                    .directory(*id)
                    .is_some_and(|dir| dir.first_child == NONE)
                {
                    'e'
                } else if entry.kind() == Kind::Hardlink {
                    'H'
                } else if entry.kind() == Kind::NonRegular {
                    '@'
                } else {
                    ' '
                };
                let mut line = format!("{flag} {} ", display::size(usage, self.config.si));
                if self.config.shared != SharedColumn::Off && width >= 70 {
                    let shared = if self.config.apparent {
                        totals.shared_apparent
                    } else {
                        totals.shared_allocated
                    };
                    line.push_str(&format!(
                        "{} ",
                        display::size(
                            if self.config.shared == SharedColumn::Unique {
                                usage.saturating_sub(shared)
                            } else {
                                shared
                            },
                            self.config.si
                        )
                    ));
                }
                if self.config.percent && width >= 50 {
                    let percent = if total == 0 {
                        0
                    } else {
                        (u128::from(usage) * 1000 / u128::from(total)) as u64
                    };
                    line.push_str(&format!("{:>3}.{}% ", percent / 10, percent % 10));
                }
                if self.config.graph && width >= 50 {
                    let segments = if max == 0 {
                        0
                    } else {
                        (u128::from(usage) * 80 / u128::from(max)) as usize
                    };
                    let filled = segments / 8;
                    line.push('[');
                    for index in 0..10 {
                        line.push(if index < filled {
                            match self.config.graph_style {
                                GraphStyle::Hash => '#',
                                _ => '█',
                            }
                        } else if index == filled && segments % 8 != 0 {
                            match self.config.graph_style {
                                GraphStyle::Hash => ' ',
                                GraphStyle::Half => {
                                    if segments % 8 >= 4 {
                                        '▌'
                                    } else {
                                        ' '
                                    }
                                }
                                GraphStyle::Eighth => {
                                    [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'][segments % 8]
                                }
                            }
                        } else {
                            ' '
                        });
                    }
                    line.push_str("] ");
                }
                if self.config.itemcount && width >= 70 {
                    line.push_str(&format!("{:>7} ", totals.items));
                }
                if self.config.mtime && self.config.scan.extended && width >= 90 {
                    let time = model
                        .directory(*id)
                        .and_then(|dir| dir.latest_mtime)
                        .or_else(|| {
                            model
                                .extended(*id)
                                .filter(|ext| ext.present & 1 != 0)
                                .map(|ext| ext.mtime)
                        });
                    line.push_str(&format!(
                        "{} ",
                        time.map_or_else(|| "?".into(), os::timestamp)
                    ));
                }
                line.push(if entry.kind().directory_like() {
                    '/'
                } else {
                    ' '
                });
                line.push_str(&display::sanitize(model.name(*id)));
                lines.push(line);
            } else {
                lines.push("  /..".into());
            }
        }
        while lines.len() < height.saturating_sub(1) as usize {
            lines.push(String::new());
        }
        lines.push(self.message.clone().unwrap_or_else(|| {
            format!(
                "Total {}  apparent {}  {} items | ? help",
                display::size(parent.allocated, self.config.si),
                display::size(parent.apparent, self.config.si),
                parent.items
            )
        }));
        terminal.frame(
            &lines,
            (!self.rows.is_empty()).then_some(2 + self.selected - self.scroll),
            self.config.color != Color::Off,
        )
    }
    fn toggle_sort(&mut self, field: SortField, descending: bool) {
        self.save();
        if self.config.sort == field {
            self.config.descending = !self.config.descending;
        } else {
            self.config.sort = field;
            self.config.descending = descending;
        }
        self.reload(None);
    }
    fn refresh(&mut self, terminal: &mut Terminal) -> io::Result<()> {
        if self.source.lazy() || !self.config.can_refresh.unwrap_or(!self.imported) {
            return Err(io::Error::other("refresh disabled for this source"));
        }
        self.refresh_observations(terminal)
    }
    fn refresh_observations(&mut self, terminal: &mut Terminal) -> io::Result<()> {
        os::validate_action_root(&self.source.model().path(self.source.model().root))?;
        self.save();
        let path = self.path();
        let relative_names = {
            let model = self.source.model();
            let mut names = Vec::new();
            let mut id = self.current;
            while id != model.root {
                names.push(model.name(id).to_vec());
                id = model.parent(id);
            }
            names.reverse();
            names
        };
        let cancel = scan::Cancellation::default();
        let mut ui_error = None;
        let mut last = std::time::Instant::now();
        let replacement =
            scan::scan_with_progress(&path, &self.config.scan, cancel.clone(), |progress| {
                if last.elapsed()
                    >= std::time::Duration::from_millis(if self.config.slow_ui {
                        2000
                    } else {
                        100
                    })
                {
                    if let Err(error) = terminal.frame(
                        &[
                            "rcdu — refreshing".into(),
                            display::sanitize(os::path_bytes(&path)),
                            format!("{} entries observed; q: abort refresh", progress.entries),
                        ],
                        None,
                        false,
                    ) {
                        ui_error = Some(error);
                        cancel.cancel();
                    }
                    last = std::time::Instant::now();
                }
                match terminal.key(0) {
                    Ok(Some(Key::Character('q' | '\u{3}'))) => cancel.cancel(),
                    Err(error) => {
                        ui_error = Some(error);
                        cancel.cancel();
                    }
                    _ => {}
                }
            });
        if let Some(error) = ui_error {
            return Err(error);
        }
        let replacement = replacement?;
        if let Source::Memory(model) = &mut self.source {
            let fresh = delete::replace_subtree(model, self.current, &replacement)?;
            *model = fresh;
            self.current = model.root;
            for name in relative_names {
                if let Some(id) = model
                    .children(self.current)
                    .find(|id| model.name(*id) == name && model.directory(*id).is_some())
                {
                    self.current = id;
                } else {
                    break;
                }
            }
        }
        self.reload(None);
        Ok(())
    }
    fn delete(&mut self, terminal: &mut Terminal) -> io::Result<()> {
        if self.source.lazy() || !self.config.can_delete.unwrap_or(!self.imported) {
            return Err(io::Error::other("deletion disabled for this source"));
        }
        let Some(target) = self.rows.get(self.selected).copied().flatten() else {
            return Ok(());
        };
        os::validate_action_root(&self.source.model().path(self.source.model().root))?;
        let path = self.source.model().path(target);
        let next_name = self
            .rows
            .get(self.selected + 1)
            .or_else(|| {
                self.selected
                    .checked_sub(1)
                    .and_then(|index| self.rows.get(index))
            })
            .copied()
            .flatten()
            .map(|id| self.source.model().name(id).to_vec());
        if self.config.confirm_delete {
            match terminal.confirm(
                &format!("Delete {}?", display::sanitize(os::path_bytes(&path))),
                true,
            )? {
                None => return Ok(()),
                Some(all) => {
                    if all {
                        self.config.confirm_delete = false;
                    }
                }
            }
        }
        self.save();
        if let Some(command) = &self.config.delete_command {
            let mut command = command.clone();
            command.extend_from_slice(b" \"$NCDU_DELETE_PATH\"");
            let status = terminal.handle.run_command(
                &[b"/bin/sh".to_vec(), b"-c".to_vec(), command],
                None,
                Some(&path),
            )?;
            if delete::confirmed_missing(&path)? {
                if let Source::Memory(model) = &mut self.source {
                    delete::detach(model, target)?;
                    model.recount();
                }
            }
            if !status.success() {
                self.message = Some(format!("custom command returned {status}"));
            }
        } else if let Source::Memory(model) = &mut self.source {
            let report =
                delete::remove_with_events(model, target, &Cancellation::default(), |event| {
                    let (path, error) = match event {
                        delete::DeleteEvent::Error { path, error } => (path, error),
                        delete::DeleteEvent::Progress {
                            path,
                            removed,
                            failed,
                        } => {
                            if terminal
                                .frame(
                                    &[
                                        format!(
                                            "Deleting {}",
                                            display::sanitize(os::path_bytes(path))
                                        ),
                                        format!("{removed} removed; {failed} failures; q: abort"),
                                    ],
                                    None,
                                    false,
                                )
                                .is_err()
                            {
                                return ErrorChoice::Abort;
                            }
                            return match terminal.key(0) {
                                Ok(Some(Key::Character('q' | '\u{3}'))) | Err(_) => {
                                    ErrorChoice::Abort
                                }
                                _ => ErrorChoice::Ignore,
                            };
                        }
                    };
                    let lines = vec![
                        format!(
                            "Delete {}: {error}",
                            display::sanitize(os::path_bytes(path))
                        ),
                        "a: abort (default), i: ignore, I: ignore all".into(),
                    ];
                    loop {
                        if terminal.frame(&lines, None, false).is_err() || os::interrupted() {
                            return ErrorChoice::Abort;
                        }
                        match terminal.key(100) {
                            Ok(Some(Key::Character('i'))) => return ErrorChoice::Ignore,
                            Ok(Some(Key::Character('I'))) => return ErrorChoice::IgnoreAll,
                            Ok(Some(Key::Enter | Key::Escape | Key::Character('a' | 'q')))
                            | Err(_) => {
                                return ErrorChoice::Abort;
                            }
                            _ => {}
                        }
                    }
                })?;
            self.message = Some(format!(
                "Removed {} entries; {} failures{}",
                report.removed,
                report.failed,
                if report.aborted { "; aborted" } else { "" }
            ));
        }
        self.reload(next_name.as_deref());
        if let Err(error) = self.refresh_observations(terminal) {
            self.message = Some(format!("Mutation applied; refresh failed: {error}"));
        }
        self.reload(None);
        Ok(())
    }
    fn details(&self, id: EntryId) -> (Vec<String>, Vec<(std::path::PathBuf, EntryId)>) {
        let model = self.source.model();
        let entry = model.entry(id);
        let totals = model.totals(id);
        let stat = model.observation(id);
        let mut information = vec![
            format!("Details: {}", display::sanitize(model.name(id))),
            format!(
                "Kind {:?}; allocated {}; apparent {}",
                entry.kind(),
                display::size(totals.allocated, self.config.si),
                display::size(totals.apparent, self.config.si)
            ),
            format!(
                "Own allocated {}; own apparent {}",
                entry.allocated(),
                entry.apparent
            ),
            format!(
                "{} descendants; shared {}; unique {}",
                totals.items,
                display::size(totals.shared_allocated, self.config.si),
                display::size(
                    totals.allocated.saturating_sub(totals.shared_allocated),
                    self.config.si
                )
            ),
        ];
        if let Some(ext) = model.extended(id) {
            if ext.present & 8 != 0 {
                information.push(display::mode(ext.mode));
            }
            if ext.present & 2 != 0 {
                information.push(format!("Owner {}", os::account(ext.uid, false)));
            }
            if ext.present & 4 != 0 {
                information.push(format!("Group {}", os::account(ext.gid, true)));
            }
            if ext.present & 1 != 0 {
                information.push(format!("Modified {}", os::timestamp(ext.mtime)));
            }
        }
        if entry.kind() == Kind::Hardlink {
            information.push(format!(
                "Device {}; inode {}; {} links",
                stat.device, stat.inode, stat.links
            ));
        }
        information.push("1: information, 2: hardlinks; j/k: select; i/q/Escape: close".into());
        let mut links = Vec::new();
        if entry.kind() == Kind::Hardlink && !self.source.lazy() {
            for part in &model.parts {
                for link in &part.hardlinks {
                    if link.key.device == stat.device && link.key.inode == stat.inode {
                        links.push((model.path(link.entry), link.entry));
                    }
                }
            }
            links.sort_by(|a, b| os::path_bytes(&a.0).cmp(os::path_bytes(&b.0)));
        }
        (information, links)
    }
    fn info(&mut self, terminal: &mut Terminal) -> io::Result<()> {
        let Some(id) = self.rows.get(self.selected).copied().flatten() else {
            return Ok(());
        };
        let (mut information, mut links) = self.details(id);
        let mut link_tab = false;
        let mut cursor: usize = 0;
        let mut offset: usize = 0;
        loop {
            let (_, height) = terminal.handle.dimensions()?;
            let page = usize::from(height).saturating_sub(2).max(1);
            if cursor < offset {
                offset = cursor;
            }
            if cursor >= offset + page {
                offset = cursor - page + 1;
            }
            let lines = if link_tab {
                let mut lines = vec!["Hardlinks (Enter jumps; q closes)".into()];
                if self.source.lazy() {
                    lines.push("Hardlink navigation unavailable in indexed mode".into());
                } else {
                    lines.extend(
                        links
                            .iter()
                            .skip(offset)
                            .take(page)
                            .map(|(path, _)| display::sanitize(os::path_bytes(path))),
                    );
                }
                lines
            } else {
                information
                    .iter()
                    .skip(offset)
                    .take(page)
                    .cloned()
                    .collect()
            };
            terminal.frame(
                &lines,
                link_tab.then_some(1 + cursor - offset),
                self.config.color != Color::Off,
            )?;
            if os::interrupted() {
                return Ok(());
            }
            match terminal.key(100)? {
                Some(Key::Character('q' | 'i') | Key::Escape) => break,
                Some(Key::Character('2') | Key::Right) => {
                    link_tab = true;
                    offset = 0;
                }
                Some(Key::Character('1') | Key::Left) => {
                    link_tab = false;
                    offset = 0;
                }
                Some(Key::Up | Key::Character('k')) => {
                    if link_tab {
                        cursor = cursor.saturating_sub(1);
                    } else {
                        let next = self.selected.saturating_sub(1);
                        if let Some(id) = self.rows.get(next).copied().flatten() {
                            self.selected = next;
                            (information, links) = self.details(id);
                            offset = 0;
                        }
                    }
                }
                Some(Key::Down | Key::Character('j')) => {
                    if link_tab {
                        cursor = (cursor + 1).min(links.len().saturating_sub(1));
                    } else {
                        let next = (self.selected + 1).min(self.rows.len().saturating_sub(1));
                        if let Some(id) = self.rows.get(next).copied().flatten() {
                            self.selected = next;
                            (information, links) = self.details(id);
                            offset = 0;
                        }
                    }
                }
                Some(Key::Enter) if link_tab && !self.source.lazy() && !links.is_empty() => {
                    let id = links[cursor].1;
                    let name = self.source.model().name(id).to_vec();
                    self.save();
                    self.current = self.source.model().parent(id);
                    self.reload(Some(&name));
                    break;
                }
                Some(key) => {
                    terminal.queue_key(key);
                    break;
                }
                None => {}
            }
        }
        Ok(())
    }
    pub fn run(&mut self, terminal: &mut Terminal) -> io::Result<()> {
        terminal.color = self.config.color;
        loop {
            if os::interrupted() {
                break;
            }
            self.draw(terminal)?;
            let Some(key) = terminal.key(100)? else {
                continue;
            };
            if self.message.take().is_some() {
                continue;
            }
            let result = match key {
                Key::Character('q' | '\u{3}') => {
                    if !self.config.confirm_quit || terminal.confirm("Quit rcdu?", false)?.is_some()
                    {
                        break;
                    }
                    Ok(())
                }
                Key::Down | Key::Character('j') => {
                    self.selected = (self.selected + 1).min(self.rows.len().saturating_sub(1));
                    Ok(())
                }
                Key::Up | Key::Character('k') => {
                    self.selected = self.selected.saturating_sub(1);
                    Ok(())
                }
                Key::Home => {
                    self.selected = 0;
                    Ok(())
                }
                Key::End => {
                    self.selected = self.rows.len().saturating_sub(1);
                    Ok(())
                }
                Key::PageDown | Key::PageUp => {
                    let (_, height) = terminal.handle.dimensions()?;
                    let page = usize::from(height).saturating_sub(3).max(1);
                    self.selected = if key == Key::PageDown {
                        (self.selected + page).min(self.rows.len().saturating_sub(1))
                    } else {
                        self.selected.saturating_sub(page)
                    };
                    Ok(())
                }
                Key::Right | Key::Enter | Key::Character('l') => {
                    if let Some(Some(id)) = self.rows.get(self.selected) {
                        self.enter(*id)
                    } else {
                        self.parent()
                    }
                }
                Key::Left | Key::Backspace | Key::Character('h' | '<') => self.parent(),
                Key::Character('n') => {
                    self.toggle_sort(SortField::Name, false);
                    Ok(())
                }
                Key::Character('s') => {
                    self.toggle_sort(
                        if self.config.apparent {
                            SortField::Apparent
                        } else {
                            SortField::Allocated
                        },
                        true,
                    );
                    Ok(())
                }
                Key::Character('C') => {
                    self.toggle_sort(SortField::Items, true);
                    Ok(())
                }
                Key::Character('M') if self.config.scan.extended => {
                    self.toggle_sort(SortField::Mtime, true);
                    Ok(())
                }
                Key::Character('a') => {
                    self.save();
                    self.config.apparent = !self.config.apparent;
                    if matches!(self.config.sort, SortField::Apparent | SortField::Allocated) {
                        self.config.sort = if self.config.apparent {
                            SortField::Apparent
                        } else {
                            SortField::Allocated
                        };
                    }
                    self.reload(None);
                    Ok(())
                }
                Key::Character('e') => {
                    self.save();
                    self.config.hidden = !self.config.hidden;
                    self.reload(None);
                    Ok(())
                }
                Key::Character('t') => {
                    self.save();
                    self.config.directories_first = !self.config.directories_first;
                    self.reload(None);
                    Ok(())
                }
                Key::Character('c') => {
                    self.config.itemcount = !self.config.itemcount;
                    Ok(())
                }
                Key::Character('m') => {
                    self.config.mtime = !self.config.mtime;
                    Ok(())
                }
                Key::Character('g') => {
                    let next = match (self.config.graph, self.config.percent) {
                        (false, false) => (true, false),
                        (true, false) => (false, true),
                        (false, true) => (true, true),
                        (true, true) => (false, false),
                    };
                    self.config.graph = next.0;
                    self.config.percent = next.1;
                    Ok(())
                }
                Key::Character('u') => {
                    self.config.shared = match self.config.shared {
                        SharedColumn::Off => SharedColumn::Shared,
                        SharedColumn::Shared => SharedColumn::Unique,
                        SharedColumn::Unique => SharedColumn::Off,
                    };
                    Ok(())
                }
                Key::Character('i') => self.info(terminal),
                Key::Character('r') => self.refresh(terminal),
                Key::Character('d') => self.delete(terminal),
                Key::Character('b') => {
                    if self.config.can_shell.unwrap_or(!self.imported) {
                        os::validate_action_root(
                            &self.source.model().path(self.source.model().root),
                        )?;
                        let shell = std::env::var_os("NCDU_SHELL")
                            .or_else(|| std::env::var_os("SHELL"))
                            .unwrap_or_else(|| "/bin/sh".into());
                        terminal
                            .handle
                            .run_command(
                                &[os::argument_bytes(&shell).to_vec()],
                                Some(&self.path()),
                                None,
                            )
                            .map(|_| ())
                    } else {
                        Err(io::Error::other("shell disabled for this source"))
                    }
                }
                Key::Character('?') => {
                    self.help(terminal)?;
                    Ok(())
                }
                _ => Ok(()),
            };
            if let Err(error) = result {
                self.message = Some(error.to_string());
            }
        }
        Ok(())
    }
    fn help(&self, terminal: &mut Terminal) -> io::Result<()> {
        let tabs: [&[&str]; 3] = [
            &[
                "Navigation: arrows / hjkl; Enter enters; Home/End/PageUp/PageDown",
                "n: sort name; s: size; C: descendants; M: latest mtime",
                "a: apparent/allocated; e: hidden entries; t: directories first",
                "c: item counts; m: mtime; g: graph/percent; u: shared/unique",
                "i: details / hardlinks; d: delete; r: refresh; b: shell; q: quit",
            ],
            &[
                "Disk usage = allocated 512-byte blocks; apparent = logical size",
                "Directories include their own metadata sizes",
                "Hardlinks count once in each containing ancestor",
                "Shared = links exist outside directory; unique = total - shared",
                "!: read error; .: descendant error; H: hardlink; @: special",
                "<: excluded; >: other filesystem; ^: kernel filesystem; e: empty directory",
            ],
            &[
                "rcdu 0.1.0 — Linux Rust rewrite",
                "Compatible with ncdu 2.9 JSON and EX1 indexed binary",
                "ncdu by Yorhel; natural sorting by Martin Pool",
                "MIT license; see LICENSE and docs/credits.md",
            ],
        ];
        let mut tab = 0usize;
        let mut offset = 0usize;
        loop {
            let mut lines = vec![format!(
                "Help [1 keys] [2 accounting] [3 about] — tab {}",
                tab + 1
            )];
            lines.extend(tabs[tab].iter().skip(offset).map(|line| (*line).to_owned()));
            terminal.frame(&lines, None, self.config.color != Color::Off)?;
            if os::interrupted() {
                break;
            }
            match terminal.key(100)? {
                Some(Key::Character('q' | '?' | 'i') | Key::Escape) => break,
                Some(Key::Character(number @ '1'..='3')) => {
                    tab = (number as usize) - ('1' as usize);
                    offset = 0;
                }
                Some(Key::Right | Key::Character('l')) => {
                    tab = (tab + 1).min(2);
                    offset = 0;
                }
                Some(Key::Left | Key::Character('h')) => {
                    tab = tab.saturating_sub(1);
                    offset = 0;
                }
                Some(Key::Down | Key::Character('j') | Key::PageDown) => {
                    offset = (offset + 1).min(tabs[tab].len() - 1)
                }
                Some(Key::Up | Key::Character('k') | Key::PageUp) => {
                    offset = offset.saturating_sub(1)
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn indexed_navigation_inherits_device_and_rejects_ancestor_cycles() {
        use std::io::Cursor;
        let bytes = include_bytes!("../tests/fixtures/inherited-device.ex1");
        let mut reader = binary::Reader::open(Cursor::new(bytes)).unwrap();
        let root = reader.root;
        let (listing, refs) = load_indexed(&mut reader, root, &[]).unwrap();
        let child = listing.children(listing.root).next().unwrap();
        let child_ref = refs[child.slot()];
        assert!(!reader.get(child_ref).unwrap().has_device);
        let ancestors = vec![(root, b"/root".to_vec(), 42)];
        let (nested, _) = load_indexed(&mut reader, child_ref, &ancestors).unwrap();
        assert_eq!(nested.observation(nested.root).device, 42);
        let file = nested.children(nested.root).next().unwrap();
        assert_eq!(nested.observation(file).device, 42);
        assert_eq!(nested.observation(file).links, 2);
        assert!(load_indexed(&mut reader, root, &ancestors).is_err());
    }
}
