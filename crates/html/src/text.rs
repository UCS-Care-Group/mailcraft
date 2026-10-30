// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Plain text: a text body as display blocks or HTML, and HTML as readable text.

use std::borrow::Cow;

use crate::entities::escape;
use crate::{Block, BlockKind, MAX_BLOCKS, MAX_INPUT, Marker, Span, Table, truncate};

/// Deepest quote written as `>` marks or `<blockquote>` elements; deeper quotes are written at this depth.
const MAX_QUOTE_MARKS: usize = 32;

/// Deepest list indent written in plain text.
const MAX_INDENT: usize = 16;

/// Longest address made into a link; a longer one stays text.
const MAX_URL: usize = 2048;

/// A plain-text body as display blocks.
///
/// Paragraphs are separated by blank lines (`space` 1); a line break inside a paragraph stays a
/// `\n` in the span text. Leading `>` marks become the block's `quote` depth and are removed; text
/// that changes its depth without a blank line starts a block right under the one before
/// (`space` 0). Bare `http://`, `https://` and `www.` addresses and e-mail addresses become link
/// spans.
pub fn text_blocks(text: &str) -> Vec<Block> {
    let text = truncate(text, MAX_INPUT);
    let text: Cow<'_, str> = if text.contains('\r') { Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n")) } else { Cow::Borrowed(text) };
    let mut blocks: Vec<Block> = Vec::new();
    let mut paragraph = String::new();
    let mut depth = 0u8;
    // Whether a blank line came before the paragraph being read, and before the line being read.
    let mut gap = false;
    let mut blank = false;
    let mut flush = |paragraph: &mut String, depth: u8, gap: bool| {
        if !paragraph.is_empty() && blocks.len() < MAX_BLOCKS {
            let space = u8::from(gap && !blocks.is_empty());
            blocks.push(Block { quote: depth, space, ..Block::new(BlockKind::Paragraph, autolink(paragraph)) });
        }
        paragraph.clear();
    };
    for line in text.split('\n') {
        let (quote, rest) = split_quote(line);
        let rest: String = rest.chars().filter(|c| *c == '\t' || !c.is_control()).collect();
        let rest = rest.trim_end();
        if rest.trim_start().is_empty() {
            flush(&mut paragraph, depth, gap);
            blank = true;
            continue;
        }
        if !paragraph.is_empty() && quote != depth {
            flush(&mut paragraph, depth, gap);
        }
        if paragraph.is_empty() {
            depth = quote;
            gap = std::mem::take(&mut blank);
        } else {
            paragraph.push('\n');
        }
        paragraph.push_str(rest);
    }
    flush(&mut paragraph, depth, gap);
    blocks
}

/// The quote depth of a line (`> > text` and `>> text` are both 2) and the line without its marks.
fn split_quote(line: &str) -> (u8, &str) {
    let mut depth = 0u8;
    let mut rest = line;
    while let Some(after) = rest.strip_prefix('>') {
        depth = depth.saturating_add(1);
        rest = after.strip_prefix(' ').unwrap_or(after);
    }
    (depth, rest)
}

/// Text as spans, with web and e-mail addresses made into links.
fn autolink(text: &str) -> Vec<Span> {
    let b = text.as_bytes();
    let mut spans = Vec::new();
    let mut plain = 0;
    let mut i = 0;
    while let Some(c) = b.get(i) {
        let hit = match c {
            b'h' | b'H' | b'w' | b'W' if i == 0 || b.get(i - 1).is_some_and(|p| !p.is_ascii_alphanumeric()) => url_at(text, i),
            b'@' => email_at(text, i, plain),
            _ => Hit::None,
        };
        match hit {
            Hit::None => i += 1,
            Hit::Skip(end) => i = end.max(i + 1),
            // Only what the safety check accepts is a link, so it is never offered to be opened and then refused.
            Hit::Link { end, ref url, .. } if !crate::is_safe_url(url) => i = end.max(i + 1),
            Hit::Link { start, end, url } => {
                if let Some(before) = text.get(plain..start).filter(|s| !s.is_empty()) {
                    spans.push(Span::plain(before));
                }
                spans.push(Span::linked(text.get(start..end).unwrap_or(""), url));
                plain = end;
                i = end.max(i + 1);
            }
        }
    }
    if let Some(rest) = text.get(plain..).filter(|s| !s.is_empty()) {
        spans.push(Span::plain(rest));
    }
    spans
}

