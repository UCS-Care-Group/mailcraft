// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Media queries: what a message's `@media` blocks and `<style media>` are tested against.
//!
//! A message is written for any screen, so its style sheet says what to do on a narrow one
//! (`@media (max-width: 480px)`) and in a dark window (`@media (prefers-color-scheme: dark)`).
//! Two facts are known here, and nothing else: the width of the reading pane, as one of a small
//! number of classes ([`Env::for_pane`]), and whether the window is dark. Nothing is asked of the
//! system and nothing is fetched.
//!
//! What is understood: the media types `all` and `screen` (true) and `print` and every other
//! type (false); `min-width`, `max-width` and `width` (and their `device-` forms) with a length in
//! `px`, `pt`, `em` or `rem` (an `em` is 16 pixels); `prefers-color-scheme` (`light` or `dark`);
//! `and`, `not` and `only`; and a comma list, of which any true query makes the list true. A
//! query that is not understood is false, as in CSS, and `not` does not turn it true.
//!
//! The text is hostile: a list is read to [`MAX_QUERY_BYTES`] bytes, [`MAX_QUERIES`] queries and
//! [`MAX_FEATURES`] features in a query; what is past those is false. `@media` inside `@media` is
//! read to a depth of [`MAX_NESTING`] ([`crate::sheet`]).

/// Longest media query list read, in bytes; a longer one is false.
pub(crate) const MAX_QUERY_BYTES: usize = 512;

/// Most queries read from one comma list; the rest are not looked at.
pub(crate) const MAX_QUERIES: usize = 8;

/// Most `and` parts read in one query; a query with more is false.
pub(crate) const MAX_FEATURES: usize = 8;

/// Deepest `@media` inside `@media` that is read (the outer one is level 1).
pub(crate) const MAX_NESTING: usize = 3;

/// Most media lists tested in one message; the blocks after them are left out.
pub(crate) const MAX_LISTS: usize = 400;

/// The widths at which the class of a pane changes. A message's breakpoints sit at the common
/// ones; a pane one pixel either side of such a width is in different classes, so `max-width:
/// 600px` and `min-width: 601px` are decided as a browser would decide them.
const BREAKS: [u16; 19] = [320, 360, 375, 414, 480, 540, 568, 600, 620, 640, 667, 700, 720, 768, 800, 900, 992, 1024, 1200];

/// The widest pane there is any need to tell from another.
const WIDEST: u16 = 1280;

/// What the media queries are tested against: how wide the reading pane is, as a class, and the
/// window's theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Env {
    width: u16,
    dark: bool,
}

impl Default for Env {
    /// A wide pane in the light theme: a desktop's, and what a reader with no pane gets.
    fn default() -> Env {
        Env { width: WIDEST, dark: false }
    }
}

impl Env {
    /// The class of a pane `points` wide (a point is a CSS pixel here), in a dark window or a
    /// light one. The class is named by the least width in it: each break and the width just
    /// above it are classes of their own, and every width from [`WIDEST`] up is one class.
    pub fn for_pane(points: f32, dark: bool) -> Env {
        let px = if points.is_finite() { points.clamp(0.0, f32::from(WIDEST)) as u16 } else { WIDEST };
        let width = if px >= WIDEST { WIDEST } else { BREAKS.iter().flat_map(|b| [*b, b + 1]).rfind(|edge| *edge <= px).unwrap_or(0) };
        Env { width, dark }
    }

    /// The width the queries are tested against, in pixels: the least width of the class.
    pub fn width(&self) -> u16 {
        self.width
    }

    /// Whether the window is dark.
    pub fn dark(&self) -> bool {
        self.dark
    }
}

/// What a media query list came to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Verdict {
    /// Some query is true.
    pub holds: bool,
    /// A query that holds names `prefers-color-scheme: dark`: the sender designed this for a dark window.
    pub dark: bool,
}

