// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Accent folding for Latin-1 Supplement and Latin Extended-A, and the casing rules
//! for dictionary stems.

/// Fold one letter. `None` means the character is unchanged.
pub(crate) fn fold_char(c: char) -> Option<&'static str> {
    Some(match c {
        'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'Ā' | 'Ă' | 'Ą' => "A",
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'Æ' => "AE",
        'æ' => "ae",
        'Ç' | 'Ć' | 'Ĉ' | 'Ċ' | 'Č' => "C",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'Ď' | 'Đ' | 'Ð' => "D",
        'ď' | 'đ' | 'ð' => "d",
        'È' | 'É' | 'Ê' | 'Ë' | 'Ē' | 'Ĕ' | 'Ė' | 'Ę' | 'Ě' => "E",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'Ĝ' | 'Ğ' | 'Ġ' | 'Ģ' => "G",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'Ĥ' | 'Ħ' => "H",
        'ĥ' | 'ħ' => "h",
        'Ì' | 'Í' | 'Î' | 'Ï' | 'Ĩ' | 'Ī' | 'Ĭ' | 'Į' | 'İ' => "I",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'Ĳ' => "IJ",
        'ĳ' => "ij",
        'Ĵ' => "J",
        'ĵ' => "j",
        'Ķ' | 'ĸ' => "K",
        'ķ' => "k",
        'Ĺ' | 'Ļ' | 'Ľ' | 'Ŀ' | 'Ł' => "L",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'Ń' | 'Ñ' | 'Ņ' | 'Ň' | 'ŉ' => "N",
        'ń' | 'ñ' | 'ņ' | 'ň' => "n",
        'Ŋ' => "NG",
        'ŋ' => "ng",
        'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ō' | 'Ŏ' | 'Ő' | 'Ø' => "O",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ō' | 'ŏ' | 'ő' | 'ø' => "o",
        'Œ' => "OE",
        'œ' => "oe",
        'Ŕ' | 'Ŗ' | 'Ř' => "R",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'Ś' | 'Ŝ' | 'Ş' | 'Š' => "S",
        'ś' | 'ŝ' | 'ş' | 'š' | 'ſ' => "s",
        'ß' => "ss",
        'Ţ' | 'Ť' | 'Ŧ' => "T",
        'ţ' | 'ť' | 'ŧ' => "t",
        'Þ' => "TH",
        'þ' => "th",
        'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ũ' | 'Ū' | 'Ŭ' | 'Ů' | 'Ű' | 'Ų' => "U",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'Ŵ' => "W",
        'ŵ' => "w",
        'Ý' | 'Ŷ' | 'Ÿ' => "Y",
        'ý' | 'ŷ' | 'ÿ' => "y",
        'Ź' | 'Ż' | 'Ž' => "Z",
        'ź' | 'ż' | 'ž' => "z",
        _ => return None,
    })
}

pub(crate) fn fold_accents(s: &str) -> String {
    if s.is_ascii() {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match fold_char(c) {
            Some(rep) => out.push_str(rep),
            None => out.push(c),
        }
    }
    out
}

pub(crate) fn to_lower(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        for low in c.to_lowercase() {
            out.push(low);
        }
    }
    out
}

/// How the owner's word is capitalised.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaseKind {
    AsIs,
    Title,
    AllCaps,
}

pub(crate) fn case_kind(s: &str) -> CaseKind {
    if is_all_caps(s) {
        CaseKind::AllCaps
    } else if is_title(s) {
        CaseKind::Title
    } else {
        CaseKind::AsIs
    }
}

fn is_all_caps(s: &str) -> bool {
    let mut upper = false;
    for c in s.chars() {
        if c.is_lowercase() {
            return false;
        }
        if c.is_uppercase() {
            upper = true;
        }
    }
    upper
}

fn is_title(s: &str) -> bool {
    let mut seen = false;
    for c in s.chars() {
        if !c.is_alphabetic() {
            continue;
        }
        if !seen {
            if !c.is_uppercase() {
                return false;
            }
            seen = true;
        } else if c.is_uppercase() {
            return false;
        }
    }
    seen
}

fn eq_ignore_case(a: &str, b: &str) -> bool {
    if a.len() == b.len() && a.is_ascii() && b.is_ascii() {
        return a.eq_ignore_ascii_case(b);
    }
    let mut ia = a.chars().flat_map(|c| c.to_lowercase());
    let mut ib = b.chars().flat_map(|c| c.to_lowercase());
    loop {
        match (ia.next(), ib.next()) {
            (None, None) => return true,
            (Some(x), Some(y)) if x == y => {}
            _ => return false,
        }
    }
}

/// `surface` is the dictionary form. `original` still has the owner's capitals and accents;
/// `folded` is `original` after accent folding.
pub(crate) fn case_ok(surface: &str, original: &str, folded: &str) -> bool {
    if surface == folded || surface == original {
        return true;
    }
    if !eq_ignore_case(surface, folded) {
        return false;
    }
    if is_all_caps(original) {
        return true;
    }
    !surface.chars().any(|c| c.is_uppercase()) && is_title(original)
}

pub(crate) fn apply_case(kind: CaseKind, suggestion: &str) -> String {
    match kind {
        CaseKind::AllCaps => suggestion.chars().flat_map(|c| c.to_uppercase()).collect(),
        CaseKind::Title => {
            // `McDonald` and `NASA` are already the dictionary spelling. Forcing
            // them to `Mcdonald` or `Nasa` makes `case_ok` reject the suggestion.
            if suggestion.chars().skip(1).any(|c| c.is_uppercase()) {
                return suggestion.to_string();
            }
            let mut chars = suggestion.chars();
            let Some(first) = chars.next() else {
                return String::new();
            };
            let mut out: String = first.to_uppercase().collect();
            for c in chars {
                for low in c.to_lowercase() {
                    out.push(low);
                }
            }
            out
        }
        CaseKind::AsIs => suggestion.to_string(),
    }
}
