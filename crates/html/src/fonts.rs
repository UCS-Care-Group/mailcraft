// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The kinds of family a message's `font-family` names: the generic families every reader knows,
//! and the well-known families of office and web mail, for a reader that has only its own system
//! fonts (the phone) to draw them in. Nothing is fetched: a name only picks one of those fonts.

/// A kind of family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Generic {
    /// The reader's own sans-serif font.
    Own,
    Serif,
    Mono,
    /// A narrow sans-serif face, where the reader has one (else its own font).
    Condensed,
}

/// The generic families, and the keywords that stand for one. `cursive`, `fantasy` and the rest
/// are the reader's own font.
pub fn generic(name: &str) -> Option<Generic> {
    Some(match name {
        "serif" | "ui-serif" => Generic::Serif,
        "monospace" | "ui-monospace" => Generic::Mono,
        "sans-serif" | "ui-sans-serif" | "system-ui" | "-apple-system" | "blinkmacsystemfont" | "cursive" | "fantasy" | "emoji" | "math"
        | "fangsong" | "ui-rounded" => Generic::Own,
        _ => return None,
    })
}

/// Well-known families, by kind (lower case, as [`crate::Span::font`] holds them).
const SERIF: &[&str] = &[
    "times new roman",
    "times",
    "georgia",
    "cambria",
    "garamond",
    "book antiqua",
    "palatino",
    "palatino linotype",
    "baskerville",
    "baskerville old face",
    "constantia",
    "didot",
    "bodoni mt",
    "century",
    "century schoolbook",
    "liberation serif",
    "tinos",
    "noto serif",
    "dejavu serif",
    "gelasio",
    "merriweather",
    "playfair display",
    "charter",
    "sitka text",
    "sitka",
    "minion pro",
    "hoefler text",
    "new york",
    "droid serif",
    "lora",
    "pt serif",
    "source serif pro",
    "iowan old style",
    "rockwell",
    "cochin",
];
const MONO: &[&str] = &[
    "courier new",
    "courier",
    "consolas",
    "cascadia code",
    "cascadia mono",
    "lucida console",
    "lucida sans typewriter",
    "monaco",
    "menlo",
    "sf mono",
    "liberation mono",
    "cousine",
    "dejavu sans mono",
    "noto sans mono",
    "source code pro",
    "fira code",
    "fira mono",
    "jetbrains mono",
    "ubuntu mono",
    "roboto mono",
    "andale mono",
    "droid sans mono",
    "inconsolata",
];
const SANS: &[&str] = &[
    "arial",
    "helvetica",
    "helvetica neue",
    "calibri",
    "carlito",
    "aptos",
    "aptos display",
    "segoe ui",
    "segoe ui variable",
    "verdana",
    "tahoma",
    "trebuchet ms",
    "geneva",
    "lucida sans",
    "lucida sans unicode",
    "lucida grande",
    "franklin gothic",
    "century gothic",
    "gill sans",
    "gill sans mt",
    "candara",
    "corbel",
    "roboto",
    "open sans",
    "lato",
    "liberation sans",
    "arimo",
    "noto sans",
    "dejavu sans",
    "source sans pro",
    "inter",
    "montserrat",
    "avenir",
    "avenir next",
    "sf pro text",
    "sf pro display",
    "ubuntu",
    "myriad pro",
    "frutiger",
    "univers",
    "futura",
    "bahnschrift",
    "selawik",
    "microsoft sans serif",
    "ms sans serif",
    "pt sans",
];

/// The words a family's name may end in that give its weight (`segoe ui light`, `aptos semibold`).
const WEIGHTS: [(&str, u16); 12] = [
    ("thin", 100),
    ("extralight", 200),
    ("ultralight", 200),
    ("light", 300),
    ("semilight", 350),
    ("medium", 500),
    ("semibold", 600),
    ("demibold", 600),
    ("extrabold", 800),
    ("ultrabold", 800),
    ("black", 900),
    ("heavy", 900),
];

/// What a well-known family (or a generic one) comes to, and the weight its name gives (400 when
/// it gives none): `segoe ui light` is light sans-serif, `arial narrow` condensed. `None` for a
/// name that is neither.
pub fn known(name: &str) -> Option<(Generic, u16)> {
    let mut base = name;
    let mut weight = 400;
    let mut narrow = false;
    // At most a weight and a width, in either order (`arial narrow bold` has its bold elsewhere).
    for _ in 0..2 {
        let Some((head, last)) = base.rsplit_once(' ') else {
            break;
        };
        if let Some((_, n)) = WEIGHTS.iter().find(|(word, _)| *word == last) {
            weight = *n;
        } else if matches!(last, "narrow" | "condensed" | "cond") {
            narrow = true;
        } else {
            break;
        }
        base = head;
    }
    let kind = if let Some(kind) = generic(base) {
        kind
    } else if SANS.contains(&base) {
        Generic::Own
    } else if SERIF.contains(&base) {
        Generic::Serif
    } else if MONO.contains(&base) {
        Generic::Mono
    } else {
        return None;
    };
    Some((if narrow && kind == Generic::Own { Generic::Condensed } else { kind }, weight))
}

/// What a span's families (lower case, joined by commas) come to on a reader with only its own
/// fonts: the first that is generic or well known, as the desktop takes the first that is
/// installed; the reader's own font at weight 400 when none is.
pub fn stand_in(list: &str) -> (Generic, u16) {
    list.split(',').take(crate::MAX_FAMILIES).find_map(|name| known(name.trim())).unwrap_or((Generic::Own, 400))
}

#[cfg(test)]
#[path = "fonts_tests.rs"]
mod fonts_tests;