enum Hit {
    None,
    /// Something address-like that is not linked: carry on after it.
    Skip(usize),
    Link {
        start: usize,
        end: usize,
        url: String,
    },
}

fn starts_with_ignore_case(s: &str, prefix: &str) -> bool {
    s.as_bytes().get(..prefix.len()).is_some_and(|p| p.eq_ignore_ascii_case(prefix.as_bytes()))
}

/// A web address starting at byte `start`.
fn url_at(text: &str, start: usize) -> Hit {
    let Some(rest) = text.get(start..) else { return Hit::None };
    let (prefix, bare) = if starts_with_ignore_case(rest, "https://") {
        (8, false)
    } else if starts_with_ignore_case(rest, "http://") {
        (7, false)
    } else if starts_with_ignore_case(rest, "www.") {
        (4, true)
    } else {
        return Hit::None;
    };
    let len = rest.find(|c: char| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"')).unwrap_or(rest.len());
    let token = rest.get(..len).unwrap_or("");
    // Punctuation after an address belongs to the sentence; a `)` belongs to the address only when it closes a `(` in it.
    let mut url = token;
    let mut unclosed = [('(', ')'), ('[', ']')].map(|(open, close)| url.matches(close).count().saturating_sub(url.matches(open).count()));
    while let Some(last) = url.chars().next_back() {
        let strip = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '*' => true,
            ')' | ']' => {
                let n = unclosed.get_mut(usize::from(last == ']'));
                n.is_some_and(|n| {
                    let extra = *n > 0;
                    *n = n.saturating_sub(1);
                    extra
                })
            }
            _ => false,
        };
        if !strip {
            break;
        }
        url = url.get(..url.len() - last.len_utf8()).unwrap_or("");
    }
    let host_starts = url.as_bytes().get(prefix).is_some_and(u8::is_ascii_alphanumeric) || (!bare && url.len() > prefix);
    if !host_starts || url.len() > MAX_URL {
        return Hit::Skip(start + len);
    }
    let href = if bare { format!("https://{url}") } else { url.to_string() };
    Hit::Link { start, end: start + url.len(), url: href }
}

/// An e-mail address around the `@` at byte `at`, starting no earlier than `floor`.
fn email_at(text: &str, at: usize, floor: usize) -> Hit {
    let b = text.as_bytes();
    let mut start = at;
    while start > floor && at - start < 64 && b.get(start - 1).is_some_and(|c| c.is_ascii_alphanumeric() || b"._%+-".contains(c)) {
        start -= 1;
    }
    // The local part must not run on to the left, and does not begin with a dot.
    if start > floor && b.get(start - 1).is_some_and(u8::is_ascii_alphanumeric) {
        return Hit::None;
    }
    while start < at && b.get(start) == Some(&b'.') {
        start += 1;
    }
    let mut end = at + 1;
    while end - at <= 255 && b.get(end).is_some_and(|c| c.is_ascii_alphanumeric() || matches!(*c, b'-' | b'.')) {
        end += 1;
    }
    while end > at + 1 && b.get(end - 1).is_some_and(|c| matches!(*c, b'-' | b'.')) {
        end -= 1;
    }
    let (Some(local), Some(domain)) = (text.get(start..at), text.get(at + 1..end)) else { return Hit::None };
    let top = domain.rsplit('.').next().unwrap_or("");
    let valid = !local.is_empty()
        && domain.contains('.')
        && !domain.contains("..")
        && !domain.starts_with(['.', '-'])
        && top.len() >= 2
        && top.bytes().all(|c| c.is_ascii_alphabetic())
        && b.get(end).is_none_or(|c| !c.is_ascii_alphanumeric() && *c != b'@');
    if !valid {
        return Hit::None;
    }
    Hit::Link { start, end, url: format!("mailto:{local}@{domain}") }
}

