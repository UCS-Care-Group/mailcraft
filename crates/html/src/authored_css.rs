// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The authored signature writer's ([`crate::authored`]) own reading of CSS text the way a CSS
//! reader (CSS Syntax Level 3) reads it: comments dropped, strings read to their quote with their
//! escapes, and `(`, `[` and `{` paired by kind. The writer echoes selector, `@media` and
//! declaration text that the cascade's reader ([`crate::sheet`]) accepted; these checks make sure
//! a recipient's program can't read that text into a different shape.
//!
//! * [`balanced`]: every bracket closed, in order, by its own kind of closer.
//! * [`functions_known`]: every `(` of a selector list opens the arguments of a pseudo-class the
//!   cascade reads them of (`:not()`, `:nth-child()`, `:nth-of-type()`).
//! * [`scoped`]: the whole written sheet, read again, is nothing but `@media` blocks and rules
//!   each of whose selectors starts with the scope class, with nothing left open at its end.
//!
//! What this doesn't read as plainly as a CSS reader is refused: a comment without its end, a
//! string with a line break or without its end, an escape of a line break or of nothing, a
//! `url(` (whose unquoted form a CSS reader reads differently), a `<` and a `-->`. The text is
//! already capped by the writer; the work here is linear in it, without recursion, and the
//! brackets and `@media` levels held open are capped too.

/// Most brackets held open at once.
const MAX_OPEN: usize = 256;

/// Most `@media` blocks inside one another the written sheet may have.
const MAX_LEVELS: usize = 8;

/// The pseudo-classes whose arguments the cascade's selector reader parses.
const FUNCTIONS: [&str; 3] = ["not", "nth-child", "nth-of-type"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// White space.
    Space,
    /// A name or number: letters, digits, `-`, `_`, non-ASCII and escapes.
    Word,
    /// A whole string, its quotes too.
    Str,
    /// `(` (alone, or after a name: a function), `[` or `{`.
    Open(u8),
    /// `)`, `]` or `}`.
    Close(u8),
    Comma,
    Semi,
    /// `@` and a name.
    At,
    /// Any other character.
    Delim(u8),
}

#[derive(Clone, Copy, Debug)]
struct Tok {
    kind: Kind,
    start: usize,
    end: usize,
}

fn space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0c)
}

fn word_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c >= 0x80
}

/// The bracket that closes `open`.
fn closer(open: u8) -> u8 {
    match open {
        b'(' => b')',
        b'[' => b']',
        _ => b'}',
    }
}

/// Where the escape whose `\` is at `i` ends, outside a string; `None` for an escape of a line
/// break or of nothing, which a CSS reader doesn't read as an escape.
fn escape_end(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i.checked_add(1)?;
    let next = *b.get(j)?;
    if matches!(next, b'\n' | b'\r' | 0x0c) {
        return None;
    }
    if next.is_ascii_hexdigit() {
        let mut digits = 0;
        while digits < 6 && b.get(j).is_some_and(u8::is_ascii_hexdigit) {
            j = j.saturating_add(1);
            digits += 1;
        }
        // One white space after the digits belongs to the escape (CR LF as one).
        if b.get(j) == Some(&b'\r') && b.get(j.saturating_add(1)) == Some(&b'\n') {
            j = j.saturating_add(2);
        } else if b.get(j).copied().is_some_and(space) {
            j = j.saturating_add(1);
        }
        return Some(j);
    }
    // Any other character, whole: its continuation bytes are word bytes anyway.
    Some(j.saturating_add(1))
}

/// Where the string whose quote is at `i` ends, after its closing quote; `None` for a string
/// with a line break or without its end.
fn string_end(b: &[u8], i: usize) -> Option<usize> {
    let quote = *b.get(i)?;
    let mut j = i.checked_add(1)?;
    loop {
        match *b.get(j)? {
            c if c == quote => return j.checked_add(1),
            b'\n' | b'\r' | 0x0c => return None,
            b'\\' => {
                let next = j.checked_add(1)?;
                // An escaped line break goes on to the next line (CR LF as one).
                j = match b.get(next) {
                    None => return None,
                    Some(b'\r') if b.get(next.saturating_add(1)) == Some(&b'\n') => next.saturating_add(2),
                    Some(_) => next.saturating_add(1),
                };
            }
            _ => j = j.saturating_add(1),
        }
    }
}

