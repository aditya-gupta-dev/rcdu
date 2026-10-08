//! Formatting changes presentation only. Raw names remain unchanged in the model.
use std::cmp::Ordering;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
pub fn escape(bytes: &[u8]) -> String {
    let mut output = String::new();
    let mut at = 0;
    while at < bytes.len() {
        match std::str::from_utf8(&bytes[at..]) {
            Ok(text) => {
                printable(text, &mut output);
                break;
            }
            Err(error) => {
                let good = error.valid_up_to();
                printable(
                    std::str::from_utf8(&bytes[at..at + good]).unwrap(),
                    &mut output,
                );
                at += good;
                output.push_str(&format!("\\x{:02X}", bytes[at]));
                at += 1;
            }
        }
    }
    output
}
fn printable(text: &str, output: &mut String) {
    for character in text.chars() {
        if character.is_control()
            || matches!(character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            for byte in character.to_string().as_bytes() {
                output.push_str(&format!("\\x{byte:02X}"));
            }
        } else {
            output.push(character);
        }
    }
}
pub fn shorten(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.into();
    }
    if width < 4 {
        return ".".repeat(width);
    }
    let mut left = String::new();
    let mut right = Vec::new();
    let mut remaining = (width - 3).div_ceil(2);
    for part in text.graphemes(true) {
        let size = UnicodeWidthStr::width(part);
        if size > remaining {
            break;
        }
        remaining -= size;
        left.push_str(part);
    }
    remaining = (width - 3) / 2;
    for part in text.graphemes(true).rev() {
        let size = UnicodeWidthStr::width(part);
        if size > remaining {
            break;
        }
        remaining -= size;
        right.push(part);
    }
    left.push_str("...");
    for part in right.into_iter().rev() {
        left.push_str(part);
    }
    left
}
pub fn size(bytes: u64, si: bool) -> String {
    let base = if si { 1000u128 } else { 1024 };
    let labels = if si {
        ["B", "kB", "MB", "GB", "TB", "PB", "EB"]
    } else {
        ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"]
    };
    let mut unit = usize::from(bytes >= 1000);
    let mut divisor = base.pow(unit as u32);
    while unit < 6 && u128::from(bytes) * 100 >= divisor * 99995 {
        divisor *= base;
        unit += 1;
    }
    let rounded = (u128::from(bytes) * 10 + divisor / 2) / divisor;
    format!("{:>3}.{} {}", rounded / 10, rounded % 10, labels[unit])
}
pub fn natural(mut left: &[u8], mut right: &[u8]) -> Ordering {
    loop {
        left = left.trim_ascii_start();
        right = right.trim_ascii_start();
        if left.is_empty() || right.is_empty() {
            return left.cmp(right);
        }
        if left[0].is_ascii_digit() && right[0].is_ascii_digit() {
            let a = left
                .iter()
                .position(|byte| !byte.is_ascii_digit())
                .unwrap_or(left.len());
            let b = right
                .iter()
                .position(|byte| !byte.is_ascii_digit())
                .unwrap_or(right.len());
            let order = if left[0] == b'0' || right[0] == b'0' {
                left[..a].cmp(&right[..b])
            } else {
                a.cmp(&b).then_with(|| left[..a].cmp(&right[..b]))
            };
            if order != Ordering::Equal {
                return order;
            }
            left = &left[a..];
            right = &right[b..];
        } else {
            let order = left[0].cmp(&right[0]);
            if order != Ordering::Equal {
                return order;
            }
            left = &left[1..];
            right = &right[1..];
        }
    }
}
pub fn permissions(mode: u32) -> String {
    let mut value = String::from(if mode & libc::S_IFMT == libc::S_IFDIR {
        "d"
    } else if mode & libc::S_IFMT == libc::S_IFLNK {
        "l"
    } else {
        "-"
    });
    for (index, bit) in [0o400, 0o200, 0o100, 0o40, 0o20, 0o10, 0o4, 0o2, 0o1]
        .into_iter()
        .enumerate()
    {
        value.push(if mode & bit != 0 {
            ['r', 'w', 'x'][index % 3]
        } else {
            '-'
        });
    }
    for (at, bit, active, passive) in [
        (3, 0o4000, 's', 'S'),
        (6, 0o2000, 's', 'S'),
        (9, 0o1000, 't', 'T'),
    ] {
        if mode & bit != 0 {
            let replacement = if value.as_bytes()[at] == b'x' {
                active
            } else {
                passive
            };
            value.replace_range(at..at + 1, &replacement.to_string());
        }
    }
    value
}