/// Readable plain text from HTML, for search and replying in plain text (a message list shows
/// [`crate::preview`] instead).
///
/// Paragraphs are separated by blank lines. List items start with `• ` or `1. `, quoted blocks
/// with `> `, a rule is a row of dashes, a picture is its `[alt text]`. A link reads
/// `text <url>` when the text doesn't already say where it goes. No-break spaces become spaces.
pub fn to_text(html: &str) -> String {
    blocks_to_text(&crate::blocks::parse_text(html))
}

/// [`to_text`], and with it the words of what the message hides (`display:none`, a style sheet's
/// `visibility:hidden`, a font size of nothing and the other tricks `style.rs` lists), as one line
/// of words. What is hidden is not in the text: it is not shown, nor quoted in a reply. It is for
/// the owner's search and for agents, who must still treat it as data, never as instructions.
pub fn text_and_hidden(html: &str) -> (String, String) {
    let built = crate::blocks::build(html, false, &crate::media::Env::default(), true);
    (blocks_to_text(&built.blocks), built.hidden)
}

/// Display blocks as plain text, the way [`to_text`] writes them. Every block is a paragraph of
/// its own, whatever its `space`. A data table is a line per row with ` | ` between the cells; a
/// layout table is what its cells hold, one after the other.
pub fn blocks_to_text(blocks: &[Block]) -> String {
    let mut pieces = Vec::new();
    pieces_of(blocks, 0, &mut pieces);
    let mut out = String::new();
    let mut prev: Option<&Piece> = None;
    for piece in &pieces {
        let quote = piece.quote.min(MAX_QUOTE_MARKS);
        if let Some(before) = prev {
            out.push('\n');
            if !(before.list && piece.list && before.quote == piece.quote) {
                // A blank line, still quoted when both neighbours are.
                out.extend(std::iter::repeat_n('>', quote.min(before.quote)));
                out.push('\n');
            }
        }
        for (i, line) in piece.body.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
            }
            if quote > 0 {
                out.extend(std::iter::repeat_n('>', quote));
                if !line.is_empty() {
                    out.push(' ');
                }
            }
            out.push_str(line);
        }
        prev = Some(piece);
    }
    out
}

/// One paragraph of the plain text.
struct Piece {
    body: String,
    quote: usize,
    list: bool,
}

/// The paragraphs `blocks` make, quoted `quote` deep on top of their own depth.
fn pieces_of(blocks: &[Block], quote: usize, out: &mut Vec<Piece>) {
    // How many paragraphs there were when the block before was done, and after.
    let mut before = (usize::MAX, usize::MAX);
    for block in blocks {
        let count = out.len();
        // What stands in the line of the block before goes on in its paragraph. So does a box on a
        // line, when it and the box before it are each a single line of text; boxes that hold
        // paragraphs, lists or more are paragraphs of their own.
        let own_quote = quote.saturating_add(usize::from(block.quote));
        let joined = if block.beside && before.1 > before.0 {
            match &block.kind {
                BlockKind::Table(table) if table.inline => {
                    let single = before.1 == before.0.saturating_add(1)
                        && out.last().is_some_and(|p| !p.list && !p.body.contains('\n') && p.quote == own_quote);
                    single.then(|| one_line(table)).flatten()
                }
                BlockKind::Table(_) => None,
                _ => Some(block_body(block)),
            }
        } else {
            None
        };
        if let Some(body) = joined {
            if let Some(last) = out.last_mut().filter(|_| !body.is_empty()) {
                last.body.push(' ');
                last.body.push_str(&body);
            }
            before = (count, out.len().max(count.saturating_add(1)));
            continue;
        }
        let quote = quote.saturating_add(usize::from(block.quote));
        match &block.kind {
            BlockKind::Table(table) if table.layout => {
                for cell in table.rows.iter().flat_map(|r| &r.cells) {
                    pieces_of(&cell.blocks, quote, out);
                }
            }
            BlockKind::Table(table) => {
                let body = table_text(table);
                if !body.is_empty() {
                    out.push(Piece { body, quote, list: false });
                }
            }
            kind => {
                let body = block_body(block);
                if !body.is_empty() {
                    out.push(Piece { body, quote, list: matches!(kind, BlockKind::ListItem { .. }) });
                }
            }
        }
        before = (count, out.len());
    }
}

