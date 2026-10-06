use std::cmp::Ordering;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
/// Escape terminal controls and invalid bytes while retaining the original bytes for identity.
pub fn sanitize(bytes: &[u8]) -> String {
    let mut result = String::new();
    let mut remaining = bytes;
    while !remaining.is_empty() {
        let length = match std::str::from_utf8(remaining) {
            Ok(_) => remaining.len(),
            Err(error) => error.valid_up_to(),
        };
        if length > 0 {
            for character in std::str::from_utf8(&remaining[..length]).unwrap().chars() {
                if character.is_control()
                    || matches!(character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
                {
                    for byte in character.to_string().as_bytes() {
                        result.push_str(&format!("\\x{byte:02X}"));
                    }
                } else {
                    result.push(character);
                }
            }
            remaining = &remaining[length..];
        } else {
            result.push_str(&format!("\\x{:02X}", remaining[0]));
            remaining = &remaining[1..];
        }
    }
    result
}
pub fn shorten(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_owned();
    }
    if width <= 3 {
        return ".".repeat(width);
    }
    let mut left = String::new();
    let mut right = String::new();
    let prefix_width = (width - 3).div_ceil(2);
    let suffix_width = (width - 3) / 2;
    let mut cells = 0;
    for grapheme in text.graphemes(true) {
        let columns = UnicodeWidthStr::width(grapheme);
        if cells + columns > prefix_width {
            break;
        }
        cells += columns;
        left.push_str(grapheme);
    }
    cells = 0;
    let mut suffix = Vec::new();
    for grapheme in text.graphemes(true).rev() {
        let columns = UnicodeWidthStr::width(grapheme);
        if cells + columns > suffix_width {
            break;
        }
        cells += columns;
        suffix.push(grapheme);
    }
    for grapheme in suffix.into_iter().rev() {
        right.push_str(grapheme);
    }
    format!("{left}...{right}")
}

pub fn size(bytes: u64, si: bool) -> String {
    let (cutoffs, divisors, units): (&[u64], &[u64], &[&str]) = if si {
        (
            &[
                1000,
                999_950,
                999_950_000,
                999_950_000_000,
                999_950_000_000_000,
                999_950_000_000_000_000,
            ],
            &[
                1,
                1000,
                1_000_000,
                1_000_000_000,
                1_000_000_000_000,
                1_000_000_000_000_000,
                1_000_000_000_000_000_000,
            ],
            &["B", "kB", "MB", "GB", "TB", "PB", "EB"],
        )
    } else {
        (
            &[
                1000,
                1023949,
                1048523572,
                1073688136909,
                1099456652194612,
                1125843611847281869,
            ],
            &[1, 1 << 10, 1 << 20, 1 << 30, 1 << 40, 1 << 50, 1 << 60],
            &["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"],
        )
    };
    let unit = cutoffs
        .iter()
        .position(|cutoff| bytes < *cutoff)
        .unwrap_or(6);
    let tenths = ((u128::from(bytes) * 10 + u128::from(divisors[unit]) / 2)
        / u128::from(divisors[unit])) as u64;
    format!("{:>3}.{} {}", tenths / 10, tenths % 10, units[unit])
}
// Altered Rust implementation of ncdu's port of Martin Pool's strnatcmp.
// Copyright (C) 2000, 2004 Martin Pool. Full notice: LICENSES/NaturalSort.txt.
pub fn natural(left: &[u8], right: &[u8]) -> Ordering {
    let mut a = 0;
    let mut b = 0;
    let at = |bytes: &[u8], index: usize| bytes.get(index).copied().unwrap_or(0);
    loop {
        while at(left, a).is_ascii_whitespace() {
            a += 1;
        }
        while at(right, b).is_ascii_whitespace() {
            b += 1;
        }
        if at(left, a).is_ascii_digit() && at(right, b).is_ascii_digit() {
            if at(left, a) == b'0' || at(right, b) == b'0' {
                loop {
                    let x = at(left, a);
                    let y = at(right, b);
                    if !x.is_ascii_digit() && !y.is_ascii_digit() {
                        break;
                    }
                    if !x.is_ascii_digit() {
                        return Ordering::Less;
                    }
                    if !y.is_ascii_digit() {
                        return Ordering::Greater;
                    }
                    let order = x.cmp(&y);
                    if order != Ordering::Equal {
                        return order;
                    }
                    a += 1;
                    b += 1;
                }
            } else {
                let mut bias = Ordering::Equal;
                loop {
                    let x = at(left, a);
                    let y = at(right, b);
                    if !x.is_ascii_digit() && !y.is_ascii_digit() {
                        if bias != Ordering::Equal || (x == 0 && y == 0) {
                            return bias;
                        }
                        break;
                    }
                    if !x.is_ascii_digit() {
                        return Ordering::Less;
                    }
                    if !y.is_ascii_digit() {
                        return Ordering::Greater;
                    }
                    if bias == Ordering::Equal {
                        bias = x.cmp(&y);
                    }
                    a += 1;
                    b += 1;
                }
            }
        }
        let x = at(left, a);
        let y = at(right, b);
        if x == 0 && y == 0 {
            return Ordering::Equal;
        }
        let order = x.cmp(&y);
        if order != Ordering::Equal {
            return order;
        }
        a += 1;
        b += 1;
    }
}
pub fn mode(mode: u16) -> String {
    let mut value = String::new();
    value.push(match u32::from(mode) & libc::S_IFMT {
        libc::S_IFDIR => 'd',
        libc::S_IFLNK => 'l',
        libc::S_IFCHR => 'c',
        libc::S_IFBLK => 'b',
        libc::S_IFIFO => 'p',
        libc::S_IFSOCK => 's',
        _ => '-',
    });
    for (read, write, execute, special, low, high) in [
        (0o400, 0o200, 0o100, 0o4000, 's', 'S'),
        (0o40, 0o20, 0o10, 0o2000, 's', 'S'),
        (0o4, 0o2, 0o1, 0o1000, 't', 'T'),
    ] {
        value.push(if mode & read != 0 { 'r' } else { '-' });
        value.push(if mode & write != 0 { 'w' } else { '-' });
        value.push(if mode & special != 0 {
            if mode & execute != 0 { low } else { high }
        } else if mode & execute != 0 {
            'x'
        } else {
            '-'
        });
    }
    value
}
