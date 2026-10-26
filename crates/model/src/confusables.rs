// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A domain's **skeleton**: what it looks like, with the characters that are drawn alike made
//! the same, after the idea of the skeleton in Unicode Technical Standard #39 (Unicode Security
//! Mechanisms, section 4). Two domains with the same skeleton read the same to the eye.
//!
//! The table is MailCraft's own and small, written for what turns up in lookalike addresses:
//! the Cyrillic and Greek letters drawn like Latin ones, Latin letters with an accent or a
//! dotless form, full-width Latin, the digits `0` and `1` for `o` and `l`, and the pairs of
//! letters that pass for one (`rn` for `m`, `vv` for `w`, `cl` for `d`). Characters that draw
//! nothing (zero-width spaces and joiners, the soft hyphen, combining accents) are left out.
//! It is not Unicode's whole data file, and it need not be: a lookalike only counts against a
//! domain the owner already writes to, so what the table misses is a lookalike not caught, never
//! a warning given wrongly.
//!
//! Bounded: at most [`MAX_CHARS`] characters are read.

/// The most characters of a domain read.
pub const MAX_CHARS: usize = 256;

/// The Latin letter (or letters) a character is drawn like, in lower case; `None` for one that
/// stands for itself.
fn prototype(c: char) -> Option<&'static str> {
    Some(match c {
        // Digits that pass for letters.
        '0' => "o",
        '1' => "l",
        // Cyrillic.
        'а' | 'ӑ' | 'ӓ' => "a",
        'с' | 'ҫ' => "c",
        'ԁ' => "d",
        'е' | 'ё' | 'ӗ' => "e",
        'һ' => "h",
        'і' | 'ї' | 'ӏ' => "i",
        'ј' => "j",
        'к' => "k",
        'о' | 'ӧ' => "o",
        'р' => "p",
        'ԛ' => "q",
        'ѕ' => "s",
        'у' | 'ў' | 'ү' => "y",
        'ԝ' => "w",
        'х' | 'ҳ' => "x",
        // Greek.
        'α' | 'ά' => "a",
        'β' => "b",
        'γ' => "y",
        'ε' | 'έ' => "e",
        'η' | 'ή' => "n",
        'ι' | 'ί' | 'ϊ' | 'ΐ' => "i",
        'κ' => "k",
        'ν' => "v",
        'ο' | 'ό' => "o",
        'ρ' => "p",
        'τ' => "t",
        'υ' | 'ύ' | 'ϋ' => "u",
        'χ' => "x",
        'ω' | 'ώ' => "w",
        // Latin letters with an accent, or a form of their own.
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' | 'ɑ' => "a",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'ď' | 'đ' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' | 'ɡ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĵ' | 'ȷ' => "j",
        'ķ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' | 'ǀ' | 'ℓ' | 'ⅼ' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' => "s",
        'ţ' | 'ť' | 'ŧ' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' | 'ʏ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => return None,
    })
}

/// Whether `c` draws nothing: a combining accent, a zero-width space or joiner, a soft hyphen,
/// a byte-order mark.
fn invisible(c: char) -> bool {
    matches!(c, '\u{300}'..='\u{36f}' | '\u{200b}'..='\u{200f}' | '\u{ad}' | '\u{feff}' | '\u{2060}')
}

/// The skeleton of `text`: lower case, each character as the Latin letter it is drawn like,
/// what draws nothing left out, and `rn`, `vv` and `cl` read as `m`, `w` and `d`.
pub fn skeleton(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_CHARS * 2));
    for c in text.chars().take(MAX_CHARS).flat_map(char::to_lowercase) {
        if invisible(c) {
            continue;
        }
        // Full-width Latin letters and digits are the narrow ones.
        let c = match c {
            '\u{ff01}'..='\u{ff5e}' => char::from_u32(u32::from(c) - 0xfee0).unwrap_or(c).to_ascii_lowercase(),
            c => c,
        };
        match prototype(c) {
            Some(letters) => out.push_str(letters),
            None => out.push(c),
        }
    }
    out.replace("rn", "m").replace("vv", "w").replace("cl", "d")
}