/// The tokens of `css` with its comments dropped, or `None` for what isn't read as plainly as a
/// CSS reader reads it (see the module's words).
fn tokens(css: &str) -> Option<Vec<Tok>> {
    let b = css.as_bytes();
    let mut out: Vec<Tok> = Vec::new();
    let mut i = 0usize;
    while let Some(&c) = b.get(i) {
        let start = i;
        let kind = match c {
            b'/' if b.get(i.saturating_add(1)) == Some(&b'*') => {
                let from = i.checked_add(2)?;
                let at = b.get(from..)?.windows(2).position(|w| w == b"*/")?;
                i = from.checked_add(at)?.checked_add(2)?;
                continue;
            }
            b'<' => return None,
            b'-' if b.get(i..i.saturating_add(3)) == Some(b"-->") => return None,
            _ if space(c) => {
                while b.get(i).copied().is_some_and(space) {
                    i = i.saturating_add(1);
                }
                Kind::Space
            }
            b'"' | b'\'' => {
                i = string_end(b, i)?;
                Kind::Str
            }
            b'\\' => {
                i = word_end(b, i)?;
                Kind::Word
            }
            _ if word_byte(c) => {
                i = word_end(b, i)?;
                Kind::Word
            }
            b'@' if b.get(i.saturating_add(1)).is_some_and(|&n| word_byte(n) || n == b'\\') => {
                i = word_end(b, i.saturating_add(1))?;
                Kind::At
            }
            b'(' => {
                // `url(` is read differently when what follows isn't a string: never written.
                let name = out.last().filter(|t| t.kind == Kind::Word && t.end == i).and_then(|t| css.get(t.start..t.end));
                if name.is_some_and(|n| n.eq_ignore_ascii_case("url") || n.contains('\\')) {
                    return None;
                }
                i = i.saturating_add(1);
                Kind::Open(c)
            }
            b'[' | b'{' => {
                i = i.saturating_add(1);
                Kind::Open(c)
            }
            b')' | b']' | b'}' => {
                i = i.saturating_add(1);
                Kind::Close(c)
            }
            b',' => {
                i = i.saturating_add(1);
                Kind::Comma
            }
            b';' => {
                i = i.saturating_add(1);
                Kind::Semi
            }
            _ => {
                i = i.saturating_add(1);
                Kind::Delim(c)
            }
        };
        out.push(Tok { kind, start, end: i });
    }
    Some(out)
}

/// Where the name that starts at `i` ends.
fn word_end(b: &[u8], mut i: usize) -> Option<usize> {
    loop {
        match b.get(i) {
            Some(b'\\') => i = escape_end(b, i)?,
            Some(&c) if word_byte(c) => i = i.saturating_add(1),
            _ => return Some(i),
        }
    }
}

/// Whether every `(`, `[` and `{` of `css` is closed, in order, by its own closer, and nothing
/// is closed that wasn't opened, as a CSS reader pairs them.
pub(crate) fn balanced(css: &str) -> bool {
    let Some(toks) = tokens(css) else { return false };
    let mut open: Vec<u8> = Vec::new();
    for t in &toks {
        match t.kind {
            Kind::Open(c) => {
                if open.len() >= MAX_OPEN {
                    return false;
                }
                open.push(c);
            }
            Kind::Close(c) => {
                let paired = open.pop().map(closer) == Some(c);
                if !paired {
                    return false;
                }
            }
            _ => {}
        }
    }
    open.is_empty()
}

/// Whether every `(` of the selector list `css` opens the arguments of a pseudo-class whose
/// arguments the cascade reads ([`FUNCTIONS`]): written `:name(`, all in one piece.
pub(crate) fn functions_known(css: &str) -> bool {
    tokens(css).is_some_and(|toks| functions_known_in(css, &toks))
}

fn functions_known_in(css: &str, toks: &[Tok]) -> bool {
    toks.iter().enumerate().all(|(n, t)| {
        if t.kind != Kind::Open(b'(') {
            return true;
        }
        let at = |back: usize| n.checked_sub(back).and_then(|p| toks.get(p));
        let named = at(1).filter(|w| w.kind == Kind::Word && w.end == t.start);
        let Some(name) = named.and_then(|w| css.get(w.start..w.end)) else { return false };
        let colon = at(2).filter(|c| c.kind == Kind::Delim(b':') && named.is_some_and(|w| c.end == w.start));
        let double = at(3).is_some_and(|d| d.kind == Kind::Delim(b':') && colon.is_some_and(|c| d.end == c.start));
        colon.is_some() && !double && FUNCTIONS.iter().any(|f| name.eq_ignore_ascii_case(f))
    })
}

/// The index of the `{` that ends the prelude starting at token `from`, its brackets paired; `None`
/// when it has a `;`, an at-keyword or an unpaired closer, or no `{`.
fn prelude_end(toks: &[Tok], from: usize) -> Option<usize> {
    let mut open: Vec<u8> = Vec::new();
    let mut k = from;
    loop {
        let t = toks.get(k)?;
        match t.kind {
            Kind::Open(b'{') if open.is_empty() => return Some(k),
            Kind::Open(b'{') | Kind::Semi | Kind::At => return None,
            Kind::Open(c) => {
                if open.len() >= MAX_OPEN {
                    return None;
                }
                open.push(c);
            }
            Kind::Close(c) => {
                let paired = open.pop().map(closer) == Some(c);
                if !paired {
                    return None;
                }
            }
            _ => {}
        }
        k = k.checked_add(1)?;
    }
}

