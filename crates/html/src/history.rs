// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where the earlier messages a reply carries under its own text begin (#1778): the block from
//! which the rest of a message is the conversation it answers, quoted, so a reader that shows
//! that conversation already can leave it out until asked.
//!
//! Read from the blocks, whatever program wrote them, by what readers see of them:
//!
//! - a header of the message answered as Outlook writes it, a line `From:` with `Sent:` or
//!   `Date:` and `To:` or `Subject:` within the next lines (in English, French, German, Spanish,
//!   Italian and Dutch), with the rule or the line of underscores Outlook draws above it, in a
//!   block of its own or in the header's first block;
//! - `-----Original Message-----` (and its dashes in other lengths);
//! - a line `On … wrote:`, as Gmail, Apple Mail and Thunderbird introduce the text they quote.
//!
//! Never from the first block (a message that is all history is shown whole), never in a forward
//! (its subject begins with `Fw:`, `Fwd:` or another language's, [`start_in`], or a line says
//! `Forwarded message`: what it carries is usually nowhere else in the conversation), and never
//! where the owner's reader would lose what the sender wrote between quotes: quoted text followed
//! by more of the sender's own is an answer written in between, and is kept, also under an
//! `On … wrote:` line. A quote at the end with nothing to say whose it is (no `On … wrote:`, no
//! header) is a passage the sender cites, and is kept too. A picture or a table that isn't quoted
//! is the sender's own as text is.
//!
//! Everything here is read from mail, so it is hostile, and the work is linear in the blocks and
//! their spans: what each block shows is worked out once, and what follows each block (an
//! answer after a quote) once, from the end. A message of more than [`MAX_BLOCKS`] blocks is shown
//! whole (what wasn't looked at could hold an answer); of each block, the first
//! [`MAX_LINE_CHARS`] characters of a line, and lines with text only, are compared; a header or
//! a rule is looked for within [`LOOK_AHEAD`] blocks.

use crate::{Block, BlockKind};

/// Most blocks looked at for where the history begins.
pub const MAX_BLOCKS: usize = 4_000;
/// Most characters of a line compared with what a header line says.
pub const MAX_LINE_CHARS: usize = 300;
/// How many lines after a `From:` line the rest of a header may be found in.
const HEADER_LINES: usize = 8;
/// How many blocks after a rule, or after a header's first line, the rest is looked for in.
pub const LOOK_AHEAD: usize = 12;

/// The index of the block from which `blocks` is the quoted history of the conversation, if
/// they have some after text of their own. [`start_in`] for a message whose subject is known.
pub fn start(blocks: &[Block]) -> Option<usize> {
    start_in(blocks, "")
}

/// [`start`] for a message with the subject `subject`: a forward has none.
pub fn start_in(blocks: &[Block], subject: &str) -> Option<usize> {
    if blocks.len() > MAX_BLOCKS || is_forward(subject) {
        return None;
    }
    let read = Read::of(blocks);
    if read.lines.iter().flatten().any(|l| is_forwarded_line(l)) {
        return None;
    }
    let cut = (1..blocks.len()).find(|&i| read.begins_history(i))?;
    // What comes before is the sender's own: there must be something to read in it.
    read.content.get(..cut).unwrap_or_default().iter().any(|c| *c).then_some(cut)
}

/// What is known of each block, worked out once.
struct Read<'a> {
    blocks: &'a [Block],
    /// Its lines with text, trimmed and capped.
    lines: Vec<Vec<String>>,
    /// It shows something: text (all of it looked at, not only the lines sampled), or a picture
    /// or a table, whose words aren't in its spans.
    content: Vec<bool>,
    /// From it on, the sender wrote something after quoted text: a block of their own that comes
    /// after a quoted one. One more than the blocks: none after the last.
    answers_from: Vec<bool>,
}

