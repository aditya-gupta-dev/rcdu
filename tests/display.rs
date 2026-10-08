use rcdu::display;
use std::cmp::Ordering;
#[test]
fn huge_sizes_natural_runs_and_escaped_graphemes() {
    assert_eq!(display::size(1024, false), "  1.0 KiB");
    assert_eq!(display::size(1000000, true), "  1.0 MB");
    assert_eq!(display::size(u64::MAX, false), " 16.0 EiB");
    assert_eq!(display::natural(b"file2", b"file10"), Ordering::Less);
    assert_eq!(display::natural(b"file02", b"file2"), Ordering::Less);
    assert_eq!(display::escape(b"bad\xff\x1b\n"), "bad\\xFF\\x1B\\x0A");
    let shortened = display::shorten("界e\u{301}界e\u{301}界e\u{301}界e\u{301}", 8);
    assert!(unicode_width::UnicodeWidthStr::width(shortened.as_str()) <= 8);
    assert_eq!(display::permissions(0o104755), "-rwsr-xr-x");
}
