// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Host names: a domain in lower-case ASCII (international labels as Punycode), an IPv4 address
//! in any of the forms a browser reads, an IPv6 address in brackets.

use crate::punycode;

/// Longest host name (RFC 1035 §2.3.4) and longest label.
const MAX_HOST: usize = 253;
const MAX_LABEL: usize = 63;
/// Longest host name read at all, before it is written in ASCII.
const MAX_HOST_INPUT: usize = 1024;

/// A character that has no place in a label: it is invisible, or it is a full stop in another
/// script (a browser would split the name at it).
fn refused(c: char) -> bool {
    c.is_control()
        || c.is_whitespace()
        || c == '\u{fffd}'
        || matches!(c, '\u{3002}' | '\u{ff0e}' | '\u{ff61}' | '\u{2024}' | '\u{fe52}')
        || (c.is_ascii() && !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_')))
        || is_format(c)
}

/// Unicode's format characters that change how text is shown without being seen (bidi controls,
/// zero-width marks and joiners, the byte-order mark, a soft hyphen, tags).
fn is_format(c: char) -> bool {
    matches!(c as u32,
        0xad | 0x600..=0x605 | 0x61c | 0x6dd | 0x70f | 0x890 | 0x891 | 0x8e2 | 0x180e
        | 0x200b..=0x200f | 0x202a..=0x202e | 0x2060..=0x2064 | 0x2066..=0x206f
        | 0xfeff | 0xfff9..=0xfffb | 0x110bd | 0x110cd | 0x13430..=0x1343f | 0x1bca0..=0x1bca3
        | 0x1d173..=0x1d17a | 0xe0001 | 0xe0020..=0xe007f)
}

/// The host name `text` (not an IPv6 literal) as it is written and looked up: lower case, ASCII.
pub(crate) fn domain(text: &str) -> Option<String> {
    if text.is_empty() || text.len() > MAX_HOST_INPUT {
        return None;
    }
    let (text, dot) = match text.strip_suffix('.') {
        Some(t) => (t, true),
        None => (text, false),
    };
    let mut labels: Vec<String> = Vec::new();
    for label in text.split('.') {
        if label.is_empty() || label.chars().any(refused) {
            return None;
        }
        let lower: String = label.chars().flat_map(char::to_lowercase).collect();
        let label = if lower.is_ascii() {
            lower
        } else {
            let chars: Vec<char> = lower.chars().collect();
            format!("xn--{}", punycode::encode(&chars)?)
        };
        if label.len() > MAX_LABEL {
            return None;
        }
        labels.push(label);
    }
    let mut host = labels.join(".");
    if host.len() > MAX_HOST {
        return None;
    }
    if ends_in_number(labels.last()?) {
        return ipv4(&labels).map(|a| format!("{}.{}.{}.{}", a >> 24, (a >> 16) & 255, (a >> 8) & 255, a & 255));
    }
    if dot {
        host.push('.');
    }
    Some(host)
}

/// Whether a browser reads a name whose last label is this as an IPv4 address (WHATWG URL
/// Standard, "ends in a number"): digits, or `0x` and hex digits.
fn ends_in_number(label: &str) -> bool {
    if label.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    label.get(..2).is_some_and(|p| p.eq_ignore_ascii_case("0x")) && label.get(2..).is_some_and(|d| d.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// One part of an IPv4 address: decimal, `0x` hex or leading-zero octal.
fn number(part: &str) -> Option<u64> {
    let (digits, radix) = match part {
        "" => return None,
        p if p.len() >= 2 && p.get(..2).is_some_and(|x| x.eq_ignore_ascii_case("0x")) => (p.get(2..)?, 16),
        p if p.len() >= 2 && p.starts_with('0') => (p.get(1..)?, 8),
        p => (p, 10),
    };
    let mut value: u64 = 0;
    for c in digits.chars() {
        value = value.checked_mul(radix)?.checked_add(u64::from(c.to_digit(u32::try_from(radix).ok()?)?))?;
        if value > u64::from(u32::MAX) * 256 {
            return None;
        }
    }
    Some(value)
}

/// The address a browser reads from up to four numbers (WHATWG URL Standard, IPv4 parser).
fn ipv4(labels: &[String]) -> Option<u32> {
    if labels.len() > 4 {
        return None;
    }
    let numbers: Vec<u64> = labels.iter().map(|l| number(l)).collect::<Option<_>>()?;
    let (last, first) = numbers.split_last()?;
    if first.iter().any(|&n| n > 255) {
        return None;
    }
    let room = 256u64.checked_pow(u32::try_from(5usize.checked_sub(numbers.len())?).ok()?)?;
    if *last >= room {
        return None;
    }
    let mut address = *last;
    for (i, &n) in first.iter().enumerate() {
        address += n << (8 * (3 - u32::try_from(i).ok()?));
    }
    u32::try_from(address).ok()
}

/// An IPv6 literal (what is inside the brackets) written as a browser writes it: lower-case hex
/// groups, the longest run of two or more zero groups as `::`.
pub(crate) fn ipv6(inner: &str) -> Option<String> {
    if inner.len() > 45 || inner.contains('%') {
        return None;
    }
    let groups = inner.parse::<std::net::Ipv6Addr>().ok()?.segments();
    let (mut best, mut run_at, mut run) = ((0usize, 0usize), 0usize, 0usize);
    for (i, &g) in groups.iter().enumerate() {
        if g == 0 {
            if run == 0 {
                run_at = i;
            }
            run += 1;
            if run > best.1 {
                best = (run_at, run);
            }
        } else {
            run = 0;
        }
    }
    let mut out = String::from("[");
    let mut i = 0;
    while let Some(&g) = groups.get(i) {
        if best.1 >= 2 && i == best.0 {
            out.push_str("::");
            i += best.1;
            continue;
        }
        out.push_str(&format!("{g:x}"));
        i += 1;
        if i < groups.len() && !(best.1 >= 2 && i == best.0) {
            out.push(':');
        }
    }
    out.push(']');
    Some(out)
}
