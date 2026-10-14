//! Which way text runs, and the order its characters are drawn in.
//!
//! A paragraph's direction is that of its first strong character (rules P2 and P3 of the Unicode
//! Bidirectional Algorithm, UAX #9): a letter of a right-to-left script (Hebrew, Arabic, Syriac,
//! Thaana, N'Ko and their neighbours) makes it right-to-left, any other letter left-to-right, and
//! a paragraph with no letter at all has no direction of its own.
//!
//! [`visual_order`] puts one line of text in the order it is drawn, left to right, for a reader
//! that lays out characters in a row. It is a reduced form of the algorithm: strong characters
//! are left-to-right or right-to-left by their script, digits stay in reading order (a number in
//! Hebrew text reads left to right), neutral characters (spaces, punctuation) between two runs of
//! one direction take that direction and elsewhere the paragraph's, and paired brackets in
//! right-to-left runs are mirrored. Explicit embedding marks are not followed. It is enough for
//! mail written in Hebrew, Arabic or Persian with words and numbers of other scripts in it.

/// Which way a paragraph runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    Ltr,
    Rtl,
}

/// The strong direction of one character, if it has one.
fn strong(c: char) -> Option<Direction> {
    let cp = u32::from(c);
    // Marks that only say which way: LRM, RLM, ALM.
    match cp {
        0x200E => return Some(Direction::Ltr),
        0x200F | 0x061C => return Some(Direction::Rtl),
        _ => {}
    }
    if !c.is_alphabetic() {
        return None;
    }
    Some(if is_rtl_script(cp) { Direction::Rtl } else { Direction::Ltr })
}

/// Whether a code point is in a block of a right-to-left script.
fn is_rtl_script(cp: u32) -> bool {
    matches!(cp,
        0x0590..=0x08FF // Hebrew, Arabic, Syriac, Arabic Supplement, Thaana, N'Ko, Samaritan, Mandaic, Arabic Extended
        | 0xFB1D..=0xFDFF // Hebrew and Arabic presentation forms A
        | 0xFE70..=0xFEFF // Arabic presentation forms B
        | 0x10800..=0x10FFF // Cypriot to Old Uyghur, Hanifi Rohingya, Yezidi…
        | 0x1E800..=0x1EFFF) // Mende Kikakui, Adlam, Arabic mathematical symbols
}

/// The direction of a paragraph: that of its first strong character, `None` when it has none.
pub fn direction(text: &str) -> Option<Direction> {
    text.chars().take(4096).find_map(strong)
}

/// Whether a paragraph runs right to left.
pub fn is_rtl(text: &str) -> bool {
    direction(text) == Some(Direction::Rtl)
}

/// What a character counts as when ordering a line.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Strong(Direction),
    /// A digit, or what joins digits into a number (`.`, `,`, `:` between digits).
    Number,
    Neutral,
}

fn class(c: char) -> Class {
    if let Some(d) = strong(c) {
        Class::Strong(d)
    } else if c.is_numeric() {
        Class::Number
    } else {
        Class::Neutral
    }
}

/// The character drawn in place of `c` in right-to-left text: brackets face the other way.
fn mirrored(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '«' => '»',
        '»' => '«',
        '‹' => '›',
        '›' => '‹',
        c => c,
    }
}

/// One line of a paragraph running `base`, in the order it is drawn from left to right. The
/// second value maps each drawn character to its index in `line` (counted in characters).
pub fn visual_order(line: &str, base: Direction) -> (String, Vec<usize>) {
    let chars: Vec<char> = line.chars().collect();
    let classes: Vec<Class> = chars.iter().map(|c| class(*c)).collect();
    // Embedding levels: 0 left to right, 1 right to left, 2 left to right inside right to left.
    let base_level: u8 = if base == Direction::Rtl { 1 } else { 0 };
    let mut levels: Vec<u8> = vec![base_level; chars.len()];
    // Joiners between digits belong to the number.
    let is_number = |i: usize| -> bool {
        match classes.get(i) {
            Some(Class::Number) => true,
            Some(Class::Neutral) => {
                let joiner = chars.get(i).is_some_and(|c| matches!(c, '.' | ',' | ':' | '/'));
                joiner && i > 0 && matches!(classes.get(i - 1), Some(Class::Number)) && matches!(classes.get(i + 1), Some(Class::Number))
            }
            _ => false,
        }
    };
    // How each character acts on the neutrals around it: a number acts as left to right after
    // left-to-right text (or at the start of a left-to-right paragraph) and as right to left
    // elsewhere (rules W7 and N1).
    let effective = |i: usize| -> Option<Direction> {
        match classes.get(i) {
            Some(Class::Strong(d)) => Some(*d),
            _ if is_number(i) => Some(preceding_strong(&classes, i).unwrap_or(base)),
            _ => None,
        }
    };
    let strong_level = |d: Direction| -> u8 {
        match (d, base_level) {
            (Direction::Rtl, _) => 1,
            (Direction::Ltr, 1) => 2,
            (Direction::Ltr, _) => 0,
        }
    };
    let mut i = 0;
    while i < chars.len() {
        if let Some(d) = effective(i) {
            // A number reads left to right inside, above right-to-left text around it (I1, I2).
            let level = if is_number(i) && d == Direction::Rtl { 2 } else { strong_level(d) };
            if let Some(l) = levels.get_mut(i) {
                *l = level;
            }
            i += 1;
            continue;
        }
        // A run of neutrals: the direction of both sides when they agree, else the paragraph's.
        let start = i;
        while i < chars.len() && effective(i).is_none() {
            i += 1;
        }
        let before = start.checked_sub(1).and_then(effective).unwrap_or(base);
        let after = effective(i).unwrap_or(base);
        let level = if before == after { strong_level(before) } else { base_level };
        for l in levels.iter_mut().take(i).skip(start) {
            *l = level;
        }
    }
    // Reverse every run at each level from the highest down to the lowest odd one (rule L2).
    let mut order: Vec<usize> = (0..chars.len()).collect();
    let highest = levels.iter().copied().max().unwrap_or(0);
    let mut level = highest;
    while level >= 1 {
        let mut k = 0;
        let at_least = |k: usize, order: &[usize]| order.get(k).and_then(|i| levels.get(*i)).is_some_and(|l| *l >= level);
        while k < order.len() {
            if at_least(k, &order) {
                let start = k;
                while k < order.len() && at_least(k, &order) {
                    k += 1;
                }
                if let Some(run) = order.get_mut(start..k) {
                    run.reverse();
                }
            } else {
                k += 1;
            }
        }
        level -= 1;
    }
    let text = order
        .iter()
        .map(|&i| {
            let c = chars.get(i).copied().unwrap_or(' ');
            if levels.get(i).is_some_and(|l| l % 2 == 1) { mirrored(c) } else { c }
        })
        .collect();
    (text, order)
}

