//! PTY-only cleanup fixture: intentionally panics after owning a terminal.
use rcdu::ui::terminal::Terminal;
fn main() {
    let mut terminal = Terminal::open().expect("controlling terminal fixture");
    terminal.handle.write(b"panic cleanup fixture\r\n").unwrap();
    panic!("intentional terminal cleanup fixture");
}
