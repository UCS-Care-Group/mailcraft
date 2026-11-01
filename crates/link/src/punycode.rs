// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Punycode (RFC 3492): a label of Unicode characters as ASCII, the part after `xn--`.

const BASE: u32 = 36;
const T_MIN: u32 = 1;
const T_MAX: u32 = 26;
const SKEW: u32 = 38;
const DAMP: u32 = 700;
const INITIAL_BIAS: u32 = 72;
const INITIAL_N: u32 = 128;

/// RFC 3492 §6.1.
fn adapt(delta: u32, points: u32, first: bool) -> u32 {
    let mut delta = if first { delta / DAMP } else { delta / 2 };
    delta += delta / points.max(1);
    let mut k = 0;
    while delta > ((BASE - T_MIN) * T_MAX) / 2 {
        delta /= BASE - T_MIN;
        k += BASE;
    }
    k + (BASE - T_MIN + 1) * delta / (delta + SKEW)
}

/// A digit of 0 to 35 as `a`-`z`, `0`-`9`.
fn digit(d: u32) -> char {
    match u8::try_from(d) {
        Ok(d) if d < 26 => char::from(b'a' + d),
        Ok(d) if d < 36 => char::from(b'0' + (d - 26)),
        _ => '-',
    }
}

/// The Punycode of `label` (RFC 3492 §6.3), without the `xn--`; `None` when the label is too
/// long to encode without overflow (the reference algorithm's limit) or empty.
pub fn encode(label: &[char]) -> Option<String> {
    if label.is_empty() || label.len() > 4096 {
        return None;
    }
    let total = u32::try_from(label.len()).ok()?;
    let mut out: String = label.iter().filter(|c| c.is_ascii()).collect();
    let basic = u32::try_from(out.len()).ok()?;
    if basic > 0 {
        out.push('-');
    }
    let (mut n, mut delta, mut bias, mut handled) = (INITIAL_N, 0u32, INITIAL_BIAS, basic);
    while handled < total {
        let m = label.iter().map(|&c| u32::from(c)).filter(|&c| c >= n).min()?;
        delta = delta.checked_add((m - n).checked_mul(handled.checked_add(1)?)?)?;
        n = m;
        for &c in label {
            let c = u32::from(c);
            if c < n {
                delta = delta.checked_add(1)?;
            }
            if c == n {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = if k <= bias {
                        T_MIN
                    } else if k >= bias + T_MAX {
                        T_MAX
                    } else {
                        k - bias
                    };
                    if q < t {
                        break;
                    }
                    out.push(digit(t + (q - t) % (BASE - t)));
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                out.push(digit(q));
                bias = adapt(delta, handled + 1, handled == basic);
                delta = 0;
                handled += 1;
            }
        }
        delta = delta.checked_add(1)?;
        n = n.checked_add(1)?;
    }
    Some(out)
}

/// The value of a Punycode digit (`a`-`z` or `A`-`Z` for 0 to 25, `0`-`9` for 26 to 35).
fn value(c: char) -> Option<u32> {
    match c {
        'a'..='z' => Some(u32::from(c) - u32::from('a')),
        'A'..='Z' => Some(u32::from(c) - u32::from('A')),
        '0'..='9' => Some(u32::from(c) - u32::from('0') + 26),
        _ => None,
    }
}

/// The label `text` (the part after `xn--`) stands for (RFC 3492 §6.2); `None` when it is no
/// Punycode, would overflow, or names something that is no character. Bounded like [`encode`].
/// As IDNA 2008 (RFC 5891 §5.4) asks, a label is only read when what it stands for has a letter
/// outside ASCII and writes back to exactly `text`: `abc-` is not `abc`, and a name read through
/// it can never pass for one written plainly.
pub fn decode(text: &str) -> Option<Vec<char>> {
    if text.is_empty() || text.len() > 4096 || !text.is_ascii() {
        return None;
    }
    let (basic, digits) = match text.rfind('-') {
        Some(dash) => (text.get(..dash)?, text.get(dash + 1..)?),
        None => ("", text),
    };
    let mut out: Vec<char> = basic.chars().collect();
    let (mut n, mut i, mut bias) = (INITIAL_N, 0u32, INITIAL_BIAS);
    let mut digits = digits.chars().peekable();
    let mut first = true;
    while digits.peek().is_some() {
        let old = i;
        let mut w = 1u32;
        let mut k = BASE;
        loop {
            let d = value(digits.next()?)?;
            i = i.checked_add(d.checked_mul(w)?)?;
            let t = if k <= bias {
                T_MIN
            } else if k >= bias.checked_add(T_MAX)? {
                T_MAX
            } else {
                k - bias
            };
            if d < t {
                break;
            }
            w = w.checked_mul(BASE - t)?;
            k = k.checked_add(BASE)?;
        }
        let points = u32::try_from(out.len()).ok()?.checked_add(1)?;
        bias = adapt(i.checked_sub(old)?, points, first);
        first = false;
        n = n.checked_add(i / points)?;
        i %= points;
        let c = char::from_u32(n).filter(|c| !c.is_ascii())?;
        out.insert(usize::try_from(i).ok()?.min(out.len()), c);
        i = i.checked_add(1)?;
    }
    let exact = out.iter().any(|c| !c.is_ascii()) && encode(&out).is_some_and(|again| again.eq_ignore_ascii_case(text));
    exact.then_some(out)
}
