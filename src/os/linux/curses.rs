//! Single-owner ncursesw adapter. ABI constants are from the installed Linux ncurses headers.
use std::{
    ffi::{CString, c_void},
    io,
    marker::PhantomData,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    rc::Rc,
    sync::Arc,
};
type Window = c_void;
type Screen = c_void;
#[link(name = "ncursesw")]
unsafe extern "C" {
    static mut stdscr: *mut Window;
    fn newterm(
        kind: *const libc::c_char,
        output: *mut libc::FILE,
        input: *mut libc::FILE,
    ) -> *mut Screen;
    fn delscreen(screen: *mut Screen);
    fn endwin() -> libc::c_int;
    fn cbreak() -> libc::c_int;
    fn noecho() -> libc::c_int;
    fn keypad(window: *mut Window, enabled: bool) -> libc::c_int;
    fn curs_set(visibility: libc::c_int) -> libc::c_int;
    fn set_escdelay(delay: libc::c_int) -> libc::c_int;
    fn getmaxx(window: *const Window) -> libc::c_int;
    fn getmaxy(window: *const Window) -> libc::c_int;
    fn werase(window: *mut Window) -> libc::c_int;
    fn wrefresh(window: *mut Window) -> libc::c_int;
    fn wtimeout(window: *mut Window, milliseconds: libc::c_int);
    fn wgetch(window: *mut Window) -> libc::c_int;
    fn wattr_set(
        window: *mut Window,
        attributes: libc::c_uint,
        pair: libc::c_short,
        options: *mut c_void,
    ) -> libc::c_int;
    fn mvwaddnstr(
        window: *mut Window,
        y: libc::c_int,
        x: libc::c_int,
        text: *const libc::c_char,
        n: libc::c_int,
    ) -> libc::c_int;
    fn start_color() -> libc::c_int;
    fn use_default_colors() -> libc::c_int;
    fn init_pair(
        number: libc::c_short,
        foreground: libc::c_short,
        background: libc::c_short,
    ) -> libc::c_int;
    fn def_prog_mode() -> libc::c_int;
    fn reset_prog_mode() -> libc::c_int;
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Character(char),
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Backspace,
    Enter,
    Escape,
    Resize,
}
type Hook = Arc<dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync>;
struct PanicRestore {
    previous: Option<Hook>,
}
impl Drop for PanicRestore {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let _ = std::panic::take_hook();
        let previous = self.previous.take().unwrap();
        std::panic::set_hook(Box::new(move |info| previous(info)));
    }
}
pub struct Curses {
    screen: *mut Screen,
    window: *mut Window,
    stream: *mut libc::FILE,
    saved: libc::termios,
    descriptor: Arc<OwnedFd>,
    _panic: PanicRestore,
    _owner: PhantomData<Rc<()>>,
}
impl Curses {
    pub fn open(color: crate::cli::Color) -> io::Result<Self> {
        // SAFETY: empty static string selects the user's locale; setlocale called on session owner.
        unsafe {
            libc::setlocale(libc::LC_ALL, c"".as_ptr());
        }
        // SAFETY: static terminated strings; successful FILE is closed by this adapter.
        let stream = unsafe { libc::fopen(c"/dev/tty".as_ptr(), c"r+".as_ptr()) };
        if stream.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: stream is a live FILE and fileno returns its descriptor.
        let fd = unsafe { libc::fileno(stream) };
        // SAFETY: termios is writable aligned zero-initialized storage.
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: fd is live and saved writable; fcntl duplicates descriptor without sharing ownership.
        let (status, duplicate) = unsafe {
            (
                libc::tcgetattr(fd, &mut saved),
                libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3),
            )
        };
        if status != 0 || duplicate < 0 {
            let error = io::Error::last_os_error();
            // SAFETY: FILE ownership remains local; any successful duplicate is not transferred.
            unsafe {
                libc::fclose(stream);
                if duplicate >= 0 {
                    libc::close(duplicate);
                }
            }
            return Err(error);
        }
        // SAFETY: successful fcntl returns a fresh descriptor.
        let descriptor = Arc::new(unsafe { OwnedFd::from_raw_fd(duplicate) });
        // SAFETY: FILE remains live for the lifetime of the screen. Null type uses TERM.
        let screen = unsafe { newterm(std::ptr::null(), stream, stream) };
        if screen.is_null() {
            // SAFETY: initialization failed, so no screen retains stream.
            unsafe {
                libc::fclose(stream);
            }
            return Err(io::Error::other("cannot initialize ncurses; check TERM"));
        }
        // SAFETY: newterm selected this screen and populated its standard window; only owner uses it.
        let window = unsafe { stdscr };
        let previous: Hook = Arc::from(std::panic::take_hook());
        let captured = Arc::clone(&previous);
        let restore = Arc::clone(&descriptor);
        std::panic::set_hook(Box::new(move |info| {
            // SAFETY: duplicate remains owned by hook; restore does not call ncurses from worker threads.
            unsafe {
                libc::tcsetattr(restore.as_raw_fd(), libc::TCSANOW, &saved);
                let sequence = b"\x1b[0m\x1b[?25h\x1b[?1049l";
                libc::write(
                    restore.as_raw_fd(),
                    sequence.as_ptr().cast(),
                    sequence.len(),
                );
            }
            captured(info);
        }));
        let terminal = Self {
            screen,
            window,
            stream,
            saved,
            descriptor,
            _panic: PanicRestore {
                previous: Some(previous),
            },
            _owner: PhantomData,
        };
        // SAFETY: initialized active screen is used only on its owner thread.
        unsafe {
            if cbreak() < 0 || noecho() < 0 || keypad(window, true) < 0 {
                return Err(io::Error::other("ncurses setup failed"));
            }
            curs_set(0);
            set_escdelay(25);
            wtimeout(window, 100);
            if color != crate::cli::Color::Off {
                start_color();
                use_default_colors();
                let background = if color == crate::cli::Color::DarkBackground {
                    0
                } else {
                    -1
                };
                init_pair(1, 6, background);
                init_pair(2, 3, background);
                init_pair(3, 7, 4);
            }
        }
        Ok(terminal)
    }
    pub fn dimensions(&self) -> (usize, usize) {
        // SAFETY: standard window belongs to this live screen and owner thread.
        unsafe {
            (
                getmaxy(self.window).max(1) as usize,
                getmaxx(self.window).max(1) as usize,
            )
        }
    }
    pub fn clear(&mut self) {
        // SAFETY: live standard window, single owner.
        unsafe {
            werase(self.window);
        }
    }
    pub fn text(
        &mut self,
        row: usize,
        column: usize,
        text: &str,
        selected: bool,
        bold: bool,
        pair: i16,
    ) -> io::Result<()> {
        let (height, width) = self.dimensions();
        if row >= height || column >= width {
            return Ok(());
        }
        let text = CString::new(text).map_err(|_| io::Error::other("NUL in displayed text"))?;
        let attributes = (if selected { 1 << 18 } else { 0 }) | (if bold { 1 << 21 } else { 0 });
        // SAFETY: text is terminated, window live, coordinates bounded. ncurses clips to its width.
        unsafe {
            wattr_set(self.window, attributes, pair, std::ptr::null_mut());
            mvwaddnstr(
                self.window,
                row as i32,
                column as i32,
                text.as_ptr(),
                text.as_bytes().len().min(i32::MAX as usize) as i32,
            );
        }
        Ok(())
    }
    fn check_tty(&self) -> io::Result<()> {
        let mut status = libc::pollfd {
            fd: self.descriptor.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one aligned pollfd remains valid for nonblocking poll.
        let result = unsafe { libc::poll(&mut status, 1, 0) };
        if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return Err(io::Error::last_os_error());
        }
        if status.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            return Err(io::Error::other("controlling terminal disconnected"));
        }
        Ok(())
    }
    pub fn refresh(&mut self) -> io::Result<()> {
        self.check_tty()?;
        // SAFETY: live owner screen/window; stream remains open until Drop.
        if unsafe { wrefresh(self.window) } < 0 || unsafe { libc::ferror(self.stream) } != 0 {
            return Err(io::Error::other("terminal redraw failed"));
        }
        Ok(())
    }
    pub fn key(&mut self, milliseconds: i32) -> io::Result<Option<Key>> {
        self.check_tty()?;
        // SAFETY: live standard window; blocking interval bounded by caller and single owner.
        let value = unsafe {
            wtimeout(self.window, milliseconds);
            wgetch(self.window)
        };
        Ok(match value {
            -1 => None,
            258 => Some(Key::Down),
            259 => Some(Key::Up),
            260 => Some(Key::Left),
            261 => Some(Key::Right),
            262 => Some(Key::Home),
            360 | 347 => Some(Key::End),
            338 => Some(Key::PageDown),
            339 => Some(Key::PageUp),
            263 | 127 | 8 => Some(Key::Backspace),
            343 | 10 | 13 => Some(Key::Enter),
            27 => Some(Key::Escape),
            410 => Some(Key::Resize),
            value if (0..256).contains(&value) => Some(Key::Character(value as u8 as char)),
            _ => None,
        })
    }
    pub fn suspend(&mut self) -> io::Result<()> {
        // SAFETY: only the screen owner calls ncurses transitions; descriptor and saved termios live.
        let restored = unsafe {
            def_prog_mode();
            endwin();
            libc::tcsetattr(self.descriptor.as_raw_fd(), libc::TCSANOW, &self.saved)
        };
        if restored != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    pub fn resume(&mut self) -> io::Result<()> {
        self.check_tty()?;
        // SAFETY: program mode saved by this adapter; active screen still owned.
        if unsafe { reset_prog_mode() } < 0 {
            return Err(io::Error::other("terminal resume failed"));
        }
        self.clear();
        Ok(())
    }
}
impl Drop for Curses {
    fn drop(&mut self) {
        // SAFETY: screen/FILE each have one owner; descriptor remains live through restoration.
        unsafe {
            endwin();
            libc::tcsetattr(self.descriptor.as_raw_fd(), libc::TCSANOW, &self.saved);
            delscreen(self.screen);
            libc::fclose(self.stream);
        }
    }
}