/// The text of a box that holds a single line, and nothing else: no paragraphs, list, quote or break.
fn one_line(table: &Table) -> Option<String> {
    let [row] = table.rows.as_slice() else { return None };
    let [cell] = row.cells.as_slice() else { return None };
    let mut pieces = Vec::new();
    pieces_of(&cell.blocks, 0, &mut pieces);
    match pieces.as_slice() {
        [piece] if !piece.list && piece.quote == 0 && !piece.body.contains('\n') => Some(piece.body.clone()),
        _ => None,
    }
}

/// A data table: a line per row, the cells joined by ` | `. Empty cells are left out.
fn table_text(table: &Table) -> String {
    let mut lines = Vec::new();
    for row in &table.rows {
        let cells: Vec<String> = row
            .cells
            .iter()
            .map(|cell| {
                let mut pieces = Vec::new();
                pieces_of(&cell.blocks, 0, &mut pieces);
                pieces.iter().flat_map(|p| p.body.split('\n')).map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ")
            })
            .filter(|text| !text.is_empty())
            .collect();
        if !cells.is_empty() {
            lines.push(cells.join(" | "));
        }
    }
    lines.join("\n")
}

/// One block's text, before quote marks.
fn block_body(block: &Block) -> String {
    match &block.kind {
        BlockKind::Rule => "----------".to_string(),
        BlockKind::Table(_) => String::new(),
        BlockKind::Image { .. } => {
            let alt = block.text();
            let alt = alt.trim();
            if alt.is_empty() { String::new() } else { format!("[{alt}]") }
        }
        BlockKind::ListItem { marker, depth } => {
            let indent = "  ".repeat(usize::from(*depth).min(MAX_INDENT));
            let marker = match marker {
                Marker::Number(n) => format!("{n}. "),
                Marker::Label(label) => format!("{label} "),
                Marker::Bullet => "• ".to_string(),
                Marker::None => "  ".to_string(),
            };
            let hang = " ".repeat(marker.chars().count());
            let mut out = String::new();
            for (i, line) in spans_text(&block.spans).split('\n').enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                out.push_str(&indent);
                out.push_str(if i == 0 { &marker } else { &hang });
                out.push_str(line);
            }
            out
        }
        _ => spans_text(&block.spans),
    }
}

/// Spans as text, with ` <url>` after a link whose text doesn't show the address.
fn spans_text(spans: &[Span]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while let Some(first) = spans.get(i) {
        let mut text = String::new();
        while let Some(span) = spans.get(i).filter(|s| s.link == first.link) {
            text.push_str(&span.text);
            i += 1;
        }
        let text = text.replace('\u{a0}', " ");
        match &first.link {
            Some(url) if !says_where(&text, url) => {
                let shown = text.trim_end();
                out.push_str(shown);
                out.push_str(" <");
                out.push_str(url);
                out.push('>');
                out.push_str(text.get(shown.len()..).unwrap_or(""));
            }
            _ => out.push_str(&text),
        }
    }
    out
}

/// Whether a link's text already is its address.
fn says_where(text: &str, url: &str) -> bool {
    fn bare(s: &str) -> &str {
        let s = s.trim();
        let s = ["https://", "http://", "mailto:", "tel:"]
            .iter()
            .find_map(|p| s.get(..p.len()).filter(|h| h.eq_ignore_ascii_case(p)).and_then(|_| s.get(p.len()..)))
            .unwrap_or(s);
        s.trim_end_matches('/')
    }
    bare(text).eq_ignore_ascii_case(bare(url))
}

