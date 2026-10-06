use rcdu::{
    browser::{Browser, Source},
    config::{Config, SortField},
    exclude::{Exclusions, Match},
    format::json,
    os::{self, Location},
    ui::display,
};
use std::{cmp::Ordering, ffi::CString, io::Cursor};
#[test]
fn size_thresholds_never_wrap_or_round_into_wrong_units() {
    for (value, expected) in [
        (0, "0.0 B"),
        (999, "999.0 B"),
        (1000, "1.0 kB"),
        (1049, "1.0 kB"),
        (1050, "1.1 kB"),
        (999949, "999.9 kB"),
        (999950, "1.0 MB"),
        (999949999, "999.9 MB"),
        (999950000, "1.0 GB"),
        (u64::MAX, "18.4 EB"),
    ] {
        assert_eq!(display::size(value, true).trim(), expected);
    }
    for (value, expected) in [
        (0, "0.0 B"),
        (999, "999.0 B"),
        (1000, "1.0 KiB"),
        (1024, "1.0 KiB"),
        (1023898, "999.9 KiB"),
        (1023949, "1.0 MiB"),
        (1048523571, "999.9 MiB"),
        (1048523572, "1.0 GiB"),
        (1073688136908, "999.9 GiB"),
        (1073688136909, "1.0 TiB"),
        (1099456652194612, "1.0 PiB"),
        (1125843611847281869, "1.0 EiB"),
        (u64::MAX, "16.0 EiB"),
    ] {
        assert_eq!(display::size(value, false).trim(), expected);
    }
}
#[test]
fn names_cannot_inject_terminal_controls_and_shortening_keeps_graphemes() {
    assert_eq!(
        display::sanitize(b"bad\xff\n\x1b[31m"),
        "bad\\xFF\\x0A\\x1B[31m"
    );
    assert!(!display::sanitize("a\u{202e}b".as_bytes()).contains('\u{202e}'));
    for text in [
        "你好世界abcdefgh",
        "e\u{301} long e\u{301}",
        "👨‍👩‍👧‍👦 long family 👨‍👩‍👧‍👦",
    ] {
        for width in 0..20 {
            let short = display::shorten(text, width);
            assert!(unicode_width::UnicodeWidthStr::width(short.as_str()) <= width);
            assert!(!short.ends_with("...\u{301}"));
        }
    }
    assert_eq!(display::mode(0o104644), "-rwSr--r--");
    assert_eq!(display::mode(0o041755), "drwxr-xr-t");
}
#[test]
fn component_exclusions_keep_filesystem_root_anchoring_and_duplicate_precedence() {
    let mut exclusions = Exclusions::default();
    for pattern in [
        "/foo/bar",
        "/foo/qoo/",
        "/foo/qoo",
        "/f??/xyz/",
        "/*o/somefile",
        "/roo?",
        "/root/",
        "excluded",
        "somefile/",
        "o*y/not[o]kay",
        r"literal\*",
    ] {
        exclusions.add(pattern.as_bytes()).unwrap();
    }
    let matches = |location: &str, name: &str| {
        exclusions.matches(
            &Location {
                parent: None,
                name: location.as_bytes().to_vec(),
            },
            &CString::new(name).unwrap(),
        )
    };
    assert_eq!(matches("/", "root"), Match::Any);
    assert_eq!(matches("/", "somefile"), Match::Directory);
    assert_eq!(matches("/foo", "bar"), Match::Any);
    assert_eq!(matches("/foo", "qoo"), Match::Any);
    assert_eq!(matches("/somedir/foo", "bar"), Match::None);
    assert_eq!(matches("/somedir/okay", "notokay"), Match::Any);
    assert_eq!(matches("/somewhere", "literal*"), Match::Any);
    assert_eq!(matches("/somewhere", "excluded"), Match::Any);
}
#[test]
fn browser_sort_ties_and_hidden_filters_do_not_change_accounting() {
    let model=json::read(Cursor::new(b"[1,2,{},[{\"name\":\"/root\"},{\"name\":\"file10\",\"asize\":4},{\"name\":\"file2\",\"asize\":4},{\"name\":\".hidden\",\"asize\":9},{\"name\":\"ignored\",\"excluded\":\"pattern\"}]]")).unwrap();
    let before = model.totals(model.root);
    let config = Config {
        sort: SortField::Name,
        descending: false,
        hidden: false,
        ..Default::default()
    };
    let browser = Browser::new(Source::Memory(model), config, false);
    assert_eq!(browser.rows.len(), 2);
    if let Source::Memory(model) = &browser.source {
        assert_eq!(model.name(browser.rows[0].unwrap()), b"file2");
        assert_eq!(model.name(browser.rows[1].unwrap()), b"file10");
        assert_eq!(model.totals(model.root), before);
    }
    assert!(os::validate_action_root(&os::byte_path(b"/root/../elsewhere")).is_err());
    assert!(os::validate_action_root(&os::byte_path(b"/root")).is_ok());
}

#[test]
fn natural_sort_upstream_pairwise_fixture() {
    let words: &[&[u8]] = &[
        b"1-02",
        b"1-2",
        b"1-20",
        b"1.002.01",
        b"1.002.03",
        b"1.002.08",
        b"1.009.02",
        b"1.009.10",
        b"1.009.20",
        b"1.010.12",
        b"1.011.02",
        b"10-20",
        b"1999-3-3",
        b"1999-12-25",
        b"2000-1-2",
        b"2000-1-10",
        b"2000-3-23",
        b"fred",
        b"jane",
        b"pic01",
        b"pic02",
        b"pic02a",
        b"pic02000",
        b"pic05",
        b"pic2",
        b"pic3",
        b"pic4",
        b"pic 4 else",
        b"pic 5",
        b"pic 5 ",
        b"pic 5 something",
        b"pic 6",
        b"pic   7",
        b"pic100",
        b"pic100a",
        b"pic120",
        b"pic121",
        b"tom",
        b"x2-g8",
        b"x2-y08",
        b"x2-y7",
        b"x8-y8",
    ];
    for (i, left) in words.iter().enumerate() {
        for (j, right) in words.iter().enumerate() {
            assert_eq!(
                display::natural(left, right),
                i.cmp(&j),
                "{left:?} / {right:?}"
            );
        }
    }
    assert_eq!(
        display::natural(b"pic2", b"pic999999999999999999999999"),
        Ordering::Less
    );
}