/// Test a media query list (the text between `@media` and `{`, or a `media` attribute).
pub(crate) fn test(list: &str, env: &Env) -> Verdict {
    let list = list.trim();
    let mut verdict = Verdict::default();
    // An empty list is `all`.
    if list.is_empty() {
        verdict.holds = true;
        return verdict;
    }
    if list.len() > MAX_QUERY_BYTES {
        return verdict;
    }
    for query in list.split(',').take(MAX_QUERIES) {
        if let Some((holds, dark)) = query_holds(query.trim(), env)
            && holds
        {
            verdict.holds = true;
            verdict.dark |= dark;
        }
    }
    verdict
}

/// Whether one query holds and whether it asks for the dark theme; `None` when it is not understood.
fn query_holds(query: &str, env: &Env) -> Option<(bool, bool)> {
    let lower = query.to_ascii_lowercase();
    let mut words = Parts::new(&lower);
    let mut negate = false;
    let mut first = words.peek()?;
    if first == "not" || first == "only" {
        negate = first == "not";
        words.next();
        first = words.peek()?;
    }
    let (mut holds, mut dark) = (true, false);
    let mut features = 0usize;
    if !first.starts_with('(') {
        // A media type: a name, and not one of the words that join queries.
        if !first.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') || matches!(first, "and" | "not" | "only" | "or") {
            return None;
        }
        words.next();
        holds = matches!(first, "all" | "screen");
        // After a type, nothing, or `and` and a feature.
        if words.peek().is_some() && (words.next()? != "and" || words.peek().is_none()) {
            return None;
        }
    }
    while let Some(part) = words.peek() {
        if !part.starts_with('(') {
            return None;
        }
        words.next();
        features += 1;
        if features > MAX_FEATURES {
            return None;
        }
        let (ok, wants_dark) = feature(part, env)?;
        holds &= ok;
        dark |= wants_dark && ok;
        match words.next() {
            None => break,
            Some("and") if words.peek().is_some() => {}
            Some(_) => return None,
        }
    }
    Some(if negate { (!holds, false) } else { (holds, dark && holds) })
}

/// One `(name: value)`, as `(holds, asks for dark)`; `None` for what is not understood.
fn feature(part: &str, env: &Env) -> Option<(bool, bool)> {
    let inner = part.strip_prefix('(')?.strip_suffix(')')?;
    let (name, value) = inner.split_once(':')?;
    let (name, value) = (name.trim(), value.trim());
    match name {
        "prefers-color-scheme" => match value {
            "dark" => Some((env.dark, true)),
            "light" => Some((!env.dark, false)),
            _ => None,
        },
        "min-width" | "min-device-width" => Some((f32::from(env.width) >= length(value)?, false)),
        "max-width" | "max-device-width" => Some((f32::from(env.width) <= length(value)?, false)),
        "width" | "device-width" => Some(((f32::from(env.width) - length(value)?).abs() < 0.5, false)),
        _ => None,
    }
}

/// A length in pixels: `600px`, `37.5em`, `0`.
fn length(value: &str) -> Option<f32> {
    let value = value.trim();
    if value.len() > 16 {
        return None;
    }
    let split = value.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let number: f32 = number.parse().ok().filter(|n: &f32| n.is_finite())?;
    let px = match unit.trim() {
        "px" => number,
        "em" | "rem" => number * 16.0,
        "pt" => number * 4.0 / 3.0,
        "" if number == 0.0 => 0.0,
        _ => return None,
    };
    Some(px.clamp(0.0, 100_000.0))
}

/// The words of a query, with a parenthesised feature as one word.
struct Parts<'a> {
    text: &'a str,
}

impl<'a> Parts<'a> {
    fn new(text: &'a str) -> Parts<'a> {
        Parts { text: text.trim_start() }
    }

    fn peek(&self) -> Option<&'a str> {
        let t = self.text;
        if t.is_empty() {
            return None;
        }
        let end = if t.starts_with('(') {
            // To the first `)`: a feature has no brackets inside.
            t.find(')').map_or(t.len(), |i| i + 1)
        } else {
            t.find(|c: char| c.is_whitespace() || c == '(').unwrap_or(t.len())
        };
        t.get(..end)
    }
}

impl<'a> Iterator for Parts<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        let word = self.peek()?;
        self.text = self.text.get(word.len()..).unwrap_or("").trim_start();
        Some(word)
    }
}

#[cfg(test)]
mod tests;