/// The index of the `}` that closes the declaration block whose `{` is token `open`, its
/// brackets paired; `None` when it holds a block, an at-keyword or an unpaired closer, or has
/// no end.
fn declarations_end(toks: &[Tok], open_at: usize) -> Option<usize> {
    let mut open: Vec<u8> = vec![b'{'];
    let mut k = open_at.checked_add(1)?;
    loop {
        let t = toks.get(k)?;
        match t.kind {
            Kind::Open(b'{') | Kind::At => return None,
            Kind::Open(c) => {
                if open.len() >= MAX_OPEN {
                    return None;
                }
                open.push(c);
            }
            Kind::Close(c) => {
                if open.pop().map(closer) != Some(c) {
                    return None;
                }
                if open.is_empty() {
                    return Some(k);
                }
            }
            _ => {}
        }
        k = k.checked_add(1)?;
    }
}

/// Whether each selector of the list `prelude` (tokens of `css`) starts with the scope class,
/// as `.SCOPE` or `div.SCOPE`, is not followed by a sibling combinator there, and has no `(`
/// but those [`functions_known`] allows.
fn selectors_scoped(css: &str, prelude: &[Tok], scope: &str) -> bool {
    if !functions_known_in(css, prelude) {
        return false;
    }
    let text = |t: &Tok| css.get(t.start..t.end).unwrap_or("");
    let mut depth = 0usize;
    let mut parts: Vec<&[Tok]> = Vec::new();
    let mut start = 0usize;
    for (n, t) in prelude.iter().enumerate() {
        match t.kind {
            Kind::Open(_) => depth = depth.saturating_add(1),
            Kind::Close(_) => depth = depth.saturating_sub(1),
            Kind::Comma if depth == 0 => {
                parts.push(prelude.get(start..n).unwrap_or(&[]));
                start = n.saturating_add(1);
            }
            _ => {}
        }
    }
    parts.push(prelude.get(start..).unwrap_or(&[]));
    parts.into_iter().all(|part| {
        let first = part.iter().position(|t| t.kind != Kind::Space).unwrap_or(part.len());
        let part = part.get(first..).unwrap_or(&[]);
        let class = |dot: Option<&Tok>, name: Option<&Tok>| match (dot, name) {
            (Some(d), Some(w)) => d.kind == Kind::Delim(b'.') && w.kind == Kind::Word && d.end == w.start && text(w) == scope,
            _ => false,
        };
        let rest = if class(part.first(), part.get(1)) {
            part.get(2..)
        } else if part.first().is_some_and(|d| d.kind == Kind::Word && text(d) == "div" && part.get(1).is_some_and(|p| p.start == d.end))
            && class(part.get(1), part.get(2))
        {
            part.get(3..)
        } else {
            None
        };
        let Some(rest) = rest else { return false };
        // The rest of the leftmost compound, to the first white space or combinator outside
        // brackets; then no `+` or `~`.
        let mut depth = 0usize;
        let mut j = 0usize;
        while let Some(t) = rest.get(j) {
            match t.kind {
                Kind::Open(_) => depth = depth.saturating_add(1),
                Kind::Close(_) => depth = depth.saturating_sub(1),
                Kind::Space | Kind::Delim(b'>' | b'+' | b'~') if depth == 0 => break,
                _ => {}
            }
            j = j.saturating_add(1);
        }
        let next = rest.get(j..).unwrap_or(&[]).iter().find(|t| t.kind != Kind::Space);
        !next.is_some_and(|t| matches!(t.kind, Kind::Delim(b'+' | b'~')))
    })
}

/// Whether the written sheet `css`, read as a CSS reader reads it, is nothing but `@media` blocks
/// and rules whose every selector is under `scope` ([`selectors_scoped`]), with declaration
/// blocks that hold no block or at-rule, every bracket paired, and nothing open at the end.
pub(crate) fn scoped(css: &str, scope: &str) -> bool {
    let Some(toks) = tokens(css) else { return false };
    let mut k = 0usize;
    let mut levels = 0usize;
    loop {
        while toks.get(k).is_some_and(|t| t.kind == Kind::Space) {
            k = k.saturating_add(1);
        }
        let Some(t) = toks.get(k) else { return levels == 0 };
        match t.kind {
            Kind::Close(b'}') if levels > 0 => {
                levels -= 1;
                k = k.saturating_add(1);
            }
            Kind::At => {
                let name = css.get(t.start.saturating_add(1)..t.end);
                if !name.is_some_and(|n| n.eq_ignore_ascii_case("media")) {
                    return false;
                }
                levels = levels.saturating_add(1);
                if levels > MAX_LEVELS {
                    return false;
                }
                let Some(open) = k.checked_add(1).and_then(|from| prelude_end(&toks, from)) else { return false };
                k = open.saturating_add(1);
            }
            _ => {
                let Some(open) = prelude_end(&toks, k) else { return false };
                let Some(prelude) = toks.get(k..open) else { return false };
                if !selectors_scoped(css, prelude, scope) {
                    return false;
                }
                let Some(close) = declarations_end(&toks, open) else { return false };
                k = close.saturating_add(1);
            }
        }
    }
}
