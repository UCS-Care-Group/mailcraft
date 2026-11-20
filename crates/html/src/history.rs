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
//!   Italian and Dutch), with the rule or the line of underscores Outlook draws above it;
//! - `-----Original Message-----` (and its dashes in other lengths);
//! - a line `On … wrote:`, as Gmail, Apple Mail and Thunderbird introduce the text they quote;
//! - from a block on, nothing but quoted text (`<blockquote>`, or `>` in plain text).
//!
//! Never from the first block (a message that is all history is shown whole), and never where
//! the owner's reader would lose what the sender wrote between quotes: quoted text followed by
//! more of the sender's own is an answer written in between, and is kept, also under an
//! `On … wrote:` line. A picture or a table that isn't quoted is the sender's own as text is. The
//! text looked at is bounded: a message of more than [`MAX_BLOCKS`] blocks is shown whole (what
//! wasn't looked at could hold an answer), and of each block the first [`MAX_LINE_CHARS`]
//! characters of a line.

use crate::{Block, BlockKind};

/// Most blocks looked at for where the history begins.
pub const MAX_BLOCKS: usize = 4_000;
/// Most characters of a line compared with what a header line says.
pub const MAX_LINE_CHARS: usize = 300;
/// How many lines after a `From:` line the rest of a header may be found in.
const HEADER_LINES: usize = 8;

/// The index of the block from which `blocks` is the quoted history of the conversation, if
/// they have some after text of their own.
pub fn start(blocks: &[Block]) -> Option<usize> {
    if blocks.len() > MAX_BLOCKS {
        return None;
    }
    // Each block's lines, trimmed, capped.
    let lines: Vec<Vec<String>> = blocks.iter().map(lines_of).collect();
    let cut = (1..blocks.len()).find(|&i| begins_history(blocks, &lines, i))?;
    // What comes before is the sender's own: there must be something to read in it.
    let own = blocks.get(..cut).unwrap_or_default().iter().zip(&lines).any(|(b, l)| has_content(b, l));
    own.then_some(cut)
}

/// Whether a block shows something of its own: text, or a picture or a table, whose words are
/// not in its spans (a rule or an empty paragraph shows nothing).
fn has_content(b: &Block, lines: &[String]) -> bool {
    matches!(b.kind, BlockKind::Image { .. } | BlockKind::Table(_)) || lines.iter().any(|l| !l.is_empty())
}

/// Whether, from block `from` on, the sender wrote something between or after quoted text: a
/// block of their own that comes after a quoted one, an answer written in the quote.
fn answers_in_quote(blocks: &[Block], lines: &[Vec<String>], from: usize) -> bool {
    let mut quoted = false;
    for (b, l) in blocks.iter().zip(lines).skip(from) {
        if b.quote > 0 {
            quoted = true;
        } else if quoted && has_content(b, l) {
            return true;
        }
    }
    false
}

/// Whether the history begins at block `i`.
fn begins_history(blocks: &[Block], lines: &[Vec<String>], i: usize) -> bool {
    let Some(first) = lines.get(i).and_then(|l| l.iter().find(|l| !l.is_empty())) else {
        // A rule or an empty line is where it begins when a header comes right under it.
        return is_rule(blocks.get(i), lines.get(i)) && next_text(lines, i + 1).is_some_and(|j| header_at(lines, j));
    };
    if is_separator(first) {
        return next_text(lines, i + 1).is_some_and(|j| header_at(lines, j));
    }
    // (A rule just above a header, Outlook's line over it, was found first: the blocks are
    // looked at in order.)
    if is_original_message(first) || header_at(lines, i) {
        return true;
    }
    // A line that introduces a quote, unless answers are written in the quote under it.
    if is_wrote(lines, i) {
        return !answers_in_quote(blocks, lines, i + 1);
    }
    // From here on, quoted text alone.
    blocks.get(i).is_some_and(|b| b.quote > 0)
        && blocks.get(i - 1).is_some_and(|b| b.quote == 0)
        && blocks.iter().zip(lines).skip(i).all(|(b, l)| b.quote > 0 || !has_content(b, l))
}

/// The block's text, line by line, each trimmed and at most [`MAX_LINE_CHARS`] characters.
fn lines_of(b: &Block) -> Vec<String> {
    let mut text = String::new();
    for span in &b.spans {
        text.push_str(&span.text);
        if text.len() > MAX_LINE_CHARS * HEADER_LINES * 2 {
            break;
        }
    }
    text.lines().take(HEADER_LINES * 2).map(|l| l.trim().chars().take(MAX_LINE_CHARS).collect::<String>()).collect()
}

/// The first block at or after `from` with any text, by index.
fn next_text(lines: &[Vec<String>], from: usize) -> Option<usize> {
    (from..lines.len()).find(|&j| lines.get(j).is_some_and(|l| l.iter().any(|l| !l.is_empty())))
}

fn is_rule(b: Option<&Block>, lines: Option<&Vec<String>>) -> bool {
    b.is_some_and(|b| matches!(b.kind, BlockKind::Rule)) || lines.and_then(|l| l.iter().find(|l| !l.is_empty())).is_some_and(|l| is_separator(l))
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

/// The names a header line of the message answered begins with, by what it says.
const FROM: [&str; 6] = ["from:", "de:", "von:", "da:", "van:", "de :"];
const SENT: [&str; 10] = ["sent:", "date:", "envoyé :", "envoyé:", "gesendet:", "datum:", "enviado:", "fecha:", "inviato:", "verzonden:"];
const TO: [&str; 9] = ["to:", "subject:", "à :", "objet :", "an:", "betreff:", "para:", "asunto:", "oggetto:"];

fn starts_with_any(line: &str, names: &[&str]) -> bool {
    let l = line.to_lowercase();
    names.iter().any(|n| l.starts_with(n))
}

/// Whether a header of the message answered begins at block `i`: its first line is `From:`, and
/// the lines that follow, in it and the blocks after it, have the date and the recipients or the
/// subject.
fn header_at(lines: &[Vec<String>], i: usize) -> bool {
    let mut following = lines.iter().skip(i).flatten().filter(|l| !l.is_empty());
    if !following.next().is_some_and(|l| starts_with_any(l, &FROM)) {
        return false;
    }
    let rest: Vec<&String> = following.take(HEADER_LINES).collect();
    rest.iter().any(|l| starts_with_any(l, &SENT)) && rest.iter().any(|l| starts_with_any(l, &TO))
}

/// Whether block `i` is a line `On … wrote:` (possibly over two lines, as a long sender breaks it),
/// or its forms in other languages.
fn is_wrote(lines: &[Vec<String>], i: usize) -> bool {
    let Some(block) = lines.get(i) else { return false };
    let text: String = block.iter().filter(|l| !l.is_empty()).take(2).cloned().collect::<Vec<_>>().join(" ");
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