impl<'a> Read<'a> {
    fn of(blocks: &'a [Block]) -> Read<'a> {
        let lines: Vec<Vec<String>> = blocks.iter().map(lines_of).collect();
        let content: Vec<bool> = blocks
            .iter()
            .zip(&lines)
            .map(|(b, l)| {
                matches!(b.kind, BlockKind::Image { .. } | BlockKind::Table(_))
                    || !l.is_empty()
                    || b.spans.iter().any(|s| s.text.chars().any(|c| !c.is_whitespace()))
            })
            .collect();
        // From the end: whether one of the sender's own blocks is at or after each, and whether
        // a quoted block comes before such a one.
        let mut answers_from = vec![false; blocks.len() + 1];
        let mut own_after = false;
        for i in (0..blocks.len()).rev() {
            let quoted = blocks.get(i).is_some_and(|b| b.quote > 0);
            let later = answers_from.get(i + 1).copied().unwrap_or(false);
            if let Some(slot) = answers_from.get_mut(i) {
                *slot = later || (quoted && own_after);
            }
            if !quoted && content.get(i).copied().unwrap_or(false) {
                own_after = true;
            }
        }
        Read { blocks, lines, content, answers_from }
    }

    fn lines(&self, i: usize) -> &[String] {
        self.lines.get(i).map(Vec::as_slice).unwrap_or_default()
    }

    /// Whether the history begins at block `i`.
    fn begins_history(&self, i: usize) -> bool {
        let lines = self.lines(i);
        let Some(first) = lines.first() else {
            // A rule is where it begins when a header comes right under it.
            return self.blocks.get(i).is_some_and(|b| matches!(b.kind, BlockKind::Rule))
                && self.next_text(i + 1).is_some_and(|j| self.header_at(j, 0));
        };
        if is_separator(first) {
            // Its own block, the header under it; or the header's first line in the same block.
            return if lines.len() > 1 { self.header_at(i, 1) } else { self.next_text(i + 1).is_some_and(|j| self.header_at(j, 0)) };
        }
        // (A rule just above a header, Outlook's line over it, was found first: the blocks are
        // looked at in order.)
        if is_original_message(first) || self.header_at(i, 0) {
            return true;
        }
        // A line that introduces a quote, unless answers are written in the quote under it.
        is_wrote(lines) && !self.answers_from.get(i + 1).copied().unwrap_or(false)
    }

    /// The first block from `from` with a line of text, within [`LOOK_AHEAD`] blocks.
    fn next_text(&self, from: usize) -> Option<usize> {
        (from..self.blocks.len().min(from.saturating_add(LOOK_AHEAD))).find(|&j| !self.lines(j).is_empty())
    }

    /// Whether a header of the message answered begins at line `skip` of block `i`: that line is
    /// `From:`, and the lines that follow, in it and the next blocks, have the date and the
    /// recipients or the subject.
    fn header_at(&self, i: usize, skip: usize) -> bool {
        let end = self.blocks.len().min(i.saturating_add(LOOK_AHEAD));
        let mut following = (i..end).flat_map(|j| self.lines(j)).skip(skip);
        if !following.next().is_some_and(|l| starts_with_any(l, &FROM)) {
            return false;
        }
        let rest: Vec<&String> = following.take(HEADER_LINES).collect();
        rest.iter().any(|l| starts_with_any(l, &SENT)) && rest.iter().any(|l| starts_with_any(l, &TO))
    }
}

/// The block's lines with text, each trimmed and at most [`MAX_LINE_CHARS`] characters, from its
/// first few kilobytes.
fn lines_of(b: &Block) -> Vec<String> {
    let mut text = String::new();
    for span in &b.spans {
        text.push_str(&span.text);
        if text.len() > MAX_LINE_CHARS * HEADER_LINES * 2 {
            break;
        }
    }
    text.lines().map(str::trim).filter(|l| !l.is_empty()).take(HEADER_LINES * 2).map(|l| l.chars().take(MAX_LINE_CHARS).collect::<String>()).collect()
}

/// A line of underscores or dashes and nothing else, as some programs draw a rule in text.
fn is_separator(line: &str) -> bool {
    line.chars().count() >= 8 && line.chars().all(|c| c == '_' || c == '-' || c == '\u{2014}' || c == '\u{2500}')
}

fn is_original_message(line: &str) -> bool {
    let l = line.to_lowercase();
    let words = l.trim_matches(|c: char| c == '-' || c.is_whitespace());
    l.starts_with("---")
        && ["original message", "ursprüngliche nachricht", "message d'origine", "mensaje original", "messaggio originale", "oorspronkelijk bericht"]
            .contains(&words)
}

/// A line that says what follows is a forwarded message, as Gmail (`---------- Forwarded message
/// ---------`) and Apple Mail (`Begin forwarded message:`) write it, or in another language.
fn is_forwarded_line(line: &str) -> bool {
    let l = line.to_lowercase();
    [
        "forwarded message",
        "begin forwarded message",
        "message transféré",
        "weitergeleitete nachricht",
        "mensaje reenviado",
        "messaggio inoltrato",
        "doorgestuurd bericht",
    ]
    .iter()
    .any(|w| l.contains(w))
}

/// Whether a subject is a forward's: its first prefix is `Fw:`, `Fwd:` or another language's
/// (`WG:`, `TR :`, `RV:`, `I:`, `Doorst:`). A forward has no history to leave out ([`start_in`]).
pub fn is_forward(subject: &str) -> bool {
    let Some((prefix, _)) = subject.trim().split_once(':') else { return false };
    let p = prefix.trim().split('[').next().unwrap_or("").trim().to_lowercase();
    ["fw", "fwd", "wg", "tr", "rv", "i", "doorst"].contains(&p.as_str())
}

/// The names a header line of the message answered begins with, by what it says.
const FROM: [&str; 6] = ["from:", "de:", "von:", "da:", "van:", "de :"];
const SENT: [&str; 10] = ["sent:", "date:", "envoyé :", "envoyé:", "gesendet:", "datum:", "enviado:", "fecha:", "inviato:", "verzonden:"];
const TO: [&str; 9] = ["to:", "subject:", "à :", "objet :", "an:", "betreff:", "para:", "asunto:", "oggetto:"];

fn starts_with_any(line: &str, names: &[&str]) -> bool {
    let l = line.to_lowercase();
    names.iter().any(|n| l.starts_with(n))
}

/// Whether a block's lines are a line `On … wrote:` (possibly over two lines, as a long sender
/// breaks it), or its forms in other languages.
fn is_wrote(lines: &[String]) -> bool {
    let text: String = lines.iter().take(2).cloned().collect::<Vec<_>>().join(" ");
    let l = text.to_lowercase();
    let n = l.chars().count();
    if !(8..=MAX_LINE_CHARS).contains(&n) {
        return false;
    }
    let ends = |words: &[&str]| words.iter().any(|w| l.trim_end().ends_with(w));
    (l.starts_with("on ") && ends(&["wrote:", "wrote :"]))
        || (l.starts_with("le ") && ends(&["a écrit :", "a écrit:"]))
        || (l.starts_with("am ") && ends(&["schrieb:", "schrieb :"]))
        || (l.starts_with("el ") && ends(&["escribió:"]))
        || (l.starts_with("il ") && ends(&["ha scritto:"]))
        || (l.starts_with("op ") && ends(&["schreef:"]))
}
