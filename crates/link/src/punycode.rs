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