/// Whether a character belongs to a right-to-left script and isn't a digit: what a text shaper
/// lays out from right to left by itself.
pub fn is_rtl_char(c: char) -> bool {
    is_rtl_script(u32::from(c)) && !c.is_numeric()
}

/// A line in drawn order ([`visual_order`]) made ready for a shaper that lays out each run of
/// right-to-left letters from right to left by itself (as HarfBuzz does for a run in one script
/// and one font): every run of such letters goes back into written order, so the shaper turns it
/// round once, not twice. `order` is turned with it.
pub fn for_shaper(drawn: &str, order: &[usize]) -> (String, Vec<usize>) {
    let mut chars: Vec<char> = drawn.chars().collect();
    let mut order: Vec<usize> = order.to_vec();
    order.resize(chars.len(), usize::MAX);
    let mut i = 0;
    while i < chars.len() {
        if !chars.get(i).copied().is_some_and(is_rtl_char) {
            i += 1;
            continue;
        }
        let start = i;
        while chars.get(i).copied().is_some_and(is_rtl_char) {
            i += 1;
        }
        if let Some(run) = chars.get_mut(start..i) {
            run.reverse();
        }
        if let Some(run) = order.get_mut(start..i) {
            run.reverse();
        }
    }
    (chars.into_iter().collect(), order)
}

/// The strong direction of the nearest strong character before `i`.
fn preceding_strong(classes: &[Class], i: usize) -> Option<Direction> {
    classes.get(..i)?.iter().rev().find_map(|c| match c {
        Class::Strong(d) => Some(*d),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_is_that_of_the_first_letter() {
        assert_eq!(direction("שלום world"), Some(Direction::Rtl));
        assert_eq!(direction("123, مرحبا"), Some(Direction::Rtl));
        assert_eq!(direction("Hello שלום"), Some(Direction::Ltr));
        assert_eq!(direction("  12 + 3 = 15"), None);
        assert_eq!(direction(""), None);
        assert_eq!(direction("\u{200F}abc"), Some(Direction::Rtl));
        assert!(is_rtl("سلام") && !is_rtl("Grüße"));
    }

    #[test]
    fn right_to_left_text_is_drawn_reversed() {
        // אבג is drawn as גבא, from left to right.
        let (text, order) = visual_order("אבג", Direction::Rtl);
        assert_eq!(text, "גבא");
        assert_eq!(order, vec![2, 1, 0]);
        // Two words keep their order from the right.
        assert_eq!(visual_order("אב גד", Direction::Rtl).0, "דג בא");
    }

    #[test]
    fn numbers_and_latin_words_read_left_to_right_inside() {
        assert_eq!(visual_order("אב 123 גד", Direction::Rtl).0, "דג 123 בא");
        assert_eq!(visual_order("אב abc גד", Direction::Rtl).0, "דג abc בא");
        assert_eq!(visual_order("אב 3.14 גד", Direction::Rtl).0, "דג 3.14 בא");
        // A Hebrew word inside English.
        assert_eq!(visual_order("say אבג now", Direction::Ltr).0, "say גבא now");
        // Brackets turn round.
        assert_eq!(visual_order("א(ב)", Direction::Rtl).0, "(ב)א");
    }

    #[test]
    fn a_shaper_gets_runs_of_letters_in_written_order() {
        let (drawn, order) = visual_order("אב גד 12", Direction::Rtl);
        assert_eq!(drawn, "12 דג בא");
        let (ready, order) = for_shaper(&drawn, &order);
        // Words right to left, each word's letters as written: the shaper turns them round.
        assert_eq!(ready, "12 גד אב");
        assert_eq!(order, vec![6, 7, 5, 3, 4, 2, 0, 1]);
    }

    #[test]
    fn left_to_right_text_is_untouched_and_orders_are_permutations() {
        let line = "Plain text, 42 times.";
        let (text, order) = visual_order(line, Direction::Ltr);
        assert_eq!(text, line);
        assert_eq!(order, (0..line.chars().count()).collect::<Vec<_>>());
        for sample in ["", " ", "אב 12 cd (ef) גד!", "١٢٣ مرحبا", "a\u{200F}b"] {
            for base in [Direction::Ltr, Direction::Rtl] {
                let (text, order) = visual_order(sample, base);
                let mut sorted = order.clone();
                sorted.sort_unstable();
                assert_eq!(sorted, (0..sample.chars().count()).collect::<Vec<_>>(), "{sample}");
                assert_eq!(text.chars().count(), sample.chars().count());
            }
        }
    }
}
