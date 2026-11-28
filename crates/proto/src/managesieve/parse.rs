// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reading what a ManageSieve server sends (RFC 5804 §4): one response at a time, made of atoms,
//! strings (quoted, or counted literals `{n}` that may run over several lines) and parentheses.
//! Everything is bounded: the length of a line ([`crate::MAX_LINE`]), of a literal
//! ([`MAX_STRING`]), of a whole response ([`MAX_RESPONSE`]) and the number of its parts
//! ([`MAX_TOKENS`]).

use std::io::{Read, Write};

use crate::ProtoError;
use crate::line::Wire;
use mailcraft_i18n::said;

/// Longest string a server may send in one piece: a script, with room to spare (RFC 5804 lets a
/// server cap scripts, and the ones MailCraft writes are a few kilobytes).
pub(crate) const MAX_STRING: usize = 1024 * 1024;
/// Most bytes one response may take, its strings included.
pub(crate) const MAX_RESPONSE: usize = MAX_STRING + 64 * 1024;
/// Most parts (atoms, strings, parentheses) one response may have.
pub(crate) const MAX_TOKENS: usize = 256;
/// Most literals one response may carry.
const MAX_LITERALS: usize = 4;

/// One part of a response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Token {
    /// A word outside quotes (`OK`, `ACTIVE`, `QUOTA/MAXSIZE`, a number), as the server wrote it.
    Atom(String),
    /// A quoted string or a literal, its bytes as they came (escapes taken out).
    Str(Vec<u8>),
    Open,
    Close,
}

impl Token {
    /// The atom upper-cased, for comparing.
    pub(crate) fn keyword(&self) -> Option<String> {
        match self {
            Token::Atom(word) => Some(word.to_ascii_uppercase()),
            _ => None,
        }
    }

    /// The string as text (a server's bytes that aren't UTF-8 are shown as best they can be).
    pub(crate) fn text(&self) -> Option<String> {
        match self {
            Token::Str(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
            _ => None,
        }
    }

    /// The string as text, only when it is UTF-8 throughout: a script's name or text, which has
    /// to be sent back byte for byte, is never mended.
    pub(crate) fn utf8(&self) -> Option<Result<String, ()>> {
        match self {
            Token::Str(bytes) => Some(String::from_utf8(bytes.clone()).map_err(|_| ())),
            _ => None,
        }
    }

    /// About how many bytes the part takes, for bounding what one command keeps.
    pub(crate) fn size(&self) -> usize {
        match self {
            Token::Atom(word) => word.len().saturating_add(1),
            Token::Str(bytes) => bytes.len().saturating_add(2),
            Token::Open | Token::Close => 1,
        }
    }
}

fn too_long() -> ProtoError {
    ProtoError::Protocol(said!("the server's response is too long"))
}

fn too_many() -> ProtoError {
    ProtoError::Protocol(said!("the server's response has too many parts"))
}

fn not_managesieve() -> ProtoError {
    ProtoError::Protocol(said!("the server sent something that isn't a ManageSieve response"))
}

/// The next response: one line, and the lines its literals carry on to.
pub(crate) fn read_response<S: Read + Write>(wire: &mut Wire<S>) -> Result<Vec<Token>, ProtoError> {
    let mut tokens = Vec::new();
    let mut size = 0usize;
    let mut literals = 0usize;
    let mut line = wire.read_line()?;
    loop {
        size = size.saturating_add(line.len());
        if size > MAX_RESPONSE {
            return Err(too_long());
        }
        match scan(&line, &mut tokens)? {
            None => return Ok(tokens),
            Some(n) => {
                literals += 1;
                if literals > MAX_LITERALS {
                    return Err(too_many());
                }
                if n > MAX_STRING || size.saturating_add(n) > MAX_RESPONSE {
                    return Err(ProtoError::Protocol(said!("the server sent {n} bytes in one piece, more than MailCraft accepts", n)));
                }
                size = size.saturating_add(n);
                tokens.push(Token::Str(wire.read_block(n)?));
                if tokens.len() > MAX_TOKENS {
                    return Err(too_many());
                }
                // What follows the literal is the rest of the same response.
                line = wire.read_line()?;
            }
        }
    }
}

/// The parts of one line, added to `tokens`; `Some(n)` when the line ends in a literal of `n`
/// bytes, which come next.
pub(crate) fn scan(line: &[u8], tokens: &mut Vec<Token>) -> Result<Option<usize>, ProtoError> {
    let mut at = 0usize;
    while let Some(&b) = line.get(at) {
        if tokens.len() >= MAX_TOKENS {
            return Err(too_many());
        }
        match b {
            b' ' | b'\t' => at += 1,
            b'(' => {
                tokens.push(Token::Open);
                at += 1;
            }
            b')' => {
                tokens.push(Token::Close);
                at += 1;
            }
            b'"' => {
                let (text, end) = quoted(line, at.saturating_add(1))?;
                tokens.push(Token::Str(text));
                at = end;
            }
            b'{' => {
                let rest = line.get(at.saturating_add(1)..).unwrap_or_default();
                let close = rest.iter().position(|c| *c == b'}').ok_or_else(not_managesieve)?;
                // Nothing may follow the count but the end of the line.
                if close.saturating_add(1) != rest.len() {
                    return Err(not_managesieve());
                }
                let count = rest.get(..close).unwrap_or_default();
                // A server's literal is `{n}`; `{n+}` is how a client writes one, taken all the same.
                let count = count.strip_suffix(b"+").unwrap_or(count);
                if count.is_empty() || count.len() > 10 || !count.iter().all(u8::is_ascii_digit) {
                    return Err(not_managesieve());
                }
                let n = std::str::from_utf8(count).ok().and_then(|n| n.parse::<usize>().ok()).ok_or_else(not_managesieve)?;
                return Ok(Some(n));
            }
            _ if is_atom_char(b) => {
                let len = line.get(at..).unwrap_or_default().iter().take_while(|c| is_atom_char(**c)).count();
                let word = line.get(at..at.saturating_add(len)).unwrap_or_default();
                tokens.push(Token::Atom(String::from_utf8_lossy(word).into_owned()));
                at = at.saturating_add(len.max(1));
            }
            _ => return Err(not_managesieve()),
        }
    }
    Ok(None)
}

/// A quoted string from just after its opening quote: its bytes, and where the line goes on.
fn quoted(line: &[u8], start: usize) -> Result<(Vec<u8>, usize), ProtoError> {
    let mut out = Vec::new();
    let mut at = start;
    loop {
        match line.get(at) {
            None => return Err(not_managesieve()),
            Some(b'"') => return Ok((out, at.saturating_add(1))),
            Some(b'\\') => {
                // `\"` and `\\` are what the grammar has; anything else escaped is taken as itself.
                let next = line.get(at.saturating_add(1)).ok_or_else(not_managesieve)?;
                out.push(*next);
                at = at.saturating_add(2);
            }
            Some(0) => return Err(not_managesieve()),
            Some(b) => {
                out.push(*b);
                at = at.saturating_add(1);
            }
        }
    }
}

/// A byte that can be part of an atom: visible ASCII but for the ones that start something else,
/// and the bytes of UTF-8 past ASCII (which some servers put in a word of their own).
fn is_atom_char(b: u8) -> bool {
    (b.is_ascii_graphic() && !matches!(b, b'(' | b')' | b'"' | b'{' | b'\\')) || b >= 0x80
}