/// A plain-text body as an HTML fragment.
///
/// Everything is escaped. Paragraphs become `<p>`, line breaks `<br>`, quoted text (`>`) nested
/// `<blockquote>`s, and web and e-mail addresses links. Runs of spaces and leading spaces are kept
/// with no-break spaces.
pub fn from_text(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for block in text_blocks(text) {
        let quote = usize::from(block.quote).min(MAX_QUOTE_MARKS);
        while depth < quote {
            out.push_str("<blockquote>\n");
            depth += 1;
        }
        while depth > quote {
            out.push_str("</blockquote>\n");
            depth -= 1;
        }
        out.push_str("<p>");
        let mut prev = '\n';
        for span in &block.spans {
            if let Some(url) = &span.link {
                out.push_str("<a href=\"");
                out.push_str(&escape(url));
                out.push_str("\">");
            }
            for c in span.text.chars() {
                match c {
                    '\n' => out.push_str("<br>\n"),
                    ' ' if prev == ' ' || prev == '\n' => out.push_str("&nbsp;"),
                    '&' => out.push_str("&amp;"),
                    '<' => out.push_str("&lt;"),
                    '>' => out.push_str("&gt;"),
                    '"' => out.push_str("&quot;"),
                    '\'' => out.push_str("&#39;"),
                    c => out.push(c),
                }
                prev = c;
            }
            if span.link.is_some() {
                out.push_str("</a>");
            }
        }
        out.push_str("</p>\n");
    }
    for _ in 0..depth {
        out.push_str("</blockquote>\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    /// A paragraph under a blank line.
    fn para(text: &str, quote: u8) -> Block {
        Block { quote, space: 1, ..Block::new(BlockKind::Paragraph, vec![Span::plain(text)]) }
    }

    /// A paragraph right under what comes before it, or the first one.
    fn first(text: &str, quote: u8) -> Block {
        Block { space: 0, ..para(text, quote) }
    }

    /// Spans as text with links in brackets.
    fn links(text: &str) -> String {
        autolink(text)
            .iter()
            .map(|s| match &s.link {
                Some(url) => format!("[{}→{url}]", s.text),
                None => s.text.clone(),
            })
            .collect()
    }

    #[test]
    fn paragraphs_split_on_blank_lines() {
        assert_eq!(text_blocks("One\ntwo\n\nThree\n\n\n\nFour\n"), [first("One\ntwo", 0), para("Three", 0), para("Four", 0)]);
        assert_eq!(text_blocks("One\r\ntwo\r\n\r\nThree\rfour"), [first("One\ntwo", 0), para("Three\nfour", 0)]);
        assert_eq!(text_blocks(""), []);
        assert_eq!(text_blocks("\n \n\t\n"), []);
        assert_eq!(text_blocks("  indented  \nline  "), [first("  indented\nline", 0)]);
        // Blank lines at the start are not a gap above the first paragraph.
        assert_eq!(text_blocks("\n\nOne"), [first("One", 0)]);
    }

    #[test]
    fn quote_marks_become_depth() {
        let text = "Hi\n\n> one\n> still one\n>> two\n> > also two\n>\n> back\nout";
        // A change of depth without a blank line starts right under the text before it.
        let want = [first("Hi", 0), para("one\nstill one", 1), first("two\nalso two", 2), para("back", 1), first("out", 0)];
        assert_eq!(text_blocks(text), want);
        assert_eq!(text_blocks(">no space"), [first("no space", 1)]);
        assert_eq!(text_blocks(" > not a quote"), [first(" > not a quote", 0)]);
        let deep = format!("{}x", ">".repeat(1000));
        assert_eq!(text_blocks(&deep), [first("x", u8::MAX)]);
    }

    #[test]
    fn control_characters_are_removed_from_text() {
        assert_eq!(text_blocks("a\u{0}b\u{1b}[0mc\td"), [first("ab[0mc\td", 0)]);
    }

    #[test]
    fn web_addresses_become_links() {
        assert_eq!(links("see https://example.org/a?b=1&c=2#d now"), "see [https://example.org/a?b=1&c=2#d→https://example.org/a?b=1&c=2#d] now");
        assert_eq!(links("HTTP://EXAMPLE.ORG"), "[HTTP://EXAMPLE.ORG→HTTP://EXAMPLE.ORG]");
        assert_eq!(links("go to www.example.org."), "go to [www.example.org→https://www.example.org].");
        assert_eq!(links("(https://example.org/a_(b))!"), "([https://example.org/a_(b)→https://example.org/a_(b)])!");
        assert_eq!(
            links("<https://example.org/x>, \"http://example.org\""),
            "<[https://example.org/x→https://example.org/x]>, \"[http://example.org→http://example.org]\""
        );
        assert_eq!(links("https://example.org/ü/é"), "[https://example.org/ü/é→https://example.org/ü/é]");
        assert_eq!(links("a https://a.example b http://b.example"), "a [https://a.example→https://a.example] b [http://b.example→http://b.example]");
    }

    #[test]
    fn things_that_are_not_addresses() {
        for s in ["http://", "https:// x", "www.", "www. x", "awww.example.org", "xhttp://example.org", "what", "www", "h", "the http protocol"] {
            assert_eq!(links(s), s, "{s}");
        }
        let long = format!("https://example.org/{}", "a".repeat(MAX_URL));
        assert_eq!(links(&long), long);
    }

    #[test]
    fn mail_addresses_become_links() {
        assert_eq!(links("write to ada@example.org."), "write to [ada@example.org→mailto:ada@example.org].");
        assert_eq!(links("<ada.lovelace+tag@mail.example.org>"), "<[ada.lovelace+tag@mail.example.org→mailto:ada.lovelace+tag@mail.example.org]>");
        assert_eq!(links("a@example.org,b@example.org"), "[a@example.org→mailto:a@example.org],[b@example.org→mailto:b@example.org]");
        assert_eq!(links("é@example.org x@é.org"), "é@example.org x@é.org");
        for s in ["@", "a@", "@b.org", "a@b", "a@b.c", "a@.org", "a@b..org", "1.5@2.0", "a@@b.org", "user@host.123"] {
            assert_eq!(links(s), s, "{s}");
        }
        // The address inside a web address is part of it.
        assert_eq!(links("https://ada@example.org/x"), "[https://ada@example.org/x→https://ada@example.org/x]");
    }

    #[test]
    fn hostile_text_is_linear() {
        for s in [
            "@".repeat(200_000),
            "a@".repeat(100_000),
            "http://a/".repeat(50_000),
            "www.".repeat(50_000),
            ">".repeat(200_000),
            "a.b@c.d ".repeat(30_000),
            ")".repeat(100_000),
        ] {
            let _ = text_blocks(&s);
            let _ = from_text(&s);
        }
        let s = format!("https://example.org/{}", ")".repeat(200_000));
        assert_eq!(text_blocks(&s).len(), 1);
    }

    #[test]
    fn html_to_text() {
        assert_eq!(to_text("<p>Hello <b>world</b></p><p>Second&nbsp;line<br>third</p>"), "Hello world\n\nSecond line\nthird");
        assert_eq!(to_text(""), "");
        assert_eq!(to_text("<h1>Title</h1><p>Body</p><hr><p>End</p>"), "Title\n\nBody\n\n----------\n\nEnd");
    }

    #[test]
    fn lists_as_text() {
        assert_eq!(to_text("<ul><li>a</li><li>b<ul><li>b1</li></ul></li></ul><p>after</p>"), "• a\n• b\n  • b1\n\nafter");
        assert_eq!(to_text("<ol start=9><li>nine</li><li>ten<br>more</li></ol>"), "9. nine\n10. ten\n    more");
    }

    #[test]
    fn links_as_text() {
        assert_eq!(to_text(r#"<a href="https://example.org/x">Click here</a> now"#), "Click here <https://example.org/x> now");
        assert_eq!(to_text(r#"<a href="https://example.org/">example.org</a>"#), "example.org");
        assert_eq!(to_text(r#"<a href="https://example.org">https://example.org/</a>"#), "https://example.org/");
        assert_eq!(to_text(r#"<a href="mailto:ada@example.org">ada@example.org</a>"#), "ada@example.org");
        assert_eq!(to_text(r#"<a href="mailto:ada@example.org">Ada</a>"#), "Ada <mailto:ada@example.org>");
        assert_eq!(to_text(r#"<a href="https://example.org"><b>Bold</b> and plain</a>"#), "Bold and plain <https://example.org>");
        assert_eq!(to_text(r#"<a href="javascript:x()">safe text</a>"#), "safe text");
    }

    #[test]
    fn quotes_and_images_as_text() {
        let html = "<p>Reply</p><blockquote><p>One</p><p>Two<br>lines</p><blockquote>Deep</blockquote></blockquote>";
        assert_eq!(to_text(html), "Reply\n\n> One\n>\n> Two\n> lines\n>\n>> Deep");
        assert_eq!(to_text(r#"<img src="cid:a" alt="Chart of sales"><img src="cid:b"><p>x</p>"#), "[Chart of sales]\n\nx");
    }

    #[test]
    fn text_from_html_reads_back_as_the_same_blocks() {
        let html = "<p>Reply</p><blockquote><p>One</p><p>Two<br>lines</p><blockquote>Deep</blockquote></blockquote><p>Bye</p>";
        let want = [first("Reply", 0), para("One", 1), para("Two\nlines", 1), para("Deep", 2), para("Bye", 0)];
        assert_eq!(text_blocks(&to_text(html)), want);
    }

    #[test]
    fn text_as_html() {
        assert_eq!(from_text("Hi <Bob> & \"co\"\nsecond\n\nNext"), "<p>Hi &lt;Bob&gt; &amp; &quot;co&quot;<br>\nsecond</p>\n<p>Next</p>\n");
        assert_eq!(from_text(""), "");
        assert_eq!(
            from_text("see https://example.org/?a=1&b=2"),
            "<p>see <a href=\"https://example.org/?a=1&amp;b=2\">https://example.org/?a=1&amp;b=2</a></p>\n"
        );
        assert_eq!(
            from_text("> quoted\n>> deeper\nback"),
            "<blockquote>\n<p>quoted</p>\n<blockquote>\n<p>deeper</p>\n</blockquote>\n</blockquote>\n<p>back</p>\n"
        );
        assert_eq!(from_text("  two  spaces"), "<p>&nbsp;&nbsp;two &nbsp;spaces</p>\n");
    }

    #[test]
    fn html_from_text_parses_back() {
        let text = "Hello Bob,\nsee https://example.org and write to ada@example.org.\n\n> you wrote\n> this\n\n<script>alert(1)</script>";
        assert_eq!(parse(&from_text(text)), text_blocks(text));
        assert_eq!(to_text(&from_text("a  b\n  c")), "a  b\n  c");
    }

    proptest::proptest! {
        #[test]
        fn any_text_converts(s in proptest::prelude::any::<String>()) {
            let blocks = text_blocks(&s);
            let _ = blocks_to_text(&blocks);
            let _ = to_text(&s);
            let html = from_text(&s);
            // Whatever the text says, it stays text.
            for block in parse(&html) {
                proptest::prop_assert!(matches!(block.kind, BlockKind::Paragraph));
            }
        }

        #[test]
        fn link_spans_cover_the_text(s in "[a-c@.:/hw ()<>,é\n-]{0,60}") {
            for block in text_blocks(&s) {
                for span in &block.spans {
                    proptest::prop_assert!(!span.text.is_empty());
                    proptest::prop_assert!(span.link.as_deref().is_none_or(crate::is_safe_url));
                }
            }
            let joined: String = autolink(&s).iter().map(|x| x.text.as_str()).collect();
            proptest::prop_assert_eq!(joined, s);
        }
    }
}
