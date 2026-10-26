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
        | 0x200b | 0x200e | 0x200f | 0x202a..=0x202e | 0x2060..=0x2064 | 0x2066..=0x206f
        | 0xfeff | 0xfff9..=0xfffb | 0x110bd | 0x110cd | 0x13430..=0x1343f | 0x1bca0..=0x1bca3
        | 0x1d173..=0x1d17a | 0xe0001 | 0xe0020..=0xe007f)
}

/// A virama: the mark of an Indic script that joins the consonant before it to the next
/// (canonical combining class 9; the common scripts).
fn virama(c: char) -> bool {
    matches!(
        c as u32,
        0x94d
            | 0x9cd
            | 0xa4d
            | 0xacd
            | 0xb4d
            | 0xbcd
            | 0xc4d
            | 0xccd
            | 0xd3b
            | 0xd3c
            | 0xd4d
            | 0xdca
            | 0xe3a
            | 0xeba
            | 0xf84
            | 0x1039
            | 0x103a
            | 0x1714
            | 0x1734
            | 0x17d2
            | 0x1a60
            | 0x1b44
            | 0x1baa
            | 0x1bab
            | 0x1bf2
            | 0x1bf3
            | 0x2d7f
            | 0xa806
            | 0xa82c
            | 0xa8c4
            | 0xa953
            | 0xa9c0
            | 0xaaf6
            | 0xabed
            | 0x10a3f
            | 0x11046
            | 0x1107f
            | 0x110b9
            | 0x11133
            | 0x111c0
            | 0x11235
            | 0x112ea
            | 0x1134d
            | 0x11442
            | 0x114c2
            | 0x115bf
            | 0x1163f
            | 0x116b6
            | 0x1172b
            | 0x11839
            | 0x1193d
            | 0x1193e
            | 0x119e0
            | 0x11a34
            | 0x11a47
            | 0x11a99
            | 0x11c3f
            | 0x11d44
            | 0x11d45
            | 0x11d97
    )
}

/// A mark that does not break a joining run in the Arabic script (joining type T).
fn transparent(c: char) -> bool {
    matches!(c as u32, 0x610..=0x61a | 0x64b..=0x65f | 0x670 | 0x6d6..=0x6dc | 0x6df..=0x6e4 | 0x6e7 | 0x6e8 | 0x6ea..=0x6ed)
}

/// An Arabic-script letter that joins to the letter after it (joining type D, or L).
fn joins_forward(c: char) -> bool {
    let n = c as u32;
    let letter = matches!(n, 0x620..=0x64a | 0x66e..=0x66f | 0x671..=0x6d3 | 0x6fa..=0x6fc | 0x6ff | 0x750..=0x77f | 0x8a0..=0x8c9);
    // The right-joining letters (alef, dal, reh, waw and their kin) do not join forward.
    let right_only = matches!(n,
        0x622..=0x625 | 0x627 | 0x629 | 0x62f..=0x632 | 0x648 | 0x671..=0x673 | 0x675..=0x677
        | 0x688..=0x699 | 0x6c0 | 0x6c3..=0x6cb | 0x6cd | 0x6cf | 0x6d2 | 0x6d3 | 0x6d5 | 0x6ff
        | 0x759..=0x75b | 0x76b | 0x76c | 0x771 | 0x773 | 0x774 | 0x778 | 0x779 | 0x8aa..=0x8ac | 0x8ae | 0x8b1 | 0x8b2 | 0x8b9);
    letter && !right_only
}

/// An Arabic-script letter that joins to the letter before it (joining type D or R).
fn joins_backward(c: char) -> bool {
    let n = c as u32;
    matches!(n, 0x620..=0x64a | 0x66e..=0x66f | 0x671..=0x6d3 | 0x6d5 | 0x6fa..=0x6fc | 0x6ff | 0x750..=0x77f | 0x8a0..=0x8c9)
}

/// Whether the zero-width non-joiner and joiner in a label are where IDNA2008 allows them
/// (RFC 5892, appendix A.1 and A.2, CONTEXTJ): either after a virama, or the non-joiner between
/// joining letters of the Arabic script (what Persian writes). A label with none is fine.
fn joiners_allowed(label: &str) -> bool {
    if !label.contains(['\u{200c}', '\u{200d}']) {
        return true;
    }
    let chars: Vec<char> = label.chars().collect();
    chars.iter().enumerate().all(|(i, &c)| {
        if c != '\u{200c}' && c != '\u{200d}' {
            return true;
        }
        let before = i.checked_sub(1).and_then(|j| chars.get(j)).copied();
        if before.is_some_and(virama) {
            return true;
        }
        if c == '\u{200d}' {
            return false;
        }
        let previous = chars.get(..i).and_then(|p| p.iter().rev().find(|&&x| !transparent(x))).copied();
        let next = chars.get(i + 1..).and_then(|n| n.iter().find(|&&x| !transparent(x))).copied();
        previous.is_some_and(joins_forward) && next.is_some_and(joins_backward)
    })
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
        if label.is_empty() || label.chars().any(refused) || !joiners_allowed(label) {
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
